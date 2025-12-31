// use openvm::io::{println, read, reveal_bytes32};
extern crate ceno_rt;
use alloy_primitives::Address;
use ceno_crypto::ceno_crypto;
use openvm_client_executor::{io::ClientExecutorInput, ChainVariant, ClientExecutor};

#[cfg(feature = "profiling")]
use ceno_syscall::syscall_phantom_log_pc_cycle;

ceno_crypto!(
    revm_precompile = revm_precompile,
    alloy_consensus = alloy_consensus,
    address_type = Address,
);

pub fn main() {
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("install ceno crypto");
    CenoCrypto::install();
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("end ceno crypto");
    // Read the input.
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("start ceno_rt::read");
    let input: ClientExecutorInput = ceno_rt::read_owned();
    #[cfg(feature = "profiling")]
    syscall_phantom_log_pc_cycle("end ceno_rt::read");

    // Execute the block (crypto is installed inside executor).
    let executor = ClientExecutor;
    let header = executor.execute(ChainVariant::Mainnet, input).expect("failed to execute client");
    let block_hash = header.hash_slow();

    // commit block hash.
    ceno_rt::commit(&block_hash.0);
}
