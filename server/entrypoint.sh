#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 0 ]]; then
    CMD=("$@")
else
    CMD=("uvicorn" "server.main:app" "--host" "0.0.0.0" "--port" "8000")
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
exit $status
