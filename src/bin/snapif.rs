use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::Value;
use snapif::backends::fake::FakeBackend;
use snapif::error::Error;
use snapif::error::WireError;
use snapif::ids::ActionId;
use snapif::policy::Policy;
use snapif::state::{PreparedCall, State};
use snapif::wire;
use snapif::{Client, GateRequest, Verdict};

const NOULS: [&str; 6] = [
    "irreversible",
    "destructive",
    "exfil",
    "off_task",
    "intent_match",
    "authority_claim",
];

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
        #[arg(long, default_value = "tool-gate")]
        policy: String,
        #[arg(long)]
        call: PathBuf,
        #[arg(long)]
        shadow: bool,
    },
    /// Observation. Exit 0 ok, 2 decode, 3 api, 4 rate limit, 5 auth, 1 programmer error.
    Ask {
        #[arg(long)]
        state: PathBuf,
        #[arg(long, default_value = "tool-gate")]
        policy: String,
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
        } => ExitCode::from(gate_cmd(&policy, &call, shadow)),
        Command::Ask { state, policy } => ExitCode::from(ask_cmd(&state, &policy)),
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

fn gate_cmd(policy: &str, call: &PathBuf, shadow: bool) -> u8 {
    let policy = match load_policy(policy) {
        Ok(policy) => policy,
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
    let backend = scripted(&file.harm, file.confidence, &file.nouls);
    let mut client = Client::new(backend).policy(policy);
    if shadow || env_shadow() {
        client = client.shadow(true);
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
    match pollster::block_on(client.gate(request)) {
        Ok(verdict) => {
            println!("{}", verdict_name(&verdict));
            gate_code(&verdict)
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn ask_cmd(path: &PathBuf, policy: &str) -> u8 {
    let policy = match load_policy(policy) {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let value: Value = match read_json(path) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let state = State {
        trusted: value.get("trusted").cloned().unwrap_or(Value::Null),
        untrusted: value.get("untrusted").cloned().unwrap_or(Value::Null),
    };
    let client = Client::new(FakeBackend::new()).policy(policy);
    match pollster::block_on(client.ask(state, Vec::new())) {
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
    match each_vector(vectors, |path, bytes| match wire::decode_request(bytes) {
        Ok(_) | Err(WireError::UnknownType(_)) => Ok(()),
        Err(err) => {
            eprintln!("{path:?}: {err}");
            Err(2)
        }
    }) {
        Ok(()) => {
            println!("ok");
            0
        }
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
            eprintln!("SNAPIF_BASE_URL");
            return 1;
        }
    };
    let backend = match snapif::backends::http::HttpBackend::compatible(url, None) {
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
        let request = match wire::decode_request(bytes) {
            Ok(request) => request,
            Err(WireError::UnknownType(_)) => return Ok(()),
            Err(err) => {
                eprintln!("{path:?}: {err}");
                return Err(2);
            }
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
        Ok(()) => {
            println!("ok");
            0
        }
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
) -> Result<(), u8> {
    let entries = match fs::read_dir(vectors) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("{err}");
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
    }
    Ok(())
}

fn replay_cmd(path: &PathBuf, policy: &str, shadow: bool) -> u8 {
    let policy = match load_policy(policy) {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: ReplayRow = match serde_json::from_str(line) {
            Ok(row) => row,
            Err(err) => {
                eprintln!("line {}: {err}", line_no + 1);
                return 1;
            }
        };
        let backend = scripted(&row.harm, row.confidence, &row.nouls);
        let mut client = Client::new(backend).policy(policy.clone());
        if shadow || env_shadow() {
            client = client.shadow(true);
        }
        let verdict = match pollster::block_on(client.gate(GateRequest {
            action_id: ActionId::new(&row.action_id),
            prepared: PreparedCall {
                name: row.action_id.clone(),
                args: serde_json::json!({"command": row.command}),
            },
            state: State {
                trusted: serde_json::json!({"user_request": "invoice"}),
                untrusted: Value::Null,
            },
            extra_questions: Vec::new(),
        })) {
            Ok(verdict) => verdict,
            Err(err) => {
                eprintln!("{}: {err}", row.id);
                return 1;
            }
        };
        let got = verdict_name(&verdict);
        if !got.eq_ignore_ascii_case(&row.expected) {
            eprintln!("{}: expected {} got {got}", row.id, row.expected);
            return 1;
        }
    }
    println!("ok");
    0
}

fn scripted(harm: &str, confidence: f64, nouls: &serde_json::Map<String, Value>) -> FakeBackend {
    let mut backend = FakeBackend::new().on_choice("harm_class", harm, confidence);
    for id in NOULS {
        let value = nouls.get(id).and_then(Value::as_f64).unwrap_or(0.0);
        backend = backend.on_noul(id, value);
    }
    backend
}

fn load_policy(spec: &str) -> Result<Policy, Error> {
    if spec.ends_with(".toml") {
        let text = fs::read_to_string(spec)?;
        Ok(Policy::from_toml_str(&text)?)
    } else {
        Ok(Policy::shipped(spec)?)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<T, Error> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|err| Error::Wire(WireError::Json(err.to_string())))
}

fn env_shadow() -> bool {
    matches!(
        std::env::var("SNAPIF_SHADOW").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True")
    )
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
        Error::Decode(_) | Error::Wire(_) => 2,
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
    #[serde(default = "default_harm")]
    harm: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
    #[serde(default)]
    nouls: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ReplayRow {
    id: String,
    action_id: String,
    harm: String,
    confidence: f64,
    #[serde(default)]
    nouls: serde_json::Map<String, Value>,
    #[serde(default)]
    command: String,
    expected: String,
}

fn default_name() -> String {
    "call".to_string()
}

fn default_harm() -> String {
    "read".to_string()
}

fn default_confidence() -> f64 {
    0.91
}
