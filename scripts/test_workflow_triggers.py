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

    def test_gitleaks_gates_ci_and_is_not_an_install_action_tool(self) -> None:
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("name: Gitleaks", text)
        self.assertIn("needs: [stealth, lint, workflows, test, gitleaks]", text)
        self.assertIn('test "$GITLEAKS" = success', text)
        self.assertNotIn("tool: gitleaks@", text)
        self.assertIn("gitleaks_8.30.1_linux_x64.tar.gz", text)

    def test_public_scanners_stay_commented(self) -> None:
        text = (WORKFLOWS / "security.yml").read_text(encoding="utf-8")
        live = "\n".join(
            line for line in text.splitlines() if not line.lstrip().startswith("#")
        )
        for needle in (
            "codeql-action",
            "dependency-review-action",
            "scorecard-action",
            "fossa-action",
            "FOSSA_API_KEY",
        ):
            self.assertNotIn(needle, live)
            self.assertIn(needle, text)
        self.assertNotIn("pull_request:", _on_block(text))
        self.assertNotIn("push:", _on_block(text))

    def test_release_please_runs_on_main_and_does_not_test(self) -> None:
        text = (WORKFLOWS / "release-please.yml").read_text(encoding="utf-8")
        on_block = _on_block(text)
        self.assertIn("push:", on_block)
        self.assertIn("branches: [main]", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertNotIn("pull_request:", on_block)
        self.assertNotIn("cargo test", text)
        self.assertIn("cargo publish --locked", text)
        self.assertIn("CARGO_REGISTRY_TOKEN", text)
        config = (ROOT / "release-please-config.json").read_text(encoding="utf-8")
        self.assertIn('"release-as": "0.1.0"', config)
        approve = (WORKFLOWS / "auto-approve.yml").read_text(encoding="utf-8")
        self.assertIn("startsWith(github.head_ref, 'release-please')", approve)


if __name__ == "__main__":
    unittest.main()
