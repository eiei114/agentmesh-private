#!/usr/bin/env python3
"""Assert copy-pasteable shell examples in README/docs stay runnable.

A shell fence that uses a literal two-character ``\\n`` as a line break is not
runnable: the shell passes ``\\n`` to the CLI as an argument and the program
exits with ``unexpected argument '\\n' found``. Documentation drift of that
shape is invisible to Markdown renderers, so assert it here.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SHELL_LANGUAGES = frozenset({"bash", "sh", "shell", "zsh", "console"})
FENCE_RE = re.compile(r"^\s*```(?P<lang>.*)$")


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def documented_markdown_files(root: Path) -> list[Path]:
    files = [root / "README.md"]
    docs = root / "docs"
    if docs.is_dir():
        files.extend(sorted(docs.rglob("*.md")))
    return [path for path in files if path.is_file()]


def iter_fences(text: str):
    """Yield ``(line_number, language, line)`` for each line inside a fence."""
    language = ""
    in_fence = False
    for number, line in enumerate(text.splitlines(), 1):
        match = FENCE_RE.match(line)
        if match is not None:
            if in_fence:
                in_fence = False
                language = ""
            else:
                in_fence = True
                language = match.group("lang").strip().split()[0].lower() if match.group("lang").strip() else ""
            continue
        if in_fence:
            yield number, language, line


def find_escaped_newline_lines(path: Path) -> list[tuple[int, str]]:
    offenders: list[tuple[int, str]] = []
    for number, language, line in iter_fences(path.read_text(encoding="utf-8")):
        if language not in SHELL_LANGUAGES:
            continue
        if "\\n" in line:
            offenders.append((number, line.strip()))
    return offenders


def assert_command_fences(root: Path | None = None) -> None:
    root = repo_root() if root is None else root
    files = documented_markdown_files(root)
    if not files:
        raise ValueError("no documented Markdown files found")

    problems: list[str] = []
    for path in files:
        for number, line in find_escaped_newline_lines(path):
            problems.append(f"{path.relative_to(root).as_posix()}:{number}: {line}")
    if problems:
        raise AssertionError(
            "shell fences contain a literal '\\n' instead of a real line break "
            "(the example is not runnable):\n  " + "\n  ".join(problems)
        )


def main() -> int:
    try:
        assert_command_fences()
    except (AssertionError, ValueError) as exc:
        print(f"doc command fence check failed: {exc}", file=sys.stderr)
        return 1
    print("doc command fence check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
