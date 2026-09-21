#!/usr/bin/env python3
"""Lock Recipe A: ci.yml compiles on PR and merge_group only."""

from __future__ import annotations

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".github" / "workflows"


def _on_block(text: str) -> str:
    start = text.index("\non:")
    rest = text[start + 1 :]
    end = rest.index("\njobs:")
    block = rest[:end]
    lines = []
    for line in block.splitlines():
        stripped = line.split("#", 1)[0].rstrip()
        if stripped:
            lines.append(stripped)
    return "\n".join(lines)


class WorkflowTriggerTests(unittest.TestCase):
    def test_ci_has_no_push_compile(self) -> None:
        on_block = _on_block((WORKFLOWS / "ci.yml").read_text(encoding="utf-8"))
        self.assertIn("pull_request:", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertIn("merge_group:", on_block)
        self.assertNotIn("push:", on_block)
        self.assertNotIn("tags:", on_block)
        self.assertNotIn("branches:", on_block)

    def test_actionlint_is_not_an_install_action_tool(self) -> None:
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertNotIn("tool: actionlint@", text)
        self.assertIn("rhysd/actionlint@", text)

    def test_dco_is_workflow_not_probot(self) -> None:
        text = (WORKFLOWS / "dco.yml").read_text(encoding="utf-8")
        on_block = _on_block(text)
        self.assertIn("pull_request:", on_block)
        self.assertIn("merge_group:", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertNotIn("branches:", on_block)
        self.assertIn("Signed-off-by:", text)


if __name__ == "__main__":
    unittest.main()
