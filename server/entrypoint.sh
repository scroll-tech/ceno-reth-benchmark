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
GPU_READY_MAX_ATTEMPTS="${GPU_READY_MAX_ATTEMPTS:-6}"
GPU_UNAVAILABLE_EXIT_CODE=75
STACK_STOP_TIMEOUT_SEC="${STACK_STOP_TIMEOUT_SEC:-10}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WATCH_READY_DIR="$(mktemp -d)"

wait_for_gpu() {
    local gpu_check_attempt=0 gpu_check_error gpu_check_status gpu_count gpu_uuids
    if [[ "$STARTUP_GPU_CHECK" != "1" ]]; then
        return 0
    fi

    while true; do
        gpu_check_attempt=$((gpu_check_attempt + 1))
        gpu_check_error=""
        if ! command -v nvidia-smi >/dev/null 2>&1; then
            gpu_check_error="nvidia-smi is not installed or not in PATH"
        else
            if gpu_uuids="$(timeout -k 1s 10s nvidia-smi --query-gpu=uuid --format=csv,noheader 2>&1)"; then
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
        if (( gpu_check_attempt >= GPU_READY_MAX_ATTEMPTS )); then
            echo "[entrypoint] GPU unavailable after ${gpu_check_attempt} attempt(s): ${gpu_check_error}; container recreation required" >&2
            return "$GPU_UNAVAILABLE_EXIT_CODE"
        fi
        echo "[entrypoint] GPU unavailable: ${gpu_check_error}; polling again in ${GPU_READY_POLL_INTERVAL_SEC}s" >&2
        sleep "${GPU_READY_POLL_INTERVAL_SEC}"
    done
}

terminate_current_stack() {
    local deadline=$((SECONDS + STACK_STOP_TIMEOUT_SEC))
    if [[ -n "${CHECK_PID:-}" ]]; then
        kill "${CHECK_PID}" 2>/dev/null || true
    fi
    if [[ -n "${UVICORN_PID:-}" ]]; then
        kill -- "-${UVICORN_PID}" 2>/dev/null || true
        kill "${UVICORN_PID}" 2>/dev/null || true
        while kill -0 -- "-${UVICORN_PID}" 2>/dev/null || kill -0 "${UVICORN_PID}" 2>/dev/null; do
            if (( SECONDS >= deadline )); then
                echo "[entrypoint] server shutdown timed out; killing remaining processes" >&2
                kill -KILL -- "-${UVICORN_PID}" 2>/dev/null || true
                kill -KILL "${UVICORN_PID}" 2>/dev/null || true
                break
            fi
            sleep 0.1
        done
    fi
    if [[ -n "${CHECK_PID:-}" ]]; then
        kill -KILL "${CHECK_PID}" 2>/dev/null || true
    fi
    # Never wait indefinitely for a process stuck in the GPU driver. Exiting
    # PID 1 lets the host supervisor recreate the container and its bindings.
    CHECK_PID=""
    UVICORN_PID=""
}
trap 'terminate_current_stack; rm -rf -- "$WATCH_READY_DIR"' EXIT
trap 'exit 0' INT TERM

while true; do
    wait_for_gpu

    rm -f -- "$WATCH_READY_DIR/ready"
    GPU_WATCH_READY_FILE="$WATCH_READY_DIR/ready" python3 "$SCRIPT_DIR/check_gpu.py" &
    CHECK_PID=$!
    ready_deadline=$((SECONDS + 10))
    until [[ -f "$WATCH_READY_DIR/ready" ]]; do
        if ! kill -0 "$CHECK_PID" 2>/dev/null || (( SECONDS >= ready_deadline )); then
            echo "[entrypoint] GPU watcher failed to become ready" >&2
            exit 1
        fi
        sleep 0.05
    done

    setsid "${CMD[@]}" &
    UVICORN_PID=$!

    set +e
    wait -n "${UVICORN_PID}" "${CHECK_PID}"
    status=$?
    set -e

    terminate_current_stack

    if [[ "$status" -eq "$GPU_UNAVAILABLE_EXIT_CODE" ]]; then
        echo "[entrypoint] GPU binding is unusable; exiting with status=${status} so the container can be recreated" >&2
        exit "$status"
    fi

    echo "[entrypoint] server stack exited with status=${status}; restarting after ${GPU_UNAVAILABLE_RESTART_DELAY_SEC}s" >&2
    sleep "${GPU_UNAVAILABLE_RESTART_DELAY_SEC}"
done
