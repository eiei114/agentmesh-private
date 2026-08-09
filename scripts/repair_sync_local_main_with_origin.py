#!/usr/bin/env python3
"""Fast-forward the local maintenance main ref to origin/main.

The Multica repository checkout keeps a shared local `main` ref in the common
repository and creates issue worktrees from it. If that shared ref lags behind
`origin/main`, generated maintenance requests can be based on stale state even
when a new issue worktree starts from the remote branch.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

if os.name == "nt":
    import msvcrt
else:
    import fcntl


class RepairError(RuntimeError):
    """Raised when the local main ref cannot be safely repaired."""


@dataclass(frozen=True)
class Divergence:
    local_sha: str
    remote_sha: str
    ahead: int
    behind: int


def run_git(repo: Path, args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if check and result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise RepairError(f"git {' '.join(args)} failed: {detail}")
    return result


def git_stdout(repo: Path, args: list[str]) -> str:
    return run_git(repo, args).stdout.strip()


def git_common_dir(repo: Path) -> Path:
    common_dir = Path(git_stdout(repo, ["rev-parse", "--git-common-dir"]))
    if not common_dir.is_absolute():
        common_dir = repo / common_dir
    return common_dir.resolve()


def repair_lock_name(branch: str) -> str:
    token = re.sub(r"[^A-Za-z0-9._-]+", "_", branch).strip("._-")
    return token or "branch"


def lock_owner_file(owner_file: BinaryIO) -> bool:
    """Try to lock the first byte; kernel releases this lock when the process dies."""

    owner_file.seek(0)
    try:
        if os.name == "nt":
            msvcrt.locking(owner_file.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            fcntl.flock(owner_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return False
    return True


def unlock_owner_file(owner_file: BinaryIO) -> None:
    owner_file.seek(0)
    if os.name == "nt":
        msvcrt.locking(owner_file.fileno(), msvcrt.LK_UNLCK, 1)
    else:
        fcntl.flock(owner_file.fileno(), fcntl.LOCK_UN)


def read_owner(owner_file: BinaryIO) -> dict[str, object]:
    owner_file.seek(0)
    try:
        value = json.loads(owner_file.read().decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}


def owner_lock_path(lock_dir: Path, owner: dict[str, object]) -> Path | None:
    token = owner.get("token")
    lock_file = owner.get("lock_file")
    expected = f".{lock_dir.name}.owner-{token}"
    if not isinstance(token, str) or lock_file != expected:
        return None
    return lock_dir.parent / expected


def retire_lock_dir(lock_dir: Path, expected_token: str | None) -> bool:
    """Atomically take a stale/owned directory out of the acquisition path."""

    owner: dict[str, object] = {}
    try:
        with (lock_dir / "owner.json").open("rb") as metadata_file:
            owner = read_owner(metadata_file)
    except FileNotFoundError:
        if expected_token is not None:
            return False
    if expected_token is not None and owner.get("token") != expected_token:
        return False

    retired = lock_dir.with_name(f"{lock_dir.name}.retired-{uuid.uuid4().hex}")
    try:
        lock_dir.rename(retired)
    except OSError:
        return False
    shutil.rmtree(retired, ignore_errors=True)
    stale_owner_lock = owner_lock_path(lock_dir, owner)
    if stale_owner_lock is not None:
        try:
            stale_owner_lock.unlink(missing_ok=True)
        except OSError:
            # A simultaneous reclaimer may briefly have this stale identity
            # open on Windows. Its unique file cannot grant lock ownership.
            pass
    return True


def reclaim_stale_lock(lock_dir: Path, branch: str) -> bool:
    try:
        with (lock_dir / "owner.json").open("rb") as metadata_file:
            owner = read_owner(metadata_file)
    except FileNotFoundError:
        # New locks publish a populated candidate directory atomically, so a
        # missing owner can only be malformed or left by the older protocol.
        return retire_lock_dir(lock_dir, None)

    stale_owner_lock = owner_lock_path(lock_dir, owner)
    if stale_owner_lock is None:
        return retire_lock_dir(lock_dir, None)
    try:
        owner_file = stale_owner_lock.open("r+b")
    except FileNotFoundError:
        return retire_lock_dir(lock_dir, owner.get("token") if isinstance(owner.get("token"), str) else None)

    with owner_file:
        if not lock_owner_file(owner_file):
            owner_pid = owner.get("pid", "unknown")
            owner_token = owner.get("token", "unknown")
            raise RepairError(
                f"repair lock already held for refs/heads/{branch}: {lock_dir} "
                f"(pid={owner_pid}, owner={owner_token})"
            )
        token = owner.get("token")
        expected_token = token if isinstance(token, str) else None
        unlock_owner_file(owner_file)

    retired = retire_lock_dir(lock_dir, expected_token)
    if not retired:
        try:
            stale_owner_lock.unlink(missing_ok=True)
        except OSError:
            pass
    return retired


def prepare_lock_candidate(
    common_dir: Path, lock_dir: Path, token: str, repo: Path, branch: str
) -> tuple[Path, Path, BinaryIO]:
    """Publish complete owner metadata before exposing a lock candidate."""

    candidate = common_dir / f".{lock_dir.name}.candidate-{token}"
    owner_lock = common_dir / f".{lock_dir.name}.owner-{token}"
    owner_file: BinaryIO | None = None
    owner_locked = False
    try:
        candidate.mkdir()
        owner_file = owner_lock.open("x+b")
        owner = {
            "version": 1,
            "token": token,
            "pid": os.getpid(),
            "repo": str(repo),
            "branch": branch,
            "lock_file": owner_lock.name,
        }
        owner_json = json.dumps(owner, sort_keys=True) + "\n"
        owner_file.write(owner_json.encode("utf-8"))
        owner_file.flush()
        (candidate / "owner.json").write_text(owner_json, encoding="utf-8")
        owner_locked = lock_owner_file(owner_file)
        if not owner_locked:
            raise RepairError(f"could not lock repair owner file: {owner_lock}")
        return candidate, owner_lock, owner_file
    except Exception as error:
        if owner_file is not None and not owner_file.closed:
            try:
                if owner_locked:
                    unlock_owner_file(owner_file)
            finally:
                owner_file.close()
        owner_lock.unlink(missing_ok=True)
        shutil.rmtree(candidate, ignore_errors=True)
        if isinstance(error, RepairError):
            raise
        raise RepairError(f"could not prepare repair lock for refs/heads/{branch}: {error}") from error


@contextmanager
def repair_lock(repo: Path, branch: str) -> Iterator[None]:
    common_dir = git_common_dir(repo)
    lock_dir = common_dir / f"agentmesh-repair-{repair_lock_name(branch)}.lock"
    token = uuid.uuid4().hex
    candidate, owner_lock, owner_file = prepare_lock_candidate(
        common_dir, lock_dir, token, repo, branch
    )

    acquired = False
    try:
        for _ in range(8):
            try:
                # The owner identity and its live kernel lock exist before the
                # directory is atomically published. Contenders never observe
                # a newly acquired lock without its owner.
                candidate.rename(lock_dir)
                acquired = True
                break
            except OSError as error:
                if not lock_dir.exists():
                    raise RepairError(f"could not publish repair lock {lock_dir}: {error}") from error
                if reclaim_stale_lock(lock_dir, branch):
                    continue
                if lock_dir.exists():
                    continue
        if not acquired:
            raise RepairError(f"could not reclaim stale repair lock for refs/heads/{branch}: {lock_dir}")
        yield
    finally:
        if not owner_file.closed:
            try:
                unlock_owner_file(owner_file)
            finally:
                owner_file.close()
        if acquired:
            retire_lock_dir(lock_dir, token)
        else:
            shutil.rmtree(candidate, ignore_errors=True)
            owner_lock.unlink(missing_ok=True)


def dirty_count(repo: Path) -> int:
    output = git_stdout(repo, ["status", "--porcelain"])
    if not output:
        return 0
    return len(output.splitlines())


def current_branch(repo: Path) -> str:
    return git_stdout(repo, ["rev-parse", "--abbrev-ref", "HEAD"])


def current_head(repo: Path) -> str:
    return git_stdout(repo, ["rev-parse", "HEAD"])


def resolve_repo(repo: Path) -> Path:
    return Path(git_stdout(repo, ["rev-parse", "--show-toplevel"])).resolve()


def fetch_remote_branch(repo: Path, branch: str, remote: str) -> None:
    remote_tracking_ref = f"refs/remotes/{remote}/{branch}"
    refspec = f"+refs/heads/{branch}:{remote_tracking_ref}"
    run_git(repo, ["fetch", "--prune", remote, refspec])


def divergence(repo: Path, branch: str, remote: str) -> Divergence:
    local_ref = f"refs/heads/{branch}"
    remote_ref = f"refs/remotes/{remote}/{branch}"
    local_sha = git_stdout(repo, ["rev-parse", local_ref])
    remote_sha = git_stdout(repo, ["rev-parse", remote_ref])
    counts = git_stdout(repo, ["rev-list", "--left-right", "--count", f"{branch}...{remote}/{branch}"])
    ahead_text, behind_text = counts.split()
    return Divergence(
        local_sha=local_sha,
        remote_sha=remote_sha,
        ahead=int(ahead_text),
        behind=int(behind_text),
    )


def checked_out_worktree(repo: Path, branch_ref: str) -> Path | None:
    """Return the non-bare worktree where branch_ref is checked out, if any."""

    output = git_stdout(repo, ["worktree", "list", "--porcelain"])
    current: dict[str, str | bool] = {}

    def finish(stanza: dict[str, str | bool]) -> Path | None:
        if not stanza or stanza.get("bare") is True:
            return None
        if stanza.get("branch") == branch_ref:
            worktree = stanza.get("worktree")
            if isinstance(worktree, str):
                return Path(worktree).resolve()
        return None

    for line in output.splitlines():
        if not line:
            found = finish(current)
            if found is not None:
                return found
            current = {}
            continue
        if line == "bare":
            current["bare"] = True
            continue
        key, _, value = line.partition(" ")
        current[key] = value

    return finish(current)


@contextmanager
def temporary_branch_worktree(repo: Path, branch: str) -> Iterator[Path]:
    """Checkout branch under Git worktree coordination for the whole update."""

    temp_root = Path(tempfile.mkdtemp(prefix="agentmesh-repair-worktree-"))
    worktree = temp_root / "checkout"
    added = run_git(repo, ["worktree", "add", str(worktree), branch], check=False)
    if added.returncode != 0:
        shutil.rmtree(temp_root, ignore_errors=True)
        run_git(repo, ["worktree", "prune"], check=False)
        detail = (added.stderr or added.stdout).strip()
        raise RepairError(
            f"could not create coordinated temporary worktree for refs/heads/{branch}; "
            f"the branch may have been checked out concurrently: {detail}"
        )

    operation_error: Exception | None = None
    try:
        yield worktree
    except Exception as error:
        operation_error = error
        raise
    finally:
        removed = run_git(repo, ["worktree", "remove", "--force", str(worktree)], check=False)
        if removed.returncode != 0:
            cleanup_error = (removed.stderr or removed.stdout).strip()
            shutil.rmtree(temp_root, ignore_errors=True)
            run_git(repo, ["worktree", "prune"], check=False)
            if operation_error is not None:
                raise RepairError(
                    f"{operation_error}; additionally could not remove temporary worktree "
                    f"{worktree}: {cleanup_error}"
                ) from operation_error
            raise RepairError(f"could not remove temporary worktree {worktree}: {cleanup_error}")
        shutil.rmtree(temp_root, ignore_errors=True)


def fast_forward(repo: Path, branch: str, remote: str, before: Divergence) -> str:
    if before.ahead != 0:
        raise RepairError(
            f"refs/heads/{branch} is ahead of {remote}/{branch}; refusing to rewrite local commits"
        )
    if before.behind == 0:
        return "already_aligned"

    ancestor = run_git(repo, ["merge-base", "--is-ancestor", branch, f"{remote}/{branch}"], check=False)
    if ancestor.returncode != 0:
        raise RepairError(f"refs/heads/{branch} is not a fast-forward ancestor of {remote}/{branch}")

    branch_ref = f"refs/heads/{branch}"
    checked_out = checked_out_worktree(repo, branch_ref)
    if checked_out is None:
        with temporary_branch_worktree(repo, branch) as worktree:
            run_git(worktree, ["merge", "--ff-only", f"{remote}/{branch}"])
        return "fast_forward_temporary_worktree"
    if checked_out != repo.resolve():
        raise RepairError(
            f"{branch_ref} is checked out in another worktree; run this command from that worktree"
        )
    if dirty_count(repo) != 0:
        raise RepairError(f"{branch_ref} worktree is dirty; commit or stash changes before syncing")
    run_git(repo, ["merge", "--ff-only", f"{remote}/{branch}"])
    return "fast_forward_worktree"


def request_ready(repo: Path, after: Divergence) -> bool:
    return after.ahead == 0 and after.behind == 0 and dirty_count(repo) == 0


def print_report(repo: Path, branch: str, remote: str, before: Divergence, after: Divergence, action: str) -> None:
    aligned = after.ahead == 0 and after.behind == 0
    current_dirty_count = dirty_count(repo)
    clean = current_dirty_count == 0
    print(f"branch={branch}")
    print(f"remote={remote}")
    print(f"current_branch={current_branch(repo)}")
    print(f"current_head={current_head(repo)}")
    print(f"dirty_count={current_dirty_count}")
    print(f"local_ref=refs/heads/{branch}")
    print(f"remote_ref=refs/remotes/{remote}/{branch}")
    print(f"before_local={before.local_sha}")
    print(f"before_remote={before.remote_sha}")
    print(f"before_ahead={before.ahead}")
    print(f"before_behind={before.behind}")
    print(f"repair_action={action}")
    print(f"after_local={after.local_sha}")
    print(f"after_remote={after.remote_sha}")
    print(f"after_ahead={after.ahead}")
    print(f"after_behind={after.behind}")
    print(f"repo_main_behind={'present' if after.behind else 'absent'}")
    print(f"repo_main_aligned={'yes' if aligned else 'no'}")
    print(f"request_action={'seed_app_requests' if aligned and clean else 'repair_first'}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fetch origin/main and fast-forward the local maintenance main ref when safe."
    )
    parser.add_argument("--repo", default=".", help="repository worktree to inspect (default: current directory)")
    parser.add_argument("--remote", default="origin", help="remote name to fetch (default: origin)")
    parser.add_argument("--branch", default="main", help="local branch to align (default: main)")
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify alignment using existing refs without fetching or mutating branch refs",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        repo = resolve_repo(Path(args.repo))
        if args.check:
            before = divergence(repo, args.branch, args.remote)
            action = "check_only"
            after = divergence(repo, args.branch, args.remote)
        else:
            with repair_lock(repo, args.branch):
                fetch_remote_branch(repo, args.branch, args.remote)
                before = divergence(repo, args.branch, args.remote)
                action = fast_forward(repo, args.branch, args.remote, before)
                after = divergence(repo, args.branch, args.remote)
        print_report(repo, args.branch, args.remote, before, after, action)
        return 0 if request_ready(repo, after) else 1
    except RepairError as error:
        print(f"error={error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
