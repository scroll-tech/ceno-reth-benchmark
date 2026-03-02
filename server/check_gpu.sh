#!/usr/bin/env bash
set -euo pipefail

JOBS_DIR=${JOBS_DIR:-/app/jobs}
ERROR_PATTERN=${GPU_ERROR_PATTERN:-"no CUDA-capable device"}
LOG_GLOB=${GPU_ERROR_LOG_GLOB:-"${JOBS_DIR}"/*/stderr.log}
SCAN_INTERVAL_SEC=${GPU_WATCH_SCAN_INTERVAL_SEC:-5}
LOWER_PATTERN=$(printf '%s' "${ERROR_PATTERN}" | tr '[:upper:]' '[:lower:]')

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
    echo "[check_gpu] CUDA error detected, exiting with failure" >&2
    terminate_children
    exit 1
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
            if [[ "$lower_line" == *"${LOWER_PATTERN}"* ]]; then
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

echo "[check_gpu] starting persistent GPU log watcher (pattern='${ERROR_PATTERN}')" >&2

while true; do
    discover_logs
    sleep "${SCAN_INTERVAL_SEC}"
done
