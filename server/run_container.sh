#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="${CONTAINER_NAME:-reth-server}"
GPU_RECREATE_DELAY_SEC="${GPU_RECREATE_DELAY_SEC:-10}"
HOST_GPU_POLL_INTERVAL_SEC="${HOST_GPU_POLL_INTERVAL_SEC:-10}"
GPU_UNAVAILABLE_EXIT_CODE=75

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 [docker run options] <image> [command ...]" >&2
  exit 2
fi

cleanup() {
  docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

wait_for_host_gpu() {
  if ! command -v nvidia-smi >/dev/null 2>&1; then
    return 0
  fi
  local gpu_uuids
  until gpu_uuids="$(timeout -k 1s 10s nvidia-smi --query-gpu=uuid --format=csv,noheader 2>/dev/null)" && [[ -n "${gpu_uuids//[[:space:]]/}" ]]; do
    echo "[run_container] host GPU unavailable; polling again in ${HOST_GPU_POLL_INTERVAL_SEC}s" >&2
    sleep "$HOST_GPU_POLL_INTERVAL_SEC"
  done
}

while true; do
  wait_for_host_gpu
  cleanup
  echo "[run_container] creating container ${CONTAINER_NAME}" >&2

  set +e
  docker run --name "$CONTAINER_NAME" "$@"
  status=$?
  set -e

  cleanup
  if [[ $status -ne $GPU_UNAVAILABLE_EXIT_CODE ]]; then
    exit "$status"
  fi

  echo "[run_container] container lost GPU access; recreating in ${GPU_RECREATE_DELAY_SEC}s" >&2
  sleep "$GPU_RECREATE_DELAY_SEC"
done
