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

`snapif hook` is a Claude Code PreToolUse hook. It reads one JSON object on stdin and prints a permission decision on stdout. A bad body still exits 0 and denies the call, because a hook error would otherwise let the call proceed. Stdin over 8 MiB is denied the same way. A missing or blank `tool_name` denies too, including with `--shadow`. A `transcript_path` that cannot be read denies the same way. An empty transcript file does not. `snapif hook --print-settings` prints a PreToolUse block that runs `snapif hook --shadow` and sets `SNAPIF_LOG` to `snapif-hook.jsonl` in the current directory. It does not read stdin.

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

A scripted `Bash` command `rm -rf /tmp` with harm `exec` and confidence `0.95` matches `bash.rm` and asks. `/bin/rm`, `RM`, and a tab in `git push` match the same rows. `sudo rm`, `sudo -nu root rm`, `FOO=1 rm`, `env rm`, `cd x && rm`, `bash -c 'rm -rf /'`, `env -S 'rm -rf /'`, and `git -C repo push` match those rows too. `$'rm'`, `$"rm"`, `bash -c $'rm -rf /'`, `exec rm`, `eval 'rm -rf /'`, `x='rm -rf /'; eval "$x"`, `{ x='rm -rf /'; eval "$x"; }`, `readonly x='rm -rf /'; eval "$x"`, `eval "$(echo 'rm -rf /')"`, `$(echo 'rm -rf /')`, `eval "$(echo 'rm -rf /' | cat)"`, `eval "$(echo 'rm -rf /' && true)"`, `printf -v x 'rm -rf /'; eval "$x"`, `read x <<< 'rm -rf /'; eval "$x"`, `timeout 1 rm`, and `xargs rm` match as well, because bash runs those words as `rm`. `trap 'rm -rf /' EXIT` and `trap rm EXIT` match `bash.rm`, because the shell runs that action when the signal arrives. `trap - EXIT`, `trap '' EXIT`, `trap EXIT`, and `trap -p` do not. `TRAP`, a path, `trap.exe`, and `sudo trap` do not match, because bash does not run that builtin. `time trap` does. `command time trap` does not, because `command` runs the `time` program. A rejected signal such as `SIGEXIT` or `64` does not match. `trap 'rm -rf /' NOTASIGNAL EXIT` still matches, because `EXIT` is a real signal. Homebrew's `gtimeout`, `gxargs`, `genv`, `gnice`, `gnohup`, `gstdbuf`, and `grm` match the same rows. `git.exe push`, `rm.exe -rf /tmp`, `timeout.exe 1 rm`, and `busybox.exe rm` match too. A hook command that arrives as a list of words is quoted before the match, so `C:/Program Files/Git/cmd/git.exe` and `push` are still the `git push` row, and `echo` plus `rm -rf /tmp` stays `echo`. The suffix is `.exe`, `.cmd`, `.bat`, or `.com`, in any letter case, and only when it is the whole extension. Dots after that suffix still count, so `git.exe. push` is the `git push` row. `rmdir`, `git push-all`, `echo rm`, `echo $'rm'`, `exec -a rm echo`, `timeout 1 echo rm`, `xargs echo rm`, `echo grm`, `rmdir.exe`, `git.exe push-all`, `rm.`, and `rm.exe.bak` do not. `git push && rm`, `git push $(rm)`, `find . -delete`, a shell heredoc or here-string whose body runs `rm`, `bash /dev/stdin` and `bash -s` with that body, `source /dev/stdin` with that body, `source` of a regular file that runs `rm`, `bash` of that file, `f() { rm; }; f`, and `cmd /c rm` match `bash.rm`. `powershell -Command rm` and `pwsh -Command rm` match `bash.rm`. `powershell -EncodedCommand`, `-E`, and `-EC` match that row when the value is UTF-16LE base64 of the script. A value that is not valid base64, or not valid UTF-16LE, stays unmatched. Snapif matches the decoded text and does not run it. `cat` with that stdin, `bash -c` with that stdin, a function that is never called, `echo ok # $(rm)`, `echo '<(rm)'`, `find . -name rm`, and `find . -name -delete` do not. `cat <(rm)` and `cat >(rm)` match `bash.rm`. A redirect is not a command word, so `rm>/dev/null`, `>/dev/null rm`, and `2>/dev/null rm` match `bash.rm`. `echo rm>/tmp/x` does not. Past eight nested shells the remainder is not read. On this policy a non-blank remainder matches `shell.deep`, which has no auto gate, so nine `bash -c` wrappers around `rm -rf /tmp/x` ask. Eight wrappers still match `bash.rm`. A policy with no `shell.deep` row leaves that deep command unmatched. `source path` and `bash path` read a regular file up to one megabyte. A missing path does not match. `bash -c` and `bash -s` do not read a later path. A larger file matches `shell.deep`. `cat` of a regular file and `$(<path)` are the text in that file. `cat` of several paths keeps each file that exists. `cat < path`, `grep . path`, and `head -n 1 path` are that text. `grep -h`, `grep -i`, `grep -w`, and `grep -o '.*'` still print the matching line. `grep -E` and `egrep` use that same match. `fgrep` searches for a fixed string. `grep -G` is the basic match. `grep -n`, `grep -H`, and `grep -o .` do not. A missing file does not match. `echo ... && true` still prints the echo. `printf -v` and `read` of a here-string set a shell value for a later `eval`. `tail -n 1 path`, `sed -n p path`, `awk '{print}' path`, and `cut -c 1- path` are that text. `awk 'NR==1 {print}'` and `awk '{ print }'` print that line too. `source <(echo 'rm -rf /')` and `bash <(echo 'rm -rf /')` run the printed text. `mapfile -t x < path` and `read x < path` set a shell value for a later `eval`. `sort path`, `uniq path`, and `tac path` print every line. `sort -s` and `sort -z` still print every line. `sort -c` and `sort -o` do not. `uniq -d` prints a repeated line. `tac -s` prints the same bytes. `sort --compress-program` runs that program. `base64 -d` prints the decoded text. `paste`, `expand`, `unexpand`, `column`, `fold`, `fmt`, `pr`, and `strings` print that line too. `cat -v` and `cat -s` still print it. `tee` copies its input. `tr` and `iconv -f utf-8 -t utf-8` print that text. `join` of a file with itself prints the line. `perl -pe ''` and `ruby -pe ''` print each line. `base32 -d` prints the decoded text. `dd if=path` copies that file. `cut -d ' ' -f 1-` keeps the line. `sed -n l` prints the line. `sed -n '/git/p'` prints a line that contains `git`. `sed -n 1l` and `sed -n '$l'` print that line too. `gsed` is that same `sed`. A python one-liner that writes `open(sys.argv[1]).read()`, a node one-liner that writes `readFileSync`, `ruby -e 'print File.read(ARGV[0])'`, and `php -r 'echo file_get_contents($argv[1]);'` print the file. `os.system(open(sys.argv[1]).read())`, node `execSync` of `readFileSync`, `ruby -e 'system(File.read(ARGV[0]))'`, and `php -r 'system(file_get_contents($argv[1]));'` run that file.

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
| `gate` | Score one tool call. Exit 0 `auto`, 10 `review`, 11 `escalate`, 1 programmer error. `--json` prints `verdict`, `reasons`, `scores`, `backend_id`, and `first_hop_error`. The exit code stays the same. |
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

`--decisions` prints each decision as JSON instead of `ok`. That object includes `backend_id` and `first_hop_error`.

`explain` names the pack and the model. `git.push` has no Auto gate:

```bash
snapif explain --action git.push
```

`calibrate` reads a JSON or JSONL file, or a directory of those files. Each row needs `trusted`, `untrusted`, and `labels`. A label key must be a question that policy asks. A boolean label is a yes/no. A string label is the expected choice. `calibrate --gate` reads log or replay rows instead, compares `expected` to the gate verdict, and scores bool labels and a `harm_class` string when `labels` is present. Ask calibration stays the command without `--gate`.

`test` checks the files in a directory. The sample directory in this repo is `tests/conformance`:

```bash
snapif test --vectors tests/conformance
```

With the `http` feature, `--base-url` posts each valid vector. When `SNAPIF_TIMEOUT_MS` is unset, that command waits 5000 milliseconds. When `SNAPIF_MODEL` is set, each posted vector uses that name instead of the model field in the file. A blank value is an error, and nothing is posted.

`replay` reads JSONL. Each row needs `id`, `gate_request`, `script`, and `expected`. `tests/fixtures/actions.jsonl` is one such file. Stdout is one JSON object per row, including `id` and `got`. `--summary` still compares `expected` and prints auto, review, and escalate counts per action on stderr, plus the five most common `reasons` when the log has them. The policy word on that line is the id stored on the row. A different `--policy` or `SNAPIF_POLICY` also prints `replayed with`. A `SNAPIF_LOG` row records those reasons, the policy id, `backend_id`, and `first_hop_error`. `first_hop_error` is true when the cascade first hop failed and the fallback answer was used. A low-confidence retry uses the same `cascade:` backend id and leaves that flag false. A backend failure sets `script.timeout`, so replay repeats the gate verdict instead of treating confidence 0 as a model score.

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

A prefix match follows the command that program runs. OpenSSH accepts `ProxyCommand` in any ASCII case, with a space or with `=`. Git ignores ASCII case in a config section and variable name, so `Credential.Helper` and `credential.https://example.com.helper` are read the same way as `credential.helper`. A credential helper runs only for the `credential` subcommand. On an ssh remote, `GIT_SSH_COMMAND` replaces `core.sshCommand`. A local path does not run that command. `export` reaches a later command when that builtin runs in the current shell. A pipeline does not publish it.

## Hook result

A hook error still exits 0 and denies the call. A non-zero exit would let the tool run. Stdin over 8 MiB is that deny, including with `--shadow`. A `transcript_path` that cannot be read is the same deny. An empty transcript file is not.

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
| `SNAPIF_MODEL` | Replaces the default model `jev-latest`. Blank is an error. `test --base-url` posts this name instead of the model field in the vector. |
| `SNAPIF_POLICY` | Shipped id or a `.toml` path. Unset uses `tool-gate`. `gate`, `ask`, `explain`, `hook`, `calibrate`, and `replay` follow it when `--policy` is unset. `test` does not read it. An explicit `--policy` wins. |
| `SNAPIF_SHADOW` | `1` or `true` is the same override as `--shadow`. |
| `SNAPIF_TIMEOUT_MS` | Milliseconds for `gate`, `ask`, `hook`, and `calibrate` without `--gate`. Unset is 2000. `test --base-url` uses the same variable; unset there is 5000. `0` is a zero-millisecond budget. |
| `SNAPIF_LOG` | Appends one replay row per gate. |
| `SNAPIF_CACHE` | Integer cache capacity for a process that evaluates more than one call. One `snapif gate` or `snapif hook` invocation does not reuse it. Unset leaves the cache off. |
| `TYPESAFE_API_KEY` | Required for `typesafe`. |
| `SNAPIF_BASE_URL` | Required for `compatible`. Origin only. |
| `SNAPIF_API_KEY` | Required for `compatible` when the base URL is not a loopback address. |
| `SNAPIF_ALLOW_PRIVATE_HTTP` | `1` or `true` allows `http` to loopback, link-local, RFC1918, and IPv6 unique-local addresses. Unset keeps `http` on loopback only. The same rule applies to `SNAPIF_CASCADE_BASE_URL` and to `test --base-url`. |
| `SNAPIF_CASCADE_BASE_URL` | When set, and the backend is not `fake`, the first hop is this origin with `SNAPIF_API_KEY`. The fallback is `typesafe` or `compatible`. Shipped battery questions always go to the fallback. If that hop errors, or it leaves one of those questions out, the gate does not score the first hop's answer. `first_hop_error` on the log, on `gate --json`, and on `ask --decisions` is true only for that failure. |

`typesafe` calls the TypeSafe API. `compatible` calls another server that speaks the same System One shape. Both send the model `jev-latest` unless `SNAPIF_MODEL` is set.

## Models

`SNAPIF_BACKEND=typesafe` with `TYPESAFE_API_KEY` calls `https://api.typesafe.ai/v1/systemone`. The model is `jev-latest` unless `SNAPIF_MODEL` names a pinned id.

A different System One host uses `compatible`. `SNAPIF_BASE_URL` is the origin only. `SNAPIF_MODEL` is that host's model id. A public origin also needs `SNAPIF_API_KEY`. Leaving the model unset sends `jev-latest`, and a host that does not serve that name fails the gate closed (exit 11).

A score legend may be a map of labels or a list of labels in order. Snapif accepts both.

The Getting started `call.json` includes `script`. That field is only for `fake`. A live gate uses a call without it:

```json
{"action_id":"tag","name":"list_files","args":{},"trusted":{"user_request":"list the workspace"},"untrusted":null}
```

```bash
snapif gate --call live.json
```

The commands below set the backend and the model, then run that file.

### TypeSafe Jev

`jev-latest` is the model these commands were checked against. `ask`, `gate`, and `snapif test --base-url` all completed. The verdict word depends on the call.

```bash
SNAPIF_BACKEND=typesafe \
SNAPIF_MODEL=jev-latest \
  snapif gate --call live.json
```

```bash
SNAPIF_API_KEY="$TYPESAFE_API_KEY" \
SNAPIF_MODEL=jev-latest \
  snapif test --vectors tests/conformance \
  --base-url https://api.typesafe.ai
```

`test --base-url` uses the compatible client, so the TypeSafe key is passed as `SNAPIF_API_KEY` for that command.

### Drex

Drex on `https://console.nace.ai` uses `compatible`. `drex-v2.0` is the model these commands were checked against. `ask` included a choice, a noul, and a score. `gate` and `snapif test --base-url` completed. The verdict word depends on the call.

```bash
SNAPIF_BACKEND=compatible \
SNAPIF_BASE_URL=https://console.nace.ai \
SNAPIF_MODEL=drex-v2.0 \
  snapif gate --call live.json
```

```bash
SNAPIF_MODEL=drex-v2.0 \
  snapif test --vectors tests/conformance \
  --base-url https://console.nace.ai
```

`SNAPIF_API_KEY` is the Nace key. `drex-latest` completed an ask on that same origin.

### Ollama

Ollama 0.35 and later serves System One at `http://127.0.0.1:11434`. Pull a decision model first. Loopback does not need `SNAPIF_API_KEY`.

`tev1:0.8b` is the model these commands were checked against. `ask`, `gate`, and `snapif test --base-url` all completed. A warm `gate` fit in the default 2000 ms budget. The verdict word depends on the call.

```bash
ollama pull tev1:0.8b
SNAPIF_BACKEND=compatible \
SNAPIF_BASE_URL=http://127.0.0.1:11434 \
SNAPIF_MODEL=tev1:0.8b \
  snapif gate --call live.json
```

```bash
SNAPIF_MODEL=tev1:0.8b \
  snapif test --vectors tests/conformance \
  --base-url http://127.0.0.1:11434
```

The same origin takes whatever decision model you pulled. Set `SNAPIF_MODEL` to that name. Only `tev1:0.8b` was checked here. With the model unset, Ollama returns 404 for `jev-latest` and `gate` exits 11.
