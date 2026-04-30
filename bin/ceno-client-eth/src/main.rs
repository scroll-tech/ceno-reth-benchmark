// use openvm::io::{println, read, reveal_bytes32};
extern crate ceno_rt;
use alloy_primitives::Address;
use ceno_crypto::ceno_crypto;
use openvm_client_executor::{
    io::{
        AncestorHeadersInput, BytecodeInput, ChunkedClientInput, ClientInputChunk,
        CurrentBlockInput, StateTrieHeader, StorageTrieCount, StorageTrieHeader,
    },
    ChainVariant, ClientExecutor,
};

#[cfg(feature = "profiling")]
use ceno_syscall::syscall_phantom_log_pc_cycle;

ceno_crypto!(
    revm_precompile = revm_precompile,
    alloy_consensus = alloy_consensus,
    address_type = Address,
);

struct CenoClientInputReader;

impl ChunkedClientInput for CenoClientInputReader {
    fn read_ancestor_headers(&mut self) -> AncestorHeadersInput {
        ceno_rt::read_owned()
    }

    fn read_state_trie_header(&mut self) -> StateTrieHeader {
        ceno_rt::read_owned()
    }

    fn read_storage_trie_count(&mut self) -> StorageTrieCount {
        ceno_rt::read_owned()
    }

    fn read_storage_trie_header(&mut self) -> StorageTrieHeader {
        ceno_rt::read_owned()
    }

    fn read_current_block(&mut self) -> CurrentBlockInput {
        ceno_rt::read_owned()
    }

    fn read_bytecode(&mut self) -> BytecodeInput {
        ceno_rt::read_owned()
    }

    fn read_witness_chunk(&mut self) -> ClientInputChunk {
        ceno_rt::read_owned()
    }

    fn read_raw_bytes(&mut self) -> &'static [u8] {
        ceno_rt::read_slice()
    }
}

pub fn main() {
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("install ceno crypto");
    CenoCrypto::install();
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("end ceno crypto");
    // Execute the block (crypto is installed inside executor).
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("start execute chunked input");
    let executor = ClientExecutor;
    let mut input = CenoClientInputReader;
    let header = executor
        .execute_chunked_from_reader(ChainVariant::Mainnet, &mut input)
        .expect("failed to execute client");
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("end execute chunked input");
    let block_hash = header.hash_slow();

    // commit block hash.
    let digest_words = unsafe { core::mem::transmute::<[u8; 32], [u32; 8]>(block_hash.0) };
    ceno_rt::commit_digest(digest_words);
}
