use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::Value;
use snapif::backends::fake::FakeBackend;
use snapif::error::Error;
use snapif::error::WireError;
use snapif::ids::{ActionId, QuestionId};
use snapif::policy::Policy;
use snapif::question::{ChoiceQ, NoulQ, Question, ScoreQ};
use snapif::state::{PreparedCall, State};
use snapif::wire::{self, WireQuestion};
use snapif::{AnyBackend, Client, GateRequest, Verdict};

#[derive(Parser)]
#[command(name = "snapif")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Action path. Exit 0 Auto, 10 Review, 11 Escalate, 1 programmer error.
    ///
    /// An optional `script` object on the call file is used only when `SNAPIF_BACKEND=fake`.
    ///
    /// `SNAPIF_LOG` appends one replay row per gate. `SNAPIF_CACHE` is an integer capacity; unset leaves the cache off.
    Gate {
        /// Shipped id or `.toml` path. Unset keeps the policy from `SNAPIF_POLICY`.
        #[arg(long)]
        policy: Option<String>,
        #[arg(long)]
        call: PathBuf,
        #[arg(long)]
        shadow: bool,
    },
    /// Observation. Exit 0 ok, 2 decode, 3 api, 4 rate limit, 5 auth, 1 programmer error.
    Ask {
        #[arg(long)]
        state: PathBuf,
        /// Shipped id or `.toml` path. Unset keeps the policy from `SNAPIF_POLICY`.
        #[arg(long)]
        policy: Option<String>,
        /// Print each decision as JSON. Without this flag, stdout stays `ok`.
        #[arg(long)]
        decisions: bool,
    },
    /// Print effective gates for one action. Does not call a backend.
    Explain {
        #[arg(long)]
        action: String,
        /// Shipped id or `.toml` path. Default is tool-gate.
        #[arg(long)]
        policy: Option<String>,
    },
    /// Claude Code PreToolUse hook. JSON on stdin, a permission decision on stdout.
    Hook {
        #[arg(long)]
        policy: Option<String>,
        #[arg(long)]
        shadow: bool,
    },
    /// Score labeled rows. Prints Brier for nouls and accuracy for choices.
    Calibrate {
        /// A JSONL file, or a directory of `.json` and `.jsonl` files.
        path: PathBuf,
        #[arg(long)]
        policy: Option<String>,
    },
    /// Check conformance JSON. With the http feature, `--base-url` posts each valid vector.
    Test {
        #[arg(long)]
        vectors: PathBuf,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Replay action fixtures on FakeBackend. Never uses the network.
    Replay {
        path: PathBuf,
        #[arg(long, default_value = "tool-gate")]
        policy: String,
        #[arg(long)]
        shadow: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Gate {
            policy,
            call,
            shadow,
        } => ExitCode::from(gate_cmd(policy.as_deref(), &call, shadow)),
        Command::Ask {
            state,
            policy,
            decisions,
        } => ExitCode::from(ask_cmd(&state, policy.as_deref(), decisions)),
        Command::Explain { action, policy } => {
            ExitCode::from(explain_cmd(&action, policy.as_deref()))
        }
        Command::Hook { policy, shadow } => ExitCode::from(hook_cmd(policy.as_deref(), shadow)),
        Command::Calibrate { path, policy } => {
            ExitCode::from(calibrate_cmd(&path, policy.as_deref()))
        }
        Command::Test { vectors, base_url } => {
            ExitCode::from(test_cmd(&vectors, base_url.as_deref()))
        }
        Command::Replay {
            path,
            policy,
            shadow,
        } => ExitCode::from(replay_cmd(&path, &policy, shadow)),
    }
}

fn gate_cmd(policy: Option<&str>, call: &PathBuf, shadow: bool) -> u8 {
    let mut client = match open_client(policy, shadow) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let file: CallFile = match read_json(call) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if let Some(script) = &file.script {
        let backend = scripted(
            &script.harm,
            script.confidence,
            &script.nouls,
            script.timeout,
        );
        if let Err(err) = client.replace_fake(backend) {
            eprintln!("{err}");
            return 1;
        }
    }
    let request = GateRequest {
        action_id: ActionId::new(&file.action_id),
        prepared: PreparedCall {
            name: file.name,
            args: file.args,
        },
        state: State {
            trusted: file.trusted,
            untrusted: file.untrusted,
        },
        extra_questions: Vec::new(),
    };
    match block_on(client.gate(request)) {
        Ok(verdict) => {
            if let Some(cause) = verdict_reasons(&verdict)
                .iter()
                .find_map(|reason| match reason {
                    snapif::verdict::UnsureReason::Backend { cause } => Some(cause.as_str()),
                    _ => None,
                })
            {
                eprintln!("backend: {cause}");
            }
            println!("{}", verdict_name(&verdict));
            gate_code(&verdict)
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn ask_cmd(path: &PathBuf, policy: Option<&str>, decisions: bool) -> u8 {
    let client = match open_client(policy, false) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return ask_code(&err);
        }
    };
    let value: Value = match read_json(path) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let pack = client.battery_id();
    let (state, questions) = match questions_from_state(&value, pack) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{err}");
            return ask_code(&err);
        }
    };
    match block_on(client.ask(state, questions)) {
        Ok(out) => {
            if decisions {
                println!("{}", decisions_json(&out));
            } else {
                println!("ok");
            }
            0
        }
        Err(err) => {
            eprintln!("{err}");
            ask_code(&err)
        }
    }
}

fn test_cmd(vectors: &PathBuf, base_url: Option<&str>) -> u8 {
    match base_url {
        None => test_local(vectors),
        Some(raw) => test_remote(vectors, raw),
    }
}

fn test_local(vectors: &PathBuf) -> u8 {
    match each_vector(vectors, |path, bytes| {
        vector_request(path, bytes).map(|_| ())
    }) {
        Ok(count) => finish_checked(count, "conformance vectors"),
        Err(code) => code,
    }
}

#[cfg(not(feature = "http"))]
fn test_remote(_vectors: &PathBuf, _raw: &str) -> u8 {
    eprintln!("--base-url needs the http feature, which this build does not include");
    1
}

#[cfg(feature = "http")]
fn test_remote(vectors: &PathBuf, raw: &str) -> u8 {
    let url = match url::Url::parse(raw) {
        Ok(url) => url,
        Err(_) => {
            eprintln!("--base-url: the value is not a URL");
            return 1;
        }
    };
    let key = std::env::var("SNAPIF_API_KEY")
        .ok()
        .filter(|value| !value.is_empty());
    let backend = match snapif::backends::http::HttpBackend::compatible(url, key) {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("{err}");
            return ask_code(&err);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let result = each_vector(vectors, |path, bytes| {
        let Some(request) = vector_request(path, bytes)? else {
            return Ok(());
        };
        let evaluated = match runtime.block_on(snapif::backend::Backend::evaluate(
            &backend,
            request.clone(),
            deadline,
        )) {
            Ok(evaluated) => evaluated,
            Err(err) => {
                eprintln!("{path:?}: {err}");
                return Err(backend_code(&err));
            }
        };
        if let Err(err) = wire::check_response(&request.questions, &evaluated.wire) {
            eprintln!("{path:?}: {err}");
            return Err(2);
        }
        Ok(())
    });
    match result {
        Ok(count) => finish_checked(count, "conformance vectors"),
        Err(code) => code,
    }
}

#[cfg(feature = "http")]
fn backend_code(err: &snapif::error::BackendError) -> u8 {
    match err {
        snapif::error::BackendError::Auth => 5,
        snapif::error::BackendError::RateLimit => 4,
        snapif::error::BackendError::Rejected { .. } => 2,
        snapif::error::BackendError::Timeout
        | snapif::error::BackendError::Overloaded
        | snapif::error::BackendError::Transport(_) => 3,
        _ => 3,
    }
}

fn each_vector(
    vectors: &PathBuf,
    mut visit: impl FnMut(&std::path::Path, &[u8]) -> Result<(), u8>,
) -> Result<usize, u8> {
    let mut checked = 0usize;
    let entries = match fs::read_dir(vectors) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("{}: {err}", vectors.display());
            return Err(1);
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("{err}");
                return Err(1);
            }
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("{path:?}: {err}");
                return Err(1);
            }
        };
        if serde_json::from_slice::<Value>(&bytes).is_err() {
            eprintln!("{path:?}: invalid json");
            return Err(2);
        }
        visit(&path, &bytes)?;
        checked += 1;
    }
    Ok(checked)
}

fn finish_checked(count: usize, what: &str) -> u8 {
    if count == 0 {
        eprintln!("no {what} checked");
        return 1;
    }
    println!("ok");
    0
}

fn replay_cmd(path: &PathBuf, policy: &str, shadow: bool) -> u8 {
    let policy = match Policy::load(policy) {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if path.is_dir() {
        eprintln!("{}: replay path must be a file", path.display());
        return 1;
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            return 1;
        }
    };
    let mut checked = 0usize;
    let mut failed = false;
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        checked += 1;
        let row: ReplayRow = match serde_json::from_str(line) {
            Ok(row) => row,
            Err(err) => {
                eprintln!("line {}: {err}", line_no + 1);
                return 1;
            }
        };
        let extras = match wire_questions(&row.gate_request.extra_questions) {
            Ok(extras) => extras,
            Err(err) => {
                eprintln!("{}: {err}", row.id);
                return 1;
            }
        };
        let name = if row.gate_request.prepared.name.is_empty() {
            row.gate_request.action_id.clone()
        } else {
            row.gate_request.prepared.name.clone()
        };
        let backend = scripted(
            &row.script.harm,
            row.script.confidence,
            &row.script.nouls,
            row.script.timeout,
        );
        let mut client = Client::new(backend).policy(policy.clone());
        if shadow || env_shadow() {
            client = client.shadow(true);
        }
        let verdict = match block_on(client.gate(GateRequest {
            action_id: ActionId::new(&row.gate_request.action_id),
            prepared: PreparedCall {
                name,
                args: row.gate_request.prepared.args,
            },
            state: State {
                trusted: row.gate_request.state.trusted,
                untrusted: row.gate_request.state.untrusted,
            },
            extra_questions: extras,
        })) {
            Ok(verdict) => verdict,
            Err(err) => {
                eprintln!("{}: {err}", row.id);
                return 1;
            }
        };
        let got = verdict_name(&verdict);
        println!(
            "{}",
            serde_json::json!({
                "id": row.id,
                "expected": row.expected,
                "got": got,
            })
        );
        if !got.eq_ignore_ascii_case(&row.expected) {
            failed = true;
        }
    }
    if checked == 0 {
        eprintln!("no replay rows checked");
        return 1;
    }
    if failed { 1 } else { 0 }
}

fn block_on<T>(future: impl std::future::Future<Output = Result<T, Error>>) -> Result<T, Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}

fn scripted(
    harm: &str,
    confidence: f64,
    nouls: &serde_json::Map<String, Value>,
    timeout: bool,
) -> FakeBackend {
    if timeout {
        return FakeBackend::new().on_timeout("harm_class");
    }
    let mut backend = FakeBackend::new().on_choice("harm_class", harm, confidence);
    for id in snapif::backends::cascade::battery_ids() {
        if id.0 == "harm_class" {
            continue;
        }
        let value = nouls.get(&id.0).and_then(Value::as_f64).unwrap_or(0.0);
        backend = backend.on_noul(&id.0, value);
    }
    backend
}

fn open_client(policy: Option<&str>, shadow: bool) -> Result<Client<AnyBackend>, Error> {
    let mut client = Client::from_env()?;
    if let Some(spec) = policy {
        client = client.policy(Policy::load(spec)?);
    }
    if shadow {
        client = client.shadow(true);
    }
    Ok(client)
}

fn questions_from_state(
    value: &Value,
    policy: Option<&str>,
) -> Result<(State, Vec<Question>), Error> {
    if !value.is_object() {
        return Err(Error::Wire(WireError::Json(
            "state must be an object".to_string(),
        )));
    }
    let state = State {
        trusted: value.get("trusted").cloned().unwrap_or(Value::Null),
        untrusted: value.get("untrusted").cloned().unwrap_or(Value::Null),
    };
    if value.get("questions").is_none()
        && let Some(pack) = pack_questions(policy)
    {
        return Ok((state, pack));
    }
    let questions = value
        .get("questions")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let questions = match questions {
        Value::Object(map) => wire_questions(&map)?,
        _ => {
            return Err(Error::Wire(WireError::Json(
                "questions must be an object".to_string(),
            )));
        }
    };
    Ok((state, questions))
}

fn pack_questions(policy: Option<&str>) -> Option<Vec<Question>> {
    match policy? {
        "triage" => Some(snapif::triage::questions()),
        "review" => Some(snapif::review::questions()),
        "screen" => Some(snapif::screen::questions()),
        _ => None,
    }
}

fn reason_value(reason: &snapif::verdict::UnsureReason) -> Value {
    use snapif::verdict::UnsureReason;
    match reason {
        UnsureReason::BelowFloor { confidence, floor } => {
            serde_json::json!({"tag": "below_floor", "confidence": confidence, "floor": floor})
        }
        UnsureReason::BelowAuto { confidence, auto } => {
            serde_json::json!({"tag": "below_auto", "confidence": confidence, "auto": auto})
        }
        UnsureReason::ReviewFloor {
            confidence,
            floor,
            auto,
        } => {
            serde_json::json!({"tag": "review_floor", "confidence": confidence, "floor": floor, "auto": auto})
        }
        UnsureReason::NoulBand { noul } => serde_json::json!({"tag": "noul_band", "noul": noul}),
        UnsureReason::Battery { id, when, excerpt } => {
            serde_json::json!({"tag": "battery", "id": id.0, "when": format!("{when:?}"), "excerpt": excerpt})
        }
        UnsureReason::AuthorityClaim { noul } => {
            serde_json::json!({"tag": "authority_claim", "noul": noul})
        }
        UnsureReason::Decode(err) => {
            serde_json::json!({"tag": "decode", "message": err.to_string()})
        }
        UnsureReason::Wire => serde_json::json!({"tag": "wire"}),
        UnsureReason::Backend { cause } => serde_json::json!({"tag": "backend", "cause": cause}),
        UnsureReason::CascadeStillUnsure => serde_json::json!({"tag": "cascade_still_unsure"}),
        UnsureReason::HarmClassBump { from, to } => {
            serde_json::json!({"tag": "harm_class_bump", "from": format!("{from:?}"), "to": format!("{to:?}")})
        }
        UnsureReason::Truncated => serde_json::json!({"tag": "truncated"}),
        _ => serde_json::json!({"tag": "other"}),
    }
}

fn decisions_json(out: &snapif::AskOut) -> String {
    use snapif::verdict::{Decision, UntypedDecision};
    let rows: Vec<Value> = out
        .decisions
        .iter()
        .map(|(id, decision)| {
            let score = out.scores.get(id.0.as_str()).copied();
            match decision {
                UntypedDecision::Choice(Decision::Known(label)) => serde_json::json!({
                    "id": id.0, "type": "choice", "known": label, "score": score,
                }),
                UntypedDecision::Choice(Decision::Unsure { reason, guess }) => serde_json::json!({
                    "id": id.0, "type": "choice", "unsure": reason_value(reason), "guess": guess, "score": score,
                }),
                UntypedDecision::Score(Decision::Known(value)) => serde_json::json!({
                    "id": id.0, "type": "score", "known": value, "score": score,
                }),
                UntypedDecision::Score(Decision::Unsure { reason, guess }) => serde_json::json!({
                    "id": id.0, "type": "score", "unsure": reason_value(reason), "guess": guess, "score": score,
                }),
                UntypedDecision::Noul(Decision::Known(value)) => serde_json::json!({
                    "id": id.0, "type": "noul", "known": value, "score": score,
                }),
                UntypedDecision::Noul(Decision::Unsure { reason, guess }) => serde_json::json!({
                    "id": id.0, "type": "noul", "unsure": reason_value(reason), "guess": guess, "score": score,
                }),
            }
        })
        .collect();
    serde_json::json!({
        "decisions": rows,
        "usage": out.usage,
        "backend_id": out.backend_id,
        "truncated": out.truncated_untrusted,
    })
    .to_string()
}

fn explain_cmd(action: &str, policy: Option<&str>) -> u8 {
    let policy = match policy {
        Some(spec) => Policy::load(spec),
        None => Policy::shipped("tool-gate").map_err(Error::Policy),
    };
    let policy = match policy {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if action.trim().is_empty() {
        eprintln!("action id must not be blank");
        return 1;
    }
    let id = ActionId::new(action);
    let gates = match snapif::policy::effective_gates(&policy, &id, None) {
        Ok(gates) => gates,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let configured = policy.actions.contains_key(&id);
    let row = policy.actions.get(&id).or(policy.default_action.as_ref());
    let Some(row) = row else {
        eprintln!("unknown action {action}");
        return 1;
    };
    let raw_auto = row
        .auto
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let effective_auto = gates
        .auto
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    println!("action {action}");
    if !configured {
        println!("source default_action");
    }
    println!("class {:?}", row.class);
    println!("when_unsure {:?}", row.when_unsure);
    println!(
        "block_on {}",
        row.block_on
            .iter()
            .map(|block| format!(
                "{}:{}",
                block.id.0,
                format!("{:?}", block.when).to_ascii_lowercase()
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    println!("raw_auto {raw_auto}");
    println!("raw_review {}", row.review);
    println!("escalate_below {}", gates.escalate_below);
    println!("review {}", gates.review);
    println!("auto {effective_auto}");
    if row.auto != gates.auto {
        println!(
            "auto moved from the raw value because a floor raised it or a harm bump cleared it"
        );
    }
    println!("battery {}", policy.battery.0);
    match &policy.shipped_id {
        Some(id) => {
            println!("pack {id}");
            println!(
                "pack_version {}",
                snapif::policy::Policy::shipped_pack_version(id)
            );
        }
        None => {
            println!("pack");
            println!("pack_version 0");
        }
    }
    let model = match std::env::var("SNAPIF_MODEL") {
        Ok(raw) => {
            let model = raw.trim();
            if model.is_empty() {
                eprintln!("SNAPIF_MODEL must not be blank");
                return 1;
            }
            model.to_string()
        }
        Err(_) => "jev-latest".to_string(),
    };
    println!("model {model}");
    0
}

fn hook_cmd(policy: Option<&str>, shadow: bool) -> u8 {
    use std::io::Read;
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("invalid json");
        return 1;
    }
    let value: Value = match serde_json::from_str::<Value>(&input) {
        Ok(value) if value.is_object() => value,
        _ => {
            eprintln!("invalid json");
            return 1;
        }
    };
    let name = value
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let args = value
        .get("tool_input")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let trusted = hook_trusted(&value);
    let client = match open_client(policy, shadow) {
        Ok(client) => client,
        Err(err) => {
            hook_decision("deny", &err.to_string());
            return 0;
        }
    };
    let request = GateRequest {
        action_id: ActionId::new(name),
        prepared: PreparedCall {
            name: name.to_string(),
            args,
        },
        state: State {
            trusted,
            untrusted: value.get("tool_input").cloned().unwrap_or(Value::Null),
        },
        extra_questions: Vec::new(),
    };
    match block_on(client.gate(request)) {
        Ok(verdict) => {
            let name = verdict_name(&verdict);
            if shadow {
                eprintln!("{name}");
                hook_decision("allow", name);
                return 0;
            }
            match verdict {
                Verdict::Auto(_) => hook_decision("allow", "auto"),
                Verdict::Review(_) => hook_decision("deny", "review"),
                Verdict::Escalate(_) => hook_decision("deny", "escalate"),
            }
            0
        }
        Err(err) => {
            hook_decision("deny", &err.to_string());
            0
        }
    }
}

fn hook_trusted(value: &Value) -> Value {
    if let Some(trusted) = value.get("trusted").filter(|item| item.is_object()) {
        return trusted.clone();
    }
    match hook_user_turn(value) {
        Some(turn) => serde_json::json!({"user_request": turn}),
        None => serde_json::json!({}),
    }
}

fn hook_user_turn(value: &Value) -> Option<String> {
    for key in ["prompt", "user_prompt"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let text = clip_turn(text);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    if let Some(text) = transcript_tail(value.get("transcript")) {
        return Some(text);
    }
    value
        .get("transcript_path")
        .and_then(Value::as_str)
        .and_then(transcript_file_tail)
}

fn clip_turn(text: &str) -> String {
    text.trim().chars().take(500).collect()
}

fn transcript_tail(value: Option<&Value>) -> Option<String> {
    let items = value?.as_array()?;
    for item in items.iter().rev() {
        let role = item.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" && role != "human" {
            continue;
        }
        let text = item
            .get("content")
            .and_then(Value::as_str)
            .map(clip_turn)
            .filter(|text| !text.is_empty())?;
        return Some(text);
    }
    None
}

fn transcript_file_tail(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines().rev().take(40) {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(turn) = transcript_tail(Some(&serde_json::json!([row]))) {
            return Some(turn);
        }
    }
    None
}

fn hook_decision(decision: &str, reason: &str) {
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason,
            }
        })
    );
}

fn calibrate_cmd(path: &PathBuf, policy: Option<&str>) -> u8 {
    let rows = match read_calibrate_rows(path) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if rows.is_empty() {
        eprintln!("{}: no calibration rows", path.display());
        return 1;
    }
    let client = match open_client(policy, false) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return ask_code(&err);
        }
    };
    let pack = client.battery_id();
    let mut card = snapif::scorecard::Scorecard::default();
    let mut unused_labels = Vec::new();
    for (line_no, line) in rows.iter().enumerate() {
        let row: Value = match serde_json::from_str(line) {
            Ok(row) => row,
            Err(err) => {
                eprintln!("line {}: {err}", line_no + 1);
                return 1;
            }
        };
        let (state, questions) = match questions_from_state(&row, pack) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("{err}");
                return ask_code(&err);
            }
        };
        let out = match block_on(client.ask(state, questions)) {
            Ok(out) => out,
            Err(err) => {
                eprintln!("{err}");
                return ask_code(&err);
            }
        };
        let labels = row.get("labels").and_then(Value::as_object);
        let Some(labels) = labels else {
            eprintln!("line {}: missing labels", line_no + 1);
            return 1;
        };
        for (id, score) in &out.scores {
            let Some(label) = labels.get(id) else {
                continue;
            };
            if let Some(truth) = label.as_bool() {
                card.add_noul(*score, truth);
            } else if let Some(expected) = label.as_str() {
                let matched = out.decisions.iter().any(|(key, decision)| {
                    key.0 == *id && choice_known(decision) == Some(expected)
                });
                card.add_choice(matched);
            }
        }
        for id in labels.keys() {
            if !out.scores.contains_key(id) && !unused_labels.iter().any(|seen| seen == id) {
                unused_labels.push(id.clone());
            }
        }
    }
    if card.is_empty() {
        if unused_labels.is_empty() {
            eprintln!("{}: no labeled scores", path.display());
        } else {
            eprintln!(
                "{}: no labeled scores; not asked: {}",
                path.display(),
                unused_labels.join(",")
            );
        }
        return 1;
    }
    if let Some(brier) = card.brier() {
        println!("brier {brier}");
        for (index, count, fraction) in card.bins() {
            println!("bin {index} count {count} true_fraction {fraction}");
        }
    }
    if let Some(accuracy) = card.choice_accuracy() {
        println!("choice_accuracy {accuracy}");
    }
    0
}

fn choice_known(decision: &snapif::verdict::UntypedDecision) -> Option<&str> {
    match decision {
        snapif::verdict::UntypedDecision::Choice(snapif::verdict::Decision::Known(label)) => {
            Some(label.as_str())
        }
        _ => None,
    }
}

fn read_calibrate_rows(path: &PathBuf) -> Result<Vec<String>, Error> {
    let mut rows = Vec::new();
    if path.is_dir() {
        let mut names: Vec<_> = fs::read_dir(path)
            .map_err(|err| {
                Error::Io(std::io::Error::new(
                    err.kind(),
                    format!("{}: {err}", path.display()),
                ))
            })?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|entry| {
                entry
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "json" || ext == "jsonl")
            })
            .collect();
        names.sort();
        if names.is_empty() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{}: no calibration rows", path.display()),
            )));
        }
        for name in names {
            push_calibrate_rows(&mut rows, &name)?;
        }
        return Ok(rows);
    }
    push_calibrate_rows(&mut rows, path)?;
    Ok(rows)
}

fn push_calibrate_rows(rows: &mut Vec<String>, path: &std::path::Path) -> Result<(), Error> {
    let text = fs::read_to_string(path).map_err(|err| {
        Error::Io(std::io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    })?;
    let json_doc = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "json");
    if json_doc {
        let value: Value = serde_json::from_str(&text)
            .map_err(|err| Error::Wire(WireError::Json(format!("{}: {err}", path.display()))))?;
        match value {
            Value::Array(items) => {
                for item in items {
                    rows.push(item.to_string());
                }
            }
            other => rows.push(other.to_string()),
        }
        return Ok(());
    }
    for line in text.lines() {
        if !line.trim().is_empty() {
            rows.push(line.to_string());
        }
    }
    Ok(())
}

fn wire_questions(map: &serde_json::Map<String, Value>) -> Result<Vec<Question>, Error> {
    if map.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::json!({
        "model": "jev-latest",
        "state": {},
        "questions": map,
    });
    let bytes =
        serde_json::to_vec(&body).map_err(|err| Error::Wire(WireError::Json(err.to_string())))?;
    let request = wire::decode_request(&bytes)?;
    Ok(request
        .questions
        .into_iter()
        .map(|(id, question)| question_from_wire(id, question))
        .collect())
}

fn question_from_wire(id: String, question: WireQuestion) -> Question {
    let id = QuestionId::new(id);
    match question {
        WireQuestion::Choice {
            instructions,
            criteria,
        } => Question::Choice(ChoiceQ {
            id,
            instructions,
            criteria,
        }),
        WireQuestion::Score {
            instructions,
            criteria,
        } => Question::Score(ScoreQ {
            id,
            instructions,
            criteria,
        }),
        WireQuestion::Noul {
            instructions,
            criteria,
        } => Question::Noul(NoulQ {
            id,
            instructions,
            criteria,
        }),
    }
}

fn vector_request(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<Option<snapif::wire::WireRequest>, u8> {
    match wire::decode_request(bytes) {
        Ok(_) if expects_reject(bytes) => {
            eprintln!("{path:?}: marked reject but decoded");
            Err(2)
        }
        Ok(request) => Ok(Some(request)),
        Err(WireError::UnknownType(_)) if expects_reject(bytes) => Ok(None),
        Err(err) => {
            eprintln!("{path:?}: {err}");
            Err(2)
        }
    }
}

fn expects_reject(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| {
            value
                .get("expect")
                .and_then(Value::as_str)
                .map(|text| text == "reject")
        })
        .unwrap_or(false)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<T, Error> {
    let text = fs::read_to_string(path).map_err(|err| {
        Error::Io(std::io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
    serde_json::from_str(&text).map_err(|err| Error::Wire(WireError::Json(err.to_string())))
}

fn env_shadow() -> bool {
    matches!(
        std::env::var("SNAPIF_SHADOW").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True")
    )
}

fn verdict_reasons(verdict: &Verdict) -> &[snapif::verdict::UnsureReason] {
    match verdict {
        Verdict::Auto(hint) | Verdict::Review(hint) | Verdict::Escalate(hint) => &hint.reasons,
    }
}

fn verdict_name(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Auto(_) => "auto",
        Verdict::Review(_) => "review",
        Verdict::Escalate(_) => "escalate",
    }
}

fn gate_code(verdict: &Verdict) -> u8 {
    match verdict {
        Verdict::Auto(_) => 0,
        Verdict::Review(_) => 10,
        Verdict::Escalate(_) => 11,
    }
}

fn ask_code(err: &Error) -> u8 {
    match err {
        Error::Decode(_) | Error::Wire(_) | Error::Rejected { .. } => 2,
        Error::Backend(_) | Error::Timeout(_) | Error::Overloaded => 3,
        Error::RateLimit => 4,
        Error::Auth(_) => 5,
        _ => 1,
    }
}

#[derive(Debug, Deserialize)]
struct CallFile {
    action_id: String,
    #[serde(default = "default_name")]
    name: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    trusted: Value,
    #[serde(default)]
    untrusted: Value,
    #[serde(default)]
    script: Option<ReplayScript>,
}

#[derive(Debug, Deserialize)]
struct ReplayRow {
    id: String,
    gate_request: ReplayRequest,
    script: ReplayScript,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    action_id: String,
    #[serde(default)]
    prepared: ReplayPrepared,
    #[serde(default)]
    state: ReplayState,
    #[serde(default)]
    extra_questions: serde_json::Map<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
struct ReplayPrepared {
    #[serde(default)]
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Default, Deserialize)]
struct ReplayState {
    #[serde(default)]
    trusted: Value,
    #[serde(default)]
    untrusted: Value,
}

#[derive(Debug, Deserialize)]
struct ReplayScript {
    #[serde(default = "default_harm")]
    harm: String,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    nouls: serde_json::Map<String, Value>,
    #[serde(default)]
    timeout: bool,
}

fn default_name() -> String {
    "call".to_string()
}

fn default_harm() -> String {
    "read".to_string()
}
