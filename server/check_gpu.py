"""Watch current job output for fatal CUDA errors; exit 75 for host recreation."""

import glob
import os
from pathlib import Path
import time


DEFAULT_PATTERNS = (
    "no CUDA-capable device|failed to initialize nvml|nvidia-smi uuid query failed|"
    "openvm_cuda_common::memory_manager::init|panic_cannot_unwind|"
    "thread caused non-unwinding panic|core dumped"
)


class LogWatcher:
    def __init__(self, log_glob, patterns):
        self.log_glob = log_glob
        self.patterns = [p.lower().encode() for p in patterns.split("|") if p]
        if not self.patterns:
            raise ValueError("GPU error patterns must not be empty")
        self.overlap = max(map(len, self.patterns)) - 1
        self.positions = {}
        # Ignore errors from previous container lifetimes. The entrypoint waits
        # for this snapshot before starting any new prover process.
        for path in glob.glob(self.log_glob):
            try:
                stat = os.stat(path)
                self.positions[path] = ((stat.st_dev, stat.st_ino), stat.st_size, b"")
            except FileNotFoundError:
                pass

    def scan(self):
        for path in glob.glob(self.log_glob):
            try:
                with open(path, "rb") as log:
                    stat = os.fstat(log.fileno())
                    identity = (stat.st_dev, stat.st_ino)
                    previous, offset, suffix = self.positions.get(path, (None, 0, b""))
                    if previous != identity or stat.st_size < offset:
                        offset, suffix = 0, b""
                    log.seek(offset)
                    # Bound each scan to the current size, even if a writer is busy.
                    while offset < stat.st_size:
                        chunk = log.read(min(65536, stat.st_size - offset))
                        if not chunk:
                            break
                        offset += len(chunk)
                        text = suffix + chunk.lower()
                        if any(pattern in text for pattern in self.patterns):
                            return path
                        suffix = text[-self.overlap:] if self.overlap else b""
                    self.positions[path] = (identity, offset, suffix)
            except FileNotFoundError:
                self.positions.pop(path, None)
        return None


def main():
    jobs_dir = os.environ.get("JOBS_DIR", "/app/jobs")
    watcher = LogWatcher(
        os.environ.get("GPU_ERROR_LOG_GLOB", f"{jobs_dir}/*/stderr.log"),
        os.environ.get("GPU_ERROR_PATTERNS")
        or os.environ.get("GPU_ERROR_PATTERN")
        or DEFAULT_PATTERNS,
    )
    interval = float(os.environ.get("GPU_WATCH_SCAN_INTERVAL_SEC", "5"))
    if interval <= 0:
        raise ValueError("GPU watcher interval must be positive")
    if ready_file := os.environ.get("GPU_WATCH_READY_FILE"):
        Path(ready_file).touch()
    print("[check_gpu] watching new job output for CUDA/NVML errors", flush=True)
    while True:
        if path := watcher.scan():
            print(f"[check_gpu] fatal GPU error in {path}; requesting container recreation", flush=True)
            return 75
        time.sleep(interval)


if __name__ == "__main__":
    raise SystemExit(main())
