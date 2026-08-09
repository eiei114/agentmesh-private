#!/usr/bin/env python3
"""Regression tests for repair_sync_local_main_with_origin.py."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("repair_sync_local_main_with_origin.py")
SPEC = importlib.util.spec_from_file_location("repair_sync_local_main_with_origin", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
repair_sync = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = repair_sync
SPEC.loader.exec_module(repair_sync)


def run(command: list[str], cwd: Path, *, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed: {' '.join(command)}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def configure_user(repo: Path) -> None:
    run(["git", "config", "user.email", "agentmesh@example.invalid"], repo)
    run(["git", "config", "user.name", "AgentMesh Test"], repo)


def commit_file(repo: Path, relative_path: str, content: str, message: str) -> None:
    path = repo / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    run(["git", "add", relative_path], repo)
    run(["git", "commit", "-m", message], repo)


class RepairSyncTests(unittest.TestCase):
    def make_repo(self, tmp_path: Path, *, remote_ahead: bool = False, checkout_work_branch: bool = False) -> Path:
        remote = tmp_path / "remote.git"
        seed = tmp_path / "seed"
        repo = tmp_path / "repo"
        upstream = tmp_path / "upstream"

        run(["git", "init", "--bare", "--initial-branch=main", str(remote)], tmp_path)
        run(["git", "init", "--initial-branch=main", str(seed)], tmp_path)
        configure_user(seed)
        commit_file(seed, "README.md", "initial\n", "initial")
        run(["git", "remote", "add", "origin", str(remote)], seed)
        run(["git", "push", "-u", "origin", "main"], seed)

        run(["git", "clone", str(remote), str(repo)], tmp_path)
        configure_user(repo)

        if remote_ahead:
            run(["git", "clone", str(remote), str(upstream)], tmp_path)
            configure_user(upstream)
            commit_file(upstream, "README.md", "initial\nremote advance\n", "remote advance")
            run(["git", "push", "origin", "main"], upstream)

        if checkout_work_branch:
            run(["git", "switch", "-c", "work"], repo)

        return repo

    def run_script(self, repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return run([sys.executable, str(SCRIPT_PATH), "--repo", str(repo), *args], repo, check=False)

    def test_fast_forward_ref_path_when_main_is_not_checked_out(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True, checkout_work_branch=True)

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("repair_action=fast_forward_ref", result.stdout)
            self.assertIn("after_behind=0", result.stdout)
            self.assertIn("request_action=seed_app_requests", result.stdout)

    def test_repair_lock_rejects_concurrent_helper_operation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True, checkout_work_branch=True)
            resolved_repo = repair_sync.resolve_repo(repo)

            with repair_sync.repair_lock(resolved_repo, "main"):
                result = self.run_script(repo)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("repair lock already held for refs/heads/main", result.stderr)

    def test_check_requires_clean_worktree_before_request_seeding(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp))
            (repo / "scratch.txt").write_text("dirty\n", encoding="utf-8")

            result = self.run_script(repo, "--check")

            self.assertEqual(result.returncode, 1)
            self.assertIn("dirty_count=1", result.stdout)
            self.assertIn("repo_main_aligned=yes", result.stdout)
            self.assertIn("request_action=repair_first", result.stdout)


if __name__ == "__main__":
    unittest.main()
