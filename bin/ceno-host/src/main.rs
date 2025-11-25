#![cfg_attr(feature = "tco", allow(incomplete_features))]
#![cfg_attr(feature = "tco", feature(explicit_tail_calls))]
use clap_builder::Parser;
use openvm_reth_benchmark::{run_ceno_reth_benchmark, HostArgs};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = HostArgs::parse();
    run_ceno_reth_benchmark(args).await
}
