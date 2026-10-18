#!/usr/bin/env python3
"""Open, replace, or close the issue for a scheduled workflow failure."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from datetime import date, datetime, timezone

GREEN = {"success", "skipped"}
LABELS = ["nightly-failure", "ready"]
DEFAULT_ASSIGNEE = "SebTardif"


def parse_results(text: str) -> list[tuple[str, str]]:
    rows: list[tuple[str, str]] = []
    for line in text.splitlines():
        stripped = line.strip()
        if "=" not in stripped:
            continue
        name, result = stripped.split("=", 1)
        name = name.strip()
        result = result.strip()
        if name and result:
            rows.append((name, result))
    return rows


def classify(results: list[tuple[str, str]]) -> tuple[str, list[str]]:
    if not results:
        return "empty", []
    failed = [name for name, result in results if result == "failure"]
    if failed:
        return "red", failed
    if any(result == "cancelled" for _, result in results):
        return "cancelled", []
    if all(result in GREEN for _, result in results):
        return "green", []
    return "empty", []


def state_comment(
    workflow: str, first_failed_on: str, signature: str, run_id: str
) -> str:
    payload = {
        "workflow": workflow,
        "first_failed_on": first_failed_on,
        "signature": signature,
        "run_id": str(run_id),
    }
    encoded = json.dumps(payload, separators=(",", ":"))
    return f"<!-- nightly-failure-state: {encoded} -->"


def issue_state(body: str) -> dict | None:
    marker = "<!-- nightly-failure-state:"
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped.startswith(marker) or not stripped.endswith("-->"):
            continue
        raw = stripped[len(marker) : -3].strip()
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return None
        if isinstance(parsed, dict):
            return parsed
    return None


def _iso_on_or_before(value: object, today: str) -> str | None:
    if not isinstance(value, str):
        return None
    try:
        date.fromisoformat(value)
    except ValueError:
        return None
    if value <= today:
        return value
    return None


def day_count(first_failed_on: str, today: str) -> int:
    start = date.fromisoformat(first_failed_on)
    end = date.fromisoformat(today)
    return max(1, (end - start).days + 1)


def issue_title(prefix: str, signature: str, days: int) -> str:
    unit = "day" if days == 1 else "days"
    return f"{prefix}: {signature} ({days} consecutive {unit})"


def failure_body(
    workflow: str, signature: str, days: int, run_url: str, comment: str
) -> str:
    return (
        f"The scheduled {workflow} run failed.\n"
        "\n"
        f"Failed jobs: {signature}.\n"
        f"This is day {days}.\n"
        f"Run: {run_url}\n"
        "\n"
        f"{comment}\n"
    )


def normalize_issues(issues: list) -> list[dict]:
    normalized = []
    for item in issues:
        if not isinstance(item, dict) or "number" not in item:
            continue
        labels: list[str] = []
        for label in item.get("labels") or []:
            if isinstance(label, str):
                labels.append(label)
            elif isinstance(label, dict) and isinstance(label.get("name"), str):
                labels.append(label["name"])
        created_raw = item.get("created_on") or item.get("createdAt") or ""
        created_on = ""
        if isinstance(created_raw, str) and len(created_raw) >= 10:
            created_on = created_raw[:10]
            try:
                date.fromisoformat(created_on)
            except ValueError:
                created_on = ""
        body = item.get("body") if isinstance(item.get("body"), str) else ""
        title = item.get("title") if isinstance(item.get("title"), str) else ""
        normalized.append(
            {
                "number": int(item["number"]),
                "title": title,
                "body": body,
                "labels": labels,
                "created_on": created_on,
            }
        )
    return normalized


def matches(issue: dict, workflow: str, prefix: str) -> bool:
    state = issue_state(issue["body"])
    if state is not None and state.get("workflow"):
        return state.get("workflow") == workflow
    return issue["title"].startswith(prefix)


def assignee_name(assignee: str) -> str:
    cleaned = assignee.strip()
    return cleaned or DEFAULT_ASSIGNEE


def plan(
    *,
    today: str,
    workflow: str,
    prefix: str,
    assignee: str,
    run_id: str,
    run_url: str,
    results: list[tuple[str, str]],
    issues: list,
) -> list[dict]:
    kind, failed = classify(results)
    matched = [
        issue
        for issue in normalize_issues(issues)
        if matches(issue, workflow, prefix)
    ]
    matched.sort(key=lambda issue: issue["number"])
    if kind == "green":
        return [
            {
                "op": "close",
                "number": issue["number"],
                "comment": f"The scheduled run is green: {run_url}",
            }
            for issue in matched
        ]
    if kind != "red":
        return []
    signature = ", ".join(failed)
    current = matched[-1] if matched else None
    others = matched[:-1]
    state = issue_state(current["body"]) if current is not None else None
    if (
        current is not None
        and state is not None
        and str(state.get("run_id")) == str(run_id)
        and state.get("signature") == signature
    ):
        return [
            {
                "op": "close",
                "number": issue["number"],
                "comment": f"Superseded by #{current['number']}.",
            }
            for issue in others
        ]
    first = today
    if current is not None:
        if state is not None:
            first = _iso_on_or_before(state.get("first_failed_on"), today) or today
        else:
            first = _iso_on_or_before(current.get("created_on"), today) or today
    days = day_count(first, today)
    rendered = failure_body(
        workflow,
        signature,
        days,
        run_url,
        state_comment(workflow, first, signature, run_id),
    )
    title = issue_title(prefix, signature, days)
    same_day = (
        current is not None
        and state is not None
        and current["created_on"] == today
        and state.get("signature") == signature
    )
    if same_day and current is not None:
        actions: list[dict] = [
            {
                "op": "update",
                "number": current["number"],
                "title": title,
                "body": rendered,
                "labels": list(LABELS),
            }
        ]
        actions.extend(
            {
                "op": "close",
                "number": issue["number"],
                "comment": f"Superseded by #{current['number']}.",
            }
            for issue in others
        )
        return actions
    actions = [
        {
            "op": "create",
            "title": title,
            "body": rendered,
            "assignee": assignee_name(assignee),
            "labels": list(LABELS),
        }
    ]
    actions.extend(
        {
            "op": "close",
            "number": issue["number"],
            "comment": "Superseded by #{created}.",
        }
        for issue in matched
    )
    return actions


def ensure_labels(repo: str, run) -> None:
    rows = json.loads(
        run(["label", "list", "--repo", repo, "--limit", "100", "--json", "name"])
    )
    names = {
        row["name"] for row in rows if isinstance(row, dict) and "name" in row
    }
    if "nightly-failure" not in names:
        run(
            [
                "label",
                "create",
                "nightly-failure",
                "--repo",
                repo,
                "--description",
                "Scheduled workflow failure",
                "--color",
                "B60205",
            ]
        )
    if "ready" not in names:
        run(
            [
                "label",
                "create",
                "ready",
                "--repo",
                repo,
                "--description",
                "Accepted backlog",
                "--color",
                "0E8A16",
            ]
        )


def execute(actions: list[dict], repo: str, run) -> None:
    created: int | None = None
    for action in actions:
        op = action["op"]
        if op == "create":
            argv = [
                "issue",
                "create",
                "--repo",
                repo,
                "--title",
                action["title"],
                "--body",
                action["body"],
                "--assignee",
                action["assignee"],
            ]
            for label in action["labels"]:
                argv.extend(["--label", label])
            argv.extend(["--json", "number", "--jq", ".number"])
            created = int(run(argv).strip())
        elif op == "update":
            argv = [
                "issue",
                "edit",
                str(action["number"]),
                "--repo",
                repo,
                "--title",
                action["title"],
                "--body",
                action["body"],
            ]
            for label in action["labels"]:
                argv.extend(["--add-label", label])
            run(argv)
        elif op == "close":
            comment = action["comment"]
            if "{created}" in comment:
                if created is None:
                    raise SystemExit("report: close ran before create")
                comment = comment.replace("{created}", str(created))
            run(
                [
                    "issue",
                    "close",
                    str(action["number"]),
                    "--repo",
                    repo,
                    "--comment",
                    comment,
                ]
            )
        else:
            raise SystemExit(f"report: unknown op {op}")


def gh(argv: list[str]) -> str:
    result = subprocess.run(
        ["gh", *argv], check=False, capture_output=True, text=True
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode or 1)
    return result.stdout


def main() -> int:
    repo = os.environ.get("REPO", "").strip()
    run_id = os.environ.get("RUN_ID", "").strip()
    run_url = os.environ.get("RUN_URL", "").strip()
    workflow = os.environ.get("REPORT_WORKFLOW", "").strip()
    prefix = os.environ.get("REPORT_PREFIX", "").strip()
    assignee = os.environ.get("ASSIGNEE", "")
    today = os.environ.get("REPORT_TODAY", "").strip()
    if not today:
        today = datetime.now(timezone.utc).date().isoformat()
    if not all((repo, run_id, run_url, workflow, prefix)):
        print(
            "report: missing REPO, RUN_ID, RUN_URL, REPORT_WORKFLOW, or REPORT_PREFIX",
            file=sys.stderr,
        )
        return 1
    try:
        date.fromisoformat(today)
    except ValueError:
        print("report: REPORT_TODAY is not YYYY-MM-DD", file=sys.stderr)
        return 1
    listed = json.loads(
        gh(
            [
                "issue",
                "list",
                "--repo",
                repo,
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title,body,labels,createdAt",
            ]
        )
    )
    if not isinstance(listed, list):
        print("report: issue list was not a list", file=sys.stderr)
        return 1
    actions = plan(
        today=today,
        workflow=workflow,
        prefix=prefix,
        assignee=assignee,
        run_id=run_id,
        run_url=run_url,
        results=parse_results(os.environ.get("REPORT_RESULTS", "")),
        issues=listed,
    )
    if not actions:
        return 0
    if any(action["op"] in {"create", "update"} for action in actions):
        ensure_labels(repo, gh)
    execute(actions, repo, gh)
    return 0


if __name__ == "__main__":
    sys.exit(main())
