#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

mkdir -p "$test_root/bin" "$test_root/jobs/test/block_data/input/1"
touch "$test_root/jobs/test/block_data/input/1/25746900.bin"

cat > "$test_root/bin/prover" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${CAPTURE_ENV:?}"
{
  printf 'mode=%s\n' "${CENO_CHIP_PROVING_MODE:-missing}"
  printf 'lanes=%s\n' "${CENO_CHIP_PROVING_LANES:-missing}"
  printf 'legacy=%s\n' "${CENO_CONCURRENT_CHIP_PROVING-unset}"
} > "$CAPTURE_ENV"
printf '{"gauge":[{"metric":"cycles","value":1},{"metric":"num_shards","value":1}]}' > "$OUTPUT_PATH"
touch "${JOBS_DIR}/test/25746900_root_proof.bin"
EOF
chmod +x "$test_root/bin/prover"

cat > "$test_root/bin/s5cmd" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$test_root/bin/s5cmd"

PATH="$test_root/bin:$PATH" \
CAPTURE_ENV="$test_root/captured.env" \
OVM_BIN="$test_root/bin/prover" \
JOBS_DIR="$test_root/jobs" \
BLOCK_NUMBER=25746900 \
PROVE_BLOCK_GPU_CHECK=0 \
SKIP_S3_UPLOAD=1 \
CENO_CONCURRENT_CHIP_PROVING=1 \
CENO_RETH_BENCHMARK_REVISION=test-revision \
  "$repo_root/server/prove_block.sh" test >/dev/null 2>&1

grep -qx 'mode=lanes' "$test_root/captured.env"
grep -qx 'lanes=4' "$test_root/captured.env"
grep -qx 'legacy=unset' "$test_root/captured.env"
grep -q '| Benchmark revision | `test-revision` |' "$test_root/jobs/test/25746900_metrics.md"
grep -q '| Chip proving mode | `lanes` |' "$test_root/jobs/test/25746900_metrics.md"
grep -q '| Chip proving lanes | `4` |' "$test_root/jobs/test/25746900_metrics.md"

if CENO_CHIP_PROVING_MODE=concurrent PROVE_BLOCK_GPU_CHECK=0 \
  "$repo_root/server/prove_block.sh" test >/dev/null 2>&1; then
  echo "legacy concurrent mode unexpectedly accepted" >&2
  exit 1
fi

if CENO_CHIP_PROVING_LANES=64 PROVE_BLOCK_GPU_CHECK=0 \
  "$repo_root/server/prove_block.sh" test >/dev/null 2>&1; then
  echo "invalid lane count unexpectedly accepted" >&2
  exit 1
fi

echo "prove_block scheduler configuration tests passed"
