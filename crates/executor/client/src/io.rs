use std::{cell::RefCell, iter::once};

use crate::error::ClientExecutionError;
use bumpalo::Bump;
use itertools::Itertools;
use openvm_mpt::{EthereumState, EthereumStateBytes, Mpt};
use reth_evm::execute::ProviderError;
use reth_primitives::{Block, Header, TransactionSigned};
use reth_trie::TrieAccount;
use revm::{
    database::BundleState,
    state::{AccountInfo, Bytecode},
    DatabaseRef,
};
use revm_primitives::{keccak256, map::DefaultHashBuilder, Address, HashMap, B256, U256};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// Bump area size in bytes.
const BUMP_AREA_SIZE: usize = 1000 * 1000;

/// The input for the client to execute a block and fully verify the STF (state transition
/// function).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientExecutorInput {
    /// The current block (which will be executed inside the client).
    #[serde_as(
        as = "reth_primitives_traits::serde_bincode_compat::Block<'_, TransactionSigned, Header>"
    )]
    pub current_block: Block<TransactionSigned, Header>,
    /// The previous block headers starting from the most recent. There must be at least one header
    /// to provide the parent state root.
    #[serde_as(as = "Vec<alloy_consensus::serde_bincode_compat::Header>")]
    pub ancestor_headers: Vec<Header>,
    /// Network state as of the parent block.
    pub parent_state_bytes: EthereumStateBytes,
    /// Account bytecodes.
    pub bytecodes: Vec<Bytecode>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentBlockInput {
    #[serde_as(
        as = "reth_primitives_traits::serde_bincode_compat::Block<'_, TransactionSigned, Header>"
    )]
    pub current_block: Block<TransactionSigned, Header>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AncestorHeadersInput {
    #[serde_as(as = "Vec<alloy_consensus::serde_bincode_compat::Header>")]
    pub ancestor_headers: Vec<Header>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTrieInput {
    pub num_nodes: usize,
    pub bytes: bytes::Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTrieCount {
    pub len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTrieInput {
    pub hashed_address: B256,
    pub num_nodes: usize,
    pub bytes: bytes::Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BytecodesInput {
    pub bytecodes: Vec<Bytecode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BytecodeInput {
    pub bytecode: Bytecode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientInputChunk {
    StorageTrie(StorageTrieInput),
    Bytecode(BytecodeInput),
}

#[derive(Debug, Clone, Copy)]
pub enum WitnessAccess {
    StorageTrie(B256),
    Bytecode(B256),
}

pub trait ChunkedClientInput {
    fn read_ancestor_headers(&mut self) -> AncestorHeadersInput;

    fn read_state_trie(&mut self) -> StateTrieInput;

    fn read_storage_trie_count(&mut self) -> StorageTrieCount;

    fn read_storage_trie(&mut self) -> StorageTrieInput;

    fn read_current_block(&mut self) -> CurrentBlockInput;

    fn read_bytecode(&mut self) -> BytecodeInput;

    fn read_witness_chunk(&mut self) -> ClientInputChunk;
}

impl From<ClientExecutorInput>
    for (
        CurrentBlockInput,
        AncestorHeadersInput,
        StateTrieInput,
        StorageTrieCount,
        Vec<StorageTrieInput>,
        BytecodesInput,
    )
{
    fn from(input: ClientExecutorInput) -> Self {
        let ClientExecutorInput { current_block, ancestor_headers, parent_state_bytes, bytecodes } =
            input;
        let (num_nodes, bytes) = parent_state_bytes.state_trie;
        let storage_tries = parent_state_bytes
            .storage_tries
            .into_iter()
            .map(|(hashed_address, num_nodes, bytes)| StorageTrieInput {
                hashed_address,
                num_nodes,
                bytes,
            })
            .collect::<Vec<_>>();
        let storage_trie_count = StorageTrieCount { len: storage_tries.len() };

        (
            CurrentBlockInput { current_block },
            AncestorHeadersInput { ancestor_headers },
            StateTrieInput { num_nodes, bytes },
            storage_trie_count,
            storage_tries,
            BytecodesInput { bytecodes },
        )
    }
}

#[derive(Debug)]
pub struct PreparedClientExecutorInput {
    pub current_block: Block<TransactionSigned, Header>,
    pub ancestor_headers: Vec<Header>,
    pub state: EthereumState,
    pub bytecodes: Vec<Bytecode>,
}

#[derive(Debug, Clone)]
pub struct ClientExecutorInputWithState {
    pub input: &'static ClientExecutorInput,
    pub state: EthereumState,
}

pub fn build_state_from_trie_inputs(
    ancestor_headers: &[Header],
    state_trie_input: StateTrieInput,
    storage_trie_inputs: impl IntoIterator<Item = StorageTrieInput>,
) -> Result<EthereumState, ClientExecutionError> {
    let bump = Box::leak(Box::new(Bump::with_capacity(BUMP_AREA_SIZE)));

    let state_trie_input = Box::leak(Box::new(state_trie_input));
    let mut state_bytes = state_trie_input.bytes.as_ref();
    let state_trie = Mpt::decode_trie(bump, &mut state_bytes, state_trie_input.num_nodes)?;
    if state_trie.hash() != ancestor_headers[0].state_root {
        return Err(ClientExecutionError::ParentStateRootMismatch {
            actual: state_trie.hash(),
            expected: ancestor_headers[0].state_root,
        });
    }

    let storage_trie_inputs = storage_trie_inputs.into_iter();
    let (lower_bound, _) = storage_trie_inputs.size_hint();
    let mut storage_tries =
        HashMap::with_capacity_and_hasher(lower_bound, DefaultHashBuilder::default());

    for storage_trie_input in storage_trie_inputs {
        let storage_trie_input = Box::leak(Box::new(storage_trie_input));
        let account_in_trie =
            state_trie.get_rlp::<TrieAccount>(storage_trie_input.hashed_address.as_slice())?;
        let expected_storage_root =
            account_in_trie.map_or(reth_trie::EMPTY_ROOT_HASH, |a| a.storage_root);

        let mut storage_trie_bytes = storage_trie_input.bytes.as_ref();
        let storage_trie =
            Mpt::decode_trie(bump, &mut storage_trie_bytes, storage_trie_input.num_nodes)?;
        if storage_trie.hash() != expected_storage_root {
            return Err(ClientExecutionError::ParentStorageRootMismatch {
                hashed_account: storage_trie_input.hashed_address,
                actual: storage_trie.hash(),
                expected: expected_storage_root,
            });
        }

        storage_tries.insert(storage_trie_input.hashed_address, storage_trie);
    }

    Ok(EthereumState { state_trie, storage_tries, bump })
}

pub fn build_state_from_chunked_input(
    ancestor_headers: &[Header],
    input: &mut impl ChunkedClientInput,
) -> Result<EthereumState, ClientExecutionError> {
    let state_trie_input = input.read_state_trie();
    let storage_trie_count = input.read_storage_trie_count();
    let storage_trie_inputs = (0..storage_trie_count.len).map(|_| input.read_storage_trie());

    build_state_from_trie_inputs(ancestor_headers, state_trie_input, storage_trie_inputs)
}

pub struct StreamingEthereumState<'a> {
    pub state_trie: Mpt<'static>,
    pub storage_tries: RefCell<HashMap<B256, Mpt<'static>>>,
    pub bump: &'static Bump,
    input: RefCell<&'a mut dyn ChunkedClientInput>,
}

impl core::fmt::Debug for StreamingEthereumState<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamingEthereumState")
            .field("state_trie", &self.state_trie)
            .field("storage_tries", &self.storage_tries)
            .field("bump", &self.bump)
            .finish_non_exhaustive()
    }
}

pub fn build_streaming_state_from_chunked_input<'a>(
    ancestor_headers: &[Header],
    input: &'a mut dyn ChunkedClientInput,
) -> Result<StreamingEthereumState<'a>, ClientExecutionError> {
    let bump = Box::leak(Box::new(Bump::with_capacity(BUMP_AREA_SIZE)));

    let state_trie_input = Box::leak(Box::new(input.read_state_trie()));
    let mut state_bytes = state_trie_input.bytes.as_ref();
    let state_trie = Mpt::decode_trie(bump, &mut state_bytes, state_trie_input.num_nodes)?;
    if state_trie.hash() != ancestor_headers[0].state_root {
        return Err(ClientExecutionError::ParentStateRootMismatch {
            actual: state_trie.hash(),
            expected: ancestor_headers[0].state_root,
        });
    }

    Ok(StreamingEthereumState {
        state_trie,
        storage_tries: RefCell::new(HashMap::with_capacity_and_hasher(
            1,
            DefaultHashBuilder::default(),
        )),
        bump,
        input: RefCell::new(input),
    })
}

impl ClientExecutorInputWithState {
    /// Parses `input.parent_state_bytes` into `EthereumState` and verifies state and storage roots.
    pub fn build(input: ClientExecutorInput) -> Result<Self, ClientExecutionError> {
        let input = Box::leak(Box::new(input));
        let bump = Box::leak(Box::new(Bump::with_capacity(BUMP_AREA_SIZE)));

        let state = {
            let (state_num_nodes, state_bytes) = &input.parent_state_bytes.state_trie;
            let state_trie = Mpt::decode_trie(bump, &mut state_bytes.as_ref(), *state_num_nodes)?;
            if state_trie.hash() != input.ancestor_headers[0].state_root {
                return Err(ClientExecutionError::ParentStateRootMismatch {
                    actual: state_trie.hash(),
                    expected: input.ancestor_headers[0].state_root,
                });
            }

            let mut storage_tries = HashMap::with_capacity_and_hasher(
                input.parent_state_bytes.storage_tries.len(),
                DefaultHashBuilder::default(),
            );
            for (hashed_address, num_nodes, storage_trie_bytes) in
                &input.parent_state_bytes.storage_tries
            {
                let account_in_trie =
                    state_trie.get_rlp::<TrieAccount>(hashed_address.as_slice())?;
                let expected_storage_root =
                    account_in_trie.map_or(reth_trie::EMPTY_ROOT_HASH, |a| a.storage_root);

                let storage_trie =
                    Mpt::decode_trie(bump, &mut storage_trie_bytes.as_ref(), *num_nodes)?;
                if storage_trie.hash() != expected_storage_root {
                    return Err(ClientExecutionError::ParentStorageRootMismatch {
                        hashed_account: *hashed_address,
                        actual: storage_trie.hash(),
                        expected: expected_storage_root,
                    });
                }

                storage_tries.insert(*hashed_address, storage_trie);
            }

            EthereumState { state_trie, storage_tries, bump }
        };

        Ok(Self { input, state })
    }
}

impl PreparedClientExecutorInput {
    pub fn build(
        current_block_input: CurrentBlockInput,
        ancestor_headers_input: AncestorHeadersInput,
        state_trie_input: StateTrieInput,
        storage_trie_inputs: impl IntoIterator<Item = StorageTrieInput>,
        bytecodes_input: BytecodesInput,
    ) -> Result<Self, ClientExecutionError> {
        let AncestorHeadersInput { ancestor_headers } = ancestor_headers_input;
        let state =
            build_state_from_trie_inputs(&ancestor_headers, state_trie_input, storage_trie_inputs)?;

        Ok(Self {
            current_block: current_block_input.current_block,
            ancestor_headers,
            state,
            bytecodes: bytecodes_input.bytecodes,
        })
    }

    #[inline(always)]
    pub fn parent_header(&self) -> &Header {
        &self.ancestor_headers[0]
    }

    pub fn witness_db(&self) -> Result<WitnessDb<'_, '_>, ClientExecutionError> {
        <Self as WitnessInput>::witness_db(self)
    }
}

impl StreamingEthereumState<'_> {
    fn provider_error(message: impl Into<String>) -> ProviderError {
        ProviderError::TrieWitnessError(message.into())
    }

    fn expected_storage_root(&self, hashed_address: B256) -> Result<B256, ProviderError> {
        let account_in_trie = self
            .state_trie
            .get_rlp::<TrieAccount>(hashed_address.as_slice())
            .map_err(|err| Self::provider_error(err.to_string()))?;
        Ok(account_in_trie.map_or(reth_trie::EMPTY_ROOT_HASH, |account| account.storage_root))
    }

    pub fn load_storage_trie(&self, hashed_address: B256) -> Result<(), ProviderError> {
        if self.storage_tries.borrow().contains_key(&hashed_address) {
            return Ok(());
        }

        let storage_trie_input = match self.input.borrow_mut().read_witness_chunk() {
            ClientInputChunk::StorageTrie(storage_trie_input) => storage_trie_input,
            ClientInputChunk::Bytecode(_) => {
                return Err(Self::provider_error(format!(
                    "expected storage trie for {hashed_address}, got bytecode"
                )));
            }
        };
        if storage_trie_input.hashed_address != hashed_address {
            return Err(Self::provider_error(format!(
                "streamed storage trie hash mismatch: expected {hashed_address}, got {}",
                storage_trie_input.hashed_address
            )));
        }

        let expected_storage_root = self.expected_storage_root(hashed_address)?;
        let storage_trie_input = Box::leak(Box::new(storage_trie_input));
        let mut storage_trie_bytes = storage_trie_input.bytes.as_ref();
        let storage_trie = Mpt::decode_trie(
            self.bump,
            &mut storage_trie_bytes,
            storage_trie_input.num_nodes,
        )
        .map_err(|err| Self::provider_error(err.to_string()))?;
        if storage_trie.hash() != expected_storage_root {
            return Err(Self::provider_error(format!(
                "parent storage root mismatch for {hashed_address}: actual {}, expected {}",
                storage_trie.hash(),
                expected_storage_root
            )));
        }

        self.storage_tries.borrow_mut().insert(hashed_address, storage_trie);
        Ok(())
    }

    pub fn read_bytecode(&self, hash: B256) -> Result<Bytecode, ProviderError> {
        let bytecode = match self.input.borrow_mut().read_witness_chunk() {
            ClientInputChunk::Bytecode(bytecode_input) => bytecode_input.bytecode,
            ClientInputChunk::StorageTrie(storage_trie_input) => {
                return Err(Self::provider_error(format!(
                    "expected bytecode for {hash}, got storage trie {}",
                    storage_trie_input.hashed_address
                )));
            }
        };
        if bytecode.hash_slow() != hash {
            return Err(Self::provider_error(format!(
                "streamed bytecode hash mismatch: expected {hash}, got {}",
                bytecode.hash_slow()
            )));
        }
        Ok(bytecode)
    }

    pub fn update_from_bundle_state(
        &mut self,
        bundle_state: &BundleState,
    ) -> Result<(), ClientExecutionError> {
        for (address, account) in &bundle_state.state {
            let hashed_address = keccak256(address);

            if let Some(info) = &account.info {
                if !account.storage.is_empty()
                    && !self.storage_tries.borrow().contains_key(&hashed_address)
                    && self.expected_storage_root(hashed_address)
                        .map_err(|err| ClientExecutionError::TrieWitnessError(err.to_string()))?
                        != reth_trie::EMPTY_ROOT_HASH
                {
                    self.load_storage_trie(hashed_address)
                        .map_err(|err| ClientExecutionError::TrieWitnessError(err.to_string()))?;
                }

                let mut storage_tries = self.storage_tries.borrow_mut();
                let storage_trie =
                    storage_tries.entry(hashed_address).or_insert(Mpt::new(self.bump));

                if account.status.was_destroyed() {
                    *storage_trie = Mpt::new(self.bump);
                }

                for (slot, value) in &account.storage {
                    let hashed_slot = keccak256(slot.to_be_bytes::<32>());
                    if value.present_value.is_zero() {
                        storage_trie.delete(hashed_slot.as_slice())?;
                    } else {
                        storage_trie.insert_rlp(hashed_slot.as_slice(), value.present_value)?;
                    }
                }
                let storage_root = storage_trie.hash();
                let state_account = TrieAccount {
                    nonce: info.nonce,
                    balance: info.balance,
                    storage_root,
                    code_hash: info.code_hash,
                };
                self.state_trie.insert_rlp(hashed_address.as_slice(), state_account)?;
            } else {
                self.state_trie.delete(hashed_address.as_slice()).unwrap();
                self.storage_tries.borrow_mut().remove(&hashed_address);
            }
        }

        Ok(())
    }
}

impl ClientExecutorInputWithState {
    /// Gets the immediate parent block's header.
    #[inline(always)]
    pub fn parent_header(&self) -> &Header {
        &self.input.ancestor_headers[0]
    }

    /// Creates a [`WitnessDb`].
    pub fn witness_db(&self) -> Result<WitnessDb<'_, '_>, ClientExecutionError> {
        <Self as WitnessInput>::witness_db(self)
    }

    pub fn witness_db_recording<'a>(
        &'a self,
        bytecode_lookup_order: &'a RefCell<Vec<B256>>,
        storage_lookup_order: &'a RefCell<Vec<B256>>,
        witness_order: Option<&'a RefCell<Vec<WitnessAccess>>>,
    ) -> Result<WitnessDb<'a, 'a>, ClientExecutionError> {
        WitnessDb::from_parts_recording(
            &self.state,
            &self.input.current_block.header,
            &self.input.ancestor_headers,
            &self.input.bytecodes,
            bytecode_lookup_order,
            storage_lookup_order,
            witness_order,
        )
    }
}

impl WitnessInput for ClientExecutorInputWithState {
    #[inline(always)]
    fn state(&self) -> &EthereumState {
        &self.state
    }

    #[inline(always)]
    fn state_anchor(&self) -> B256 {
        self.parent_header().state_root
    }

    #[inline(always)]
    fn bytecodes(&self) -> impl Iterator<Item = &Bytecode> {
        self.input.bytecodes.iter()
    }

    #[inline(always)]
    fn headers(&self) -> impl Iterator<Item = &Header> {
        once(&self.input.current_block.header).chain(self.input.ancestor_headers.iter())
    }

    #[inline(always)]
    fn headers_len(&self) -> usize {
        1 + self.input.ancestor_headers.len()
    }
}

impl WitnessInput for PreparedClientExecutorInput {
    #[inline(always)]
    fn state(&self) -> &EthereumState {
        &self.state
    }

    #[inline(always)]
    fn state_anchor(&self) -> B256 {
        self.parent_header().state_root
    }

    #[inline(always)]
    fn bytecodes(&self) -> impl Iterator<Item = &Bytecode> {
        self.bytecodes.iter()
    }

    #[inline(always)]
    fn headers(&self) -> impl Iterator<Item = &Header> {
        once(&self.current_block.header).chain(self.ancestor_headers.iter())
    }

    #[inline(always)]
    fn headers_len(&self) -> usize {
        1 + self.ancestor_headers.len()
    }
}

/// A trait for constructing [`WitnessDb`].
pub trait WitnessInput {
    /// Gets a reference to the state from which account info and storage slots are loaded.
    fn state(&self) -> &EthereumState;

    /// Gets the state trie root hash that the state referenced by
    /// [state()](trait.WitnessInput#tymethod.state) must conform to.
    fn state_anchor(&self) -> B256;

    /// Gets an iterator over account bytecodes.
    fn bytecodes(&self) -> impl Iterator<Item = &Bytecode>;

    /// Gets an iterator over references to a consecutive, reverse-chronological block headers
    /// starting from the current block header.
    fn headers(&self) -> impl Iterator<Item = &Header>;

    /// Gets the number of headers.
    fn headers_len(&self) -> usize;

    /// Creates a [`WitnessDb`] from a [`WitnessInput`] implementation. To do so, it verifies the
    /// state root, ancestor headers and account bytecodes, and constructs the account and
    /// storage values by reading against state tries.
    ///
    /// NOTE: For some unknown reasons, calling this trait method directly from outside of the type
    /// implementing this trait causes a zkVM run to cost over 5M cycles more. To avoid this, define
    /// a method inside the type that calls this trait method instead.
    #[inline(always)]
    fn witness_db(&self) -> Result<WitnessDb<'_, '_>, ClientExecutionError> {
        let state = self.state();

        let bytecode_by_hash =
            self.bytecodes().map(|code| (code.hash_slow(), code)).collect::<HashMap<_, _>>();

        // Verify and build block hashes
        let mut block_hashes: HashMap<u64, B256, _> =
            HashMap::with_capacity_and_hasher(self.headers_len(), DefaultHashBuilder::default());
        for (child_header, parent_header) in self.headers().tuple_windows() {
            if parent_header.number != child_header.number - 1 {
                return Err(ClientExecutionError::NonConsecutiveBlockHeaders {
                    parent_block_number: parent_header.number,
                    child_block_number: child_header.number,
                });
            }

            if parent_header.hash_slow() != child_header.parent_hash {
                return Err(ClientExecutionError::ParentBlockHashMismatch {
                    parent_block_number: parent_header.number,
                    expected: parent_header.hash_slow(),
                    actual: child_header.parent_hash,
                });
            }

            block_hashes.insert(parent_header.number, child_header.parent_hash);
        }

        Ok(WitnessDb {
            state: StateProvider::Eager { state, storage_lookup_order: None, witness_order: None },
            block_hashes,
            bytecodes: BytecodeProvider::Eager {
                bytecode_by_hash,
                lookup_order: None,
                witness_order: None,
            },
        })
    }
}

enum BytecodeProvider<'a> {
    Eager {
        bytecode_by_hash: HashMap<B256, &'a Bytecode>,
        lookup_order: Option<&'a RefCell<Vec<B256>>>,
        witness_order: Option<&'a RefCell<Vec<WitnessAccess>>>,
    },
    Streaming,
}

enum StateProvider<'a, 'input> {
    Eager {
        state: &'a EthereumState,
        storage_lookup_order: Option<&'a RefCell<Vec<B256>>>,
        witness_order: Option<&'a RefCell<Vec<WitnessAccess>>>,
    },
    Streaming(&'a StreamingEthereumState<'input>),
}

pub struct WitnessDb<'a, 'input> {
    state: StateProvider<'a, 'input>,
    block_hashes: HashMap<u64, B256>,
    bytecodes: BytecodeProvider<'a>,
}

impl core::fmt::Debug for WitnessDb<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WitnessDb")
            .field("block_hashes", &self.block_hashes)
            .finish_non_exhaustive()
    }
}

impl<'a> WitnessDb<'a, 'a> {
    pub fn new(
        inner: &'a EthereumState,
        block_hashes: HashMap<u64, B256>,
        bytecode_by_hash: HashMap<B256, &'a Bytecode>,
    ) -> Self {
        Self {
            state: StateProvider::Eager {
                state: inner,
                storage_lookup_order: None,
                witness_order: None,
            },
            block_hashes,
            bytecodes: BytecodeProvider::Eager {
                bytecode_by_hash,
                lookup_order: None,
                witness_order: None,
            },
        }
    }

    pub fn from_parts(
        state: &'a EthereumState,
        current_header: &'a Header,
        ancestor_headers: &'a [Header],
        bytecodes: &'a [Bytecode],
    ) -> Result<Self, ClientExecutionError> {
        let bytecode_by_hash =
            bytecodes.iter().map(|code| (code.hash_slow(), code)).collect::<HashMap<_, _>>();

        let headers = once(current_header).chain(ancestor_headers.iter());
        let mut block_hashes: HashMap<u64, B256, _> = HashMap::with_capacity_and_hasher(
            1 + ancestor_headers.len(),
            DefaultHashBuilder::default(),
        );
        for (child_header, parent_header) in headers.tuple_windows() {
            if parent_header.number != child_header.number - 1 {
                return Err(ClientExecutionError::NonConsecutiveBlockHeaders {
                    parent_block_number: parent_header.number,
                    child_block_number: child_header.number,
                });
            }

            if parent_header.hash_slow() != child_header.parent_hash {
                return Err(ClientExecutionError::ParentBlockHashMismatch {
                    parent_block_number: parent_header.number,
                    expected: parent_header.hash_slow(),
                    actual: child_header.parent_hash,
                });
            }

            block_hashes.insert(parent_header.number, child_header.parent_hash);
        }

        Ok(Self {
            state: StateProvider::Eager { state, storage_lookup_order: None, witness_order: None },
            block_hashes,
            bytecodes: BytecodeProvider::Eager {
                bytecode_by_hash,
                lookup_order: None,
                witness_order: None,
            },
        })
    }

    pub fn from_parts_recording(
        state: &'a EthereumState,
        current_header: &'a Header,
        ancestor_headers: &'a [Header],
        bytecodes: &'a [Bytecode],
        bytecode_lookup_order: &'a RefCell<Vec<B256>>,
        storage_lookup_order: &'a RefCell<Vec<B256>>,
        witness_order: Option<&'a RefCell<Vec<WitnessAccess>>>,
    ) -> Result<Self, ClientExecutionError> {
        let mut witness_db = Self::from_parts(state, current_header, ancestor_headers, bytecodes)?;
        if let BytecodeProvider::Eager {
            lookup_order: order,
            witness_order: bytecode_witness_order,
            ..
        } = &mut witness_db.bytecodes
        {
            *order = Some(bytecode_lookup_order);
            *bytecode_witness_order = witness_order;
        }
        if let StateProvider::Eager {
            storage_lookup_order: order,
            witness_order: storage_witness_order,
            ..
        } = &mut witness_db.state
        {
            *order = Some(storage_lookup_order);
            *storage_witness_order = witness_order;
        }
        Ok(witness_db)
    }
}

impl<'a, 'input> WitnessDb<'a, 'input> {
    pub fn from_streaming_parts(
        state: &'a StreamingEthereumState<'input>,
        current_header: &'a Header,
        ancestor_headers: &'a [Header],
    ) -> Result<WitnessDb<'a, 'input>, ClientExecutionError> {
        let headers = once(current_header).chain(ancestor_headers.iter());
        let mut block_hashes: HashMap<u64, B256, _> = HashMap::with_capacity_and_hasher(
            1 + ancestor_headers.len(),
            DefaultHashBuilder::default(),
        );
        for (child_header, parent_header) in headers.tuple_windows() {
            if parent_header.number != child_header.number - 1 {
                return Err(ClientExecutionError::NonConsecutiveBlockHeaders {
                    parent_block_number: parent_header.number,
                    child_block_number: child_header.number,
                });
            }

            if parent_header.hash_slow() != child_header.parent_hash {
                return Err(ClientExecutionError::ParentBlockHashMismatch {
                    parent_block_number: parent_header.number,
                    expected: parent_header.hash_slow(),
                    actual: child_header.parent_hash,
                });
            }

            block_hashes.insert(parent_header.number, child_header.parent_hash);
        }

        Ok(Self {
            state: StateProvider::Streaming(state),
            block_hashes,
            bytecodes: BytecodeProvider::Streaming,
        })
    }
}

impl DatabaseRef for WitnessDb<'_, '_> {
    /// The database error type.
    type Error = ProviderError;

    /// Get basic account information.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let hashed_address = keccak256(address);

        let account_in_trie = match &self.state {
            StateProvider::Eager { state, .. } => state
                .state_trie
                .get_rlp::<TrieAccount>(hashed_address.as_slice())
                .unwrap(),
            StateProvider::Streaming(state) => state
                .state_trie
                .get_rlp::<TrieAccount>(hashed_address.as_slice())
                .unwrap(),
        };

        let account = account_in_trie.map(|account_in_trie| AccountInfo {
            balance: account_in_trie.balance,
            nonce: account_in_trie.nonce,
            code_hash: account_in_trie.code_hash,
            code: None,
        });

        Ok(account)
    }

    /// Get account code by its hash.
    fn code_by_hash_ref(&self, hash: B256) -> Result<Bytecode, Self::Error> {
        match &self.bytecodes {
            BytecodeProvider::Eager { bytecode_by_hash, lookup_order, witness_order } => {
                if let Some(lookup_order) = lookup_order {
                    lookup_order.borrow_mut().push(hash);
                }
                if let Some(witness_order) = witness_order {
                    witness_order.borrow_mut().push(WitnessAccess::Bytecode(hash));
                }
                // Cloning here is fine as `Bytes` is cheap to clone.
                Ok(bytecode_by_hash.get(&hash).map(|code| (*code).clone()).unwrap())
            }
            BytecodeProvider::Streaming => match &self.state {
                StateProvider::Streaming(state) => state.read_bytecode(hash),
                StateProvider::Eager { .. } => Err(ProviderError::TrieWitnessError(
                    "streaming bytecode requested for eager state".to_string(),
                )),
            }
        }
    }

    /// Get storage value of address at index.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let hashed_address = keccak256(address);

        let hashed_slot = keccak256(index.to_be_bytes::<32>());
        match &self.state {
            StateProvider::Eager { state, storage_lookup_order, witness_order } => {
                let mut first_lookup = false;
                if let Some(storage_lookup_order) = storage_lookup_order {
                    let mut order = storage_lookup_order.borrow_mut();
                    if !order.contains(&hashed_address) {
                        order.push(hashed_address);
                        first_lookup = true;
                    }
                }
                if first_lookup {
                    if let Some(witness_order) = witness_order {
                        witness_order.borrow_mut().push(WitnessAccess::StorageTrie(hashed_address));
                    }
                }
                let storage_trie = state
                    .storage_tries
                    .get(&hashed_address)
                    .expect("A storage trie must be provided for each account");
                Ok(storage_trie
                    .get_rlp::<U256>(hashed_slot.as_slice())
                    .expect("Can get from MPT")
                    .unwrap_or_default())
            }
            StateProvider::Streaming(state) => {
                state.load_storage_trie(hashed_address)?;
                let storage_tries = state.storage_tries.borrow();
                let storage_trie = storage_tries
                    .get(&hashed_address)
                    .expect("streaming storage trie was loaded");
                Ok(storage_trie
                    .get_rlp::<U256>(hashed_slot.as_slice())
                    .expect("Can get from MPT")
                    .unwrap_or_default())
            }
        }
    }

    /// Get block hash by block number.
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        Ok(*self
            .block_hashes
            .get(&number)
            .expect("A block hash must be provided for each block number"))
    }
}
