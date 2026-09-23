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
        Command::Ask { state, policy } => ExitCode::from(ask_cmd(&state, policy.as_deref())),
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
    let client = match open_client(policy, shadow) {
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

fn ask_cmd(path: &PathBuf, policy: Option<&str>) -> u8 {
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
    let (state, questions) = match questions_from_state(&value) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{err}");
            return ask_code(&err);
        }
    };
    match block_on(client.ask(state, questions)) {
        Ok(_) => {
            println!("ok");
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
        let backend = scripted(&row.script.harm, row.script.confidence, &row.script.nouls);
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

fn scripted(harm: &str, confidence: f64, nouls: &serde_json::Map<String, Value>) -> FakeBackend {
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

fn questions_from_state(value: &Value) -> Result<(State, Vec<Question>), Error> {
    if !value.is_object() {
        return Err(Error::Wire(WireError::Json(
            "state must be an object".to_string(),
        )));
    }
    let state = State {
        trusted: value.get("trusted").cloned().unwrap_or(Value::Null),
        untrusted: value.get("untrusted").cloned().unwrap_or(Value::Null),
    };
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
    harm: String,
    confidence: f64,
    #[serde(default)]
    nouls: serde_json::Map<String, Value>,
}

fn default_name() -> String {
    "call".to_string()
}
