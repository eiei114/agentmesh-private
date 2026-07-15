#!/usr/bin/env python3
"""Refuse private prerelease publish when tag or release already exists.

No asset replacement is allowed. Exit 0 only when both are absent.
Requires `gh` auth against the private AgentMesh repository.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys


def run_gh(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["gh", *args],
        check=False,
        text=True,
        capture_output=True,
    )


def release_exists(tag: str) -> bool:
    proc = run_gh(["release", "view", tag, "--json", "tagName,isDraft,isPrerelease"])
    if proc.returncode == 0:
        return True
    # gh prints "release not found" on stderr for missing releases
    combined = (proc.stderr or "") + (proc.stdout or "")
    if "not found" in combined.lower() or "could not find" in combined.lower():
        return False
    # Unexpected errors should fail closed.
    print(combined.strip() or f"gh release view failed rc={proc.returncode}", file=sys.stderr)
    raise SystemExit(3)


def tag_exists(tag: str) -> bool:
    # Prefer remote tag list via API to avoid relying on local fetch state.
    proc = run_gh(
        [
            "api",
            f"repos/{{owner}}/{{repo}}/git/ref/tags/{tag}",
            "--jq",
            ".ref",
        ]
    )
    if proc.returncode == 0 and proc.stdout.strip():
        return True
    combined = (proc.stderr or "") + (proc.stdout or "")
    if "404" in combined or "Not Found" in combined:
        return False
    # Some gh versions return 1 with empty body
    if proc.returncode != 0 and not combined.strip():
        return False
    print(combined.strip() or f"gh api tag lookup failed rc={proc.returncode}", file=sys.stderr)
    raise SystemExit(3)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="Exact prerelease tag, e.g. v0.2.0-dev.1")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    tag = args.tag.strip()
    if not tag or "/" in tag or "\\" in tag or ".." in tag:
        print("error: invalid tag", file=sys.stderr)
        return 2
    if not (tag.startswith("v") or tag.startswith("agentmesh-")):
        print("error: tag must start with v or agentmesh-", file=sys.stderr)
        return 2

    has_release = release_exists(tag)
    has_tag = tag_exists(tag)
    ok = (not has_release) and (not has_tag)
    payload = {
        "ok": ok,
        "tag": tag,
        "release_exists": has_release,
        "tag_exists": has_tag,
        "policy": "no_asset_replacement",
    }
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        print(
            f"prerelease absent check: tag={tag} release_exists={has_release} "
            f"tag_exists={has_tag} ok={ok}"
        )
    if not ok:
        print(
            "refusing publish: tag and/or release already exist "
            "(immutable private prereleases never replace assets)",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
