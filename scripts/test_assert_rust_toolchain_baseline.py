#!/usr/bin/env python3
"""Regression tests for assert_rust_toolchain_baseline.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("assert_rust_toolchain_baseline.py")
SPEC = importlib.util.spec_from_file_location("assert_rust_toolchain_baseline", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
assert_baseline = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = assert_baseline
SPEC.loader.exec_module(assert_baseline)


class RustToolchainBaselineTests(unittest.TestCase):
    def write_baseline(self, root: Path) -> None:
        (root / "Cargo.toml").write_text(
            'rust-version = "1.97"\n',
            encoding="utf-8",
        )
        (root / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.97.0"\n',
            encoding="utf-8",
        )
        workflows = root / ".github" / "workflows"
        workflows.mkdir(parents=True)
        (workflows / "ci.yml").write_text(
            'toolchain: "1.97.0"\nrust-version: "1.97.0"\n',
            encoding="utf-8",
        )

    def test_repo_baseline_is_aligned(self) -> None:
        assert_baseline.assert_baseline(assert_baseline.repo_root())

    def test_detects_workspace_msrv_drift(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_baseline(root)
            (root / "Cargo.toml").write_text(
                'rust-version = "1.85"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(AssertionError, "workspace rust-version drift"):
                assert_baseline.assert_baseline(root)

    def test_detects_ci_toolchain_drift(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_baseline(root)
            (root / ".github" / "workflows" / "ci.yml").write_text(
                'toolchain: "1.85.0"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(AssertionError, "CI Rust toolchain drift"):
                assert_baseline.assert_baseline(root)


if __name__ == "__main__":
    unittest.main()
