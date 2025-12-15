Ceno Reth Benchmark
===

Repo fork from https://github.com/axiom-crypto/openvm-reth-benchmark 
and patched with [Ceno](https://github.com/scroll-tech/ceno/) 

### Compiling the Guest Program

before running benchmark, need to compile guest program

first install ceno-cli tools

```
JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,metadata_thp:always,thp:always,dirty_decay_ms:-1,muzzy_decay_ms:-1,abort_conf:true" \
    cargo install --git https://github.com/scroll-tech/ceno.git --features jemalloc --features nightly-features cargo-ceno
```

then build guest program

```
cd bin/ceno-client-eth
cargo ceno build --release
```

To show some profiling log of guest program, just build binary with `profiling` feature

```
cargo build --release --feature profiling
```

### Example command to prove block without recursion

below command fetch L1 block `23817600` (without recursion)

```
OUTPUT_PATH="metrics.json" RUSTFLAGS="-C target-feature=+avx2" JEMALLOC_SYS_WITH_MALLOC_CONF=retain:true,metadata_thp:always,thp:always,dirty_decay_ms:-1,muzzy_decay_ms:-1 RUST_LOG=info cargo run --features jemalloc --bin ceno-reth-benchmark-bin --release -- --block-number 23817600 --rpc-url <rpc url> --cache-dir block_data --mode prove-app
```

To prove with GPU, add `--features gpu --config net.git-fetch-with-cli=true` as [ceno-gpu](https://github.com/scroll-tech/ceno-gpu) is non-public repo

### Prove with Recursion

to prove with recursion, set mode to `--mode prove-stark`

### (Optional) key-gen pk for next use

let say fixture dir in `./pk`

> need to assure pk exist

then run with `--mode generate-fixtures --fixtures-path ./pk`

Then for next run, set agg pk path with `--agg-pk-path pk/agg_pk.bitcode`


### update Ceno dependency
just run

```
bash cargo_update_ceno.sh
```

which will update cargo dependency related to ceno accordingly

### More details
Check [openvm-reth-benchmark README](https://github.com/axiom-crypto/openvm-reth-benchmark/blob/main/README.md) for more details