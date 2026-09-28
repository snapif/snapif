# Snapif

[![Snapif](docs/brand/readme-banner.svg)](https://github.com/snapif/snapif)

[![CI](https://github.com/snapif/snapif/actions/workflows/ci.yml/badge.svg?event=pull_request)](https://github.com/snapif/snapif/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/snapif?logo=rust)](https://crates.io/crates/snapif)
[![docs.rs](https://img.shields.io/docsrs/snapif?logo=docs.rs)](https://docs.rs/snapif)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](./LICENSE)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/snapif/snapif/badge)](https://securityscorecards.dev/viewer/?uri=github.com/snapif/snapif)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B62586%2Fgithub.com%2Fsnapif%2Fsnapif.svg?type=shield&issueType=license)](https://app.fossa.com/projects/custom%2B62586%2Fgithub.com%2Fsnapif%2Fsnapif?ref=badge_shield&issueType=license)

Snapif scores one tool call. The score is Auto, Review, or Escalate.

## Getting started

Build the `snapif` binary. The `cli` feature includes `http`.

```bash
cargo install snapif --features cli
```

From this repo, `cargo run --features cli -- --help` prints the same commands.

### Gate

Write one tool call to `call.json`:

```json
{"action_id":"tag","name":"list_files","args":{},"trusted":{"user_request":"list the workspace"},"untrusted":null,"script":{"harm":"read","confidence":0.91}}
```

```bash
SNAPIF_BACKEND=fake snapif gate --call call.json
```

The process prints one word and exits:

| Exit | Word | Meaning |
| --- | --- | --- |
| 0 | `auto` | The call may proceed. |
| 10 | `review` | A person should look first. |
| 11 | `escalate` | Do not run the call. |

`script` is only for `SNAPIF_BACKEND=fake`. A live score uses `typesafe` or `compatible`.

### Hook

`snapif hook` is a Claude Code PreToolUse hook. It reads one JSON object on stdin and prints a permission decision on stdout. A bad body still exits 0 and denies the call, because a hook error would otherwise let the call proceed.

```bash
printf '%s\n' '{"tool_name":"bash","tool_input":{"command":"ls"}}' | SNAPIF_BACKEND=fake snapif hook
```

### Example

This run uses a scripted fake scorer and prints `auto`:

```bash
cargo run --example pre_tool_use
```

The other example is in [examples/README.md](examples/README.md).

## Live score

- `SNAPIF_BACKEND` is `typesafe` or `compatible`.
- `SNAPIF_BASE_URL` is required for `compatible` and must be a URL.
- `SNAPIF_API_KEY` is required when the base URL is not a loopback address.
