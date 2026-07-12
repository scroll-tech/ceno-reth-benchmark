#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 0 ]]; then
    CMD=("$@")
else
    CMD=("uvicorn" "server.main:app" "--host" "0.0.0.0" "--port" "8000")
fi

GPU_UNAVAILABLE_RESTART_DELAY_SEC="${GPU_UNAVAILABLE_RESTART_DELAY_SEC:-60}"
STARTUP_GPU_CHECK="${STARTUP_GPU_CHECK:-1}"

if [[ "$STARTUP_GPU_CHECK" == "1" ]]; then
    if ! command -v nvidia-smi >/dev/null 2>&1 || ! nvidia-smi -L >/dev/null 2>&1; then
        echo "[entrypoint] GPU unavailable at startup; waiting ${GPU_UNAVAILABLE_RESTART_DELAY_SEC}s before container restart" >&2
        sleep "${GPU_UNAVAILABLE_RESTART_DELAY_SEC}"
        exit 1
    fi
fi

/app/server/check_gpu.sh &
CHECK_PID=$!

cleanup() {
    kill "${CHECK_PID}" 2>/dev/null || true
    if [[ -n "${UVICORN_PID:-}" ]]; then
        kill "${UVICORN_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

"${CMD[@]}" &
UVICORN_PID=$!

set +e
wait -n "${UVICORN_PID}" "${CHECK_PID}"
status=$?
set -e

cleanup
wait "${UVICORN_PID}" 2>/dev/null || true
wait "${CHECK_PID}" 2>/dev/null || true
if [[ "$status" -ne 0 ]]; then
    echo "[entrypoint] server stack exited with status=${status}; waiting ${GPU_UNAVAILABLE_RESTART_DELAY_SEC}s before container restart" >&2
    sleep "${GPU_UNAVAILABLE_RESTART_DELAY_SEC}"
fi
exit $status
