#!/usr/bin/env python3
"""Assert copy-pasteable shell examples in README/docs stay runnable.

A shell fence that uses a literal two-character ``\\n`` as a line break is not
runnable: the shell passes ``\\n`` to the CLI as an argument and the program
exits with ``unexpected argument '\\n' found``. Documentation drift of that
shape is invisible to Markdown renderers, so assert it here.

The scan follows the CommonMark fenced-code-block rules that decide whether a
line is a fence at all:

* an opener is a run of three or more backticks or tildes, indented at most
  three columns past the content start of its container;
* a block is closed only by a line using the same delimiter character with a
  run at least as long and no info string;
* block quotes (``>``) and list items (``-``, ``*``, ``+``, ``1.``) are
  containers, so their prefixes are removed before the fence check and the
  indentation limit applies relative to the container;
* four or more columns past the container make an indented code block, not a
  fence, so such a line neither opens a shell block nor closes one.

Without the container handling a ``~~~bash`` block or a quoted/list shell
example would be skipped silently, and a four-space-indented `` ```bash ``
example would be reported although Markdown renders it as an indented code
block.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SHELL_LANGUAGES = frozenset({"bash", "sh", "shell", "zsh", "console"})
FENCE_RE = re.compile(r"(?P<fence>`{3,}|~{3,})(?P<info>.*)$", re.DOTALL)
QUOTE_MARKER_RE = re.compile(r"^ {0,3}>[ \t]?")
LIST_MARKER_RE = re.compile(r"^(?P<marker>[-*+]|\d{1,9}[.)])(?P<gap>[ \t]+|$)")
MAX_FENCE_INDENT = 3


class Fence:
    """A CommonMark fenced code block opener."""

    __slots__ = ("character", "length", "language")

    def __init__(self, delimiter: str, info: str) -> None:
        self.character = delimiter[0]
        self.length = len(delimiter)
        stripped = info.strip()
        self.language = stripped.split()[0].lower() if stripped else ""

    def closes_with(self, content: str, indent: int) -> bool:
        """Return whether ``content`` closes this block."""
        if indent > MAX_FENCE_INDENT:
            return False
        match = FENCE_RE.match(content)
        if match is None or match.group("info").strip():
            return False
        delimiter = match.group("fence")
        return delimiter[0] == self.character and len(delimiter) >= self.length


def parse_fence(content: str, indent: int) -> Fence | None:
    """Return the fence opened by ``content``, or ``None`` if it opens none."""
    if indent > MAX_FENCE_INDENT:
        return None
    match = FENCE_RE.match(content)
    if match is None:
        return None
    delimiter = match.group("fence")
    info = match.group("info")
    # A backtick fence's info string may not contain a backtick (CommonMark),
    # so inline code such as `` `a` `` is not mistaken for a fence.
    if delimiter[0] == "`" and "`" in info:
        return None
    return Fence(delimiter, info)


def line_context(line: str, list_indents: list[int]) -> tuple[str | None, int]:
    """Strip the block-quote and list-item prefixes from ``line``.

    ``list_indents`` holds the content column of each open list item and is
    updated in place. Returns ``(content, indent)`` where ``indent`` is the
    column of ``content`` measured from the innermost container's content
    start, or ``(None, 0)`` for a blank line.
    """
    column = 0
    rest = line
    while True:
        match = QUOTE_MARKER_RE.match(rest)
        if match is None:
            break
        column += match.end()
        rest = rest[match.end() :]

    if not rest.strip():
        return None, 0

    lead = len(rest) - len(rest.lstrip(" "))
    # A non-blank line that dedents past an open list item ends that item.
    while list_indents and column + lead < list_indents[-1]:
        list_indents.pop()
    column += lead
    rest = rest[lead:]

    marker = LIST_MARKER_RE.match(rest)
    if marker is not None:
        gap = marker.group("gap")
        # CommonMark: a gap wider than four columns belongs to an indented
        # code block inside the item, so only one column counts as content.
        width = 1 if len(gap) > 4 or not gap else len(gap)
        list_indents.append(column + len(marker.group("marker")) + width)
        column += marker.end()
        rest = rest[marker.end() :]
        lead = len(rest) - len(rest.lstrip(" "))
        column += lead
        rest = rest[lead:]

    base = list_indents[-1] if list_indents else 0
    return rest, max(column - base, 0)


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def documented_markdown_files(root: Path) -> list[Path]:
    files = [root / "README.md"]
    docs = root / "docs"
    if docs.is_dir():
        files.extend(sorted(docs.rglob("*.md")))
    return [path for path in files if path.is_file()]


def iter_fences(text: str):
    """Yield ``(line_number, language, content)`` for each line in a fence."""
    list_indents: list[int] = []
    fence: Fence | None = None
    for number, line in enumerate(text.splitlines(), 1):
        if fence is not None:
            # Fence content is literal text, so containers are measured on a
            # scratch copy and the real open-item state is left untouched.
            content, indent = line_context(line, list(list_indents))
            if content is None:
                continue
            if fence.closes_with(content, indent):
                fence = None
                continue
            yield number, fence.language, content
            continue
        content, indent = line_context(line, list_indents)
        if content is None:
            continue
        fence = parse_fence(content, indent)


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
