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

# Wrapper around the Ceno benchmark binary to allow post-processing
# after proving completes. All arguments are forwarded to the binary.

BIN_PATH="${OVM_BIN:-/usr/local/bin/ceno-reth-benchmark-bin}"
JOBS_DIR="${JOBS_DIR:-/app/jobs}"
MODE="${MODE:-prove-stark}"
APP_LOG_BLOWUP="${APP_LOG_BLOWUP:-1}"
LEAF_LOG_BLOWUP="${LEAF_LOG_BLOWUP:-1}"
INTERNAL_LOG_BLOWUP="${INTERNAL_LOG_BLOWUP:-2}"
ROOT_LOG_BLOWUP="${ROOT_LOG_BLOWUP:-3}"
MAX_SEGMENT_LENGTH="${MAX_SEGMENT_LENGTH:-4194304}"
SEGMENT_MAX_CELLS="${SEGMENT_MAX_CELLS:-1200000000}"
VPMM_PAGE_SIZE=$((4 << 20))
VPMM_PAGES=$((12 * $MAX_SEGMENT_LENGTH/ $VPMM_PAGE_SIZE))

if [[ $# -lt 1 ]]; then
  echo "[prove_block.sh] Usage: $0 <proof_uuid>" >&2
  exit 2
fi

PROOF_UUID="$1"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "[prove_block.sh] Error: Binary not found at $BIN_PATH" >&2
  exit 127
fi

job_dir="${JOBS_DIR}/${PROOF_UUID}"
mkdir -p "$job_dir"

CENO_STATUS_API_BASE_URL="${CENO_STATUS_API_BASE_URL%/}"

post_status() {
  local endpoint="$1"
  local payload="$2"
  if [[ -z "$CENO_STATUS_API_BASE_URL" ]]; then
    return
  fi
  curl -sS -X POST \
    -H "Content-Type: application/json" \
    ${CENO_STATUS_API_KEY:+-H "Authorization: Bearer ${CENO_STATUS_API_KEY}"} \
    -d "$payload" \
    "${CENO_STATUS_API_BASE_URL}/${endpoint}"
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
  echo "[prove_block.sh] Fetching latest block number from $ETH_RPC_URL" >&2
  BLOCK_NUMBER="$(curl -s -X POST \
    -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    "$ETH_RPC_URL" | python3 -c 'import json,sys; data=json.load(sys.stdin); print(int(data["result"], 16))' 2>/dev/null)" || true
  if [[ -z "$BLOCK_NUMBER" ]]; then
    echo "[prove_block.sh] Failed to fetch latest block number from RPC" >&2
    exit 1
  fi
  echo "[prove_block.sh] Latest block number: $BLOCK_NUMBER" >&2
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
    post_status "proofs/queue" "{\"block_number\":${BLOCK_NUMBER},\"cluster_id\":\"${CENO_CLUSTER_ID}\"}"
  fi
  echo "[prove_block.sh] Generating input locally via --mode make-input" >&2
  "$BIN_PATH" \
    --mode make-input \
    --block-number "$BLOCK_NUMBER" \
    --rpc-url "$ETH_RPC_URL" \
    --generated-input-path "$cache_root"

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

echo "[prove_block.sh] Starting proof with --mode $MODE for block $BLOCK_NUMBER" >&2
if [[ -n "$CENO_STATUS_API_BASE_URL" ]]; then
  post_status "proofs/proving" "{\"block_number\":${BLOCK_NUMBER},\"cluster_id\":\"${CENO_CLUSTER_ID}\"}"
fi

start_ts_ms=$(date +%s%3N)
PROOF_JSON="$job_dir/proof.json"

OUTPUT_PATH="$job_dir/metrics.json"

"$BIN_PATH" \
  --mode "$MODE" \
  --block-number "$BLOCK_NUMBER" \
  --input-path "$INPUT_PATH" \
  --cache-dir "$cache_root" \
  --rpc-url "$ETH_RPC_URL" \
  --app-log-blowup "$APP_LOG_BLOWUP" \
  --leaf-log-blowup "$LEAF_LOG_BLOWUP" \
  --internal-log-blowup "$INTERNAL_LOG_BLOWUP" \
  --root-log-blowup "$ROOT_LOG_BLOWUP" \
  --max-segment-length "$MAX_SEGMENT_LENGTH" \
  --segment-max-cells "$SEGMENT_MAX_CELLS" \
  --output-dir "$job_dir" \
  --skip-comparison
  # --app-pk-path /app/app_pk \
  # --agg-pk-path /app/agg_pk \

status=$?

end_ts_ms=$(date +%s%3N)
duration_ms=$(( end_ts_ms - start_ts_ms ))
echo "$duration_ms" > "$job_dir/latency_ms.txt"

proof_b64=""
if [[ -f "$PROOF_JSON" ]]; then
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

if [[ -n "$CENO_STATUS_API_BASE_URL" ]]; then
  read -r -d '' proved_payload <<EOF || true
{"block_number":${BLOCK_NUMBER},"cluster_id":"${CENO_CLUSTER_ID}","proving_time":${duration_ms},"proving_cycles":10000,"proof":"${proof_b64}","verifier_id":"${VERIFIER_ID}"}
EOF
  post_status "proofs/proved" "$proved_payload"
fi

# Post-processing hook: customize as needed
echo "[prove_block.sh] Proof finished with status=$status in ${duration_ms}ms at $(date -Is)" >&2

# Upload proof.json to S3 (best-effort)
if [[ -f "$PROOF_JSON" ]]; then
  set +e
  s5cmd cp "$PROOF_JSON" "s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/proof.json"
  upload_rc=$?
  if [[ $upload_rc -ne 0 ]]; then
    echo "[prove_block.sh] Warning: failed to upload proof.json to S3 (rc=$upload_rc)" >&2
  fi
  set -e
else
  echo "[prove_block.sh] Warning: proof.json not found at $PROOF_JSON" >&2
fi

if [[ -f "$OUTPUT_PATH" ]]; then
  s5cmd cp "$OUTPUT_PATH" "s3://${S3_BUCKET}/${S3_PREFIX}/${PROOF_UUID}/metrics.json"
  upload_rc=$?
  if [[ $upload_rc -ne 0 ]]; then
    echo "[prove_block.sh] Warning: failed to upload metrics.json to S3 (rc=$upload_rc)" >&2
  fi
else
  echo "[prove_block.sh] Warning: metrics.json not found at $OUTPUT_PATH" >&2
fi

exit $status
