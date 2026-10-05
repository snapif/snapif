#!/usr/bin/env python3
"""Print decision=merge or decision=skip for one Dependabot group.

fetch-metadata labels a commit pin as version-update:semver-major
because the two SHAs differ in the first dotted field. A pin may
share a group with a patch. A real semver-major in that group stays
manual.
"""

from __future__ import annotations

import json
import os
import re
import sys

MAJOR = "version-update:semver-major"
PIN = re.compile(r"^[0-9a-f]{40}$")


def decision(update_type: str, previous: str, deps: list | None) -> str:
    if isinstance(deps, list) and deps:
        for dep in deps:
            if not isinstance(dep, dict):
                return "skip"
            kind = str(dep.get("updateType") or "")
            prev = str(dep.get("prevVersion") or "")
            if kind == MAJOR and PIN.fullmatch(prev) is None:
                return "skip"
        return "merge"
    if update_type in (
        "version-update:semver-patch",
        "version-update:semver-minor",
    ):
        return "merge"
    if update_type == MAJOR and PIN.fullmatch(previous or "") is not None:
        return "merge"
    return "skip"


def main() -> int:
    raw = os.environ.get("DEPS", "")
    deps: list | None = None
    if raw and raw != "null":
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            print("decision=skip")
            return 0
        if not isinstance(parsed, list):
            print("decision=skip")
            return 0
        deps = parsed
    print(
        "decision="
        + decision(
            os.environ.get("UPDATE_TYPE", ""),
            os.environ.get("PREV", ""),
            deps,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
