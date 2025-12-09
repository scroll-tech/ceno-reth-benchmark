export RUST_BACKTRACE=1
export CENO_GPU_CACHE_LEVEL=0
export RUSTFLAGS="-C target-feature=+avx2"
export RUST_LOG=info,ceno_zkvm=debug

if [ -f "fixtures/app_vk.bitcode" ]; then
    echo "App VK fixture already exists."
else
    cargo run --features "jemalloc" --config net.git-fetch-with-cli=true \
     --bin ceno-reth-benchmark-bin --profile release -- \
     --block-number 23587691 \
     --rpc-url https://eth-mainnet.g.alchemy.com/v2/dpJmxzwalHmaOsZwAB-Djvz2N651QahS \
     --fixtures-path fixtures --cache-dir block_data --output-dir output \
     --mode generate-fixtures 2>&1 | tee fixture_generation.log
fi

# 1. generate app proof in release mode
if [ -f "output/app_proof.bitcode" ]; then
    echo "App proof already exists, skipping generation."
else
    # generate app proof using cpu prover for now
    cargo run --features "jemalloc" --config net.git-fetch-with-cli=true \
        --bin ceno-reth-benchmark-bin --profile release -- --block-number 23587691 \
        --rpc-url https://eth-mainnet.g.alchemy.com/v2/dpJmxzwalHmaOsZwAB-Djvz2N651QahS \
        --cache-dir block_data --output-dir output --mode prove-app 2>&1 | tee app.log
fi

# 2. compress app proofs into one root stark proof in release mode
cargo run --features "jemalloc,gpu" --config net.git-fetch-with-cli=true \
 --bin ceno-reth-benchmark-bin --release -- --block-number 23587691 \
 --rpc-url https://eth-mainnet.g.alchemy.com/v2/dpJmxzwalHmaOsZwAB-Djvz2N651QahS \
 --app-vk-path fixtures/app_vk.bitcode --app-proofs output/app_proof.bitcode \
 --cache-dir block_data --output-dir output \
 --mode prove-stark-only 2>&1 | tee stark.log
