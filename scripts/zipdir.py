#!/usr/bin/env python3
"""Create or extract a zip archive portably (no external zip tooling)."""

from __future__ import annotations

import argparse
import zipfile
from pathlib import Path


def zip_dir(src: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for path in sorted(src.rglob("*")):
            if path.is_file():
                zf.write(path, path.relative_to(src).as_posix())


def unzip(src: Path, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(src, "r") as zf:
        zf.extractall(dest)


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    z = sub.add_parser("zip")
    z.add_argument("src")
    z.add_argument("dest")
    u = sub.add_parser("unzip")
    u.add_argument("src")
    u.add_argument("dest")
    args = parser.parse_args()
    if args.cmd == "zip":
        zip_dir(Path(args.src), Path(args.dest))
    else:
        unzip(Path(args.src), Path(args.dest))


if __name__ == "__main__":
    main()
