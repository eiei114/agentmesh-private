#!/usr/bin/env python3
"""Assert copy-pasteable shell examples in README/docs stay runnable.

A shell fence that uses a literal two-character ``\\n`` as a line break is not
runnable: the shell passes ``\\n`` to the CLI as an argument and the program
exits with ``unexpected argument '\\n' found``. Documentation drift of that
shape is invisible to Markdown renderers, so assert it here.

CommonMark accepts `` ``` `` and ``~~~`` as fence delimiters, requires an opener
run of at least three characters, and closes it only with the same character in
a run at least as long that carries no info string. Scanning only exactly three
backticks would silently skip a ``~~~bash`` block or a ```` ````bash ```` block,
so every delimiter length is captured here.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SHELL_LANGUAGES = frozenset({"bash", "sh", "shell", "zsh", "console"})
FENCE_OPEN_RE = re.compile(r"^[ \t]*(?P<fence>`{3,}|~{3,})(?P<info>.*)$")


class Fence:
    """A CommonMark fenced code block opener."""

    __slots__ = ("character", "length", "language")

    def __init__(self, fence: str, info: str) -> None:
        self.character = fence[0]
        self.length = len(fence)
        stripped = info.strip()
        self.language = stripped.split()[0].lower() if stripped else ""

    def closes_with(self, line: str) -> bool:
        match = FENCE_OPEN_RE.match(line)
        if match is None or match.group("info").strip():
            return False
        candidate = match.group("fence")
        return candidate[0] == self.character and len(candidate) >= self.length


def parse_fence(line: str) -> Fence | None:
    """Return the fence opened by ``line``, or ``None`` if it opens none."""
    match = FENCE_OPEN_RE.match(line)
    if match is None:
        return None
    fence = match.group("fence")
    info = match.group("info")
    # A backtick fence's info string may not contain a backtick (CommonMark),
    # so inline code such as `` `a` `` is not mistaken for a fence.
    if fence[0] == "`" and "`" in info:
        return None
    return Fence(fence, info)


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
    fence: Fence | None = None
    for number, line in enumerate(text.splitlines(), 1):
        if fence is not None:
            if fence.closes_with(line):
                fence = None
                continue
            yield number, fence.language, line
            continue
        fence = parse_fence(line)


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
