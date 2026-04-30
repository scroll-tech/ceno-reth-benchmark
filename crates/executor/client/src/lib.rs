pub mod error;
/// Client program input data types.
pub mod io;

use std::{cell::RefCell, fmt::Debug, sync::Arc};

use alloy_consensus::TxReceipt;
use alloy_primitives::{keccak256, Bloom, B256};
use openvm_primitives::chain_spec::{dev, mainnet};
use reth_consensus::{Consensus, HeaderValidator};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_evm::execute::{BasicBlockExecutor, Executor};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::Header;
use reth_primitives_traits::block::Block as _;
use reth_trie::EMPTY_ROOT_HASH;
use reth_revm::db::CacheDB;

use crate::{
    error::ClientExecutionError,
    io::{
        AncestorHeadersInput, BytecodesInput, ChunkedClientInput, ClientExecutorInput,
        ClientExecutorInputWithState, CurrentBlockInput, PreparedClientExecutorInput,
        StateTrieInput, StorageTrieInput, WitnessAccess,
    },
};

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

    pub fn execute_recording_bytecodes(
        &self,
        chain_variant: ChainVariant,
        pre_input: ClientExecutorInput,
    ) -> Result<(Header, Vec<B256>), ClientExecutionError> {
        let (header, _, bytecode_lookup_order, _) =
            self.execute_recording_witness_chunks(chain_variant, pre_input)?;
        Ok((header, bytecode_lookup_order))
    }

    pub fn execute_recording_witness_chunks(
        &self,
        chain_variant: ChainVariant,
        pre_input: ClientExecutorInput,
    ) -> Result<(Header, Vec<B256>, Vec<B256>, Vec<WitnessAccess>), ClientExecutionError> {
        let input = ClientExecutorInputWithState::build(pre_input)?;
        let storage_lookup_order = RefCell::new(Vec::new());
        let bytecode_lookup_order = RefCell::new(Vec::new());
        let witness_order = RefCell::new(Vec::new());
        let header = self.execute_with_state(
            chain_variant,
            input,
            Some((&storage_lookup_order, &bytecode_lookup_order, &witness_order)),
        )?;
        Ok((
            header,
            storage_lookup_order.into_inner(),
            bytecode_lookup_order.into_inner(),
            witness_order.into_inner(),
        ))
    }

    pub fn execute_chunked(
        &self,
        chain_variant: ChainVariant,
        current_block_input: CurrentBlockInput,
        ancestor_headers_input: AncestorHeadersInput,
        state_trie_input: StateTrieInput,
        storage_trie_inputs: impl IntoIterator<Item = StorageTrieInput>,
        bytecodes_input: BytecodesInput,
    ) -> Result<Header, ClientExecutionError> {
        let input = PreparedClientExecutorInput::build(
            current_block_input,
            ancestor_headers_input,
            state_trie_input,
            storage_trie_inputs,
            bytecodes_input,
        )?;
        self.execute_prepared(chain_variant, input)
    }

    pub fn execute_chunked_from_reader(
        &self,
        chain_variant: ChainVariant,
        input: &mut impl ChunkedClientInput,
    ) -> Result<Header, ClientExecutionError> {
        let AncestorHeadersInput { ancestor_headers } = input.read_ancestor_headers();
        let current_block = input.read_current_block().current_block;
        let mut state = io::build_streaming_state_from_chunked_input(&ancestor_headers, input)?;

        let witness_db = io::WitnessDb::from_streaming_parts(
            &state,
            &current_block.header,
            &ancestor_headers,
        )?;
        let cache_db = CacheDB::new(&witness_db);

        let spec = Arc::new(match chain_variant {
            ChainVariant::Mainnet => mainnet(),
            ChainVariant::Dev => dev(),
        });
        let recovered_block = current_block
            .clone()
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

        let block_executor = BasicBlockExecutor::new(EthEvmConfig::new(spec.clone()), cache_db);
        let executor_output = block_executor.execute(&recovered_block)?;

        validate_block_post_execution(
            &recovered_block,
            &spec,
            &executor_output.receipts,
            &executor_output.requests,
        )
        .map_err(ClientExecutionError::InvalidBlockPostExecution)?;

        let mut logs_bloom = Bloom::default();
        executor_output.receipts.iter().for_each(|r| {
            logs_bloom.accrue_bloom(&r.bloom());
        });

        let executor_outcome = ExecutionOutcome::new(
            executor_output.state,
            vec![executor_output.result.receipts],
            current_block.header.number,
            vec![executor_output.result.requests],
        );

        drop(witness_db);

        let state_root = {
            state.update_from_bundle_state(&executor_outcome.bundle)?;
            state.state_trie.hash()
        };

        if state_root != current_block.state_root {
            return Err(ClientExecutionError::StateRootMismatch {
                actual: state_root,
                expected: current_block.state_root,
            });
        }

        let mut header = current_block.header.clone();
        header.parent_hash = ancestor_headers[0].hash_slow();
        header.ommers_hash = current_block.body.calculate_ommers_root();
        header.state_root = current_block.state_root;
        header.transactions_root = current_block.transactions_root;
        header.receipts_root = current_block.header.receipts_root;
        header.withdrawals_root = current_block.body.calculate_withdrawals_root();
        header.logs_bloom = logs_bloom;
        header.requests_hash = current_block.requests_hash;

        Ok(header)
    }

    fn execute_with_state(
        &self,
        chain_variant: ChainVariant,
        mut input: ClientExecutorInputWithState,
        lookup_orders: Option<(
            &RefCell<Vec<B256>>,
            &RefCell<Vec<B256>>,
            &RefCell<Vec<WitnessAccess>>,
        )>,
    ) -> Result<Header, ClientExecutionError> {
        // Initialize the witnessed database with verified storage proofs.
        let witness_db = match lookup_orders {
            Some((storage_lookup_order, bytecode_lookup_order, witness_order)) => {
                input.witness_db_recording(
                    bytecode_lookup_order,
                    storage_lookup_order,
                    Some(witness_order),
                )?
            }
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

        let block_executor = BasicBlockExecutor::new(EthEvmConfig::new(spec.clone()), cache_db);
        let executor_output = block_executor.execute(&current_block)?;

        // Validate the block post execution.
        validate_block_post_execution(
            &current_block,
            &spec,
            &executor_output.receipts,
            &executor_output.requests,
        )
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

        if let Some((storage_lookup_order, _, witness_order)) = lookup_orders {
            let mut storage_order = storage_lookup_order.borrow_mut();
            let mut witness_order = witness_order.borrow_mut();
            for (address, account) in &executor_outcome.bundle.state {
                let hashed_address = keccak256(address);
                if account.info.is_some()
                    && !account.storage.is_empty()
                    && !storage_order.contains(&hashed_address)
                    && input
                        .state
                        .storage_tries
                        .get(&hashed_address)
                        .map_or(false, |storage_trie| storage_trie.hash() != EMPTY_ROOT_HASH)
                {
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

    fn execute_prepared(
        &self,
        chain_variant: ChainVariant,
        mut input: PreparedClientExecutorInput,
    ) -> Result<Header, ClientExecutionError> {
        let witness_db = input.witness_db()?;
        let cache_db = CacheDB::new(&witness_db);

        let spec = Arc::new(match chain_variant {
            ChainVariant::Mainnet => mainnet(),
            ChainVariant::Dev => dev(),
        });
        let current_block = input
            .current_block
            .clone()
            .try_into_recovered()
            .map_err(|err| ClientExecutionError::BlockSenderRecoveryError(err.into()))?;

        {
            let consensus = EthBeaconConsensus::new(spec.clone());

            consensus
                .validate_header(current_block.sealed_header())
                .map_err(ClientExecutionError::InvalidHeader)?;

            consensus
                .validate_block_pre_execution(&current_block)
                .map_err(ClientExecutionError::InvalidBlockPreExecution)?;
        };

        let block_executor = BasicBlockExecutor::new(EthEvmConfig::new(spec.clone()), cache_db);
        let executor_output = block_executor.execute(&current_block)?;

        validate_block_post_execution(
            &current_block,
            &spec,
            &executor_output.receipts,
            &executor_output.requests,
        )
        .map_err(ClientExecutionError::InvalidBlockPostExecution)?;

        let mut logs_bloom = Bloom::default();
        executor_output.receipts.iter().for_each(|r| {
            logs_bloom.accrue_bloom(&r.bloom());
        });

        let executor_outcome = ExecutionOutcome::new(
            executor_output.state,
            vec![executor_output.result.receipts],
            input.current_block.header.number,
            vec![executor_output.result.requests],
        );

        drop(witness_db);

        let state_root = {
            input.state.update_from_bundle_state(&executor_outcome.bundle)?;
            input.state.state_trie.hash()
        };

        if state_root != input.current_block.state_root {
            return Err(ClientExecutionError::StateRootMismatch {
                actual: state_root,
                expected: input.current_block.state_root,
            });
        }

        let mut header = input.current_block.header.clone();
        header.parent_hash = input.parent_header().hash_slow();
        header.ommers_hash = input.current_block.body.calculate_ommers_root();
        header.state_root = input.current_block.state_root;
        header.transactions_root = input.current_block.transactions_root;
        header.receipts_root = input.current_block.header.receipts_root;
        header.withdrawals_root = input.current_block.body.calculate_withdrawals_root();
        header.logs_bloom = logs_bloom;
        header.requests_hash = input.current_block.requests_hash;

        Ok(header)
    }
}
