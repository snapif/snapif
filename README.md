# Snapif

[![Snapif](docs/brand/readme-banner.svg)](https://github.com/snapif/snapif)

[![CI](https://github.com/snapif/snapif/actions/workflows/ci.yml/badge.svg?event=pull_request)](https://github.com/snapif/snapif/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/snapif?logo=rust)](https://crates.io/crates/snapif)
[![docs.rs](https://img.shields.io/docsrs/snapif?logo=docs.rs)](https://docs.rs/snapif)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](./LICENSE)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/snapif/snapif/badge)](https://securityscorecards.dev/viewer/?uri=github.com/snapif/snapif)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B62586%2Fgithub.com%2Fsnapif%2Fsnapif.svg?type=shield&issueType=license)](https://app.fossa.com/projects/custom%2B62586%2Fgithub.com%2Fsnapif%2Fsnapif?ref=badge_shield&issueType=license)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/15006/badge)](https://www.bestpractices.dev/en/projects/15006)

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

## Scenarios

### Same call, three scores

`bash.rm` is an exec action. With `SNAPIF_BACKEND=fake`, only `script.confidence` changes the word. Harm stays `exec`.

```json
{"action_id":"bash.rm","name":"bash","args":{"command":"rm -rf /tmp/demo"},"trusted":{"user_request":"clean the temp dir"},"untrusted":null,"script":{"harm":"exec","confidence":1.0}}
```

```bash
SNAPIF_BACKEND=fake snapif gate --call call.json
```

| `script.confidence` | Prints | Exit |
| --- | --- | --- |
| `1.0` | `auto` | 0 |
| `0.95` | `review` | 10 |
| `0.50` | `escalate` | 11 |

Auto on this row needs confidence `1.0`. `0.95` is high enough to review and not high enough to pass. `0.50` is below the floor, so the row escalates.

### Shadow, then enforce

A hook with no script gives the fake scorer nothing to answer, so the gate fails closed. `--shadow` still allows the tool and prints the verdict on stderr. The same hook without `--shadow` denies.

```bash
printf '%s\n' '{"tool_name":"bash","tool_input":{"command":"ls"}}' | SNAPIF_BACKEND=fake snapif hook --shadow
```

Stderr prints `escalate`. Stdout still allows:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"escalate"}}
```

Drop `--shadow` and stdout denies. The process still exits 0. The reason names the verdict and the short cause.

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"escalate: decode"}}
```

A scripted `Bash` command `rm -rf /tmp` with harm `exec` and confidence `0.95` matches `bash.rm` and asks:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"review: review_floor"}}
```

Watch with `--shadow` while you read the verdicts. Remove it when a deny should stop the tool.

### The host still decides

[examples/pre_tool_use.rs](examples/pre_tool_use.rs) scores `list_files` with `FakeBackend` and prints `auto`. `Review` and `Escalate` exit 1 in this example because the example stops. A host can show a person, or refuse, instead.

```rust
use serde_json::json;
use snapif::backends::fake::FakeBackend;
use snapif::ids::ActionId;
use snapif::policy::Policy;
use snapif::state::{PreparedCall, State};
use snapif::{Client, GateRequest, Verdict};

fn main() {
    let mut backend = FakeBackend::new().on_choice("harm_class", "read", 0.91);
    for id in snapif::backends::cascade::battery_ids() {
        if id.0 == "harm_class" {
            continue;
        }
        backend = backend.on_noul(&id.0, 0.0);
    }
    let client = Client::new(backend).policy(Policy::shipped("tool-gate").expect("policy"));
    let verdict = pollster::block_on(client.gate(GateRequest {
        action_id: ActionId::new("tag"),
        prepared: PreparedCall {
            name: "list_files".to_string(),
            args: json!({}),
        },
        state: State {
            trusted: json!({"user_request": "list the workspace"}),
            untrusted: json!(null),
        },
        extra_questions: vec![],
    }))
    .expect("gate");
    match verdict {
        Verdict::Auto(_) => println!("auto"),
        Verdict::Review(_) => {
            eprintln!("review");
            std::process::exit(1);
        }
        Verdict::Escalate(_) => {
            eprintln!("escalate");
            std::process::exit(1);
        }
    }
}
```

`Verdict::Auto` means this policy has no objection. The host still chooses whether the tool runs.

```bash
cargo run --example pre_tool_use
```

## Commands

| Command | What it does |
| --- | --- |
| `gate` | Score one tool call. Exit 0 `auto`, 10 `review`, 11 `escalate`, 1 programmer error. `--json` prints `verdict`, `reasons`, and `scores`. The exit code stays the same. |
| `ask` | Ask the questions in a policy. Exit 0 prints `ok`. Exit 2 is a bad body, 3 an API error, 4 rate limit, 5 auth, 1 programmer error. |
| `explain` | Print the gates for one action. It does not call a backend. |
| `hook` | Claude Code PreToolUse hook. Always exits 0. |
| `calibrate` | Score labeled rows. Prints `brier` for yes/no labels and `choice_accuracy` for choice labels. |
| `test` | Check conformance JSON files under a directory. |
| `replay` | Replay action fixtures on the fake backend. It never uses the network. |

`ask` reads a state file and prints `ok` when the policy has nothing further to ask:

```bash
printf '%s\n' '{"trusted":{"user_request":"list"},"untrusted":null}' > state.json
SNAPIF_BACKEND=fake snapif ask --policy tool-gate --state state.json
```

`--decisions` prints each decision as JSON instead of `ok`.

`explain` names the pack and the model. `git.push` has no Auto gate:

```bash
snapif explain --action git.push
```

`calibrate` reads a JSON or JSONL file, or a directory of those files. Each row needs `trusted`, `untrusted`, and `labels`. A label key must be a question that policy asks. A boolean label is a yes/no. A string label is the expected choice. `calibrate --gate` reads log or replay rows instead, compares `expected` to the gate verdict, and scores bool labels and a `harm_class` string when `labels` is present. Ask calibration stays the command without `--gate`.

`test` checks the files in a directory. The sample directory in this repo is `tests/conformance`:

```bash
snapif test --vectors tests/conformance
```

With the `http` feature, `--base-url` posts each valid vector.

`replay` reads JSONL. Each row needs `id`, `gate_request`, `script`, and `expected`. `tests/fixtures/actions.jsonl` is one such file. Stdout is one JSON object per row, including `id` and `got`. `--summary` still compares `expected` and prints auto, review, and escalate counts per action on stderr, plus the five most common `reasons` when the log has them. A `SNAPIF_LOG` row records those reasons and the policy id.

## Call JSON

`gate --call` reads one object. Use either flat fields or nested `prepared` and `state`.

| Field | Role |
| --- | --- |
| `action_id` | Selects the policy row. When it is omitted, `name` is the action. |
| `name` | Tool name, such as `list_files`. |
| `args` | The tool arguments. |
| `trusted` | Text the host wrote, such as `user_request`. |
| `untrusted` | Text from outside the host. `null` when there is none. |
| `script` | Only for `SNAPIF_BACKEND=fake`. Sets `harm` and `confidence`. A one-label choice keeps probability 1.0, so `top_prob` and `margin` stay 1.0. With more labels, the chosen label's probability is `confidence` and the others share the rest. |

`prepared` is `{ "name", "args" }`. `state` is `{ "trusted", "untrusted" }`.

## Hook result

A hook error still exits 0 and denies the call. A non-zero exit would let the tool run.

Allow:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"auto"}}
```

Review uses `permissionDecision` `ask`. The reason names the verdict and its short cause, such as `review: review_floor`. Escalate uses `deny`. The reason is `escalate` plus that cause, or the error text, such as `invalid json`. Auto uses `allow`.

`--shadow` prints the verdict on stderr and allows the tool. On `gate`, `--shadow` prints the verdict and keeps the same exit code.

## From Rust

The default features are empty. `cargo add snapif` does not build the binary or the HTTP client.

```toml
[dependencies]
snapif = "0.1"
```

```bash
cargo install snapif --features cli
```

The library entry points are `Policy::shipped`, `GateRequest`, `Client`, and `Verdict`. `Policy::shipped("tool-gate")` is the tool gate. `review` and `screen` are ask-only packs. They cannot return Auto. `SNAPIF_BACKEND=fake` is `FakeBackend`. A live client uses `SNAPIF_BACKEND=typesafe` or `compatible`.

`Verdict::Auto` means this policy has no objection. The host still decides whether to run the tool.

## Environment

| Variable | Role |
| --- | --- |
| `SNAPIF_BACKEND` | `fake`, `typesafe`, or `compatible`. Unset is an error. |
| `SNAPIF_MODEL` | Replaces the default model `jev-latest`. Blank is an error. |
| `SNAPIF_POLICY` | Shipped id or a `.toml` path. Unset uses `tool-gate`. `explain` and `replay` follow it when `--policy` is unset. An explicit `--policy` wins. |
| `SNAPIF_LOG` | Appends one replay row per gate. |
| `SNAPIF_CACHE` | Integer cache capacity for a process that evaluates more than one call. One `snapif gate` or `snapif hook` invocation does not reuse it. Unset leaves the cache off. |
| `TYPESAFE_API_KEY` | Required for `typesafe`. |
| `SNAPIF_BASE_URL` | Required for `compatible`. Origin only. |
| `SNAPIF_API_KEY` | Required for `compatible` when the base URL is not a loopback address. |

`typesafe` calls the TypeSafe API. `compatible` calls another server that speaks the same System One shape. Both send the model `jev-latest` unless `SNAPIF_MODEL` is set.
