#!/usr/bin/env python3
"""Assert workspace MSRV metadata matches pinned rust-toolchain and CI."""

from __future__ import annotations

import re
import sys
from pathlib import Path


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def read_workspace_rust_version(root: Path) -> str:
    cargo_toml = (root / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^\s*rust-version\s*=\s*"([^"]+)"\s*$', cargo_toml, re.MULTILINE)
    if match is None:
        raise ValueError("workspace Cargo.toml is missing rust-version")
    return match.group(1)


def read_rust_toolchain_channel(root: Path) -> str:
    toolchain_toml = (root / "rust-toolchain.toml").read_text(encoding="utf-8")
    match = re.search(r'^\s*channel\s*=\s*"([^"]+)"\s*$', toolchain_toml, re.MULTILINE)
    if match is None:
        raise ValueError("rust-toolchain.toml is missing channel")
    return match.group(1)


def read_ci_toolchain_versions(root: Path) -> list[str]:
    versions: list[str] = []
    for workflow in sorted((root / ".github" / "workflows").glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        versions.extend(re.findall(r'toolchain:\s*"([^"]+)"', text))
        versions.extend(re.findall(r'rust-version:\s*"([^"]+)"', text))
    if not versions:
        raise ValueError("no Rust toolchain pins found in .github/workflows")
    return versions


def normalize_rust_version(version: str) -> str:
    parts = version.split(".")
    if len(parts) < 2:
        raise ValueError(f"invalid Rust version: {version!r}")
    return f"{parts[0]}.{parts[1]}"


def assert_baseline(root: Path | None = None) -> None:
    root = repo_root() if root is None else root
    workspace_msrv = read_workspace_rust_version(root)
    toolchain_channel = read_rust_toolchain_channel(root)
    ci_versions = read_ci_toolchain_versions(root)

    expected = normalize_rust_version(toolchain_channel)
    if normalize_rust_version(workspace_msrv) != expected:
        raise AssertionError(
            "workspace rust-version drift: "
            f"Cargo.toml={workspace_msrv!r}, rust-toolchain.toml channel={toolchain_channel!r}"
        )

    mismatched = [
        version
        for version in ci_versions
        if normalize_rust_version(version) != expected
    ]
    if mismatched:
        raise AssertionError(
            "CI Rust toolchain drift: "
            f"expected {expected!r}, found {sorted(set(mismatched))!r}"
        )


def main() -> int:
    try:
        assert_baseline()
    except (AssertionError, ValueError) as exc:
        print(f"rust toolchain baseline check failed: {exc}", file=sys.stderr)
        return 1
    print("rust toolchain baseline check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
