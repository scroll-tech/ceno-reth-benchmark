#!/usr/bin/env bash
set -euo pipefail

S3_BUCKET="${S3_BUCKET:-cloud-proving-staging-data}"
S3_PREFIX="${S3_PREFIX:-proofs/testing}"
ETH_RPC_URL="${ETH_RPC_URL:-}"
BLOCK_NUMBER_OVERRIDE="${BLOCK_NUMBER:-}"
CENO_STATUS_API_BASE_URL="${CENO_STATUS_API_BASE_URL:-}"
CENO_STATUS_API_KEY="${CENO_STATUS_API_KEY:-}"
CENO_CLUSTER_ID="${CENO_CLUSTER_ID:-}"
VERIFIER_ID="${VERIFIER_ID:-0.1}"
CENO_GPU_CACHE_LEVEL="${CENO_GPU_CACHE_LEVEL:-1}"
CENO_GPU_ENABLE_WITGEN="${CENO_GPU_ENABLE_WITGEN:-0}"
CENO_CONCURRENT_CHIP_PROVING="${CENO_CONCURRENT_CHIP_PROVING:-1}"
# Magic number: the old shard cap was 1207959552 = ((1 << 30) * 9 / 4 / 2).
# Keccak preflight cost is now estimated with a 33/16 blowup instead of the old
# coarse 2x factor, so raise the shard cap by (33/16) / 2 = 33/32:
# 1207959552 * 33 / 32 = 1245708288.
CENO_MAX_CELL_PER_SHARD="${CENO_MAX_CELL_PER_SHARD:-1245708288}"
CENO_GPU_JAGGED_RESHAPE_LOG_HEIGHT="${CENO_GPU_JAGGED_RESHAPE_LOG_HEIGHT:-23}"
CENO_GPU_LARGE_TASK_BOOKING_MARGIN_MB="${CENO_GPU_LARGE_TASK_BOOKING_MARGIN_MB:-3048}"
RUST_MIN_STACK="${RUST_MIN_STACK:-536870912}"
CHAIN_ID="${CHAIN_ID:-1}"

# Wrapper around the Ceno benchmark binary to allow post-processing
# after proving completes. All arguments are forwarded to the binary.

BIN_PATH="${OVM_BIN:-/usr/local/bin/ceno-reth-benchmark-bin}"
JOBS_DIR="${JOBS_DIR:-/app/jobs}"
MODE="${MODE:-prove-stark}"

if [[ $# -lt 1 ]]; then
  echo "[prove_block.sh] Usage: $0 <proof_uuid>" >&2
  exit 2
fi

PROOF_UUID="$1"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "[prove_block.sh] Error: Binary not found at $BIN_PATH" >&2
  exit 127
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

job_dir="${JOBS_DIR}/${PROOF_UUID}"
mkdir -p "$job_dir"

CENO_STATUS_API_BASE_URL="${CENO_STATUS_API_BASE_URL%/}"
POST_STATUS_HTTP_STATUS=""

post_status() {
  local endpoint="$1"
  local payload="$2"
  POST_STATUS_HTTP_STATUS=""
  if [[ -z "$CENO_STATUS_API_BASE_URL" ]]; then
    return
  fi
  local payload_file payload_size
  payload_file="$(mktemp)"
  printf '%s' "$payload" > "$payload_file"
  payload_size="$(wc -c <"$payload_file" | tr -d '[:space:]')"
  echo "[post_status] POST ${endpoint} payload_size=${payload_size}B" >&2
  local response status
  response=$(curl -sS -w "%{http_code}" -o /tmp/post_status_resp.$$ \
    -X POST \
    -H "Content-Type: application/json" \
    ${CENO_STATUS_API_KEY:+-H "Authorization: Bearer ${CENO_STATUS_API_KEY}"} \
    --data-binary "@${payload_file}" \
    "${CENO_STATUS_API_BASE_URL}/${endpoint}")
  status="$response"
  POST_STATUS_HTTP_STATUS="$status"
  echo "[post_status] status=${status}" >&2
  if [[ -s /tmp/post_status_resp.$$ ]]; then
    echo "[post_status] response=$(cat /tmp/post_status_resp.$$)" >&2
  fi
  rm -f /tmp/post_status_resp.$$
  rm -f "$payload_file"
}

echo "[prove_block.sh] Starting proof at $(date -Is) with BIN=$BIN_PATH" >&2
echo "[prove_block.sh] Job dir: $job_dir" >&2

# Determine block number: either override or fetch latest via RPC.
if [[ -n "$BLOCK_NUMBER_OVERRIDE" ]]; then
  BLOCK_NUMBER="$BLOCK_NUMBER_OVERRIDE"
  echo "[prove_block.sh] Using provided block number: $BLOCK_NUMBER" >&2
else
  if [[ -z "$ETH_RPC_URL" ]]; then
    echo "[prove_block.sh] ETH_RPC_URL not set and BLOCK_NUMBER not provided" >&2
    exit 1
  fi
  echo "[prove_block.sh] Fetching latest block number from configured RPC" >&2
  BLOCK_NUMBER="$(curl -s -X POST \
    -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    "$ETH_RPC_URL" | python3 -c 'import json,sys; data=json.load(sys.stdin); print(int(data["result"], 16))' 2>/dev/null)" || true
  if [[ -z "$BLOCK_NUMBER" ]]; then
    echo "[prove_block.sh] Failed to fetch latest block number from RPC" >&2
    exit 1
  fi
  raw_block_number="$BLOCK_NUMBER"
  remainder=$(( BLOCK_NUMBER % 100 ))
  BLOCK_NUMBER=$(( BLOCK_NUMBER - remainder ))
  echo "[prove_block.sh] Latest block number: $raw_block_number (rounded down to $BLOCK_NUMBER)" >&2

fi

PROOF_FILENAME="${BLOCK_NUMBER}_proof.json"
ROOT_PROOF_FILENAME="${BLOCK_NUMBER}_root_proof.bin"
JOB_PROOF_S3_URI="s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${PROOF_FILENAME}"
JOB_ROOT_PROOF_S3_URI="s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${ROOT_PROOF_FILENAME}"

if [[ -z "$BLOCK_NUMBER_OVERRIDE" ]]; then
  echo "[prove_block.sh] Checking for existing root proof at ${JOB_ROOT_PROOF_S3_URI}" >&2
  if s5cmd ls "$JOB_ROOT_PROOF_S3_URI" >/dev/null 2>&1; then
    echo "[prove_block.sh] Found existing root proof for block ${BLOCK_NUMBER}; sleeping 300s then exiting" >&2
    sleep 300
    exit 0
  fi

  echo "[prove_block.sh] Checking for existing proof at ${JOB_PROOF_S3_URI}" >&2
  if s5cmd ls "$JOB_PROOF_S3_URI" >/dev/null 2>&1; then
    echo "[prove_block.sh] Found existing proof for block ${BLOCK_NUMBER}; sleeping 300s then exiting" >&2
    sleep 300
    exit 0
  fi
fi

cache_root="$job_dir/block_data"
mkdir -p "$cache_root"

find_generated_input() {
  if [[ ! -d "$cache_root/input" ]]; then
    echo ""
    return
  fi
  local candidate
  candidate="$(find "$cache_root/input" -maxdepth 2 -type f -name "${BLOCK_NUMBER}.bin" 2>/dev/null | head -n1 || true)"
  echo "$candidate"
}

GENERATED_INPUT_PATH="$(find_generated_input)"

if [[ -n "$GENERATED_INPUT_PATH" ]]; then
  echo "[prove_block.sh] Reusing existing generated input $GENERATED_INPUT_PATH" >&2
else
  if [[ -n "$CENO_STATUS_API_BASE_URL" ]]; then
    post_status "proofs/queued" "{\"block_number\":${BLOCK_NUMBER},\"cluster_id\":${CENO_CLUSTER_ID}}"
    if [[ "$POST_STATUS_HTTP_STATUS" == "409" ]]; then
      echo "[prove_block.sh] Status API reports block ${BLOCK_NUMBER} is already proved; sleeping 300s then exiting" >&2
      sleep 300
      exit 0
    fi
  fi
  echo "[prove_block.sh] Generating input locally via --mode make-input" >&2
  "$BIN_PATH" \
    --mode make-input \
    --block-number "$BLOCK_NUMBER" \
    --rpc-url "$ETH_RPC_URL" \
    --generated-input-path "$cache_root" \
    --chain-id "$CHAIN_ID"

  GENERATED_INPUT_PATH="$(find_generated_input)"
  if [[ -z "$GENERATED_INPUT_PATH" ]]; then
    echo "[prove_block.sh] Generated input not found for block $BLOCK_NUMBER under $cache_root" >&2
    exit 1
  fi
fi

if [[ "${SKIP_S3_UPLOAD:-0}" != "1" ]]; then
  echo "[prove_block.sh] Uploading generated input to s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${BLOCK_NUMBER}.bin" >&2
  set +e
  s5cmd cp "$GENERATED_INPUT_PATH" "s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${BLOCK_NUMBER}.bin"
  upload_rc=$?
  if [[ $upload_rc -ne 0 ]]; then
    echo "[prove_block.sh] Warning: failed to upload generated input to S3 (rc=$upload_rc)" >&2
  fi
  set -e
else
  echo "[prove_block.sh] SKIP_S3_UPLOAD=1; not uploading generated input" >&2
fi

INPUT_PATH="$GENERATED_INPUT_PATH"
echo "[prove_block.sh] Using input: $INPUT_PATH" >&2

METRICS_MD="$job_dir/${BLOCK_NUMBER}_metrics.md"

echo "[prove_block.sh] Starting proof with --mode $MODE for block $BLOCK_NUMBER" >&2
if [[ -n "$CENO_STATUS_API_BASE_URL" ]]; then
  post_status "proofs/proving" "{\"block_number\":${BLOCK_NUMBER},\"cluster_id\":${CENO_CLUSTER_ID}}"
  if [[ "$POST_STATUS_HTTP_STATUS" == "409" ]]; then
    echo "[prove_block.sh] Status API reports block ${BLOCK_NUMBER} is already proved; sleeping 300s then exiting" >&2
    sleep 300
    exit 0
  fi
fi

start_ts_ms=$(date +%s%3N)
PROOF_JSON="$job_dir/${PROOF_FILENAME}"
ROOT_PROOF_BIN="$job_dir/${ROOT_PROOF_FILENAME}"

OUTPUT_PATH="$job_dir/metrics.json"

export CENO_GPU_CACHE_LEVEL
export CENO_GPU_ENABLE_WITGEN
export CENO_CONCURRENT_CHIP_PROVING
export CENO_MAX_CELL_PER_SHARD
export CENO_GPU_JAGGED_RESHAPE_LOG_HEIGHT
export CENO_GPU_LARGE_TASK_BOOKING_MARGIN_MB
export RUST_MIN_STACK
export OUTPUT_PATH
ulimit -s 1048576

set +e
"$BIN_PATH" \
  --mode "$MODE" \
  --block-number "$BLOCK_NUMBER" \
  --input-path "$INPUT_PATH" \
  --cache-dir "$cache_root" \
  --rpc-url "$ETH_RPC_URL" \
  --output-dir "$job_dir" \
  --skip-comparison \
  --chain-id "$CHAIN_ID"
  # --app-pk-path /app/app_pk \

status=$?
set -e

end_ts_ms=$(date +%s%3N)
duration_ms=$(( end_ts_ms - start_ts_ms ))
echo "$duration_ms" > "$job_dir/latency_ms.txt"

proof_b64=""
if [[ -f "$ROOT_PROOF_BIN" ]]; then
  proof_b64="$(base64 -w0 "$ROOT_PROOF_BIN" 2>/dev/null || base64 "$ROOT_PROOF_BIN" | tr -d '\n')" || proof_b64=""
elif [[ -f "$PROOF_JSON" ]]; then
  proof_output_path="$job_dir/proofs/${BLOCK_NUMBER}.json"
  proof_b64="$(python3 - "$PROOF_JSON" "$proof_output_path" <<'PY'
import json, sys, base64, pathlib
src = pathlib.Path(sys.argv[1])
dst = pathlib.Path(sys.argv[2])
data = json.loads(src.read_text())
dst.parent.mkdir(parents=True, exist_ok=True)
dst.write_text(json.dumps(data, indent=2))
compact = json.dumps(data, separators=(",", ":"))
print(base64.b64encode(compact.encode()).decode(), end="")
PY
)" || proof_b64=""
fi

proving_cycles=""
if [[ -f "$OUTPUT_PATH" ]]; then
  proving_cycles="$(python3 - "$OUTPUT_PATH" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
for entry in data.get("gauge", []):
    if entry.get("metric") == "cycles":
        print(entry.get("value", ""), end="")
        break
PY
)" || proving_cycles=""
fi

reth_block_time_ms=""
if [[ -f "$OUTPUT_PATH" ]]; then
  reth_block_time_ms="$(python3 - "$OUTPUT_PATH" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
for entry in data.get("gauge", []):
    if entry.get("metric") == "reth-block_time_ms":
        print(entry.get("value", ""), end="")
        break
PY
)" || reth_block_time_ms=""
fi
reported_duration_ms="${reth_block_time_ms:-$duration_ms}"
num_shards=""
if [[ -f "$OUTPUT_PATH" ]]; then
  num_shards="$(python3 - "$OUTPUT_PATH" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
for entry in data.get("gauge", []):
    if entry.get("metric") == "num_shards":
        print(entry.get("value", ""), end="")
        break
PY
)" || num_shards=""
fi

if [[ -f "$OUTPUT_PATH" ]]; then
  if ! python3 "$SCRIPT_DIR/metrics_to_markdown.py" "$OUTPUT_PATH" "$METRICS_MD" --block-number "$BLOCK_NUMBER"; then
    echo "[prove_block.sh] Warning: failed to convert metrics.json to markdown" >&2
  fi
fi

if [[ -n "$CENO_STATUS_API_BASE_URL" ]]; then
  if [[ $status -ne 0 ]]; then
    echo "[prove_block.sh] Skipping proofs/proved status because proof command failed with status=$status" >&2
  elif [[ -z "$proof_b64" ]]; then
    echo "[prove_block.sh] Skipping proofs/proved status because proof output is missing" >&2
  elif ! [[ "$proving_cycles" =~ ^[1-9][0-9]*$ ]]; then
    echo "[prove_block.sh] Skipping proofs/proved status because proving_cycles is not a positive integer: ${proving_cycles:-<empty>}" >&2
  else
    read -r -d '' proved_payload <<EOF || true
{"block_number":${BLOCK_NUMBER},"cluster_id":${CENO_CLUSTER_ID},"proving_time":${reported_duration_ms},"proving_cycles":${proving_cycles},"proof":"${proof_b64}","verifier_id":"${VERIFIER_ID}"}
EOF
    post_status "proofs/proved" "$proved_payload"
  fi
fi

# Post-processing hook: customize as needed
echo "[prove_block.sh] Proof finished with status=$status in ${duration_ms}ms at $(date -Is)" >&2

# Upload proof file to S3 (best-effort)
if [[ -f "$PROOF_JSON" ]]; then
  set +e
  if ! s5cmd cp "$PROOF_JSON" "$JOB_PROOF_S3_URI"; then
    echo "[prove_block.sh] Warning: failed to upload proof file to ${JOB_PROOF_S3_URI}" >&2
  fi
  set -e
else
  if [[ ! -f "$ROOT_PROOF_BIN" ]]; then
    echo "[prove_block.sh] Warning: proof file not found at $PROOF_JSON" >&2
  fi
fi

if [[ -f "$ROOT_PROOF_BIN" ]]; then
  set +e
  if ! s5cmd cp "$ROOT_PROOF_BIN" "$JOB_ROOT_PROOF_S3_URI"; then
    echo "[prove_block.sh] Warning: failed to upload root proof file to ${JOB_ROOT_PROOF_S3_URI}" >&2
  fi
  set -e
else
  echo "[prove_block.sh] Warning: root proof file not found at $ROOT_PROOF_BIN" >&2
fi

if [[ -f "$OUTPUT_PATH" ]]; then
  s5cmd cp "$OUTPUT_PATH" "s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${BLOCK_NUMBER}_metrics.json"
  upload_rc=$?
  if [[ $upload_rc -ne 0 ]]; then
    echo "[prove_block.sh] Warning: failed to upload metrics.json to S3 (rc=$upload_rc)" >&2
  fi
else
  echo "[prove_block.sh] Warning: metrics.json not found at $OUTPUT_PATH" >&2
fi

if [[ -f "$METRICS_MD" ]]; then
  s5cmd cp "$METRICS_MD" "s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/${BLOCK_NUMBER}_metrics.md"
  upload_rc=$?
  if [[ $upload_rc -ne 0 ]]; then
    echo "[prove_block.sh] Warning: failed to upload metrics markdown to S3 (rc=$upload_rc)" >&2
  fi
else
  echo "[prove_block.sh] Warning: metrics markdown not found at $METRICS_MD" >&2
fi

PROCESSED_BLOCKS_URI="s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/processed_block.txt"
tmp_processed="$(mktemp)"
set +e
s5cmd cp "$PROCESSED_BLOCKS_URI" "$tmp_processed" >/dev/null 2>&1
set -e
if [[ -n "$num_shards" ]]; then
  printf '%s,%s\n' "$BLOCK_NUMBER" "$num_shards" >> "$tmp_processed"
else
  printf '%s\n' "$BLOCK_NUMBER" >> "$tmp_processed"
fi
if ! s5cmd cp "$tmp_processed" "$PROCESSED_BLOCKS_URI"; then
  echo "[prove_block.sh] Warning: failed to update processed_block.txt on S3" >&2
fi
rm -f "$tmp_processed"

exit $status
