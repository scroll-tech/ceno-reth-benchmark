use std::collections::BTreeSet;

use alloy_consensus::{Transaction, TxEnvelope, TxReceipt};
use alloy_primitives::Bloom;
use alloy_provider::{network::Ethereum, Provider};
use eyre::{eyre, Ok};
use futures::{stream, StreamExt, TryStreamExt};
use openvm_client_executor::io::ClientExecutorInput;
use openvm_mpt::from_proof::transition_proofs_to_tries;
use openvm_primitives::account_proof::eip1186_proof_to_account_proof;
use openvm_rpc_db::RpcDb;
use reth_chainspec::MAINNET;
use reth_consensus::{Consensus, HeaderValidator};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_evm::execute::{BasicBlockExecutor, Executor};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::Block;
use reth_primitives_traits::block::Block as _;
use revm::database::CacheDB;
use revm_primitives::{B256, U256};

/// An executor that fetches data from a [Provider] to execute blocks in the [ClientExecutor].
#[derive(Debug, Clone)]
pub struct HostExecutor<P: Provider<Ethereum> + Clone> {
    /// The provider which fetches data.
    pub provider: P,
}

impl<P: Provider<Ethereum> + Clone + std::fmt::Debug> HostExecutor<P> {
    /// Create a new [`HostExecutor`] with a specific [Provider] and [Transport].
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// Executes the block with the given block number.
    pub async fn execute(&self, block_number: u64) -> eyre::Result<ClientExecutorInput> {
        // Fetch the current block and the previous block from the provider.
        tracing::info!("fetching the current block and the previous block");
        let current_block = self
            .provider
            .get_block_by_number(block_number.into())
            .full()
            .await?
            .map(into_primitive_block)
            .ok_or(eyre!("couldn't fetch block: {}", block_number))?;
        let previous_block = self
            .provider
            .get_block_by_number((block_number - 1).into())
            .full()
            .await?
            .map(into_primitive_block)
            .ok_or(eyre!("couldn't fetch block: {}", block_number))?;

        // Setup the spec for the block executor.
        tracing::info!("setting up the spec for the block executor");
        let spec = MAINNET.clone();

        // Setup the database for the block executor.
        tracing::info!("setting up the database for the block executor");
        let rpc_db = RpcDb::new(self.provider.clone(), block_number - 1);
        let access_list_entries = collect_access_list_entries(&current_block);
        if !access_list_entries.is_empty() {
            let concurrency = rpc_concurrency("CENO_RPC_PREFETCH_CONCURRENCY", 32);
            tracing::info!(
                account_count = access_list_entries.len(),
                concurrency,
                "prefetching transaction access-list state"
            );
            rpc_db.prefetch_access_list(access_list_entries, concurrency).await?;
        }
        let cache_db = CacheDB::new(&rpc_db);

        // Execute the block and fetch all the necessary data along the way.
        tracing::info!(
            "executing the block and with rpc db: block_number={}, transaction_count={}",
            block_number,
            current_block.body.transactions.len()
        );

        let block = current_block.clone().try_into_recovered()?;

        tracing::info!("validate_block_consensus");
        let consensus = EthBeaconConsensus::new(spec.clone());
        consensus.validate_header(block.sealed_header())?;
        consensus.validate_block_pre_execution(&block)?;

        let block_executor = BasicBlockExecutor::new(EthEvmConfig::new(spec.clone()), cache_db);

        let executor_output = block_executor.execute(&block)?;

        // Validate the block post execution.
        tracing::info!("validating the block post execution");
        validate_block_post_execution(
            &block,
            &spec,
            &executor_output.receipts,
            &executor_output.requests,
        )?;

        // Accumulate the logs bloom.
        tracing::info!("accumulating the logs bloom");
        let mut logs_bloom = Bloom::default();
        executor_output.receipts.iter().for_each(|r| {
            logs_bloom.accrue_bloom(&r.bloom());
        });

        // Convert the output to an execution outcome.
        let executor_outcome = ExecutionOutcome::new(
            executor_output.state,
            vec![executor_output.result.receipts],
            current_block.header.number,
            vec![executor_output.result.requests],
        );

        let state_requests = rpc_db.get_state_requests();

        // For every account we touched, fetch the storage proofs for all the slots we touched.
        tracing::info!("fetching storage proofs");
        let proof_requests = state_requests
            .iter()
            .map(|(address, used_keys)| {
                let modified_keys = executor_outcome
                    .state()
                    .state
                    .get(address)
                    .map(|account| {
                        account.storage.keys().map(|key| B256::from(*key)).collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<Vec<_>>();

                let keys = used_keys
                    .iter()
                    .map(|key| B256::from(*key))
                    .chain(modified_keys.clone().into_iter())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();

                (*address, keys, modified_keys)
            })
            .collect::<Vec<_>>();

        let proof_concurrency = rpc_concurrency("CENO_RPC_PROOF_CONCURRENCY", 32);
        tracing::info!(
            account_count = proof_requests.len(),
            concurrency = proof_concurrency,
            "fetching storage proofs with bounded concurrency"
        );
        let proof_pairs = stream::iter(proof_requests)
            .map(|(address, keys, modified_keys)| {
                let provider = self.provider.clone();
                async move {
                    let before = provider
                        .get_proof(address, keys)
                        .block_id((block_number - 1).into())
                        .await?;
                    let after = provider
                        .get_proof(address, modified_keys)
                        .block_id(block_number.into())
                        .await?;
                    eyre::Ok((
                        eip1186_proof_to_account_proof(before),
                        eip1186_proof_to_account_proof(after),
                    ))
                }
            })
            .buffer_unordered(proof_concurrency)
            .try_collect::<Vec<_>>()
            .await?;
        let (before_storage_proofs, after_storage_proofs): (Vec<_>, Vec<_>) =
            proof_pairs.into_iter().unzip();

        let state = transition_proofs_to_tries(
            previous_block.state_root,
            &before_storage_proofs.iter().map(|item| (item.address, item.clone())).collect(),
            &after_storage_proofs.iter().map(|item| (item.address, item.clone())).collect(),
        )?;

        // Skip state root verification for now.
        // It works with Alchemy but for some reason not with Quicknode.
        // It is checked on the client (guest) side and works with all providers.

        // Derive the block header.
        //
        // Note: the receipts root and gas used are verified by `validate_block_post_execution`.
        let mut header = current_block.header.clone();
        header.parent_hash = previous_block.hash_slow();
        header.ommers_hash = current_block.body.calculate_ommers_root();
        header.state_root = current_block.state_root;
        header.transactions_root = current_block.transactions_root;
        header.receipts_root = current_block.header.receipts_root;
        header.withdrawals_root = current_block.body.calculate_withdrawals_root();
        header.logs_bloom = logs_bloom;
        header.requests_hash = current_block.requests_hash;

        // Assert the derived header is correct.
        assert_eq!(header.hash_slow(), current_block.header.hash_slow(), "header mismatch");

        // Log the result.
        tracing::info!(
            "successfully executed block: block_number={}, block_hash={}, state_root={}",
            current_block.header.number,
            header.hash_slow(),
            current_block.state_root
        );

        // Fetch the parent headers needed to constrain the BLOCKHASH opcode.
        let oldest_ancestor = *rpc_db.oldest_ancestor.borrow();
        let mut ancestor_headers = vec![];
        tracing::info!("fetching {} ancestor headers", block_number - oldest_ancestor);
        for height in (oldest_ancestor..=(block_number - 1)).rev() {
            let block = self.provider.get_block_by_number(height.into()).await?.unwrap();
            ancestor_headers.push(block.header.into());
        }

        let state_bytes = state.encode_to_state_bytes();

        // Create the client input.
        let client_input = ClientExecutorInput {
            current_block,
            ancestor_headers,
            parent_state_bytes: state_bytes,
            bytecodes: rpc_db.get_bytecodes(),
        };
        tracing::info!("successfully generated client input");

        Ok(client_input)
    }
}

fn into_primitive_block(block: alloy_rpc_types::Block) -> Block {
    let block = block.map_transactions(|tx| TxEnvelope::from(tx).into());
    block.into_consensus()
}

fn rpc_concurrency(env_key: &str, default: usize) -> usize {
    std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn collect_access_list_entries(block: &Block) -> Vec<(alloy_primitives::Address, Vec<U256>)> {
    let mut entries = std::collections::BTreeMap::<_, BTreeSet<_>>::new();
    for tx in &block.body.transactions {
        let Some(access_list) = tx.access_list() else {
            continue;
        };
        for item in &access_list.0 {
            let entry = entries.entry(item.address).or_default();
            entry.extend(item.storage_keys.iter().map(|key| U256::from_be_slice(key.as_slice())));
        }
    }

    entries.into_iter().map(|(address, keys)| (address, keys.into_iter().collect())).collect()
}
