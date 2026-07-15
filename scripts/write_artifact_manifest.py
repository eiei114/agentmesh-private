#!/usr/bin/env python3
"""Write an immutable AgentMesh Phase 0 artifact manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--protocol-version", required=True)
    parser.add_argument("--host-version", required=True)
    parser.add_argument("--plugin-version", required=True)
    parser.add_argument("--toolchain", required=True)
    parser.add_argument("--bin", action="append", default=[])
    args = parser.parse_args()

    out = Path(args.out)
    base = out.parent
    binaries = []
    for name in args.bin:
        path = base / name
        binaries.append(
            {
                "name": name,
                "sha256": sha256_file(path),
                "size": path.stat().st_size,
            }
        )

    manifest = {
        "schema_version": "2026-07-15",
        "commit": args.commit,
        "protocol_version": args.protocol_version,
        "host_version": args.host_version,
        "plugin_version": args.plugin_version,
        "rust_toolchain": args.toolchain,
        "target": args.target,
        "binaries": binaries,
    }
    out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
