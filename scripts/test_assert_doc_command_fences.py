#!/usr/bin/env python3
"""Regression tests for assert_doc_command_fences.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("assert_doc_command_fences.py")
SPEC = importlib.util.spec_from_file_location("assert_doc_command_fences", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
doc_fences = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = doc_fences
SPEC.loader.exec_module(doc_fences)


class DocCommandFenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "docs").mkdir()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write(self, relative: str, text: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def test_escaped_newline_in_bash_fence_is_rejected(self) -> None:
        self.write(
            "README.md",
            "# Title\n\n```bash\nagentmesh app validate \\n  --manifest a.toml\n```\n",
        )
        with self.assertRaises(AssertionError) as raised:
            doc_fences.assert_command_fences(self.root)
        self.assertIn("README.md:4", str(raised.exception))

    def test_escaped_newline_in_nested_doc_is_rejected(self) -> None:
        self.write("README.md", "# Title\n")
        self.write(
            "docs/app.md",
            "```bash\nagentmesh app run \\n  --input in.json\n```\n",
        )
        with self.assertRaises(AssertionError) as raised:
            doc_fences.assert_command_fences(self.root)
        self.assertIn("docs/app.md:2", str(raised.exception))

    def test_real_line_continuation_is_accepted(self) -> None:
        self.write("README.md", "# Title\n")
        self.write(
            "docs/app.md",
            "```bash\nagentmesh app validate \\\n  --manifest a.toml\n```\n",
        )
        doc_fences.assert_command_fences(self.root)

    def test_escaped_newline_outside_shell_fence_is_accepted(self) -> None:
        self.write(
            "README.md",
            '# Title\n\n```json\n{"markdown": "---\\ntitle: x\\n---\\n"}\n```\n',
        )
        doc_fences.assert_command_fences(self.root)

    def test_escaped_newline_outside_any_fence_is_accepted(self) -> None:
        self.write("README.md", "# Title\n\nprose with \\n marker only\n")
        doc_fences.assert_command_fences(self.root)

    def test_fence_state_resets_after_closing_fence(self) -> None:
        self.write(
            "README.md",
            "# Title\n\n```bash\nagentmesh docs list\n```\n\nplain \\n text\n",
        )
        doc_fences.assert_command_fences(self.root)


if __name__ == "__main__":
    unittest.main()
