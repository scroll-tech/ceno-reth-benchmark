#!/usr/bin/env python3
"""Build the benchmark, then profile exactly one proof with durable diagnostics."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    profile_dir = Path(os.environ["CENO_PROFILE_DIR"])
    profile_dir.mkdir(parents=True, exist_ok=True)
    env_keys = [
        "CENO_GPU_MEM_TRACKING", "CENO_CHIP_PROVING_MODE", "CENO_CHIP_PROVING_LANES",
        "CENO_GPU_LARGE_TASK_BOOKING_MARGIN_MB", "CENO_MAX_CELL_PER_SHARD",
        "CENO_GPU_JAGGED_RESHAPE_LOG_HEIGHT", "CENO_GPU_CACHE_LEVEL",
        "CENO_GPU_ENABLE_WITGEN", "CENO_GPU_WITGEN", "RUST_MIN_STACK",
        "RUSTFLAGS", "CUDA_ARCH", "RUST_LOG", "JEMALLOC_SYS_WITH_MALLOC_CONF",
        "CUDA_VISIBLE_DEVICES",
    ]
    context = {"cwd": str(Path.cwd()), "environment": {
        key: os.environ.get(key) for key in env_keys
    }}

    def run(stage, command, stdout=None):
        record = dict(context, command=command,
                      started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
        process = subprocess.Popen(command, stdout=stdout, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        record.update(pid=process.pid, pgid=process.pid)
        path = profile_dir / (stage + ".json")
        path.write_text(json.dumps(record, indent=2) + "\n")
        status = process.wait()
        record.update(exit_code=status,
                      ended_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
        path.write_text(json.dumps(record, indent=2) + "\n")
        return status

    with (profile_dir / "build.log").open("w") as log:
        status = run("build", [
            "cargo", "build", "--features", "jemalloc,gpu,aot,parallel",
            "--config", "net.git-fetch-with-cli=true", "--release",
            "--bin", "ceno-reth-benchmark-bin",
        ], log)
    if status:
        return status
    binary = Path("target/release/ceno-reth-benchmark-bin").resolve()
    binary_hash = sha256(binary)
    (profile_dir / "binary.sha256").write_text(binary_hash + "  " + str(binary) + "\n")
    prefix = profile_dir / "proof"
    status = run("profile", [
        "nsys", "profile", "--sample=none", "--cpuctxsw=none", "--trace=cuda,nvtx",
        "--output=" + str(prefix), str(binary), *sys.argv[1:],
    ])
    if sha256(binary) != binary_hash:
        raise RuntimeError("Benchmark executable changed during profiling")
    report = prefix.with_suffix(".nsys-rep")
    if not report.is_file():
        raise RuntimeError("Nsight Systems did not produce a report")
    with (profile_dir / "export.log").open("w") as log:
        export_status = run("export", [
            "nsys", "export", "--type", "sqlite", "--output=" + str(prefix.with_suffix(".sqlite")),
            str(report),
        ], log)
    return status or export_status


if __name__ == "__main__":
    sys.exit(main())
