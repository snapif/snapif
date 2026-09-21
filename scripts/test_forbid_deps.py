#!/usr/bin/env python3
"""Named [dependencies.*] tables are still dependencies."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "forbid-deps.sh"


def _run(toml_text: str) -> subprocess.CompletedProcess[str]:
    with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as fh:
        fh.write(toml_text)
        path = fh.name
    return subprocess.run(
        ["bash", str(SCRIPT), path],
        check=False,
        capture_output=True,
        text=True,
    )


class ForbidDepsTests(unittest.TestCase):
    def test_bare_table_key_is_caught(self) -> None:
        result = _run("[dependencies]\nwiremux = \"0.1\"\n")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_named_table_is_caught(self) -> None:
        result = _run("[dependencies.wiremux]\nversion = \"0.1\"\n")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_reqwest_named_table_is_ok(self) -> None:
        result = _run("[dependencies.reqwest]\nversion = \"0.12\"\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_real_cargo_toml_is_ok(self) -> None:
        result = subprocess.run(
            ["bash", str(SCRIPT), str(ROOT / "Cargo.toml")],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
