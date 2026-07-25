mod capacity;
pub mod error;
/// Client program input data types.
pub mod io;

use std::{cell::RefCell, fmt::Debug, sync::Arc};

use alloy_consensus::{Header, TxReceipt};
use alloy_primitives::{keccak256, Bloom, B256};
use alloy_trie::EMPTY_ROOT_HASH;
use openvm_primitives::chain_spec::{dev, mainnet};
use reth_consensus::{Consensus, HeaderValidator};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_evm::execute::Executor;
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::block::Block as _;
use reth_revm::db::CacheDB;

use crate::{
    capacity::{revm_state_with_capacity, CapacityBlockExecutor},
    error::ClientExecutionError,
    io::{
        AncestorHeadersInput, ClientExecutorInput, ClientExecutorInputWithState, ClientInputReader,
        WitnessAccess,
    },
};

type LookupOrders<'a> = (
    &'a RefCell<Vec<B256>>,
    &'a RefCell<Vec<B256>>,
    &'a RefCell<Vec<B256>>,
    &'a RefCell<Vec<WitnessAccess>>,
);

/// Chain ID for Ethereum Mainnet.
pub const CHAIN_ID_ETH_MAINNET: u64 = 0x1;

/// An executor that executes a block inside a zkVM.
#[derive(Debug, Clone, Default)]
pub struct ClientExecutor;

/// EVM chain variants that implement different execution/validation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChainVariant {
    Mainnet,
    Dev,
}

impl ClientExecutor {
    pub fn execute(
        &self,
        chain_variant: ChainVariant,
        pre_input: ClientExecutorInput,
    ) -> Result<Header, ClientExecutionError> {
        let input = ClientExecutorInputWithState::build(pre_input)?;
        self.execute_with_state(chain_variant, input, None)
    }

    pub fn execute_recording_witness_order(
        &self,
        chain_variant: ChainVariant,
        pre_input: ClientExecutorInput,
    ) -> Result<(Header, Vec<WitnessAccess>), ClientExecutionError> {
        let input = ClientExecutorInputWithState::build(pre_input)?;
        let account_lookup_order = RefCell::new(Vec::new());
        let storage_lookup_order = RefCell::new(Vec::new());
        let bytecode_lookup_order = RefCell::new(Vec::new());
        let witness_order = RefCell::new(Vec::new());
        let header = self.execute_with_state(
            chain_variant,
            input,
            Some((
                &account_lookup_order,
                &storage_lookup_order,
                &bytecode_lookup_order,
                &witness_order,
            )),
        )?;
        Ok((header, witness_order.into_inner()))
    }

    pub fn execute_from_reader(
        &self,
        chain_variant: ChainVariant,
        input: &mut impl ClientInputReader,
    ) -> Result<Header, ClientExecutionError> {
        let AncestorHeadersInput { ancestor_headers } = input.read_ancestor_headers();
        let current_block = input.read_current_block().current_block;
        let current_header = current_block.header.clone();
        let current_block_number = current_header.number;
        let current_state_root = current_block.state_root;
        let current_transactions_root = current_block.transactions_root;
        let current_ommers_hash = current_block.body.calculate_ommers_root();
        let current_withdrawals_root = current_block.body.calculate_withdrawals_root();
        let current_requests_hash = current_block.requests_hash;

        let spec = Arc::new(match chain_variant {
            ChainVariant::Mainnet => mainnet(),
            ChainVariant::Dev => dev(),
        });
        let recovered_block = current_block
            .try_into_recovered()
            .map_err(|err| ClientExecutionError::BlockSenderRecoveryError(err.into()))?;

        {
            let consensus = EthBeaconConsensus::new(spec.clone());

            consensus
                .validate_header(recovered_block.sealed_header())
                .map_err(ClientExecutionError::InvalidHeader)?;

            consensus
                .validate_block_pre_execution(&recovered_block)
                .map_err(ClientExecutionError::InvalidBlockPreExecution)?;
        };

        let mut state = io::build_streaming_state_from_input_reader(&ancestor_headers, input)?;
        let witness_db =
            io::WitnessDb::from_streaming_parts(&state, &current_header, &ancestor_headers)?;
        let cache_db = CacheDB::new(&witness_db);
        let revm_state = revm_state_with_capacity(
            cache_db,
            recovered_block.body().transactions.len(),
            ancestor_headers.len(),
        );
        let block_executor =
            CapacityBlockExecutor::new_with_state(EthEvmConfig::new(spec.clone()), revm_state);
        let executor_output = block_executor.execute(&recovered_block)?;

        validate_block_post_execution(&recovered_block, &spec, &executor_output, None, None)
            .map_err(ClientExecutionError::InvalidBlockPostExecution)?;

        let mut logs_bloom = Bloom::default();
        executor_output.receipts.iter().for_each(|r| {
            logs_bloom.accrue_bloom(&r.bloom());
        });

        let executor_outcome = ExecutionOutcome::new(
            executor_output.state,
            vec![executor_output.result.receipts],
            current_block_number,
            vec![executor_output.result.requests],
        );

        drop(witness_db);

        let state_root = {
            state.update_from_bundle_state(executor_outcome.bundle)?;
            state.state_root()
        };

        if state_root != current_state_root {
            return Err(ClientExecutionError::StateRootMismatch {
                actual: state_root,
                expected: current_state_root,
            });
        }

        Ok(header_from_known_block(
            current_header,
            &ancestor_headers,
            current_ommers_hash,
            current_state_root,
            current_transactions_root,
            current_withdrawals_root,
            logs_bloom,
            current_requests_hash,
        ))
    }

    fn execute_with_state(
        &self,
        chain_variant: ChainVariant,
        mut input: ClientExecutorInputWithState,
        lookup_orders: Option<LookupOrders<'_>>,
    ) -> Result<Header, ClientExecutionError> {
        // Initialize the witnessed database with verified storage proofs.
        let witness_db = match lookup_orders {
            Some((
                account_lookup_order,
                storage_lookup_order,
                bytecode_lookup_order,
                witness_order,
            )) => input.witness_db_recording(
                account_lookup_order,
                bytecode_lookup_order,
                storage_lookup_order,
                Some(witness_order),
            )?,
            None => input.witness_db()?,
        };
        let cache_db = CacheDB::new(&witness_db);

        // Execute the block.
        let spec = Arc::new(match chain_variant {
            ChainVariant::Mainnet => mainnet(),
            ChainVariant::Dev => dev(),
        });
        // Recover senders
        let current_block = input
            .input
            .current_block
            .clone()
            .try_into_recovered()
            .map_err(|err| ClientExecutionError::BlockSenderRecoveryError(err.into()))?;

        // validate the block pre-execution
        {
            let consensus = EthBeaconConsensus::new(spec.clone());

            consensus
                .validate_header(current_block.sealed_header())
                .map_err(ClientExecutionError::InvalidHeader)?;

            consensus
                .validate_block_pre_execution(&current_block)
                .map_err(ClientExecutionError::InvalidBlockPreExecution)?;
        };

        let state = revm_state_with_capacity(
            cache_db,
            current_block.body().transactions.len(),
            input.input.ancestor_headers.len(),
        );
        let block_executor =
            CapacityBlockExecutor::new_with_state(EthEvmConfig::new(spec.clone()), state);
        let executor_output = block_executor.execute(&current_block)?;

        // Validate the block post execution.
        validate_block_post_execution(&current_block, &spec, &executor_output, None, None)
            .map_err(ClientExecutionError::InvalidBlockPostExecution)?;

        // Accumulate the logs bloom.
        let mut logs_bloom = Bloom::default();
        executor_output.receipts.iter().for_each(|r| {
            logs_bloom.accrue_bloom(&r.bloom());
        });

        // Convert the output to an execution outcome.
        let executor_outcome = ExecutionOutcome::new(
            executor_output.state,
            vec![executor_output.result.receipts],
            input.input.current_block.header.number,
            vec![executor_output.result.requests],
        );

        if let Some((account_lookup_order, storage_lookup_order, _, witness_order)) = lookup_orders
        {
            let mut account_order = account_lookup_order.borrow_mut();
            let mut storage_order = storage_lookup_order.borrow_mut();
            let mut witness_order = witness_order.borrow_mut();
            witness_order.push(WitnessAccess::StateTrie);
            for (address, account) in &executor_outcome.bundle.state {
                let hashed_address = keccak256(address);
                witness_order
                    .push(WitnessAccess::ModifiedAccount(hashed_address, account.storage.len()));
                if account.info.is_some() &&
                    !account.storage.is_empty() &&
                    !storage_order.contains(&hashed_address) &&
                    input
                        .state
                        .storage_tries
                        .get(&hashed_address)
                        .is_some_and(|storage_trie| storage_trie.hash() != EMPTY_ROOT_HASH)
                {
                    if !account_order.contains(&hashed_address) {
                        account_order.push(hashed_address);
                        witness_order.push(WitnessAccess::Account(hashed_address));
                    }
                    storage_order.push(hashed_address);
                    witness_order.push(WitnessAccess::StorageTrie(hashed_address));
                }
            }
        }

        drop(witness_db);

        // Verify the state root.
        let state_root = {
            input.state.update_from_bundle_state(&executor_outcome.bundle)?;
            input.state.state_trie.hash()
        };

        if state_root != input.input.current_block.state_root {
            return Err(ClientExecutionError::StateRootMismatch {
                actual: state_root,
                expected: input.input.current_block.state_root,
            });
        }

        // Derive the block header.
        //
        // Note: the receipts root and gas used are verified by `validate_block_post_execution`.
        let mut header = input.input.current_block.header.clone();
        header.parent_hash = input.parent_header().hash_slow();
        header.ommers_hash = input.input.current_block.body.calculate_ommers_root();
        header.state_root = input.input.current_block.state_root;
        header.transactions_root = input.input.current_block.transactions_root;
        header.receipts_root = input.input.current_block.header.receipts_root;
        header.withdrawals_root = input.input.current_block.body.calculate_withdrawals_root();
        header.logs_bloom = logs_bloom;
        header.requests_hash = input.input.current_block.requests_hash;

        Ok(header)
    }
}

fn header_from_known_block(
    mut header: Header,
    ancestor_headers: &[Header],
    ommers_hash: B256,
    state_root: B256,
    transactions_root: B256,
    withdrawals_root: Option<B256>,
    logs_bloom: Bloom,
    requests_hash: Option<B256>,
) -> Header {
    header.parent_hash = ancestor_headers[0].hash_slow();
    header.ommers_hash = ommers_hash;
    header.state_root = state_root;
    header.transactions_root = transactions_root;
    header.withdrawals_root = withdrawals_root;
    header.logs_bloom = logs_bloom;
    header.requests_hash = requests_hash;
    header
}
