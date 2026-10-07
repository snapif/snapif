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

`snapif hook` is a Claude Code PreToolUse hook. It reads one JSON object on stdin and prints a permission decision on stdout. A bad body still exits 0 and denies the call, because a hook error would otherwise let the call proceed. A missing or blank `tool_name` denies too, including with `--shadow`. `snapif hook --print-settings` prints a PreToolUse block that runs `snapif hook --shadow` and sets `SNAPIF_LOG` to `snapif-hook.jsonl` in the current directory. It does not read stdin.

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

Auto on this row needs confidence `1.0` and harm `exec`. `0.95` is high enough to review and not high enough to pass. `0.50` is below the floor, so the row escalates. A lower label does not auto: `rm` scored as `read` at confidence `1.0` asks. Omitting `action_id` still selects that row when `name` is `bash` and `args.command` is `rm`.

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

A scripted `Bash` command `rm -rf /tmp` with harm `exec` and confidence `0.95` matches `bash.rm` and asks. `/bin/rm`, `RM`, and a tab in `git push` match the same rows. `sudo rm`, `sudo -nu root rm`, `FOO=1 rm`, `env rm`, `cd x && rm`, `bash -c 'rm -rf /'`, `env -S 'rm -rf /'`, and `git -C repo push` match those rows too. `$'rm'`, `$"rm"`, `bash -c $'rm -rf /'`, `exec rm`, `eval 'rm -rf /'`, `timeout 1 rm`, and `xargs rm` match as well, because bash runs those words as `rm`. Homebrew's `gtimeout`, `gxargs`, `genv`, `gnice`, `gnohup`, `gstdbuf`, and `grm` match the same rows. `git.exe push`, `rm.exe -rf /tmp`, `timeout.exe 1 rm`, and `busybox.exe rm` match too. A hook command that arrives as a list of words is quoted before the match, so `C:/Program Files/Git/cmd/git.exe` and `push` are still the `git push` row, and `echo` plus `rm -rf /tmp` stays `echo`. The suffix is `.exe`, `.cmd`, `.bat`, or `.com`, in any letter case, and only when it is the whole extension. Dots after that suffix still count, so `git.exe. push` is the `git push` row. `rmdir`, `git push-all`, `echo rm`, `echo $'rm'`, `exec -a rm echo`, `timeout 1 echo rm`, `xargs echo rm`, `echo grm`, `rmdir.exe`, `git.exe push-all`, `rm.`, and `rm.exe.bak` do not. `git push && rm`, `git push $(rm)`, `find . -delete`, a shell heredoc or here-string whose body runs `rm`, `bash /dev/stdin` and `bash -s` with that body, `source /dev/stdin` with that body, `f() { rm; }; f`, and `cmd /c rm` match `bash.rm`. `cat` with that stdin, `bash -c` with that stdin, a function that is never called, `echo ok # $(rm)`, `echo '<(rm)'`, `find . -name rm`, and `find . -name -delete` do not. `cat <(rm)` and `cat >(rm)` match `bash.rm`. A redirect is not a command word, so `rm>/dev/null`, `>/dev/null rm`, and `2>/dev/null rm` match `bash.rm`. `echo rm>/tmp/x` does not.

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

With the `http` feature, `--base-url` posts each valid vector. When `SNAPIF_TIMEOUT_MS` is unset, that command waits 5000 milliseconds.

`replay` reads JSONL. Each row needs `id`, `gate_request`, `script`, and `expected`. `tests/fixtures/actions.jsonl` is one such file. Stdout is one JSON object per row, including `id` and `got`. `--summary` still compares `expected` and prints auto, review, and escalate counts per action on stderr, plus the five most common `reasons` when the log has them. A `SNAPIF_LOG` row records those reasons and the policy id. A backend failure sets `script.timeout`, so replay repeats the gate verdict instead of treating confidence 0 as a model score.

## Call JSON

`gate --call` reads one object. Use either flat fields or nested `prepared` and `state`.

| Field | Role |
| --- | --- |
| `action_id` | Selects the policy row. When it is omitted, a host tool name plus `args.command` selects a prefix row such as `bash.rm` or `git.push`. Otherwise `name` is the action. |
| `name` | Tool name, such as `list_files`. |
| `args` | The tool arguments. |
| `trusted` | Text the host wrote, such as `user_request`. |
| `untrusted` | Text from outside the host. `null` when there is none. |
| `script` | Only for `SNAPIF_BACKEND=fake`. Sets `harm` and `confidence`. A one-label choice keeps probability 1.0, so `top_prob` and `margin` stay 1.0. With more labels, the chosen label gets `confidence` and the others share the rest, so `top_prob` equals `confidence` while no other label exceeds that mass. Otherwise every label is `1/n`. |

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
snapif = "0.2"
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
| `SNAPIF_POLICY` | Shipped id or a `.toml` path. Unset uses `tool-gate`. `gate`, `ask`, `explain`, `hook`, `calibrate`, and `replay` follow it when `--policy` is unset. `test` does not read it. An explicit `--policy` wins. |
| `SNAPIF_SHADOW` | `1` or `true` is the same override as `--shadow`. |
| `SNAPIF_TIMEOUT_MS` | Milliseconds for `gate`, `ask`, `hook`, and `calibrate` without `--gate`. Unset is 2000. `test --base-url` uses the same variable; unset there is 5000. `0` is a zero-millisecond budget. |
| `SNAPIF_LOG` | Appends one replay row per gate. |
| `SNAPIF_CACHE` | Integer cache capacity for a process that evaluates more than one call. One `snapif gate` or `snapif hook` invocation does not reuse it. Unset leaves the cache off. |
| `TYPESAFE_API_KEY` | Required for `typesafe`. |
| `SNAPIF_BASE_URL` | Required for `compatible`. Origin only. |
| `SNAPIF_API_KEY` | Required for `compatible` when the base URL is not a loopback address. |
| `SNAPIF_ALLOW_PRIVATE_HTTP` | `1` or `true` allows `http` to loopback, link-local, RFC1918, and IPv6 unique-local addresses. Unset keeps `http` on loopback only. The same rule applies to `SNAPIF_CASCADE_BASE_URL` and to `test --base-url`. |
| `SNAPIF_CASCADE_BASE_URL` | When set, and the backend is not `fake`, the first hop is this origin with `SNAPIF_API_KEY`. The fallback is `typesafe` or `compatible`. |

`typesafe` calls the TypeSafe API. `compatible` calls another server that speaks the same System One shape. Both send the model `jev-latest` unless `SNAPIF_MODEL` is set.
