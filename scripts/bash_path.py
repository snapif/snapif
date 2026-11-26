"""Turn a local path into an argument the `bash` on PATH can open."""

from __future__ import annotations

import os
import subprocess
from functools import cache
from pathlib import Path


@cache
def _wsl_bash() -> bool:
    if os.name != "nt":
        return False
    result = subprocess.run(
        ["bash", "-c", "uname -s"],
        check=False,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() == "Linux"


def share_with_bash(env: dict[str, str], *names: str) -> dict[str, str]:
    """Let WSL bash see the named variables. Other bashes already see them."""
    if not _wsl_bash():
        return env
    out = dict(env)
    parts = [part for part in out.get("WSLENV", "").split(":") if part]
    known = {part.split("/")[0] for part in parts}
    for name in names:
        if name not in known:
            parts.append(name)
            known.add(name)
    out["WSLENV"] = ":".join(parts)
    return out


def bash_path(path: Path | str) -> str:
    """Linux bash keeps the path. WSL bash needs /mnt/<drive>/...

    A backslash in a bash argument is an escape, so C:\\Users becomes
    C:Users before the script starts. Git Bash opens C:/Users.
    """
    text = str(Path(path).resolve())
    if os.name != "nt":
        return text
    if _wsl_bash() and len(text) >= 2 and text[1] == ":":
        rest = text[2:].replace("\\", "/")
        return f"/mnt/{text[0].lower()}{rest}"
    return text.replace("\\", "/")
