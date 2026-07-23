use std::{cell::RefCell, iter::once};

use crate::{error::ClientExecutionError, trim};
use alloy_consensus::Header;
use alloy_rlp::{Decodable, Encodable};
use alloy_trie::{TrieAccount, EMPTY_ROOT_HASH};
use bumpalo::Bump;
use itertools::Itertools;
use openvm_mpt::{EthereumState, EthereumStateBytes, Mpt};
use reth_ethereum_primitives::Block;
use reth_evm::execute::ProviderError;
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

/// Validates a chain of consecutive headers and builds a block-number → hash map.
///
/// `headers` must be in reverse-chronological order (current block first, then ancestors).
fn build_block_hashes<'a>(
    headers: impl Iterator<Item = &'a Header>,
    capacity: usize,
) -> Result<HashMap<u64, B256>, ClientExecutionError> {
    let mut block_hashes =
        HashMap::with_capacity_and_hasher(capacity, DefaultHashBuilder::default());
    for (child, parent) in headers.tuple_windows() {
        if parent.number != child.number - 1 {
            return Err(ClientExecutionError::NonConsecutiveBlockHeaders {
                parent_block_number: parent.number,
                child_block_number: child.number,
            });
        }
        if parent.hash_slow() != child.parent_hash {
            return Err(ClientExecutionError::ParentBlockHashMismatch {
                parent_block_number: parent.number,
                expected: parent.hash_slow(),
                actual: child.parent_hash,
            });
        }
        block_hashes.insert(parent.number, child.parent_hash);
    }
    Ok(block_hashes)
}

/// The input for the client to execute a block and fully verify the STF (state transition
/// function).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientExecutorInput {
    /// The current block (which will be executed inside the client).
    #[serde_as(as = "serde_bincode_compat::Block")]
    pub current_block: Block,
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
    #[serde_as(as = "serde_bincode_compat::Block")]
    pub current_block: Block,
}

mod serde_bincode_compat {
    use super::*;
    use serde::{de::Error as _, Deserializer, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible block serde using canonical RLP bytes.
    pub(super) struct Block;

    impl SerializeAs<super::Block> for Block {
        fn serialize_as<S>(source: &super::Block, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut bytes = Vec::with_capacity(source.length());
            source.encode(&mut bytes);
            bytes.serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::Block> for Block {
        fn deserialize_as<D>(deserializer: D) -> Result<super::Block, D::Error>
        where
            D: Deserializer<'de>,
        {
            let bytes = Vec::<u8>::deserialize(deserializer)?;
            let mut buf = bytes.as_slice();
            let block = super::Block::decode(&mut buf).map_err(D::Error::custom)?;
            if !buf.is_empty() {
                return Err(D::Error::custom("trailing bytes in RLP block"));
            }
            Ok(block)
        }
    }
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
pub struct StateTrieHeader {
    pub num_nodes: usize,
    pub post_update_witness_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTrieInput {
    pub hashed_address: B256,
    pub num_nodes: usize,
    pub bytes: bytes::Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTrieHeader {
    pub hashed_address: B256,
    pub num_nodes: usize,
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
pub struct AccountInput {
    pub hashed_address: B256,
    pub account: Option<TrieAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientWitnessInput {
    Account(AccountInput),
    StorageTrie(StorageTrieHeader),
    Bytecode(BytecodeInput),
}

#[derive(Debug, Clone, Copy)]
pub enum WitnessAccess {
    Account(B256),
    StorageTrie(B256),
    Bytecode(B256),
    StateTrie,
}

pub type WitnessDbLookupOrders<'a> = (
    &'a RefCell<Vec<B256>>,
    &'a RefCell<Vec<B256>>,
    &'a RefCell<Vec<B256>>,
    Option<&'a RefCell<Vec<WitnessAccess>>>,
);

pub trait ClientInputReader {
    fn read_ancestor_headers(&mut self) -> AncestorHeadersInput;

    fn read_state_trie_header(&mut self) -> StateTrieHeader;

    fn read_current_block(&mut self) -> CurrentBlockInput;

    fn read_witness_input(&mut self) -> ClientWitnessInput;

    fn read_raw_bytes(&mut self) -> &'static [u8];
}

impl From<ClientExecutorInput>
    for (
        CurrentBlockInput,
        AncestorHeadersInput,
        StateTrieInput,
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

        (
            CurrentBlockInput { current_block },
            AncestorHeadersInput { ancestor_headers },
            StateTrieInput { num_nodes, bytes },
            storage_tries,
            BytecodesInput { bytecodes },
        )
    }
}

#[derive(Debug, Clone)]
pub struct ClientExecutorInputWithState {
    pub input: &'static ClientExecutorInput,
    pub state: EthereumState,
}

pub struct StreamingEthereumState<'a> {
    state_trie_header: StateTrieHeader,
    parent_state_root: B256,
    pub state_trie: Option<Mpt<'static>>,
    account_cache: RefCell<HashMap<B256, Option<TrieAccount>>>,
    pub storage_tries: RefCell<HashMap<B256, Mpt<'static>>>,
    pub bump: &'static Bump,
    input: RefCell<&'a mut dyn ClientInputReader>,
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

pub fn build_streaming_state_from_input_reader<'a>(
    ancestor_headers: &[Header],
    input: &'a mut dyn ClientInputReader,
) -> Result<StreamingEthereumState<'a>, ClientExecutionError> {
    let bump = Box::leak(Box::new(Bump::with_capacity(BUMP_AREA_SIZE)));

    let state_trie_header = input.read_state_trie_header();

    Ok(StreamingEthereumState {
        state_trie_header,
        parent_state_root: ancestor_headers[0].state_root,
        state_trie: None,
        account_cache: RefCell::new(HashMap::with_capacity_and_hasher(
            1,
            DefaultHashBuilder::default(),
        )),
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
                    account_in_trie.map_or(EMPTY_ROOT_HASH, |a| a.storage_root);

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

impl StreamingEthereumState<'_> {
    fn provider_error(message: impl Into<String>) -> ProviderError {
        ProviderError::TrieWitnessError(message.into())
    }

    fn ensure_state_trie_loaded(&mut self) -> Result<(), ClientExecutionError> {
        if self.state_trie.is_some() {
            return Ok(());
        }

        let mut input = self.input.borrow_mut();
        let mut state_bytes = input.read_raw_bytes();
        let state_trie =
            Mpt::decode_trie(self.bump, &mut state_bytes, self.state_trie_header.num_nodes)?;
        if state_trie.hash() != self.parent_state_root {
            return Err(ClientExecutionError::ParentStateRootMismatch {
                actual: state_trie.hash(),
                expected: self.parent_state_root,
            });
        }
        self.state_trie = Some(state_trie);
        Ok(())
    }

    pub fn state_root(&self) -> B256 {
        self.state_trie.as_ref().expect("state trie must be loaded before root calculation").hash()
    }

    fn validate_account_cache(&self) -> Result<(), ClientExecutionError> {
        let state_trie = self.state_trie.as_ref().expect("state trie was loaded");
        for (hashed_address, streamed_account) in self.account_cache.borrow().iter() {
            let trie_account = state_trie.get_rlp::<TrieAccount>(hashed_address.as_slice())?;
            if &trie_account != streamed_account {
                return Err(ClientExecutionError::TrieWitnessError(format!(
                    "streamed account mismatch for {hashed_address}: streamed {streamed_account:?}, trie {trie_account:?}"
                )));
            }
        }
        Ok(())
    }

    fn account_from_state_trie(
        &self,
        hashed_address: B256,
    ) -> Result<Option<TrieAccount>, ProviderError> {
        self.state_trie
            .as_ref()
            .expect("state trie was loaded")
            .get_rlp::<TrieAccount>(hashed_address.as_slice())
            .map_err(|err| Self::provider_error(err.to_string()))
    }

    fn expected_storage_root_from_state_trie(
        &self,
        hashed_address: B256,
    ) -> Result<B256, ProviderError> {
        Ok(self
            .account_from_state_trie(hashed_address)?
            .map_or(EMPTY_ROOT_HASH, |account| account.storage_root))
    }

    pub fn read_account(&self, hashed_address: B256) -> Result<Option<TrieAccount>, ProviderError> {
        if let Some(account) = self.account_cache.borrow().get(&hashed_address) {
            return Ok(*account);
        }

        loop {
            let account_input = match self.input.borrow_mut().read_witness_input() {
                ClientWitnessInput::Account(account_input) => account_input,
                ClientWitnessInput::StorageTrie(storage_trie_input) => {
                    return Err(Self::provider_error(format!(
                        "expected account for {hashed_address}, got storage trie {}",
                        storage_trie_input.hashed_address
                    )));
                }
                ClientWitnessInput::Bytecode(_) => {
                    return Err(Self::provider_error(format!(
                        "expected account for {hashed_address}, got bytecode"
                    )));
                }
            };

            let streamed_address = account_input.hashed_address;
            if self.account_cache.borrow().contains_key(&streamed_address) {
                return Err(Self::provider_error(format!(
                    "duplicate streamed account {streamed_address}"
                )));
            }

            let streamed_account = account_input.account;
            self.account_cache.borrow_mut().insert(streamed_address, streamed_account);
            if streamed_address == hashed_address {
                return Ok(streamed_account);
            }
        }
    }

    fn expected_storage_root(&self, hashed_address: B256) -> Result<B256, ProviderError> {
        let account_in_trie = self.read_account(hashed_address)?;
        Ok(account_in_trie.map_or(EMPTY_ROOT_HASH, |account| account.storage_root))
    }

    fn cache_post_update_account(&self, account_input: AccountInput) -> Result<(), ProviderError> {
        let hashed_address = account_input.hashed_address;
        if self.account_cache.borrow().contains_key(&hashed_address) {
            return Err(Self::provider_error(format!(
                "duplicate streamed post-update account {hashed_address}"
            )));
        }

        let trie_account = self.account_from_state_trie(hashed_address)?;
        if trie_account != account_input.account {
            return Err(Self::provider_error(format!(
                "streamed post-update account mismatch for {hashed_address}: streamed {:?}, trie {:?}",
                account_input.account, trie_account
            )));
        }

        self.account_cache.borrow_mut().insert(hashed_address, account_input.account);
        Ok(())
    }

    fn cache_post_update_storage_trie(
        &self,
        storage_trie_header: StorageTrieHeader,
    ) -> Result<(), ProviderError> {
        let hashed_address = storage_trie_header.hashed_address;
        if self.storage_tries.borrow().contains_key(&hashed_address) {
            return Err(Self::provider_error(format!(
                "duplicate streamed post-update storage trie {hashed_address}"
            )));
        }

        let expected_storage_root = self.expected_storage_root_from_state_trie(hashed_address)?;
        let mut storage_trie_bytes = self.input.borrow_mut().read_raw_bytes();
        let storage_trie =
            Mpt::decode_trie(self.bump, &mut storage_trie_bytes, storage_trie_header.num_nodes)
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

    fn materialize_post_update_witnesses(&self) -> Result<(), ClientExecutionError> {
        if trim::enabled("skip_post_update_witnesses") {
            return Ok(());
        }

        for _ in 0..self.state_trie_header.post_update_witness_count {
            match self.input.borrow_mut().read_witness_input() {
                ClientWitnessInput::Account(account_input) => self
                    .cache_post_update_account(account_input)
                    .map_err(|err| ClientExecutionError::TrieWitnessError(err.to_string()))?,
                ClientWitnessInput::StorageTrie(storage_trie_header) => self
                    .cache_post_update_storage_trie(storage_trie_header)
                    .map_err(|err| ClientExecutionError::TrieWitnessError(err.to_string()))?,
                ClientWitnessInput::Bytecode(_) => {
                    return Err(ClientExecutionError::TrieWitnessError(
                        "expected post-update account or storage trie, got bytecode".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn load_storage_trie(&self, hashed_address: B256) -> Result<(), ProviderError> {
        if self.storage_tries.borrow().contains_key(&hashed_address) {
            return Ok(());
        }

        if trim::enabled("skip_storage_trie_decode") {
            self.storage_tries.borrow_mut().insert(hashed_address, Mpt::new(self.bump));
            return Ok(());
        }

        let storage_trie_header = match self.input.borrow_mut().read_witness_input() {
            ClientWitnessInput::StorageTrie(storage_trie_header) => storage_trie_header,
            ClientWitnessInput::Account(account_input) => {
                return Err(Self::provider_error(format!(
                    "expected storage trie for {hashed_address}, got account {}",
                    account_input.hashed_address
                )));
            }
            ClientWitnessInput::Bytecode(_) => {
                return Err(Self::provider_error(format!(
                    "expected storage trie for {hashed_address}, got bytecode"
                )));
            }
        };
        if storage_trie_header.hashed_address != hashed_address {
            return Err(Self::provider_error(format!(
                "streamed storage trie hash mismatch: expected {hashed_address}, got {}",
                storage_trie_header.hashed_address
            )));
        }

        let expected_storage_root = self.expected_storage_root(hashed_address)?;
        let mut storage_trie_bytes = self.input.borrow_mut().read_raw_bytes();
        let storage_trie =
            Mpt::decode_trie(self.bump, &mut storage_trie_bytes, storage_trie_header.num_nodes)
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
        let bytecode = match self.input.borrow_mut().read_witness_input() {
            ClientWitnessInput::Bytecode(bytecode_input) => bytecode_input.bytecode,
            ClientWitnessInput::Account(account_input) => {
                return Err(Self::provider_error(format!(
                    "expected bytecode for {hash}, got account {}",
                    account_input.hashed_address
                )));
            }
            ClientWitnessInput::StorageTrie(storage_trie_input) => {
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
        self.ensure_state_trie_loaded()?;
        self.validate_account_cache()?;
        self.materialize_post_update_witnesses()?;

        for (address, account) in &bundle_state.state {
            let hashed_address = keccak256(address);

            if let Some(info) = &account.info {
                let storage_root = if account.status.was_destroyed() || !account.storage.is_empty()
                {
                    if !self.storage_tries.borrow().contains_key(&hashed_address) &&
                        self.expected_storage_root_from_state_trie(hashed_address).map_err(
                            |err| ClientExecutionError::TrieWitnessError(err.to_string()),
                        )? != EMPTY_ROOT_HASH &&
                        !account.status.was_destroyed()
                    {
                        return Err(ClientExecutionError::TrieWitnessError(format!(
                            "missing materialized post-update storage trie for {hashed_address}"
                        )));
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
                    storage_trie.hash()
                } else {
                    self.expected_storage_root_from_state_trie(hashed_address)
                        .map_err(|err| ClientExecutionError::TrieWitnessError(err.to_string()))?
                };
                let state_account = TrieAccount {
                    nonce: info.nonce,
                    balance: info.balance,
                    storage_root,
                    code_hash: info.code_hash,
                };
                self.state_trie
                    .as_mut()
                    .expect("state trie was loaded")
                    .insert_rlp(hashed_address.as_slice(), state_account)?;
            } else {
                self.state_trie
                    .as_mut()
                    .expect("state trie was loaded")
                    .delete(hashed_address.as_slice())
                    .unwrap();
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
        account_lookup_order: &'a RefCell<Vec<B256>>,
        bytecode_lookup_order: &'a RefCell<Vec<B256>>,
        storage_lookup_order: &'a RefCell<Vec<B256>>,
        witness_order: Option<&'a RefCell<Vec<WitnessAccess>>>,
    ) -> Result<WitnessDb<'a, 'a>, ClientExecutionError> {
        WitnessDb::from_parts_recording(
            &self.state,
            &self.input.current_block.header,
            &self.input.ancestor_headers,
            &self.input.bytecodes,
            (account_lookup_order, bytecode_lookup_order, storage_lookup_order, witness_order),
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

        let block_hashes = build_block_hashes(self.headers(), self.headers_len())?;

        Ok(WitnessDb {
            state: StateProvider::Eager {
                state,
                account_lookup_order: None,
                storage_lookup_order: None,
                witness_order: None,
            },
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
        account_lookup_order: Option<&'a RefCell<Vec<B256>>>,
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
                account_lookup_order: None,
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

        let block_hashes = build_block_hashes(
            once(current_header).chain(ancestor_headers.iter()),
            1 + ancestor_headers.len(),
        )?;

        Ok(Self {
            state: StateProvider::Eager {
                state,
                account_lookup_order: None,
                storage_lookup_order: None,
                witness_order: None,
            },
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
        lookup_orders: WitnessDbLookupOrders<'a>,
    ) -> Result<Self, ClientExecutionError> {
        let (
            account_lookup_order_input,
            bytecode_lookup_order,
            storage_lookup_order,
            witness_order,
        ) = lookup_orders;
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
            account_lookup_order: account_order,
            storage_lookup_order: storage_order,
            witness_order: storage_witness_order,
            ..
        } = &mut witness_db.state
        {
            *account_order = Some(account_lookup_order_input);
            *storage_order = Some(storage_lookup_order);
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
        let block_hashes = build_block_hashes(
            once(current_header).chain(ancestor_headers.iter()),
            1 + ancestor_headers.len(),
        )?;

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
            StateProvider::Eager { state, account_lookup_order, witness_order, .. } => {
                let mut first_lookup = false;
                if let Some(account_lookup_order) = account_lookup_order {
                    let mut order = account_lookup_order.borrow_mut();
                    if !order.contains(&hashed_address) {
                        order.push(hashed_address);
                        first_lookup = true;
                    }
                }
                if first_lookup && let Some(witness_order) = witness_order {
                    witness_order.borrow_mut().push(WitnessAccess::Account(hashed_address));
                }
                state.state_trie.get_rlp::<TrieAccount>(hashed_address.as_slice()).unwrap()
            }
            StateProvider::Streaming(state) => state.read_account(hashed_address)?,
        };

        let account = account_in_trie.map(|account_in_trie| AccountInfo {
            balance: account_in_trie.balance,
            nonce: account_in_trie.nonce,
            code_hash: account_in_trie.code_hash,
            account_id: None,
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
            },
        }
    }

    /// Get storage value of address at index.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        if trim::enabled("zero_storage_reads") {
            return Ok(U256::ZERO);
        }

        let hashed_address = keccak256(address);

        let hashed_slot = keccak256(index.to_be_bytes::<32>());
        match &self.state {
            StateProvider::Eager {
                state,
                account_lookup_order,
                storage_lookup_order,
                witness_order,
            } => {
                let mut first_account_lookup = false;
                if let Some(account_lookup_order) = account_lookup_order {
                    let mut order = account_lookup_order.borrow_mut();
                    if !order.contains(&hashed_address) {
                        order.push(hashed_address);
                        first_account_lookup = true;
                    }
                }
                if first_account_lookup && let Some(witness_order) = witness_order {
                    witness_order.borrow_mut().push(WitnessAccess::Account(hashed_address));
                }

                let mut first_lookup = false;
                if let Some(storage_lookup_order) = storage_lookup_order {
                    let mut order = storage_lookup_order.borrow_mut();
                    if !order.contains(&hashed_address) {
                        order.push(hashed_address);
                        first_lookup = true;
                    }
                }
                if first_lookup && let Some(witness_order) = witness_order {
                    witness_order.borrow_mut().push(WitnessAccess::StorageTrie(hashed_address));
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
                let storage_trie =
                    storage_tries.get(&hashed_address).expect("streaming storage trie was loaded");
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
