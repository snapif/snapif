# Contributing to Snapif

Human contributors can start here. `AGENTS.md` is for coding agents.

## Where to start

Look at [good first issues](https://github.com/snapif/snapif/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) and [help wanted](https://github.com/snapif/snapif/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22).

## Setup

```bash
git clone https://github.com/snapif/snapif.git
cd snapif
make check
```

`make check` is the local gate: format, clippy, tests, deny, and workflow lint.

## Pull requests

- Use a conventional title: `feat:`, `fix:`, `docs:`, `chore:`, `test:`, `ci:`, `refactor:`, `perf:`, `style:`, or `build:`.
- Sign every commit with `git commit -s`. The sign-off email is `git config user.email`.
- Do not add a `bline-*`, `canact`, `wiremux`, or `wiremux-auth` dependency.
- A release pull request publishes the crate. Leave it for a maintainer to merge.

## Reporting

Use the issue templates. For a vulnerability, follow [SECURITY.md](SECURITY.md).
