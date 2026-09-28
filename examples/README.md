# Examples

| Example | Run | What it does |
| --- | --- | --- |
| `pre_tool_use.rs` | `cargo run --example pre_tool_use` | Scores a `list_files` call with a fake backend and prints `auto`. |
| `triage.rs` | `cargo run --example triage` | Asks the shipped triage questions with a fake backend and prints `billing`. |

Both examples use `FakeBackend`. They do not call a live scorer.
