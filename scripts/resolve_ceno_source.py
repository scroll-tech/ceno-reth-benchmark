#!/usr/bin/env python3
from __future__ import annotations

import argparse
import tomllib
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Resolve the ceno source used by benchmark workflows"
    )
    parser.add_argument("--benchmark-root", required=True)
    parser.add_argument("--requested-ref", default="")
    args = parser.parse_args()

    requested_ref = args.requested_ref.strip()
    if requested_ref:
        print("CENO_USE_PINNED=false")
        print(f"CENO_REQUESTED_VERSION={requested_ref}")
        return 0

    cargo_toml = Path(args.benchmark_root).resolve() / "Cargo.toml"
    data = tomllib.loads(cargo_toml.read_text())
    spec = data["workspace"]["dependencies"]["ceno_cli"]

    print("CENO_USE_PINNED=true")
    print(f"CENO_PINNED_GIT={spec['git']}")
    print(f"CENO_PINNED_PACKAGE={spec.get('package', 'cargo-ceno')}")

    for key in ("branch", "tag", "rev"):
        if key in spec:
            print(f"CENO_PINNED_REF_KIND={key}")
            print(f"CENO_PINNED_REF_VALUE={spec[key]}")
            return 0

    raise SystemExit("ceno_cli dependency is missing branch, tag, or rev")


if __name__ == "__main__":
    raise SystemExit(main())
