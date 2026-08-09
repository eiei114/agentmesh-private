#!/usr/bin/env python3
"""Fast-forward the local maintenance main ref to origin/main.

The Multica repository checkout keeps a shared local `main` ref in the common
repository and creates issue worktrees from it. If that shared ref lags behind
`origin/main`, generated maintenance requests can be based on stale state even
when a new issue worktree starts from the remote branch.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path


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


@contextmanager
def repair_lock(repo: Path, branch: str) -> Iterator[None]:
    lock_dir = git_common_dir(repo) / f"agentmesh-repair-{repair_lock_name(branch)}.lock"
    try:
        lock_dir.mkdir()
    except FileExistsError as error:
        raise RepairError(f"repair lock already held for refs/heads/{branch}: {lock_dir}") from error

    owner_file = lock_dir / "owner"
    try:
        owner_file.write_text(f"pid={os.getpid()}\nrepo={repo}\nbranch={branch}\n", encoding="utf-8")
        yield
    finally:
        owner_file.unlink(missing_ok=True)
        lock_dir.rmdir()


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
        run_git(repo, ["update-ref", branch_ref, before.remote_sha, before.local_sha])
        return "fast_forward_ref"
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
