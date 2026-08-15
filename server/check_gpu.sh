#!/usr/bin/env bash
set -euo pipefail

JOBS_DIR=${JOBS_DIR:-/app/jobs}
ERROR_PATTERN=${GPU_ERROR_PATTERN:-"no CUDA-capable device"}
ERROR_PATTERNS=${GPU_ERROR_PATTERNS:-"no CUDA-capable device|failed to initialize nvml|nvidia-smi uuid query failed|openvm_cuda_common::memory_manager::init|panic_cannot_unwind|thread caused non-unwinding panic|core dumped"}
LOG_GLOB=${GPU_ERROR_LOG_GLOB:-"${JOBS_DIR}"/*/stderr.log}
SCAN_INTERVAL_SEC=${GPU_WATCH_SCAN_INTERVAL_SEC:-5}
LOWER_PATTERNS=$(printf '%s' "${ERROR_PATTERNS:-$ERROR_PATTERN}" | tr '[:upper:]' '[:lower:]')
IFS='|' read -r -a CUDA_ERROR_PATTERNS <<<"$LOWER_PATTERNS"

declare -A WATCHERS=()
PARENT_PID=$$

terminate_children() {
    for pid in "${WATCHERS[@]}"; do
        if [[ -n "${pid}" ]]; then
            kill "$pid" 2>/dev/null || true
        fi
    done
}

handle_usr1() {
    echo "[check_gpu] CUDA/NVML error detected, requesting container recreation" >&2
    terminate_children
    exit 75
}

matches_cuda_error() {
    local line="$1"
    local pattern
    if [[ "$line" == *"[prove_block.sh] gpu unavailable:"* ]]; then
        return 1
    fi
    for pattern in "${CUDA_ERROR_PATTERNS[@]}"; do
        [[ -z "$pattern" ]] && continue
        if [[ "$line" == *"$pattern"* ]]; then
            return 0
        fi
    done
    return 1
}

trap terminate_children EXIT
trap handle_usr1 USR1
trap 'terminate_children; exit 0' TERM INT

start_watcher() {
    local log_path="$1"
    {
        tail -n 0 -F "$log_path" | while IFS= read -r line; do
            local lower_line
            lower_line=$(printf '%s' "$line" | tr '[:upper:]' '[:lower:]')
            if matches_cuda_error "$lower_line"; then
                echo "[check_gpu] detected CUDA error pattern in ${log_path}: $line" >&2
                kill -s USR1 "${PARENT_PID}"
                exit 0
            fi
        done
    } &
    WATCHERS["$log_path"]=$!
    echo "[check_gpu] following ${log_path} (pid=${WATCHERS["$log_path"]})" >&2
}

discover_logs() {
    shopt -s nullglob
    for log_path in ${LOG_GLOB}; do
        [[ -f "$log_path" ]] || continue
        local current_pid=${WATCHERS["$log_path"]:-}
        if [[ -z "$current_pid" ]] || ! kill -0 "$current_pid" 2>/dev/null; then
            start_watcher "$log_path"
        fi
    done
    shopt -u nullglob
}

echo "[check_gpu] starting persistent GPU log watcher (patterns='${ERROR_PATTERNS:-$ERROR_PATTERN}')" >&2

while true; do
    discover_logs
    sleep "${SCAN_INTERVAL_SEC}"
done
