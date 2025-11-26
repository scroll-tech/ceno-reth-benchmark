Ceno Reth Benchmark
===

Repo fork from https://github.com/axiom-crypto/openvm-reth-benchmark 
and patched with [Ceno](https://github.com/scroll-tech/ceno/) 

### Compiling the Guest Program

before running benchmark, need to compile guest program manually

```
cd bin/ceno-client-eth
cargo build --release
```

To show some profiling log of guest program, just build binary with `profiling` feature

```
cargo build --release --feature profiling
```

### Example command to prove block

below command fetch L1 block `23817600` (without recursion)

```
RUSTFLAGS="-C target-feature=+avx2" JEMALLOC_SYS_WITH_MALLOC_CONF=retain:true,metadata_thp:always,thp:always,dirty_decay_ms:-1,muzzy_decay_ms:-1 RUST_LOG=off,ceno_emul=debug,ceno_host=debug,ceno_zkvm=debug cargo run --features jemalloc --bin ceno-reth-benchmark-bin --release -- --block-number 23817600 --rpc-url <rpc url> --cache-dir block_data --mode prove-app
```

To prove with GPU, add `--features gpu --config net.git-fetch-with-cli=true` as [ceno-gpu](https://github.com/scroll-tech/ceno-gpu) is non-public repo

### More details
Check [openvm-reth-benchmark README](https://github.com/axiom-crypto/openvm-reth-benchmark/blob/main/README.md) for more details