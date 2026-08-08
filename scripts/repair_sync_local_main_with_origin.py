#!/usr/bin/env python3
"""Fast-forward the local maintenance main ref to origin/main.

The Multica repository checkout keeps a shared local `main` ref in the common
repository and creates issue worktrees from it. If that shared ref lags behind
`origin/main`, generated maintenance requests can be based on stale state even
when a new issue worktree starts from the remote branch.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
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
        raise RepairError(
            f"{branch_ref} is not checked out in any worktree; run this command from the worktree where {branch} is checked out"
        )
    if checked_out != repo.resolve():
        raise RepairError(
            f"{branch_ref} is checked out in another worktree; run this command from that worktree"
        )
    run_git(repo, ["merge", "--ff-only", f"{remote}/{branch}"])
    return "fast_forward_worktree"


def print_report(branch: str, remote: str, before: Divergence, after: Divergence, action: str) -> None:
    print(f"branch={branch}")
    print(f"remote={remote}")
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
    print(f"repo_main_aligned={'yes' if after.ahead == 0 and after.behind == 0 else 'no'}")


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
        if not args.check:
            fetch_remote_branch(repo, args.branch, args.remote)
        before = divergence(repo, args.branch, args.remote)
        if args.check:
            action = "check_only"
        else:
            action = fast_forward(repo, args.branch, args.remote, before)
        after = divergence(repo, args.branch, args.remote)
        print_report(args.branch, args.remote, before, after, action)
        return 0 if after.ahead == 0 and after.behind == 0 else 1
    except RepairError as error:
        print(f"error={error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
