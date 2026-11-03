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

    def write_doc(self, text: str) -> None:
        self.write("README.md", "# Title\n")
        self.write("docs/app.md", text)

    def assert_rejected(self, text: str, location: str) -> None:
        self.write_doc(text)
        with self.assertRaises(AssertionError) as raised:
            doc_fences.assert_command_fences(self.root)
        self.assertIn(location, str(raised.exception))

    def assert_accepted(self, text: str) -> None:
        self.write_doc(text)
        doc_fences.assert_command_fences(self.root)

    # --- triple backticks (pre-existing behaviour) -------------------------

    def test_escaped_newline_in_bash_fence_is_rejected(self) -> None:
        self.write(
            "README.md",
            "# Title\n\n```bash\nagentmesh app validate \\n  --manifest a.toml\n```\n",
        )
        with self.assertRaises(AssertionError) as raised:
            doc_fences.assert_command_fences(self.root)
        self.assertIn("README.md:4", str(raised.exception))

    def test_escaped_newline_in_nested_doc_is_rejected(self) -> None:
        self.assert_rejected(
            "```bash\nagentmesh app run \\n  --input in.json\n```\n",
            "docs/app.md:2",
        )

    def test_real_line_continuation_is_accepted(self) -> None:
        self.assert_accepted(
            "```bash\nagentmesh app validate \\\n  --manifest a.toml\n```\n"
        )

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

    # --- tilde fences (CommonMark alternative delimiter) -------------------

    def test_escaped_newline_in_tilde_fence_is_rejected(self) -> None:
        self.assert_rejected(
            "~~~bash\nagentmesh app validate \\n  --manifest a.toml\n~~~\n",
            "docs/app.md:2",
        )

    def test_real_line_continuation_in_tilde_fence_is_accepted(self) -> None:
        self.assert_accepted(
            "~~~bash\nagentmesh app validate \\\n  --manifest a.toml\n~~~\n"
        )

    def test_tilde_fence_is_not_closed_by_backticks(self) -> None:
        # The ``` line is content inside the ~~~ block, so the offender on the
        # following line is still inside a shell fence.
        self.assert_rejected(
            "~~~bash\n```\nagentmesh app run \\n  --input in.json\n~~~\n",
            "docs/app.md:3",
        )

    def test_backtick_fence_is_not_closed_by_tildes(self) -> None:
        self.assert_rejected(
            "```bash\n~~~\nagentmesh app run \\n  --input in.json\n```\n",
            "docs/app.md:3",
        )

    # --- longer fence runs -------------------------------------------------

    def test_escaped_newline_in_four_backtick_fence_is_rejected(self) -> None:
        self.assert_rejected(
            "````bash\nagentmesh app run \\n  --input in.json\n````\n",
            "docs/app.md:2",
        )

    def test_four_backtick_fence_is_not_closed_by_three_backticks(self) -> None:
        # A shorter run does not close a longer opener; the ``` line and the
        # offender after it stay inside the shell fence.
        self.assert_rejected(
            "````bash\n```\nagentmesh app run \\n  --input in.json\n````\n",
            "docs/app.md:3",
        )

    def test_four_backtick_fence_is_closed_by_five_backticks(self) -> None:
        # A longer closer ends the block, so the trailing prose is not checked.
        self.assert_accepted(
            "````bash\nagentmesh app run --input in.json\n`````\n\nprose \\n text\n"
        )

    def test_longer_fence_body_may_contain_three_backtick_fences(self) -> None:
        # Nested short fences are literal content of the outer block.
        self.assert_accepted(
            "````text\n```bash\nexample \\n only\n```\n````\n"
        )

    # --- closer requirements ----------------------------------------------

    def test_closing_fence_with_info_string_does_not_close(self) -> None:
        self.assert_rejected(
            "```bash\n```bash\nagentmesh app run \\n  --input in.json\n```\n",
            "docs/app.md:3",
        )

    def test_closing_fence_may_be_indented(self) -> None:
        self.assert_accepted(
            "```bash\nagentmesh app run --input in.json\n   ```\n\nprose \\n text\n"
        )

    def test_inline_code_span_is_not_a_fence(self) -> None:
        self.write(
            "README.md",
            "# Title\n\nUse `` `\\n` `` in prose, not a fence.\n",
        )
        doc_fences.assert_command_fences(self.root)

    def test_unterminated_shell_fence_is_still_scanned(self) -> None:
        self.assert_rejected(
            "```bash\nagentmesh app run \\n  --input in.json\n",
            "docs/app.md:2",
        )


if __name__ == "__main__":
    unittest.main()
