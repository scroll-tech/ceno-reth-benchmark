use alloy_consensus::BlockHeader;
use alloy_eip7928::BlockAccessList;
use reth_evm::{
    execute::{BlockExecutionError, BlockExecutor, Executor},
    ConfigureEvm, Database, Evm, OnStateHook,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{NodePrimitives, RecoveredBlock};
use revm::{
    database::{states::bundle_state::BundleRetention, CacheDB, State},
    state::bal::Bal,
};

const MAX_ACCOUNT_CAPACITY: usize = 16_384;
const MAX_CONTRACT_CAPACITY: usize = 4_096;
const MAX_BLOCK_HASH_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapacityHints {
    accounts: usize,
    contracts: usize,
    block_hashes: usize,
}

fn capacity_hints(transaction_count: usize, ancestor_count: usize) -> CapacityHints {
    CapacityHints {
        accounts: transaction_count
            .saturating_mul(4)
            .saturating_add(4_096)
            .min(MAX_ACCOUNT_CAPACITY),
        contracts: transaction_count.saturating_add(256).min(MAX_CONTRACT_CAPACITY),
        block_hashes: ancestor_count.min(MAX_BLOCK_HASH_CAPACITY),
    }
}

/// Builds REVM's state with conservative, bounded capacity hints.
///
/// These are hints only: every map retains its ordinary growth behavior if a
/// block exceeds the estimate.
pub(crate) fn revm_state_with_capacity<DB: revm::DatabaseRef>(
    mut database: CacheDB<DB>,
    transaction_count: usize,
    ancestor_count: usize,
) -> State<CacheDB<DB>> {
    let hints = capacity_hints(transaction_count, ancestor_count);

    database.cache.accounts.reserve(hints.accounts);
    database.cache.contracts.reserve(hints.contracts);
    database.cache.block_hashes.reserve(hints.block_hashes);

    let mut state = State::builder().with_database(database).with_bundle_update().build();
    state.cache.accounts.reserve(hints.accounts);
    state.cache.contracts.reserve(hints.contracts);
    state
        .transition_state
        .as_mut()
        .expect("bundle updates initialize transition state")
        .transitions
        .reserve(hints.accounts);
    state.bundle_state.state.reserve(hints.accounts);
    state.bundle_state.contracts.reserve(hints.contracts);
    state
}

/// Reth's `BasicBlockExecutor` execution sequence with a caller-supplied
/// [`State`]. This is the narrow seam needed to install capacity hints before
/// block execution.
pub(crate) struct CapacityBlockExecutor<F, DB> {
    strategy_factory: F,
    db: State<DB>,
}

impl<F, DB> CapacityBlockExecutor<F, DB> {
    pub(crate) fn new_with_state(strategy_factory: F, db: State<DB>) -> Self {
        Self { strategy_factory, db }
    }
}

impl<F, DB> Executor<DB> for CapacityBlockExecutor<F, DB>
where
    F: ConfigureEvm,
    DB: Database,
{
    type Primitives = F::Primitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;

        let has_bal = block.header().block_access_list_hash().is_some();
        executor.evm_mut().db_mut().bal_state.bal_builder = has_bal.then(Bal::new);

        executor.apply_pre_execution_changes()?;
        if has_bal {
            executor.evm_mut().db_mut().bump_bal_index();
        }

        for tx in block.transactions_recovered() {
            executor.execute_transaction(tx)?;
            if has_bal {
                executor.evm_mut().db_mut().bump_bal_index();
            }
        }

        let result = executor.apply_post_execution_changes()?;
        self.db.merge_transitions(BundleRetention::Reverts);
        Ok(result)
    }

    fn execute_one_with_state_hook<H>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: H,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        H: OnStateHook + 'static,
    {
        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;
        executor.evm_mut().db_mut().set_state_hook(Some(Box::new(state_hook)));

        let result = executor.execute_block(block.transactions_recovered());

        self.db.set_state_hook(None);
        self.db.merge_transitions(BundleRetention::Reverts);
        result
    }

    fn into_state(self) -> State<DB> {
        self.db
    }

    fn size_hint(&self) -> usize {
        self.db.bundle_state.size_hint()
    }

    fn take_bal(&mut self) -> Option<BlockAccessList> {
        self.db.take_built_alloy_bal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_hints_are_bounded_and_overflow_safe() {
        assert_eq!(
            capacity_hints(0, 0),
            CapacityHints { accounts: 4_096, contracts: 256, block_hashes: 0 }
        );
        assert_eq!(
            capacity_hints(896, 3),
            CapacityHints { accounts: 7_680, contracts: 1_152, block_hashes: 3 }
        );
        assert_eq!(
            capacity_hints(usize::MAX, usize::MAX),
            CapacityHints {
                accounts: MAX_ACCOUNT_CAPACITY,
                contracts: MAX_CONTRACT_CAPACITY,
                block_hashes: MAX_BLOCK_HASH_CAPACITY,
            }
        );
    }
}
