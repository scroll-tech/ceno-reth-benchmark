#!/usr/bin/env bash
set -euo pipefail

RUNNER_DIR="/home/docker/actions-runner"
RUNNER_URL="${RUNNER_URL:-https://github.com/scroll-tech/ceno-reth-benchmark}"
RUNNER_NAME="${RUNNER_NAME:-}"
RUNNER_TOKEN="${RUNNER_TOKEN:-}"
RUNNER_LABELS="${RUNNER_LABELS:-gpu}"

if [[ -z "$RUNNER_NAME" ]]; then
  echo "[runner-entrypoint] RUNNER_NAME must be provided" >&2
  exit 1
fi

if [[ -z "$RUNNER_TOKEN" ]]; then
  echo "[runner-entrypoint] RUNNER_TOKEN must be provided" >&2
  exit 1
fi

cleanup() {
  if [[ -f "$RUNNER_DIR/.runner" ]]; then
    echo "[runner-entrypoint] Removing runner registration" >&2
    RUNNER_ALLOW_RUNASROOT=1 "$RUNNER_DIR/config.sh" remove --token "$RUNNER_TOKEN" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

if [[ ! -f "$RUNNER_DIR/.runner" ]]; then
  echo "[runner-entrypoint] Configuring runner ${RUNNER_NAME} for ${RUNNER_URL}" >&2
  RUNNER_ALLOW_RUNASROOT=1 "$RUNNER_DIR/config.sh" \
    --unattended \
    --url "$RUNNER_URL" \
    --token "$RUNNER_TOKEN" \
    --name "$RUNNER_NAME" \
    --labels "$RUNNER_LABELS"
fi

cd "$RUNNER_DIR"
echo "[runner-entrypoint] Starting GitHub Actions runner" >&2
exec env RUNNER_ALLOW_RUNASROOT=1 ./run.sh
