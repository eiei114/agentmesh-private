#!/usr/bin/env python3
"""Regression tests for repair_sync_local_main_with_origin.py."""

from __future__ import annotations

import errno
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


def commit_file(repo: Path, relative_path: str, content: str, message: str, *, force: bool = False) -> None:
    path = repo / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    run(["git", "add", *(["-f"] if force else []), relative_path], repo)
    run(["git", "commit", "-m", message], repo)


def make_upstream(
    tmp_path: Path,
    commit_paths: dict[str, str],
    message: str,
    *,
    remove_paths: tuple[str, ...] = (),
) -> Path:
    upstream = tmp_path / "upstream"
    if not upstream.exists():
        run(["git", "clone", str(tmp_path / "remote.git"), str(upstream)], tmp_path)
        configure_user(upstream)
    for relative_path in remove_paths:
        entry = upstream / relative_path
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()
    for relative_path, content in commit_paths.items():
        path = upstream / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    run(["git", "add", "--force", "-A", *commit_paths, *remove_paths], upstream)
    run(["git", "commit", "-m", message], upstream)
    run(["git", "push", "origin", "main"], upstream)
    return upstream


def add_submodule(repo: Path, tmp_path: Path) -> None:
    submodule_remote = tmp_path / "submodule.git"
    submodule_seed = tmp_path / "submodule-seed"
    run(["git", "init", "--bare", "--initial-branch=main", str(submodule_remote)], tmp_path)
    run(["git", "init", "--initial-branch=main", str(submodule_seed)], tmp_path)
    configure_user(submodule_seed)
    (submodule_seed / "file.txt").write_text("submodule initial\n", encoding="utf-8")
    run(["git", "add", "file.txt"], submodule_seed)
    run(["git", "commit", "-m", "submodule seed"], submodule_seed)
    run(["git", "remote", "add", "origin", str(submodule_remote)], submodule_seed)
    run(["git", "push", "-u", "origin", "main"], submodule_seed)
    run(
        ["git", "-c", "protocol.file.allow=always", "submodule", "add", str(submodule_remote), "mod"],
        repo,
    )
    run(["git", "commit", "-m", "add submodule"], repo)
    run(["git", "push", "origin", "main"], repo)


def advance_upstream(tmp_path: Path, relative_path: str, content: str, message: str) -> None:
    upstream = tmp_path / "upstream"
    if not upstream.exists():
        run(["git", "clone", str(tmp_path / "remote.git"), str(upstream)], tmp_path)
        configure_user(upstream)
    path = upstream / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    run(["git", "add", relative_path], upstream)
    run(["git", "commit", "-m", message], upstream)
    run(["git", "push", "origin", "main"], upstream)


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
        commit_file(seed, "NOTES.md", "notes\n", "notes")
        commit_file(seed, ".gitignore", "ignored/\n", "ignore directory")
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

    def test_fast_forward_uses_temporary_worktree_when_main_is_not_checked_out(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True, checkout_work_branch=True)

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("repair_action=fast_forward_temporary_worktree", result.stdout)
            self.assertIn("after_behind=0", result.stdout)
            self.assertIn("request_action=seed_app_requests", result.stdout)
            worktrees = run(["git", "worktree", "list", "--porcelain"], repo).stdout
            self.assertNotIn("agentmesh-repair-worktree-", worktrees)

    def test_repair_lock_rejects_concurrent_helper_operation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True, checkout_work_branch=True)
            resolved_repo = repair_sync.resolve_repo(repo)

            with repair_sync.repair_lock(resolved_repo, "main"):
                result = self.run_script(repo)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("repair lock already held for refs/heads/main", result.stderr)

    def test_repair_lock_reclaims_dead_owner(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True, checkout_work_branch=True)
            helper = """
import sys
import time
from pathlib import Path
sys.path.insert(0, sys.argv[2])
import repair_sync_local_main_with_origin as repair
repo = repair.resolve_repo(Path(sys.argv[1]))
with repair.repair_lock(repo, "main"):
    print("locked", flush=True)
    time.sleep(60)
"""
            process = subprocess.Popen(
                [sys.executable, "-c", helper, str(repo), str(SCRIPT_PATH.parent)],
                cwd=repo,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                assert process.stdout is not None
                self.assertEqual(process.stdout.readline().strip(), "locked")
                process.kill()
                process.communicate(timeout=10)

                lock_dir = repair_sync.git_common_dir(repo) / "agentmesh-repair-main.lock"
                self.assertTrue(lock_dir.is_dir())
                result = self.run_script(repo)

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("repair_action=fast_forward_temporary_worktree", result.stdout)
                self.assertFalse(lock_dir.exists())
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=10)

    def test_repair_lock_cleans_up_owner_metadata_write_failure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp))
            resolved_repo = repair_sync.resolve_repo(repo)
            common_dir = repair_sync.git_common_dir(repo)

            with mock.patch.object(Path, "write_text", side_effect=OSError("disk failure")):
                with self.assertRaisesRegex(repair_sync.RepairError, "could not prepare repair lock"):
                    with repair_sync.repair_lock(resolved_repo, "main"):
                        self.fail("lock body must not run")

            leftovers = list(common_dir.glob("*agentmesh-repair-main.lock*"))
            self.assertEqual(leftovers, [])

    def test_ordinary_checkout_race_does_not_move_branch_behind_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path, remote_ahead=True, checkout_work_branch=True)
            repair_sync.fetch_remote_branch(repo, "main", "origin")
            before = repair_sync.divergence(repo, "main", "origin")
            other = tmp_path / "main-worktree"
            original_checked_out_worktree = repair_sync.checked_out_worktree

            def checkout_after_inspection(_repo: Path, _branch_ref: str) -> None:
                run(["git", "worktree", "add", str(other), "main"], repo)
                return None

            repair_sync.checked_out_worktree = checkout_after_inspection
            try:
                with self.assertRaisesRegex(
                    repair_sync.RepairError, "could not create coordinated temporary worktree"
                ):
                    repair_sync.fast_forward(repo, "main", "origin", before)
            finally:
                repair_sync.checked_out_worktree = original_checked_out_worktree

            after = run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip()
            self.assertEqual(after, before.local_sha)
            self.assertEqual((other / "README.md").read_text(encoding="utf-8"), "initial\n")

    def test_check_requires_clean_worktree_before_request_seeding(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp))
            (repo / "scratch.txt").write_text("dirty\n", encoding="utf-8")

            result = self.run_script(repo, "--check")

            self.assertEqual(result.returncode, 1)
            self.assertIn("dirty_count=1", result.stdout)
            self.assertIn("repo_main_aligned=yes", result.stdout)
            self.assertIn("request_action=repair_first", result.stdout)

    def test_fast_forward_preserves_local_changes_outside_the_incoming_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True)
            local_change = "notes\nlocal-only\n"
            (repo / "NOTES.md").write_text(local_change, encoding="utf-8")
            scratch = repo / "scratch.txt"
            scratch.write_text("untracked\n", encoding="utf-8")

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("repair_action=fast_forward_worktree_preserving_local_changes", result.stdout)
            self.assertIn("local_changes=preserved", result.stdout)
            self.assertIn("preserved_local_paths=NOTES.md,scratch.txt", result.stdout)
            self.assertIn("after_behind=0", result.stdout)
            self.assertIn("repo_main_behind=absent", result.stdout)
            self.assertIn("repo_main_aligned=yes", result.stdout)
            self.assertEqual((repo / "NOTES.md").read_text(encoding="utf-8"), local_change)
            self.assertEqual(scratch.read_text(encoding="utf-8"), "untracked\n")
            self.assertIn("remote advance", (repo / "README.md").read_text(encoding="utf-8"))

    def test_fast_forward_keeps_main_when_a_local_change_collides_with_a_new_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True)
            upstream = Path(tmp) / "upstream"
            commit_file(upstream, "new.md", "upstream\n", "upstream adds new.md")
            run(["git", "push", "origin", "main"], upstream)
            (repo / "new.md").write_text("local\n", encoding="utf-8")
            before_sha = run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip()

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 1)
            self.assertIn("new.md", result.stderr)
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(), before_sha
            )
            self.assertEqual((repo / "new.md").read_text(encoding="utf-8"), "local\n")

    def test_fast_forward_keeps_main_when_an_ignored_file_would_be_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make_repo(Path(tmp), remote_ahead=True)
            upstream = Path(tmp) / "upstream"
            commit_file(upstream, "ignored/data.txt", "upstream\n", "upstream adds ignored file", force=True)
            run(["git", "push", "origin", "main"], upstream)
            local_only = repo / "ignored" / "data.txt"
            local_only.parent.mkdir(parents=True, exist_ok=True)
            local_only.write_text("local-ignored\n", encoding="utf-8")
            before_sha = run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip()

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 1)
            self.assertIn("ignored/data.txt", result.stderr)
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(), before_sha
            )
            self.assertEqual(local_only.read_text(encoding="utf-8"), "local-ignored\n")

    def test_fast_forward_keeps_main_when_an_ignored_file_blocks_an_incoming_directory(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path, remote_ahead=True)
            make_upstream(
                tmp_path,
                {"ignored/data.txt": "upstream\n"},
                "upstream adds ignored directory",
            )
            blocking_file = repo / "ignored"
            blocking_file.write_text("local-ignored-payload\n", encoding="utf-8")
            before_sha = run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip()

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("repair_action=refused_local_changes_collide", result.stdout)
            self.assertIn("local_changes=collision", result.stdout)
            self.assertIn("collision_paths=ignored", result.stdout)
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(), before_sha
            )
            self.assertTrue(blocking_file.is_file())
            self.assertEqual(blocking_file.read_text(encoding="utf-8"), "local-ignored-payload\n")

    def test_fast_forward_allows_a_tracked_file_replaced_by_an_incoming_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path)
            tracked_file = repo / "cache"
            tracked_file.write_text("tracked\n", encoding="utf-8")
            run(["git", "add", "cache"], repo)
            run(["git", "commit", "-m", "track cache file"], repo)
            run(["git", "push", "origin", "main"], repo)
            make_upstream(
                tmp_path,
                {"cache/data.txt": "upstream\n"},
                "upstream replaces cache file with a directory",
                remove_paths=("cache",),
            )

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("repair_action=fast_forward_worktree", result.stdout)
            self.assertIn("local_changes=none", result.stdout)
            self.assertNotIn("collision_paths", result.stdout)
            self.assertEqual((repo / "cache" / "data.txt").read_text(encoding="utf-8"), "upstream\n")
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(),
                run(["git", "rev-parse", "refs/remotes/origin/main"], repo).stdout.strip(),
            )

    def test_fast_forward_preserves_a_modified_symlink_by_target(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path)
            link = repo / "link"
            try:
                link.symlink_to("NOTES.md")
            except (OSError, NotImplementedError):
                self.skipTest("symlinks are not supported in this environment")
            if not (link.is_symlink() and os.readlink(link) == "NOTES.md"):
                self.skipTest("this Git checkout stores symlinks as plain files")
            run(["git", "add", "link"], repo)
            run(["git", "commit", "-m", "track symlink"], repo)
            run(["git", "push", "origin", "main"], repo)
            link.unlink()
            link.symlink_to("README.md")
            make_upstream(tmp_path, {"README.md": "initial\nremote advance\n"}, "remote advance")

            result = self.run_script(repo)

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("repair_action=fast_forward_worktree_preserving_local_changes", result.stdout)
            self.assertIn("local_changes=preserved", result.stdout)
            self.assertNotIn("changed preserved local paths", result.stderr)
            self.assertEqual(os.readlink(link), "README.md")
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(),
                run(["git", "rev-parse", "refs/remotes/origin/main"], repo).stdout.strip(),
            )

    def test_fast_forward_preserves_a_dirty_submodule_without_reading_it_as_a_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path)
            add_submodule(repo, tmp_path)
            advance_upstream(tmp_path, "README.md", "initial\nremote advance\n", "remote advance")
            (repo / "mod" / "file.txt").write_text("submodule local change\n", encoding="utf-8")

            result = self.run_script(repo)

            self.assertNotIn("Traceback", result.stderr)
            self.assertNotIn("changed preserved local paths", result.stderr)
            self.assertIn("repair_action=fast_forward_worktree_preserving_local_changes", result.stdout)
            self.assertIn("preserved_local_paths=mod", result.stdout)
            self.assertEqual(
                (repo / "mod" / "file.txt").read_text(encoding="utf-8"), "submodule local change\n"
            )
            self.assertEqual(
                run(["git", "rev-parse", "refs/heads/main"], repo).stdout.strip(),
                run(["git", "rev-parse", "refs/remotes/origin/main"], repo).stdout.strip(),
            )
            self.assertEqual(run(["git", "status", "--porcelain"], repo).stdout.strip(), "M mod")

    def test_check_reports_no_collision_when_the_repair_uses_a_separate_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path, remote_ahead=True, checkout_work_branch=True)
            make_upstream(tmp_path, {"new.md": "upstream\n"}, "upstream adds new.md")
            local_new = repo / "new.md"
            local_new.write_text("local\n", encoding="utf-8")

            check_result = self.run_script(repo, "--check")

            self.assertIn("repair_action=check_only", check_result.stdout)
            self.assertIn("local_changes=preserved", check_result.stdout)
            self.assertIn("preserved_local_paths=new.md", check_result.stdout)
            self.assertNotIn("collision_paths", check_result.stdout)

            repair_result = self.run_script(repo)

            self.assertIn("repair_action=fast_forward_temporary_worktree", repair_result.stdout)
            self.assertIn("local_changes=preserved", repair_result.stdout)
            self.assertEqual(local_new.read_text(encoding="utf-8"), "local\n")

    @unittest.skipIf(os.name == "nt", "POSIX pathnames can carry arbitrary bytes")
    def test_non_utf8_pathname_is_reported_instead_of_crashing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            repo = self.make_repo(tmp_path, checkout_work_branch=True)
            name = b"\xff-undecodable.txt"
            try:
                fd = os.open(os.fsencode(str(repo)) + b"/" + name, os.O_CREAT | os.O_WRONLY, 0o644)
            except OSError as error:
                if error.errno != errno.EILSEQ:
                    raise
                # APFS and HFS+ refuse path names that are not valid UTF-8, so
                # this filesystem cannot hold the probe at all.
                self.skipTest(f"this filesystem rejects undecodable path name bytes: {error}")
            try:
                os.write(fd, b"payload\n")
            finally:
                os.close(fd)

            result = self.run_script(repo, "--check")

            self.assertNotIn("Traceback", result.stderr)
            self.assertEqual(result.returncode, 1)
            self.assertIn("local_changes=preserved", result.stdout)
            self.assertIn("\\xff-undecodable.txt", result.stdout)


if __name__ == "__main__":
    unittest.main()
