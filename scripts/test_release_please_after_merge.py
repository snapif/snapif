#!/usr/bin/env python3
"""The dispatcher finds a pull from its head commit and fails closed."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE = ROOT / "scripts" / "release_please_after_merge.py"
WORKFLOW = ROOT / ".github" / "workflows" / "release-please-after-merge.yml"

spec = importlib.util.spec_from_file_location("release_please_after_merge", MODULE)
assert spec is not None and spec.loader is not None
dispatch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dispatch)


HEAD = "3f6af63e468966bd836e5fa6e3eef0c557a80b92"


def _pull(number: int, ref: str, sha: str) -> dict:
    return {"number": number, "head": {"ref": ref, "sha": sha}}


class ResolveTests(unittest.TestCase):
    def test_commit_pull_when_the_run_list_is_empty(self) -> None:
        number = dispatch.pull_number_from_run({"pull_requests": []})
        self.assertIsNone(number)
        found = dispatch.pull_number_from_commit(
            [_pull(295, "fix/wsh-execute-bracket", HEAD)],
            HEAD,
            "fix/wsh-execute-bracket",
        )
        self.assertEqual(found, 295)

    def test_run_list_wins(self) -> None:
        number = dispatch.pull_number_from_run(
            {"pull_requests": [{"number": 12}]}
        )
        self.assertEqual(number, 12)

    def test_sha_match_beats_another_branch(self) -> None:
        found = dispatch.pull_number_from_commit(
            [
                _pull(1, "other", "abc"),
                _pull(295, "fix/wsh-execute-bracket", HEAD),
            ],
            HEAD,
            "fix/wsh-execute-bracket",
        )
        self.assertEqual(found, 295)

    def test_auto_approve_without_a_pull_fails(self) -> None:
        self.assertEqual(dispatch.missing_exit_code("Auto-approve"), 1)
        self.assertEqual(dispatch.missing_exit_code("CI"), 0)


class ClassifyTests(unittest.TestCase):
    def test_actions_bot_merge_dispatches(self) -> None:
        view = {
            "headRefName": "ci/release-please-after-merge",
            "isDraft": False,
            "state": "MERGED",
            "mergedBy": {"login": "github-actions[bot]"},
            "autoMergeRequest": None,
        }
        self.assertEqual(dispatch.classify(view, "Auto-approve", 1), "dispatch")

    def test_human_merge_lets_the_push_start_release_please(self) -> None:
        view = {
            "headRefName": "ci/example",
            "isDraft": False,
            "state": "MERGED",
            "mergedBy": {"login": "SebTardif"},
            "autoMergeRequest": None,
        }
        self.assertEqual(dispatch.classify(view, "CI", 1), "skip-human")

    def test_release_please_head_is_left_for_a_person(self) -> None:
        view = {
            "headRefName": "release-please--branches--main",
            "isDraft": False,
            "state": "OPEN",
            "mergedBy": None,
            "autoMergeRequest": {"enabledBy": {}},
        }
        self.assertEqual(dispatch.classify(view, "Auto-approve", 1), "skip-release")

    def test_draft_defers(self) -> None:
        view = {
            "headRefName": "ci/example",
            "isDraft": True,
            "state": "OPEN",
            "mergedBy": None,
            "autoMergeRequest": None,
        }
        self.assertEqual(dispatch.classify(view, "Auto-approve", 1), "skip-draft")

    def test_auto_approve_waits_briefly_before_giving_up(self) -> None:
        view = {
            "headRefName": "ci/example",
            "isDraft": False,
            "state": "OPEN",
            "mergedBy": None,
            "autoMergeRequest": None,
        }
        self.assertEqual(dispatch.classify(view, "Auto-approve", 1), "wait")
        self.assertEqual(dispatch.classify(view, "Auto-approve", 7), "skip-not-auto")


class MainTests(unittest.TestCase):
    def test_empty_run_uses_the_commit_and_dispatches(self) -> None:
        calls: list[list[str]] = []

        def gh_json(args: list[str]) -> object:
            calls.append(args)
            if "actions/runs/" in args[1]:
                return {
                    "pull_requests": [],
                    "head_sha": HEAD,
                    "name": "Auto-approve",
                }
            if args[1].endswith("/pulls"):
                return [_pull(295, "fix/wsh-execute-bracket", HEAD)]
            return {
                "state": "MERGED",
                "autoMergeRequest": None,
                "mergedBy": {"login": "app/github-actions"},
                "headRefName": "fix/wsh-execute-bracket",
                "isDraft": False,
            }

        def gh_run(args: list[str]) -> None:
            calls.append(args)

        code = dispatch.main(
            env={
                "REPO": "snapif/snapif",
                "RUN_ID": "45556178252",
                "TRIGGER": "Auto-approve",
                "HEAD_SHA": HEAD,
                "HEAD_BRANCH": "fix/wsh-execute-bracket",
            },
            gh_json=gh_json,
            gh_run=gh_run,
            sleep=lambda _seconds: None,
        )
        self.assertEqual(code, 0)
        self.assertIn(
            ["workflow", "run", "Release Please", "--ref", "main", "--repo", "snapif/snapif"],
            calls,
        )

    def test_auto_approve_with_no_pull_exits_1(self) -> None:
        calls: list[list[str]] = []

        def gh_json(args: list[str]) -> object:
            calls.append(args)
            if "actions/runs/" in args[1]:
                return {"pull_requests": []}
            return []

        def gh_run(args: list[str]) -> None:
            calls.append(args)

        code = dispatch.main(
            env={
                "REPO": "snapif/snapif",
                "RUN_ID": "45556178252",
                "TRIGGER": "Auto-approve",
                "HEAD_SHA": HEAD,
                "HEAD_BRANCH": "fix/wsh-execute-bracket",
            },
            gh_json=gh_json,
            gh_run=gh_run,
            sleep=lambda _seconds: None,
        )
        self.assertEqual(code, 1)
        self.assertFalse(any(call[:2] == ["workflow", "run"] for call in calls))

    def test_workflow_passes_the_head_sha(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("python3 scripts/release_please_after_merge.py", text)
        self.assertIn("HEAD_SHA:", text)
        self.assertIn("github.event.workflow_run.head_sha", text)
        self.assertNotIn("pull_requests[0].number", text)


if __name__ == "__main__":
    unittest.main()
