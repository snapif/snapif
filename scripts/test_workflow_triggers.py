#!/usr/bin/env python3
"""Lock Recipe A: ci.yml compiles on PR and merge_group only."""

from __future__ import annotations

import os
import subprocess
import tempfile
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
        self.assertIn("anthropic\\.com", text)
        self.assertIn("Claude-Session:", text)

    def test_gitleaks_gates_ci_and_is_not_an_install_action_tool(self) -> None:
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("name: Gitleaks", text)
        self.assertIn(
            "needs: [stealth, lint, workflows, test, gitleaks, fuzz, publish-dry-run, semver-checks]",
            text,
        )
        self.assertIn('test "$PUBLISH" = success', text)
        self.assertIn('test "$SEMVER" = success', text)
        self.assertIn("cargo publish --dry-run --locked", text)
        self.assertIn("cargo-semver-checks@0.50.0", text)
        self.assertIn("max_total_time=10", text)
        self.assertIn("cargo-fuzz@0.13.2", text)
        self.assertIn("RUSTUP_TOOLCHAIN: nightly", text)
        self.assertIn("--target x86_64-unknown-linux-gnu", text)
        self.assertIn('test "$GITLEAKS" = success', text)
        self.assertIn('test "$FUZZ" = success', text)
        self.assertNotIn("tool: gitleaks@", text)
        self.assertIn("gitleaks_8.30.1_linux_x64.tar.gz", text)

    def test_public_scanners_are_live(self) -> None:
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
            self.assertIn(needle, live)
        on_block = _on_block(text)
        self.assertIn("pull_request:", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertNotIn("push:", on_block)

    def test_release_please_runs_on_main_and_does_not_test(self) -> None:
        text = (WORKFLOWS / "release-please.yml").read_text(encoding="utf-8")
        on_block = _on_block(text)
        self.assertIn("push:", on_block)
        self.assertIn("branches: [main]", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertNotIn("pull_request:", on_block)
        self.assertNotIn("cargo test", text)
        self.assertIn("bash publisher/scripts/publish-crate.sh", text)
        self.assertIn("CARGO_REGISTRY_TOKEN", text)
        self.assertIn("git tag -s -f", text)
        self.assertIn("allow-loopback-pinentry", text)
        self.assertIn("passphrase-fd 3", text)
        self.assertIn("gpg.program", text)
        self.assertIn("GPG_PRIVATE_KEY", text)
        self.assertIn("actions: write", text)
        self.assertIn('gh workflow run Release --ref main --repo "$REPO" -f tag="$TAG"', text)
        self.assertIn(
            "github.event_name == 'workflow_dispatch' && inputs.tag != ''",
            text,
        )
        publish = (ROOT / "scripts" / "publish-crate.sh").read_text(encoding="utf-8")
        self.assertIn("cargo publish --locked", publish)
        self.assertIn("snapif-release", publish)
        release = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        release_on = _on_block(release)
        self.assertIn("workflow_dispatch:", release_on)
        self.assertIn("push:", release_on)
        self.assertIn('tags:', release_on)
        self.assertIn('"snapif-v[0-9]+.[0-9]+.[0-9]+"', release)
        self.assertNotIn("pull_request:", release_on)
        for needle in (
            "bash scripts/install-dist.sh",
            "actions/attest-build-provenance@",
            "sigstore/cosign-installer@",
            "cargo-cyclonedx@0.5.9",
            "HOMEBREW_TAP_TOKEN",
            "snapif-sbom.cdx.json",
            "bash scripts/attach-release-signatures.sh",
            'toolchain: "1.95"',
            "publishing=false",
            "plan-dist-manifest.json",
        ):
            self.assertIn(needle, release)
        for banned in (
            "cargo-dist-installer.sh",
            "cargo-dist-installer.ps1",
            "sh.rustup.rs",
            "publish-scoop",
            "winget",
            "chocolatey",
            "npm-package",
        ):
            self.assertNotIn(banned, release)
        config = (ROOT / "release-please-config.json").read_text(encoding="utf-8")
        self.assertNotIn("release-as", config)
        self.assertIn('"release-type": "rust"', config)
        approve = (WORKFLOWS / "auto-approve.yml").read_text(encoding="utf-8")
        self.assertIn("startsWith(github.head_ref, 'release-please')", approve)
        after = (WORKFLOWS / "release-please-after-merge.yml").read_text(
            encoding="utf-8"
        )
        on_block = _on_block(after)
        self.assertIn("workflow_run:", on_block)
        self.assertIn("workflow_dispatch:", on_block)
        self.assertIn('workflows: ["CI"]', after)
        self.assertIn('gh workflow run "Release Please"', after)
        self.assertIn("github-actions", after)
        self.assertIn("release-please*)", after)

    def test_composite_action_does_not_interpolate_inputs_in_run(self) -> None:
        text = (ROOT / ".github" / "actions" / "snapif" / "action.yml").read_text(
            encoding="utf-8"
        )
        run = text.split("run:", 1)[1]
        self.assertNotIn("${{", run)
        for name in (
            "SNAPIF_ACTION_PATH",
            "SNAPIF_MODE",
            "SNAPIF_CALL",
            "SNAPIF_STATE",
            "SNAPIF_POLICY",
            "SNAPIF_QUESTION",
            "SNAPIF_DENY_LABEL",
            "SNAPIF_VERSION",
        ):
            self.assertIn(f"{name}:", text)
            self.assertIn(f"${name}", run)

    def test_ci_light_path_is_the_release_bot(self) -> None:
        text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        needle = "run: cargo check --locked --all-targets"
        bot = "github.event.pull_request.user.login == 'github-actions[bot]'"
        self.assertEqual(text.count(needle), 2)
        start = 0
        for _ in range(2):
            at = text.index(needle, start)
            window = text[max(0, at - 500) : at]
            self.assertIn(bot, window)
            self.assertIn("startsWith(github.head_ref, 'release-please')", window)
            start = at + len(needle)
        self.assertEqual(
            text.count("github.event.pull_request.user.login != 'github-actions[bot]'"),
            4,
        )

    def test_signature_script_rejects_a_plain_version_tag(self) -> None:
        script = ROOT / "scripts" / "attach-release-signatures.sh"
        with tempfile.TemporaryDirectory() as tmp:
            asset = Path(tmp) / "snapif.tar.xz"
            asset.write_text("not a real archive\n", encoding="utf-8")
            env = os.environ.copy()
            env.update(
                {
                    "DRY_RUN": "1",
                    "TAG": "snapif-v0.2.0",
                    "REPO": "snapif/snapif",
                    "ARTIFACTS": tmp,
                }
            )
            ok = subprocess.run(
                ["bash", str(script)],
                check=False,
                env=env,
                capture_output=True,
                text=True,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertIn("SUBJECT: snapif.tar.xz", ok.stdout)
            env["TAG"] = "v0.2.0"
            bad = subprocess.run(
                ["bash", str(script)],
                check=False,
                env=env,
                capture_output=True,
                text=True,
            )
            self.assertEqual(bad.returncode, 1)
            self.assertIn("snapif-vX.Y.Z", bad.stderr)

    def test_make_scans_fuzz_manifest(self) -> None:
        text = (ROOT / "Makefile").read_text(encoding="utf-8")
        deny = "cargo deny --manifest-path fuzz/Cargo.toml check"
        forbid = "bash scripts/forbid-deps.sh fuzz/Cargo.toml"
        self.assertEqual(text.count(deny), 3)
        self.assertEqual(text.count(forbid), 3)


if __name__ == "__main__":
    unittest.main()
