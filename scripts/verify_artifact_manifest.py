#!/usr/bin/env python3
"""Verify AgentMesh artifact manifest SHA-256 entries."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: verify_artifact_manifest.py <manifest.json>", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    data = json.loads(path.read_text(encoding="utf-8"))
    base = path.parent
    for binary in data.get("binaries", []):
        name = binary["name"]
        expected = binary["sha256"]
        actual = sha256_file(base / name)
        if actual != expected:
            print(f"SHA mismatch for {name}: expected={expected} actual={actual}", file=sys.stderr)
            return 1
        print(f"ok {name} {actual}")
    print("manifest verification passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
