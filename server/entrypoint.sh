#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -gt 0 ]]; then
    CMD=("$@")
else
    CMD=("uvicorn" "server.main:app" "--host" "0.0.0.0" "--port" "8000")
fi

GPU_UNAVAILABLE_RESTART_DELAY_SEC="${GPU_UNAVAILABLE_RESTART_DELAY_SEC:-60}"
STARTUP_GPU_CHECK="${STARTUP_GPU_CHECK:-1}"
GPU_READY_POLL_INTERVAL_SEC="${GPU_READY_POLL_INTERVAL_SEC:-10}"

wait_for_gpu() {
    local gpu_check_error gpu_check_status gpu_count gpu_uuids
    if [[ "$STARTUP_GPU_CHECK" != "1" ]]; then
        return 0
    fi

    while true; do
        gpu_check_error=""
        if ! command -v nvidia-smi >/dev/null 2>&1; then
            gpu_check_error="nvidia-smi is not installed or not in PATH"
        else
            if gpu_uuids="$(nvidia-smi --query-gpu=uuid --format=csv,noheader 2>&1)"; then
                gpu_check_status=0
            else
                gpu_check_status=$?
            fi
            if [[ $gpu_check_status -eq 0 && -n "${gpu_uuids//[[:space:]]/}" ]]; then
                gpu_count="$(printf '%s\n' "$gpu_uuids" | sed '/^[[:space:]]*$/d' | wc -l | tr -d '[:space:]')"
                echo "[entrypoint] GPU ready (${gpu_count} device(s))" >&2
                return 0
            fi
            if [[ $gpu_check_status -ne 0 ]]; then
                gpu_check_error="nvidia-smi UUID query failed (status=${gpu_check_status}): ${gpu_uuids%%$'\n'*}"
            else
                gpu_check_error="nvidia-smi UUID query returned no devices"
            fi
        fi
        echo "[entrypoint] GPU unavailable: ${gpu_check_error}; polling again in ${GPU_READY_POLL_INTERVAL_SEC}s" >&2
        sleep "${GPU_READY_POLL_INTERVAL_SEC}"
    done
}

terminate_current_stack() {
    if [[ -n "${CHECK_PID:-}" ]]; then
        kill "${CHECK_PID}" 2>/dev/null || true
    fi
    if [[ -n "${UVICORN_PID:-}" ]]; then
        kill -- "-${UVICORN_PID}" 2>/dev/null || true
        kill "${UVICORN_PID}" 2>/dev/null || true
    fi
}
trap 'terminate_current_stack; exit 0' INT TERM

while true; do
    wait_for_gpu

    /app/server/check_gpu.sh &
    CHECK_PID=$!

    setsid "${CMD[@]}" &
    UVICORN_PID=$!

    set +e
    wait -n "${UVICORN_PID}" "${CHECK_PID}"
    status=$?
    set -e

    terminate_current_stack
    wait "${UVICORN_PID}" 2>/dev/null || true
    wait "${CHECK_PID}" 2>/dev/null || true
    CHECK_PID=""
    UVICORN_PID=""

    echo "[entrypoint] server stack exited with status=${status}; restarting after ${GPU_UNAVAILABLE_RESTART_DELAY_SEC}s" >&2
    sleep "${GPU_UNAVAILABLE_RESTART_DELAY_SEC}"
done
