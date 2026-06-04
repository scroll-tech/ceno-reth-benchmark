#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path
import tomllib

PUBLIC_CENO_GIT = "https://github.com/scroll-tech/ceno.git"
PUBLIC_CENO_REPOSITORY = "scroll-tech/ceno"
PRIVATE_CENO_GIT = "ssh://git@github.com/scroll-tech/ceno-pro.git"
PRIVATE_CENO_REPOSITORY = "scroll-tech/ceno-pro"
PRIVATE_CENO_BRANCH = "ceno"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Resolve the ceno source used by benchmark workflows"
    )
    parser.add_argument("--benchmark-root", required=True)
    parser.add_argument("--requested-ref", default="")
    parser.add_argument("--is-ceno-pro", choices=("0", "1"), default="0")
    args = parser.parse_args()

    is_ceno_pro = args.is_ceno_pro == "1"
    ceno_git = PRIVATE_CENO_GIT if is_ceno_pro else PUBLIC_CENO_GIT
    ceno_repository = PRIVATE_CENO_REPOSITORY if is_ceno_pro else PUBLIC_CENO_REPOSITORY

    print(f"CENO_SOURCE_GIT={ceno_git}")
    print(f"CENO_SOURCE_REPOSITORY={ceno_repository}")

    requested_ref = args.requested_ref.strip()
    if requested_ref:
        print("CENO_USE_PINNED=false")
        print(f"CENO_REQUESTED_VERSION={requested_ref}")
        return 0

    cargo_toml = Path(args.benchmark_root).resolve() / "Cargo.toml"
    data = tomllib.loads(cargo_toml.read_text())
    spec = data["workspace"]["dependencies"]["ceno_cli"]

    print("CENO_USE_PINNED=true")
    if is_ceno_pro:
        print(f"CENO_PINNED_GIT={PRIVATE_CENO_GIT}")
        print(f"CENO_PINNED_PACKAGE={spec.get('package', 'cargo-ceno')}")
        print("CENO_PINNED_REF_KIND=branch")
        print(f"CENO_PINNED_REF_VALUE={PRIVATE_CENO_BRANCH}")
        return 0

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
