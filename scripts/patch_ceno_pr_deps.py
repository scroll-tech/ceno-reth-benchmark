#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
import tomllib

DEFAULT_CENO_GIT = "https://github.com/scroll-tech/ceno.git"
CENO_WORKSPACE_CRATES: dict[str, dict[str, object]] = {
    "ceno_emul": {},
    "ceno_host": {},
    "ceno_zkvm": {},
    "ceno_cli": {"package": "cargo-ceno"},
    "ceno_recursion": {},
    "gkr_iop": {},
}
GUEST_CENO_CRATES: dict[str, dict[str, object]] = {
    "ceno_rt": {"default-features": False},
    "ceno_crypto": {"default-features": False},
}
GKR_CRATES = ("ff_ext", "mpcs")


def load_toml(path: Path) -> dict:
    with path.open("rb") as f:
        return tomllib.load(f)


def format_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return f'"{value}"'
    if isinstance(value, list):
        return "[" + ", ".join(format_value(v) for v in value) + "]"
    raise TypeError(f"Unsupported TOML value: {value!r}")


def format_inline_table(items: list[tuple[str, object]]) -> str:
    rendered = ", ".join(f"{key} = {format_value(value)}" for key, value in items)
    return "{ " + rendered + " }"


def replace_inline_dep(text: str, dep_name: str, inline_value: str, path: Path) -> str:
    pattern = re.compile(rf"^(?P<prefix>{re.escape(dep_name)}\s*=\s*)\{{.*\}}\s*$", re.MULTILINE)
    new_text, count = pattern.subn(rf"\g<prefix>{inline_value}", text, count=1)
    if count != 1:
        raise RuntimeError(f"Failed to replace dependency '{dep_name}' in {path}")
    return new_text


def patch_workspace_cargo(
    benchmark_cargo: Path, ceno_cargo: Path, ceno_git: str, ceno_ref: str
) -> None:
    benchmark_text = benchmark_cargo.read_text()
    ceno_toml = load_toml(ceno_cargo)
    ceno_workspace_deps = ceno_toml["workspace"]["dependencies"]

    for dep_name, extra in CENO_WORKSPACE_CRATES.items():
        items: list[tuple[str, object]] = [("git", ceno_git)]
        if "package" in extra:
            items.append(("package", extra["package"]))
        items.append(("rev", ceno_ref))
        benchmark_text = replace_inline_dep(
            benchmark_text,
            dep_name,
            format_inline_table(items),
            benchmark_cargo,
        )

    for dep_name in GKR_CRATES:
        spec = ceno_workspace_deps[dep_name]
        items: list[tuple[str, object]] = [("git", spec["git"])]
        if "package" in spec:
            items.append(("package", spec["package"]))
        for key in ("branch", "tag", "rev"):
            if key in spec:
                items.append((key, spec[key]))
                break
        benchmark_text = replace_inline_dep(
            benchmark_text,
            dep_name,
            format_inline_table(items),
            benchmark_cargo,
        )

    benchmark_cargo.write_text(benchmark_text)


def patch_guest_cargo(guest_cargo: Path, ceno_git: str, ceno_ref: str) -> None:
    guest_text = guest_cargo.read_text()
    for dep_name, extra in GUEST_CENO_CRATES.items():
        items: list[tuple[str, object]] = [("git", ceno_git), ("rev", ceno_ref)]
        for key, value in extra.items():
            items.append((key, value))
        guest_text = replace_inline_dep(
            guest_text,
            dep_name,
            format_inline_table(items),
            guest_cargo,
        )
    guest_cargo.write_text(guest_text)


def main() -> int:
    parser = argparse.ArgumentParser(description="Patch ceno-reth-benchmark to use a Ceno PR ref")
    parser.add_argument("--benchmark-root", required=True)
    parser.add_argument("--ceno-root", required=True)
    parser.add_argument("--ceno-ref", required=True)
    parser.add_argument("--ceno-git", default=DEFAULT_CENO_GIT)
    args = parser.parse_args()

    benchmark_root = Path(args.benchmark_root).resolve()
    ceno_root = Path(args.ceno_root).resolve()

    benchmark_cargo = benchmark_root / "Cargo.toml"
    guest_cargo = benchmark_root / "bin" / "ceno-client-eth" / "Cargo.toml"
    ceno_cargo = ceno_root / "Cargo.toml"

    if not benchmark_cargo.exists() or not guest_cargo.exists() or not ceno_cargo.exists():
        raise SystemExit("Required Cargo.toml file is missing")

    patch_workspace_cargo(benchmark_cargo, ceno_cargo, args.ceno_git, args.ceno_ref)
    patch_guest_cargo(guest_cargo, args.ceno_git, args.ceno_ref)
    return 0


if __name__ == "__main__":
    sys.exit(main())
