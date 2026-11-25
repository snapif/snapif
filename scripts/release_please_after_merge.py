#!/usr/bin/env python3
"""Start Release Please after github-actions merges a pull request.

The runs API often leaves pull_requests empty for a same-repo
pull_request. The commit at the run's head SHA still lists the pull.
An Auto-approve run that still has no pull fails. A CI run with no
pull exits 0, which is the old behavior for that trigger.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from collections.abc import Callable

BOTS = {"github-actions[bot]", "app/github-actions"}
ATTEMPTS = 30
WAIT_SECONDS = 10


def _number(value: object) -> int | None:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, str) and value.isdigit():
        value = int(value)
    if isinstance(value, int) and value > 0:
        return value
    return None


def pull_number_from_run(run: dict) -> int | None:
    pulls = run.get("pull_requests") or []
    if not pulls:
        return None
    return _number(pulls[0].get("number"))


def pull_number_from_commit(
    pulls: list, head_sha: str, head_branch: str
) -> int | None:
    if not isinstance(pulls, list):
        return None

    def head(pull: dict) -> dict:
        value = pull.get("head") or {}
        return value if isinstance(value, dict) else {}

    sha_hits = [
        pull for pull in pulls if head_sha and head(pull).get("sha") == head_sha
    ]
    pool = sha_hits
    if head_branch:
        branched = [pull for pull in pool if head(pull).get("ref") == head_branch]
        if branched:
            pool = branched
        elif not pool:
            pool = [
                pull for pull in pulls if head(pull).get("ref") == head_branch
            ]
    if not pool:
        return None
    return _number(pool[0].get("number"))


def missing_exit_code(trigger: str) -> int:
    if trigger == "Auto-approve":
        return 1
    return 0


def classify(view: dict, trigger: str, attempt: int) -> str:
    """One poll of a resolved pull. `wait` means sleep and try again."""
    head = view.get("headRefName") or ""
    if head.startswith("release-please"):
        return "skip-release"
    if view.get("isDraft"):
        return "skip-draft"
    if view.get("state") == "MERGED":
        who = ""
        merged_by = view.get("mergedBy") or {}
        if isinstance(merged_by, dict):
            who = merged_by.get("login") or ""
        if who in BOTS:
            return "dispatch"
        return "skip-human"
    if view.get("autoMergeRequest") is None and not (
        trigger == "Auto-approve" and attempt <= 6
    ):
        return "skip-not-auto"
    return "wait"


def _gh_json(args: list[str]) -> object:
    raw = subprocess.check_output(["gh", *args], text=True)
    return json.loads(raw)


def _gh_run(args: list[str]) -> None:
    subprocess.check_call(["gh", *args])


def main(
    env: dict[str, str] | None = None,
    gh_json: Callable[[list[str]], object] | None = None,
    gh_run: Callable[[list[str]], None] | None = None,
    sleep: Callable[[float], None] | None = None,
) -> int:
    env = os.environ if env is None else env
    gh_json = _gh_json if gh_json is None else gh_json
    gh_run = _gh_run if gh_run is None else gh_run
    sleep = time.sleep if sleep is None else sleep

    repo = env["REPO"]
    run_id = env["RUN_ID"]
    trigger = env.get("TRIGGER", "")
    head_sha = env.get("HEAD_SHA", "")
    head_branch = env.get("HEAD_BRANCH", "")

    run = gh_json(["api", f"repos/{repo}/actions/runs/{run_id}"])
    if not isinstance(run, dict):
        run = {}
    number = pull_number_from_run(run)
    if number is None and head_sha:
        commit_pulls = gh_json(
            [
                "api",
                f"repos/{repo}/commits/{head_sha}/pulls",
                "-H",
                "Accept: application/vnd.github+json",
            ]
        )
        if not isinstance(commit_pulls, list):
            commit_pulls = []
        number = pull_number_from_commit(commit_pulls, head_sha, head_branch)
    if number is None:
        if missing_exit_code(trigger) == 1:
            print(
                f"Auto-approve run {run_id} has no pull request",
                file=sys.stderr,
            )
            return 1
        print("CI run has no pull request")
        return 0

    for attempt in range(1, ATTEMPTS + 1):
        view = gh_json(
            [
                "pr",
                "view",
                str(number),
                "--repo",
                repo,
                "--json",
                "state,autoMergeRequest,mergedBy,headRefName,isDraft",
            ]
        )
        if not isinstance(view, dict):
            view = {}
        action = classify(view, trigger, attempt)
        if action == "dispatch":
            gh_run(
                [
                    "workflow",
                    "run",
                    "Release Please",
                    "--ref",
                    "main",
                    "--repo",
                    repo,
                ]
            )
            print(f"started Release Please after Actions merged {number}")
            return 0
        if action == "skip-release":
            print(f"release pull request {number} stays for a person to merge")
            return 0
        if action == "skip-draft":
            print(f"draft pull request {number}: auto-merge is deferred")
            return 0
        if action == "skip-human":
            who = ""
            merged_by = view.get("mergedBy") or {}
            if isinstance(merged_by, dict):
                who = merged_by.get("login") or ""
            print(f"merged by {who}; the push starts Release Please")
            return 0
        if action == "skip-not-auto":
            print(f"pull request {number} is not waiting on auto-merge")
            return 0
        sleep(WAIT_SECONDS)
    print(f"auto-merge for {number} did not finish", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
