# Contributing to Snapif

Human contributors can start here. `AGENTS.md` is for coding agents.

## Where to start

Look at [good first issues](https://github.com/snapif/snapif/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) and [help wanted](https://github.com/snapif/snapif/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22).

## Setup

Rust 1.95 comes from `rust-toolchain.toml` when rustup is installed.
The local gate also needs GNU make, bash, python3, cargo-deny, actionlint, and zizmor.
On Windows, `bash` is often the WSL launcher. `scripts/assert-stealth.sh` uses `gh` or `gh.exe`.

```bash
git clone https://github.com/snapif/snapif.git
cd snapif
make check
```

`make check` runs format, stealth, clippy, rustdoc, tests, deny (`cargo deny` and `scripts/forbid-deps.sh`), `scripts/test_workflow_triggers.py`, `scripts/test_forbid_deps.py`, `scripts/test_release_please_after_merge.py`, actionlint, and zizmor.

CI also runs gitleaks, a 10-second nightly fuzz (`make fuzz`), `cargo publish --dry-run --locked`, and `cargo semver-checks`.

## Pull requests

- Use a conventional title: `feat:`, `fix:`, `docs:`, `chore:`, `test:`, `ci:`, `refactor:`, `perf:`, `style:`, or `build:`.
- Sign every commit with `git commit -s`. The sign-off email is `git config user.email`.
- Do not add a `bline-*`, `canact`, `wiremux`, or `wiremux-auth` dependency.
- A release pull request publishes the crate. Leave it for a maintainer to merge.

## Reporting

Use the issue templates. For a vulnerability, follow [SECURITY.md](SECURITY.md).
