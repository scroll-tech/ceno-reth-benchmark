#!/usr/bin/env python3
"""Retain source, toolchain and GPU identities alongside benchmark artifacts."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

output = Path("output/identity")
output.mkdir(parents=True, exist_ok=True)


def command(*args):
    try:
        result = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        return {"exit_code": result.returncode, "output": result.stdout}
    except OSError as error:
        return {"error": str(error)}


identity = {
    "benchmark_commit": command("git", "rev-parse", "HEAD"),
    "ceno_checkout_commit": command("git", "-C", "ceno-src", "rev-parse", "HEAD"),
    "gpu_hardware": command("nvidia-smi", "--query-gpu=index,name,uuid,driver_version,memory.total,pci.bus_id", "--format=csv"),
    "nvcc": command("nvcc", "--version"),
    "rustc": command("rustc", "-Vv"),
    "cargo": command("cargo", "-Vv"),
    "sha256": {},
}
paths = [
    "Cargo.lock", "Cargo.toml", ".github/workflows/run-benchmark-v2.yml",
    "scripts/capture_benchmark_identity.py", "target/release/ceno-reth-benchmark-bin",
    "bin/ceno-client-eth/Cargo.lock", "bin/ceno-client-eth/Cargo.toml",
]
for name in paths:
    path = Path(name)
    if path.is_file():
        with path.open("rb") as source:
            identity["sha256"][name] = hashlib.file_digest(source, "sha256").hexdigest()
        if path.name in ("Cargo.lock", "Cargo.toml"):
            destination = output / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)
    else:
        identity["sha256"][name] = None

manifest = tomllib.loads(Path("Cargo.toml").read_text())
identity["gpu_patch"] = manifest["patch"]["https://github.com/scroll-tech/ceno-gpu-mock.git"]
if Path("Cargo.lock").is_file():
    lock = tomllib.loads(Path("Cargo.lock").read_text())
    identity["resolved_prover_sources"] = [
        {key: package[key] for key in ("name", "version", "source") if key in package}
        for package in lock["package"]
        if package["name"] == "cuda_hal" or package["name"].startswith("ceno_")
        or package["name"] == "gkr_iop"
    ]
(output / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
