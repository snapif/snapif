#![cfg(feature = "cli")]

use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_snapif"))
}

fn manifest(path: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path)
}

#[test]
fn test_empty_directory_exits_1() {
    let dir = std::env::temp_dir().join(format!("snapif-empty-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("no conformance vectors"));
}

#[test]
fn test_leading_bom_is_still_json() {
    let dir = std::env::temp_dir().join(format!("snapif-bom-test-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::write(
        dir.join("boolean_reject.json"),
        "\u{feff}{\"expect\":\"reject\",\"state\":\"x\",\"model\":\"jev-latest\",\"questions\":{\"flag\":{\"type\":\"boolean\",\"instructions\":\"yes?\"}}}\n",
    )
    .expect("write");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(0), "{err} {out}");
    assert_eq!(out.trim(), "ok", "{out}");
}

#[test]
fn test_invalid_json_names_the_file_without_debug_quotes() {
    let dir = std::env::temp_dir().join(format!("snapif-bad-json-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("bad.json");
    fs::write(&path, "not-json").expect("write");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(2), "{err}");
    let shown = format!("{}: invalid json", path.display());
    assert!(err.contains(&shown), "{err}");
    assert!(!err.contains(&format!("{path:?}")), "{err}");
}

#[test]
fn test_uppercase_json_extension_is_checked() {
    let dir = std::env::temp_dir().join(format!("snapif-upper-json-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("Bad.JSON");
    fs::write(&path, "not-json").expect("write");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(
        err.contains(&format!("{}: invalid json", path.display())),
        "{err}"
    );
    assert!(!err.contains("no conformance vectors"), "{err}");
}

#[test]
fn missing_paths_name_the_file() {
    let missing = std::env::temp_dir().join(format!("snapif-missing-{}", std::process::id()));
    let missing_s = missing.display().to_string();
    let cases: &[(&[&str], &str)] = &[
        (&["gate", "--call"], &missing_s),
        (&["ask", "--state"], &missing_s),
        (&["replay"], &missing_s),
        (&["test", "--vectors"], &missing_s),
    ];
    for (args, path) in cases {
        let output = bin()
            .args(*args)
            .arg(path)
            .env("SNAPIF_BACKEND", "fake")
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?} {err}");
        assert!(err.contains(path), "{args:?} {err}");
        assert!(!err.contains("os error"), "{args:?} {err}");
    }
    let call = std::env::temp_dir().join(format!("snapif-call-ok-{}", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("call");
    let policy = format!("{missing_s}.toml");
    let output = bin()
        .args(["gate", "--policy", &policy, "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains(&policy), "{err}");
    let _ = fs::remove_file(call);
}

#[test]
fn gate_and_ask_directory_name_the_path() {
    let path = std::env::temp_dir().join(format!("snapif-call-dir-{}", std::process::id()));
    fs::create_dir_all(&path).expect("dir");
    for args in [vec!["gate", "--call"], vec!["ask", "--state"]] {
        let output = bin()
            .args(&args)
            .arg(&path)
            .env("SNAPIF_BACKEND", "fake")
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?} {err}");
        assert!(err.contains("must be a file"), "{args:?} {err}");
        assert!(err.contains(&path.display().to_string()), "{args:?} {err}");
        assert!(!err.contains("os error"), "{args:?} {err}");
    }
    let _ = fs::remove_dir(&path);
}

#[test]
fn gate_array_must_be_an_object() {
    let path = std::env::temp_dir().join(format!("snapif-gate-array-{}.json", std::process::id()));
    fs::write(&path, "[]").expect("write");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("call file must be a JSON object"), "{err}");
    assert!(!err.contains("CallFile"), "{err}");
}

#[test]
fn ask_array_is_not_ok() {
    let path = std::env::temp_dir().join(format!("snapif-array-{}.json", std::process::id()));
    fs::write(&path, "[]").expect("write");
    let output = bin()
        .args(["ask", "--state"])
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(err.contains("state must be an object"), "{err}");
}

#[test]
fn ask_invalid_json_exits_2_and_missing_file_exits_1() {
    let path = std::env::temp_dir().join(format!("snapif-ask-badjson-{}.json", std::process::id()));
    fs::write(&path, "not-json").expect("write");
    let output = bin()
        .args(["ask", "--state"])
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(&path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(err.contains("invalid json"), "{err}");
    let missing =
        std::env::temp_dir().join(format!("snapif-ask-missing-{}.json", std::process::id()));
    let gone = bin()
        .args(["ask", "--state"])
        .arg(&missing)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let gone_err = String::from_utf8_lossy(&gone.stderr);
    assert_eq!(gone.status.code(), Some(1), "{gone_err}");
}

#[cfg(feature = "http")]
#[test]
fn origin_userinfo_is_not_a_threshold() {
    let call = std::env::temp_dir().join(format!("snapif-origin-{}.json", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("call");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "compatible")
        .env("SNAPIF_BASE_URL", "http://user:pass@127.0.0.1:9")
        .output()
        .expect("run");
    let _ = fs::remove_file(call);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("origin userinfo"), "{err}");
    assert!(!err.contains("threshold invariant"), "{err}");
}

#[cfg(feature = "http")]
#[test]
fn base_url_missing_and_unparsed_are_different() {
    let call = std::env::temp_dir().join(format!("snapif-base-{}.json", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("call");
    let missing = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "compatible")
        .env_remove("SNAPIF_BASE_URL")
        .output()
        .expect("run");
    let missing_err = String::from_utf8_lossy(&missing.stderr);
    assert_eq!(missing.status.code(), Some(1), "{missing_err}");
    assert!(
        missing_err.contains("SNAPIF_BASE_URL is required"),
        "{missing_err}"
    );
    let bad = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "compatible")
        .env("SNAPIF_BASE_URL", "not a url")
        .output()
        .expect("run");
    let bad_err = String::from_utf8_lossy(&bad.stderr);
    assert_eq!(bad.status.code(), Some(1), "{bad_err}");
    assert!(
        bad_err.contains("SNAPIF_BASE_URL must be a URL"),
        "{bad_err}"
    );
    assert!(!bad_err.contains("is required"), "{bad_err}");
    let _ = fs::remove_file(call);
}

#[cfg(feature = "http")]
#[test]
fn refused_loopback_says_backend() {
    let call = std::env::temp_dir().join(format!("snapif-refused-{}.json", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("call");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "compatible")
        .env("SNAPIF_BASE_URL", "http://127.0.0.1:9")
        .env("SNAPIF_API_KEY", "abc")
        .env("SNAPIF_TIMEOUT_MS", "400")
        .output()
        .expect("run");
    let _ = fs::remove_file(call);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(11), "{err}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "escalate");
    assert!(err.contains("backend:"), "{err}");
    assert!(err.len() > "backend:".len() + 4, "{err}");
}

#[test]
fn replay_blank_file_exits_1() {
    let path = std::env::temp_dir().join(format!("snapif-blank-{}.jsonl", std::process::id()));
    fs::write(&path, "\n\n").expect("write");
    let output = bin().arg("replay").arg(&path).output().expect("run");
    let _ = fs::remove_file(&path);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("no replay rows"));
}

#[test]
fn replay_ignores_a_leading_bom() {
    let path = std::env::temp_dir().join(format!("snapif-replay-bom-{}.jsonl", std::process::id()));
    let row = concat!(
        r#"{"id":"row-a","gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null}},"script":{"harm":"read","confidence":0.91},"expected":"Auto"}"#,
        "\n",
    );
    fs::write(&path, format!("\u{feff}{row}")).expect("write");
    let output = bin()
        .arg("replay")
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_file(&path);
    assert_eq!(output.status.code(), Some(0), "{err} {out}");
    assert!(out.contains("\"got\":\"auto\""), "{out}");
}

#[test]
fn replay_directory_exits_1() {
    let path = std::env::temp_dir().join(format!("snapif-replay-dir-{}", std::process::id()));
    fs::create_dir_all(&path).expect("dir");
    let output = bin().arg("replay").arg(&path).output().expect("run");
    let _ = fs::remove_dir(&path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("replay path must be a file"), "{err}");
    assert!(!err.contains("os error"), "{err}");
}

#[test]
fn replay_gate_call_names_the_row_fields() {
    let path =
        std::env::temp_dir().join(format!("snapif-replay-gate-{}.jsonl", std::process::id()));
    fs::write(
        &path,
        r#"{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null},"script":{"harm":"read","confidence":0.91}}"#,
    )
    .expect("write");
    let output = bin().arg("replay").arg(&path).output().expect("run");
    let _ = fs::remove_file(&path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains("a replay row needs id, gate_request, script, and expected"),
        "{err}"
    );
    let help = bin().args(["replay", "--help"]).output().expect("help");
    let help_out = String::from_utf8_lossy(&help.stdout);
    assert!(help_out.contains("gate_request"), "{help_out}");
}

#[test]
fn replay_matches_fixtures_without_network() {
    let output = bin()
        .arg("replay")
        .arg(manifest("tests/fixtures/actions.jsonl"))
        .env("SNAPIF_CASCADE_BASE_URL", "http://127.0.0.1:9")
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\":\"tag-auto\""));
    assert!(stdout.contains("\"id\":\"delimiter-break\""));
    assert!(stdout.contains("\"got\":\"auto\""));
    assert!(stdout.contains("\"got\":\"escalate\""));
}

#[test]
fn replay_shadow_keeps_the_same_verdicts() {
    let output = bin()
        .arg("replay")
        .arg(manifest("tests/fixtures/actions.jsonl"))
        .arg("--shadow")
        .env("SNAPIF_SHADOW", "1")
        .env("SNAPIF_CASCADE_BASE_URL", "http://127.0.0.1:9")
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn gate_without_backend_exits_1_and_fake_does_not_auto() {
    let dir = std::env::temp_dir().join(format!("snapif-call-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    fs::write(
        &call,
        r#"{"action_id":"tag","harm":"read","confidence":0.91,"args":{"command":"rm -rf /"},"trusted":{"user_request":"list"}}"#,
    )
    .expect("write");
    let unset = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("run");
    assert_eq!(
        unset.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&unset.stderr)
    );
    let unset_err = String::from_utf8_lossy(&unset.stderr);
    assert!(
        unset_err.contains("SNAPIF_BACKEND must be fake, typesafe, or compatible"),
        "{unset_err}"
    );
    assert!(!unset_err.contains("threshold invariant"), "{unset_err}");
    assert!(!String::from_utf8_lossy(&unset.stdout).contains("auto"));

    let fake = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    assert_eq!(
        fake.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&fake.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&fake.stdout).trim(), "escalate");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_fake_script_prints_auto_and_refuses_other_backends() {
    let dir = std::env::temp_dir().join(format!("snapif-script-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"list_files","args":{},"trusted":{"user_request":"list the workspace"},"untrusted":null,"script":{"harm":"read","confidence":0.91}}"#,
    )
    .expect("write");
    let auto = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let auto_out = String::from_utf8_lossy(&auto.stdout);
    let auto_err = String::from_utf8_lossy(&auto.stderr);
    assert_eq!(auto.status.code(), Some(0), "{auto_out}{auto_err}");
    assert_eq!(auto_out.trim(), "auto");

    let refused = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "compatible")
        .env("SNAPIF_BASE_URL", "http://127.0.0.1:9")
        .env_remove("SNAPIF_API_KEY")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let refused_err = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{refused_err}");
    assert!(
        refused_err.contains("script is only used when SNAPIF_BACKEND=fake"),
        "{refused_err}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_call_without_action_id_uses_name() {
    let dir = std::env::temp_dir().join(format!("snapif-name-action-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    fs::write(
        &call,
        r#"{"name":"tag","args":{"path":"notes.md"},"trusted":{"user_request":"tag the notes file"},"untrusted":"hello","script":{"harm":"read","confidence":0.91}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{out}{err}");
    assert_eq!(out.trim(), "auto");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_nested_call_keeps_the_prepared_tool_in_the_log() {
    let dir = std::env::temp_dir().join(format!("snapif-nested-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    let log = dir.join("log.jsonl");
    fs::write(
        &call,
        r#"{"action_id":"tag","prepared":{"name":"tag","args":{"api_key":"secret-value"}},"state":{"trusted":{"user_request":"tag the file"},"untrusted":{"blob":"hidden"}},"script":{"harm":"read","confidence":0.91}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "auto");
    let text = fs::read_to_string(&log).expect("log");
    assert!(text.contains("\"name\":\"tag\""), "{text}");
    assert!(text.contains("tag the file"), "{text}");
    assert!(text.contains("\"len\":"), "{text}");
    assert!(text.contains("redacted"), "{text}");
    assert!(!text.contains("secret-value"), "{text}");
    assert!(!text.contains("hidden"), "{text}");
    assert!(!text.contains("\"name\":\"call\""), "{text}");
    let mixed = dir.join("mixed.json");
    let mixed_log = dir.join("mixed.jsonl");
    fs::write(
        &mixed,
        r#"{"action_id":"tag","name":"flat","args":null,"trusted":null,"untrusted":null,"prepared":{"name":"nested","args":{"keep":true}},"state":{"trusted":{"user_request":"from nested"},"untrusted":{"blob":"hidden"}},"script":{"harm":"read","confidence":0.91}}"#,
    )
    .expect("write");
    let mixed_out = bin()
        .args(["gate", "--call"])
        .arg(&mixed)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &mixed_log)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    assert_eq!(mixed_out.status.code(), Some(0));
    let mixed_text = fs::read_to_string(&mixed_log).expect("log");
    assert!(mixed_text.contains("\"name\":\"flat\""), "{mixed_text}");
    assert!(!mixed_text.contains("nested"), "{mixed_text}");
    assert!(!mixed_text.contains("from nested"), "{mixed_text}");
    assert!(!mixed_text.contains("keep"), "{mixed_text}");
    assert!(!mixed_text.contains("\"len\":"), "{mixed_text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_script_confidence_outside_zero_to_one_exits_1() {
    let dir = std::env::temp_dir().join(format!("snapif-conf-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    for (name, confidence) in [("over", "2"), ("neg", "-1")] {
        let call = dir.join(format!("{name}.json"));
        fs::write(
            &call,
            format!(
                r#"{{"action_id":"tag","name":"list_files","args":{{}},"script":{{"harm":"read","confidence":{confidence}}}}}"#
            ),
        )
        .expect("write");
        let output = bin()
            .args(["gate", "--call"])
            .arg(&call)
            .env("SNAPIF_BACKEND", "fake")
            .env_remove("SNAPIF_POLICY")
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(1), "{name} {out}{err}");
        assert!(
            err.contains("script confidence must be from 0 to 1"),
            "{err}"
        );
        assert!(out.trim().is_empty(), "{out}");
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_script_harm_names_an_unknown_label() {
    let dir = std::env::temp_dir().join(format!("snapif-harm-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    for harm in ["nope", "EXEC", ""] {
        let call = dir.join(format!("bad-{}.json", harm.len()));
        fs::write(
            &call,
            format!(
                r#"{{"action_id":"bash.rm","name":"bash","args":{{"command":"rm -rf /tmp/demo"}},"trusted":{{"user_request":"clean"}},"untrusted":null,"script":{{"harm":"{harm}","confidence":0.95}}}}"#
            ),
        )
        .expect("write");
        let output = bin()
            .args(["gate", "--call"])
            .arg(&call)
            .env("SNAPIF_BACKEND", "fake")
            .env_remove("SNAPIF_POLICY")
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(1), "{harm:?} {out}{err}");
        assert!(
            err.contains("script harm must be none, read, write, exec, network, or money"),
            "{err}"
        );
        assert!(err.contains(&format!("{harm:?}")), "{err}");
        assert!(out.trim().is_empty(), "{out}");
    }
    let money = dir.join("money.json");
    fs::write(
        &money,
        r#"{"action_id":"bash.rm","name":"bash","args":{"command":"rm -rf /tmp/demo"},"trusted":{"user_request":"clean"},"untrusted":null,"script":{"harm":"money","confidence":0.95}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&money)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert_ne!(output.status.code(), Some(1), "{out}{err}");
    assert!(!err.contains("script harm"), "{err}");
    assert!(
        matches!(out.trim(), "auto" | "review" | "escalate"),
        "{out}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bad_policy_exits_one() {
    let output = bin()
        .args([
            "replay",
            "--policy",
            "missing",
            "tests/fixtures/actions.jsonl",
        ])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn vector_missing_state_names_the_fields() {
    let dir = std::env::temp_dir().join(format!("snapif-vec-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::write(dir.join("one.json"), r#"{"questions":{}}"#).expect("write");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(
        err.contains("a conformance vector needs state and questions"),
        "{err}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn vectors_pass_locally_and_base_url_does_not_connect() {
    let output = bin()
        .args(["test", "--vectors"])
        .arg(manifest("tests/conformance"))
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let blocked = bin()
        .args(["test", "--vectors"])
        .arg(manifest("tests/conformance"))
        .args(["--base-url", "http://127.0.0.1:9"])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    #[cfg(not(feature = "http"))]
    assert_eq!(blocked.status.code(), Some(1), "{stderr}");
    #[cfg(feature = "http")]
    assert_eq!(blocked.status.code(), Some(3), "{stderr}");
}

#[cfg(feature = "http")]
#[test]
fn base_url_that_is_not_a_url_exits_1() {
    let output = bin()
        .args(["test", "--vectors"])
        .arg(manifest("tests/conformance"))
        .args(["--base-url", "not a url"])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("--base-url"), "{stderr}");
}

#[cfg(feature = "http")]
#[test]
fn base_url_posts_a_vector_and_checks_the_response() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&buf[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                if name.eq_ignore_ascii_case("content-length") {
                                    value.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let body = r#"{"model":"loop","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":1.0},"confidence":0.91}},"usage":{"input_tokens":1,"output_tokens":2}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let dir = std::env::temp_dir().join(format!("snapif-remote-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("department_choice.json"),
    )
    .expect("copy");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .args(["--base-url", &format!("http://127.0.0.1:{port}")])
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
}

#[cfg(feature = "http")]
#[test]
fn base_url_gives_each_vector_its_own_deadline() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&buf[..end]);
                            let length = header
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    if name.eq_ignore_ascii_case("content-length") {
                                        value.trim().parse::<usize>().ok()
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            thread::sleep(Duration::from_secs(3));
            let body = r#"{"model":"loop","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":1.0},"confidence":0.91}},"usage":{"input_tokens":1,"output_tokens":2}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let dir = std::env::temp_dir().join(format!("snapif-remote-budget-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("one.json"),
    )
    .expect("copy");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("two.json"),
    )
    .expect("copy");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .args(["--base-url", &format!("http://127.0.0.1:{port}")])
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
}

#[cfg(feature = "http")]
#[test]
fn base_url_one_slow_vector_still_exits_3() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        thread::sleep(Duration::from_secs(6));
        let body = "{}";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let dir = std::env::temp_dir().join(format!("snapif-remote-slow-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("one.json"),
    )
    .expect("copy");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .args(["--base-url", &format!("http://127.0.0.1:{port}")])
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("deadline"), "{stderr}");
}

#[cfg(feature = "http")]
fn http_reply(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[cfg(feature = "http")]
fn read_http_request(stream: &mut std::net::TcpStream) {
    use std::io::Read;
    use std::time::Duration;

    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&buf[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(feature = "http")]
fn serve_status(replies: Vec<Vec<u8>>) -> (u16, std::sync::mpsc::Receiver<usize>) {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut posts = 0usize;
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            read_http_request(&mut stream);
            let _ = stream.write_all(&reply);
            posts += 1;
        }
        let _ = tx.send(posts);
    });
    (port, rx)
}

#[cfg(feature = "http")]
fn remote_exit(replies: Vec<Vec<u8>>) -> (std::process::Output, usize) {
    use std::time::Duration;

    let (port, posts) = serve_status(replies);
    let dir = std::env::temp_dir().join(format!("snapif-exit-{}-{port}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("department_choice.json"),
    )
    .expect("copy");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .args(["--base-url", &format!("http://127.0.0.1:{port}")])
        .output()
        .expect("run");
    let count = posts
        .recv_timeout(Duration::from_secs(2))
        .expect("server finished");
    let _ = fs::remove_dir_all(&dir);
    (output, count)
}

#[cfg(feature = "http")]
#[test]
fn base_url_sends_snapif_api_key() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buf);
        let body = r#"{"model":"loop","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":1.0},"confidence":0.91}},"usage":{"input_tokens":1,"output_tokens":2}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    let dir = std::env::temp_dir().join(format!("snapif-key-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(
        manifest("tests/conformance/department_choice.json"),
        dir.join("department_choice.json"),
    )
    .expect("copy");
    let output = bin()
        .args(["test", "--vectors"])
        .arg(&dir)
        .args(["--base-url", &format!("http://127.0.0.1:{port}")])
        .env("SNAPIF_API_KEY", "abc")
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv_timeout(Duration::from_secs(2)).expect("request");
    let header = String::from_utf8_lossy(&request);
    assert!(
        header
            .to_ascii_lowercase()
            .contains("authorization: bearer abc"),
        "{header}"
    );
}

#[cfg(feature = "http")]
#[test]
fn ask_rejected_exits_2() {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let mut buf = [0u8; 2048];
        let _ = std::io::Read::read(&mut stream, &mut buf);
        let body = "no";
        let response = format!(
            "HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    let dir = std::env::temp_dir().join(format!("snapif-ask-reject-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let state = dir.join("state.json");
    fs::write(
        &state,
        r#"{"trusted":{"user_request":"list"},"untrusted":null,"questions":{"department":{"type":"choice","instructions":"Which team","criteria":{"billing":"pay","technical":"bugs"}}}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["ask", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "compatible")
        .env("SNAPIF_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env("SNAPIF_API_KEY", "abc")
        .env_remove("SNAPIF_CASCADE_BASE_URL")
        .output()
        .expect("run");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "http")]
#[test]
fn base_url_401_exits_5() {
    let (output, posts) = remote_exit(vec![http_reply("401 Unauthorized", "", "")]);
    assert_eq!(posts, 1);
    assert_eq!(
        output.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "http")]
#[test]
fn base_url_four_429s_exit_4() {
    let (output, posts) = remote_exit(vec![
        http_reply(
            "429 Too Many Requests",
            "Retry-After: 0\r\n",
            ""
        );
        4
    ]);
    assert_eq!(posts, 4);
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "http")]
#[test]
fn base_url_422_exits_2() {
    let (output, posts) = remote_exit(vec![http_reply("422 Unprocessable Entity", "", "too big")]);
    assert_eq!(posts, 1);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "http")]
#[test]
fn base_url_decoded_mismatch_exits_2() {
    let wrong_choice = r#"{"model":"loop","answers":{"department":{"type":"choice","choice":"other","probabilities":{"other":1.0},"confidence":0.91}},"usage":{"input_tokens":1,"output_tokens":2}}"#;
    let missing_answer =
        r#"{"model":"loop","answers":{},"usage":{"input_tokens":0,"output_tokens":0}}"#;
    for (label, body) in [
        ("wrong choice", wrong_choice),
        ("missing answer", missing_answer),
    ] {
        let (output, posts) = remote_exit(vec![http_reply("200 OK", "", body)]);
        assert_eq!(posts, 1, "{label}");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn ask_empty_questions_exits_zero() {
    let dir = std::env::temp_dir().join(format!("snapif-ask-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let state = dir.join("state.json");
    fs::write(
        &state,
        r#"{"trusted":{"user_request":"list"},"untrusted":null}"#,
    )
    .expect("write");
    let output = bin()
        .args(["ask", "--policy", "tool-gate", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ask_unknown_question_exits_2_and_unset_backend_exits_1() {
    let dir = std::env::temp_dir().join(format!("snapif-ask-bad-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let state = dir.join("state.json");
    fs::write(
        &state,
        r#"{"trusted":{"user_request":"list"},"untrusted":null,"questions":{"flag":{"type":"boolean","instructions":"yes?"}}}"#,
    )
    .expect("write");
    let decoded = bin()
        .args(["ask", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let decoded_err = String::from_utf8_lossy(&decoded.stderr);
    assert_eq!(decoded.status.code(), Some(2), "{decoded_err}");
    assert!(
        decoded_err.contains("unknown question/answer type boolean")
            && decoded_err.contains("expected choice, score, or noul"),
        "{decoded_err}"
    );
    let unset = bin()
        .args(["ask", "--state"])
        .arg(&state)
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("run");
    assert_eq!(
        unset.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&unset.stderr)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ask_missing_type_names_it() {
    let path = std::env::temp_dir().join(format!("snapif-no-type-{}.json", std::process::id()));
    fs::write(
        &path,
        r#"{"trusted":{},"untrusted":null,"questions":{"q":{"prompt":"ship?"}}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["ask", "--state"])
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(&path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(
        err.contains("unknown question/answer type missing"),
        "{err}"
    );
    assert!(err.contains("expected choice, score, or noul"), "{err}");
}

#[test]
fn gate_strips_a_utf8_bom() {
    let path = std::env::temp_dir().join(format!("snapif-bom-{}.json", std::process::id()));
    fs::write(
        &path,
        "\u{feff}{\"action_id\":\"tag\",\"name\":\"tag\",\"args\":{},\"trusted\":{},\"untrusted\":null}\n",
    )
    .expect("write");
    let output = bin()
        .args(["gate", "--call"])
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(&path);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(11), "{err}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "escalate");
}

#[test]
fn replay_prints_every_row_and_continues_after_a_mismatch() {
    let path = std::env::temp_dir().join(format!("snapif-replay-{}.jsonl", std::process::id()));
    fs::write(
        &path,
        concat!(
            r#"{"id":"wrong","gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null}},"script":{"harm":"read","confidence":0.91},"expected":"Escalate"}"#,
            "\n",
            r#"{"id":"later","gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null}},"script":{"harm":"read","confidence":0.91},"expected":"Auto"}"#,
            "\n",
        ),
    )
    .expect("write");
    let output = bin().arg("replay").arg(&path).output().expect("run");
    let _ = fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("\"id\":\"wrong\""));
    assert!(stdout.contains("\"id\":\"later\""));
    assert!(stdout.contains("\"got\":\"auto\""));
}

#[test]
fn ask_decisions_flag_prints_json_and_screen_loads_the_pack() {
    let dir = std::env::temp_dir().join(format!("snapif-dec-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let state = dir.join("state.json");
    fs::write(&state, r#"{"trusted":{"text":"hello"},"untrusted":null}"#).expect("write");
    let plain = bin()
        .args(["ask", "--decisions", "--policy", "tool-gate", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    assert_eq!(plain.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(stdout.contains("\"decisions\""), "{stdout}");
    let screen = bin()
        .args(["ask", "--policy", "screen", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&screen.stderr);
    assert_eq!(screen.status.code(), Some(2), "{err}");
    assert!(err.contains("missing answer sensitive"), "{err}");
    let from_env = bin()
        .args(["ask", "--state"])
        .arg(&state)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_POLICY", "screen")
        .output()
        .expect("run");
    let env_err = String::from_utf8_lossy(&from_env.stderr);
    assert_eq!(from_env.status.code(), Some(2), "{env_err}");
    assert!(env_err.contains("missing answer sensitive"), "{env_err}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn explain_reads_an_uppercase_toml_suffix() {
    let dir = std::env::temp_dir().join(format!("snapif-toml-case-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("Policy.TOML");
    fs::write(
        &path,
        r#"
schema_version = 1
fail = "closed"
cascade_min = 0.99
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 0.99
signal = "top_prob"
[default_action]
auto = 0.99
review = 0.8
when_unsure = "review_guess"
class = "read"
"#,
    )
    .expect("write");
    let output = bin()
        .args(["explain", "--action", "tag", "--policy"])
        .arg(&path)
        .env_remove("SNAPIF_BACKEND")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{err} {stdout}");
    assert!(stdout.contains("source default_action"), "{stdout}");
    let bak = dir.join("Policy.toml.bak");
    fs::write(&bak, fs::read_to_string(&path).expect("read")).expect("write");
    let skipped = bin()
        .args(["explain", "--action", "tag", "--policy"])
        .arg(&bak)
        .env_remove("SNAPIF_BACKEND")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let skipped_err = String::from_utf8_lossy(&skipped.stderr);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(skipped.status.code(), Some(1), "{skipped_err}");
    assert!(skipped_err.contains("unknown policy"), "{skipped_err}");
}

#[test]
fn explain_git_push_has_no_auto_and_names_a_missing_policy() {
    let output = bin()
        .args(["explain", "--action", "git.push"])
        .env_remove("SNAPIF_BACKEND")
        .env_remove("SNAPIF_MODEL")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("auto none"), "{stdout}");
    assert!(stdout.contains("pack tool-gate"), "{stdout}");
    assert!(stdout.contains("pack_version 1"), "{stdout}");
    assert!(stdout.contains("model jev-latest"), "{stdout}");
    let blank_model = bin()
        .args(["explain", "--action", "git.push"])
        .env("SNAPIF_MODEL", " ")
        .output()
        .expect("run");
    let blank_model_err = String::from_utf8_lossy(&blank_model.stderr);
    assert_eq!(blank_model.status.code(), Some(1), "{blank_model_err}");
    assert!(
        blank_model_err.contains("SNAPIF_MODEL must not be blank"),
        "{blank_model_err}"
    );
    assert!(stdout.contains("exfil:yes"), "{stdout}");
    assert!(stdout.contains("class network"), "{stdout}");
    assert!(stdout.contains("when_unsure escalate"), "{stdout}");
    assert!(!stdout.contains("source default_action"), "{stdout}");
    let unknown = bin()
        .args(["explain", "--action", "no-such-action"])
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("run");
    let unknown_out = String::from_utf8_lossy(&unknown.stdout);
    assert_eq!(unknown.status.code(), Some(0), "{unknown_out}");
    assert!(
        unknown_out.contains("source default_action"),
        "{unknown_out}"
    );
    assert!(
        unknown_out.contains("when_unsure review_guess"),
        "{unknown_out}"
    );
    assert!(unknown_out.contains("class read"), "{unknown_out}");
    assert!(!unknown_out.contains("ReviewGuess"), "{unknown_out}");
    let missing =
        std::env::temp_dir().join(format!("snapif-missing-policy-{}.toml", std::process::id()));
    let bad = bin()
        .args(["explain", "--action", "git.push", "--policy"])
        .arg(&missing)
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&bad.stderr);
    assert_eq!(bad.status.code(), Some(1), "{err}");
    assert!(err.contains(&missing.display().to_string()), "{err}");
    let blank = bin()
        .args(["explain", "--action", ""])
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("run");
    let blank_err = String::from_utf8_lossy(&blank.stderr);
    assert_eq!(blank.status.code(), Some(1), "{blank_err}");
    assert!(
        blank_err.contains("action id must not be blank"),
        "{blank_err}"
    );
}

#[test]
fn log_failure_names_the_path_and_cache_rejects_words() {
    let call = std::env::temp_dir().join(format!("snapif-logfail-{}.json", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("write");
    let missing = std::env::temp_dir().join(format!(
        "snapif-missing-log-dir-{}/log.jsonl",
        std::process::id()
    ));
    let logged = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &missing)
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&logged.stderr);
    assert_eq!(logged.status.code(), Some(11), "{err}");
    assert!(err.contains(&missing.display().to_string()), "{err}");
    let cache = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_CACHE", "lots")
        .output()
        .expect("run");
    let cache_err = String::from_utf8_lossy(&cache.stderr);
    assert_eq!(cache.status.code(), Some(1), "{cache_err}");
    assert!(cache_err.contains("must be an integer"), "{cache_err}");
    let help = bin().args(["gate", "--help"]).output().expect("help");
    let help_out = String::from_utf8_lossy(&help.stdout);
    assert!(help_out.contains("SNAPIF_LOG"), "{help_out}");
    assert!(help_out.contains("SNAPIF_CACHE"), "{help_out}");
    assert!(help_out.contains("does not reuse it"), "{help_out}");
    assert!(
        help_out.contains("optional `script` sets harm and confidence"),
        "{help_out}"
    );
    let timeout = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_TIMEOUT_MS", "nope")
        .output()
        .expect("run");
    let timeout_err = String::from_utf8_lossy(&timeout.stderr);
    assert_eq!(timeout.status.code(), Some(1), "{timeout_err}");
    assert!(
        timeout_err.contains("SNAPIF_TIMEOUT_MS must be an integer, got nope"),
        "{timeout_err}"
    );
    let model = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_MODEL", " ")
        .output()
        .expect("run");
    let model_err = String::from_utf8_lossy(&model.stderr);
    assert_eq!(model.status.code(), Some(1), "{model_err}");
    assert!(
        model_err.contains("SNAPIF_MODEL must not be blank"),
        "{model_err}"
    );
    let _ = fs::remove_file(&call);
}

fn hook_output(body: &[u8], shadow: bool) -> std::process::Output {
    let mut command = bin();
    command
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if shadow {
        command.arg("--shadow");
    }
    let mut child = command.spawn().expect("spawn");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(body).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn hook_denies_an_unscripted_call_and_rejects_bad_json() {
    let denied = hook_output(
        br#"{"tool_name":"bash","tool_input":{"command":"ls"}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&denied.stdout);
    assert_eq!(denied.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"deny\""),
        "{stdout}"
    );
    let shadow = hook_output(
        br#"{"tool_name":"bash","tool_input":{"command":"ls"}}"#,
        true,
    );
    let shadow_out = String::from_utf8_lossy(&shadow.stdout);
    assert_eq!(shadow.status.code(), Some(0), "{shadow_out}");
    assert!(
        shadow_out.contains("\"permissionDecision\":\"allow\""),
        "{shadow_out}"
    );
    let bad = hook_output(b"not-json", false);
    let bad_out = String::from_utf8_lossy(&bad.stdout);
    assert_eq!(bad.status.code(), Some(0), "{bad_out}");
    assert!(
        bad_out.contains("\"permissionDecision\":\"deny\""),
        "{bad_out}"
    );
    assert!(bad_out.contains("invalid json"), "{bad_out}");
    let array = hook_output(b"[1,2]", false);
    let array_out = String::from_utf8_lossy(&array.stdout);
    assert_eq!(array.status.code(), Some(0), "{array_out}");
    assert!(
        array_out.contains("\"permissionDecision\":\"deny\""),
        "{array_out}"
    );
    assert!(array_out.contains("invalid json"), "{array_out}");
}

#[test]
fn hook_denies_an_unknown_script_harm() {
    let denied = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"nope","confidence":0.95}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&denied.stdout);
    assert_eq!(denied.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"deny\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("script harm must be none, read, write, exec, network, or money"),
        "{stdout}"
    );
    let harm = "nope";
    let in_json = format!("{harm:?}").replace('"', "\\\"");
    assert!(stdout.contains(&in_json), "{stdout}");
    assert!(
        !stdout.contains("\"permissionDecision\":\"allow\""),
        "{stdout}"
    );
}

#[test]
fn hook_denies_a_missing_tool_name_instead_of_the_default_action() {
    let body =
        br#"{"tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"read","confidence":0.99}}"#;
    for shadow in [false, true] {
        let denied = hook_output(body, shadow);
        let stdout = String::from_utf8_lossy(&denied.stdout);
        assert_eq!(denied.status.code(), Some(0), "{shadow} {stdout}");
        assert!(
            stdout.contains("\"permissionDecision\":\"deny\""),
            "{shadow} {stdout}"
        );
        assert!(
            stdout.contains("tool_name is required"),
            "{shadow} {stdout}"
        );
        assert!(
            !stdout.contains("\"permissionDecision\":\"allow\""),
            "{shadow} {stdout}"
        );
    }
    let blank = hook_output(
        br#"{"tool_name":"  ","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"read","confidence":0.99}}"#,
        false,
    );
    let blank_out = String::from_utf8_lossy(&blank.stdout);
    assert!(
        blank_out.contains("\"permissionDecision\":\"deny\""),
        "{blank_out}"
    );
    assert!(blank_out.contains("tool_name is required"), "{blank_out}");
}

fn hook_env(body: &[u8], shadow: Option<&str>) -> std::process::Output {
    let mut command = bin();
    command
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match shadow {
        Some(value) => {
            command.env("SNAPIF_SHADOW", value);
        }
        None => {
            command.env_remove("SNAPIF_SHADOW");
        }
    }
    let mut child = command.spawn().expect("spawn");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(body).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn hook_shadow_env_allows_like_the_flag() {
    let plain = br#"{"tool_name":"bash","tool_input":{"command":"ls"}}"#;
    let on = hook_env(plain, Some("1"));
    let out = String::from_utf8_lossy(&on.stdout);
    let err = String::from_utf8_lossy(&on.stderr);
    assert_eq!(on.status.code(), Some(0), "{out}{err}");
    assert!(out.contains("\"permissionDecision\":\"allow\""), "{out}");
    assert!(
        out.contains("\"permissionDecisionReason\":\"escalate\""),
        "{out}"
    );
    assert_eq!(err.trim(), "escalate");
    let word = hook_env(plain, Some("true"));
    let word_out = String::from_utf8_lossy(&word.stdout);
    assert!(
        word_out.contains("\"permissionDecision\":\"allow\""),
        "{word_out}"
    );
    let off = hook_env(plain, Some("yes"));
    let off_out = String::from_utf8_lossy(&off.stdout);
    assert!(
        off_out.contains("\"permissionDecision\":\"deny\""),
        "{off_out}"
    );
    let asked = br#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"exec","confidence":0.95}}"#;
    let review = hook_env(asked, Some("1"));
    let review_out = String::from_utf8_lossy(&review.stdout);
    let review_err = String::from_utf8_lossy(&review.stderr);
    assert_eq!(review.status.code(), Some(0), "{review_out}{review_err}");
    assert!(
        review_out.contains("\"permissionDecision\":\"allow\""),
        "{review_out}"
    );
    assert!(
        review_out.contains("\"permissionDecisionReason\":\"review\""),
        "{review_out}"
    );
    assert!(
        !review_out.contains("\"permissionDecision\":\"ask\""),
        "{review_out}"
    );
    assert_eq!(review_err.trim(), "review");
}

#[test]
fn hook_asks_on_review_and_explain_names_the_bash_rule() {
    let asked = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"exec","confidence":0.95}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&asked.stdout);
    assert_eq!(asked.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    assert!(stdout.contains("review:"), "{stdout}");
    let listed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#,
        false,
    );
    let listed_out = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed_out.contains("\"permissionDecision\":\"deny\""),
        "{listed_out}"
    );
    assert!(
        !listed_out.contains("\"permissionDecision\":\"ask\""),
        "{listed_out}"
    );
    let matched = bin()
        .args(["explain", "--action", "Bash", "--command", "rm -rf /tmp"])
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let matched_out = String::from_utf8_lossy(&matched.stdout);
    assert_eq!(matched.status.code(), Some(0), "{matched_out}");
    assert!(matched_out.contains("matched bash.rm"), "{matched_out}");
    assert!(
        !matched_out.contains("source default_action"),
        "{matched_out}"
    );
    let fallback = bin()
        .args(["explain", "--action", "Bash", "--command", "ls"])
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let fallback_out = String::from_utf8_lossy(&fallback.stdout);
    assert_eq!(fallback.status.code(), Some(0), "{fallback_out}");
    assert!(
        fallback_out.contains("source default_action"),
        "{fallback_out}"
    );
    assert!(!fallback_out.contains("matched "), "{fallback_out}");
}

#[test]
fn hook_asks_when_rm_is_scored_as_a_read() {
    let under = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&under.stdout);
    assert_eq!(under.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    assert!(
        !stdout.contains("\"permissionDecision\":\"allow\""),
        "{stdout}"
    );
    let same = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /tmp"},"script":{"harm":"exec","confidence":1.0}}"#,
        false,
    );
    let same_out = String::from_utf8_lossy(&same.stdout);
    assert!(
        same_out.contains("\"permissionDecision\":\"allow\""),
        "{same_out}"
    );
}

#[test]
fn hook_does_not_auto_a_windows_image_suffix() {
    for (harm, command) in [
        ("none", "git.exe push"),
        ("read", "git.exe push"),
        ("none", "rm.exe -rf /tmp/x"),
        ("read", "rm.exe -rf /tmp/x"),
        ("read", "timeout.exe 1 rm.exe -rf /tmp/x"),
        ("read", "GIT.EXE push origin"),
    ] {
        let body = format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":"{command}"}},"script":{{"harm":"{harm}","confidence":1.0}}}}"#
        );
        let out = hook_output(body.as_bytes(), false);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{harm} {command} {stdout}");
        assert!(
            stdout.contains("\"permissionDecision\":\"ask\""),
            "{harm} {command} {stdout}"
        );
        assert!(
            !stdout.contains("\"permissionDecision\":\"allow\""),
            "{harm} {command} {stdout}"
        );
    }
    let shadowed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"git.exe push"},"script":{"harm":"read","confidence":1.0}}"#,
        true,
    );
    let shadowed_out = String::from_utf8_lossy(&shadowed.stdout);
    assert!(
        shadowed_out.contains("\"permissionDecision\":\"allow\""),
        "{shadowed_out}"
    );
    assert!(
        shadowed_out.contains("\"permissionDecisionReason\":\"review\""),
        "{shadowed_out}"
    );
    assert!(
        !shadowed_out.contains("\"permissionDecisionReason\":\"auto\""),
        "{shadowed_out}"
    );
    let words = hook_output(
        br#"{"tool_name":"Bash","tool_input":["rm.exe","-rf","/tmp"],"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let words_out = String::from_utf8_lossy(&words.stdout);
    assert!(
        words_out.contains("\"permissionDecision\":\"ask\""),
        "{words_out}"
    );
    let unmatched = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"rm.exe.bak -rf /tmp"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let unmatched_out = String::from_utf8_lossy(&unmatched.stdout);
    assert!(
        unmatched_out.contains("\"permissionDecision\":\"allow\""),
        "{unmatched_out}"
    );
    let explained = bin()
        .args(["explain", "--action", "Bash", "--command", "git.exe push"])
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let explained_out = String::from_utf8_lossy(&explained.stdout);
    assert_eq!(explained.status.code(), Some(0), "{explained_out}");
    assert!(
        explained_out.contains("matched git.push"),
        "{explained_out}"
    );
    assert!(
        !explained_out.contains("source default_action"),
        "{explained_out}"
    );
}

#[test]
fn hook_asks_when_tool_input_is_an_argv_array() {
    let hidden = hook_output(
        br#"{"tool_name":"Bash","tool_input":["rm","-rf","/tmp"],"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&hidden.stdout);
    assert_eq!(hidden.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    assert!(
        !stdout.contains("\"permissionDecision\":\"allow\""),
        "{stdout}"
    );
    let echoed = hook_output(
        br#"{"tool_name":"Bash","tool_input":["echo","rm"],"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_out = String::from_utf8_lossy(&echoed.stdout);
    assert!(
        echoed_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_out}"
    );
    for shadow in [false, true] {
        let denied = hook_output(
            br#"{"tool_name":"Bash","tool_input":["rm",1],"script":{"harm":"read","confidence":1.0}}"#,
            shadow,
        );
        let denied_out = String::from_utf8_lossy(&denied.stdout);
        assert_eq!(denied.status.code(), Some(0), "{shadow} {denied_out}");
        assert!(
            denied_out.contains("\"permissionDecision\":\"deny\""),
            "{shadow} {denied_out}"
        );
        assert!(
            denied_out.contains("command must be a string"),
            "{shadow} {denied_out}"
        );
        assert!(
            !denied_out.contains("\"permissionDecision\":\"allow\""),
            "{shadow} {denied_out}"
        );
    }
}

#[test]
fn hook_denies_a_tool_input_that_is_not_an_object() {
    for (body, shadow) in [
        (
            &br#"{"tool_name":"Bash","tool_input":1,"script":{"harm":"read","confidence":1.0}}"#[..],
            false,
        ),
        (
            &br#"{"tool_name":"Bash","tool_input":true,"script":{"harm":"read","confidence":1.0}}"#
                [..],
            true,
        ),
    ] {
        let denied = hook_output(body, shadow);
        let denied_out = String::from_utf8_lossy(&denied.stdout);
        assert_eq!(denied.status.code(), Some(0), "{shadow} {denied_out}");
        assert!(
            denied_out.contains("\"permissionDecision\":\"deny\""),
            "{shadow} {denied_out}"
        );
        assert!(
            denied_out.contains("command must be a string"),
            "{shadow} {denied_out}"
        );
        assert!(
            !denied_out.contains("\"permissionDecision\":\"allow\""),
            "{shadow} {denied_out}"
        );
    }
    let empty = hook_output(
        br#"{"tool_name":"Bash","tool_input":{},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let empty_out = String::from_utf8_lossy(&empty.stdout);
    assert!(
        empty_out.contains("\"permissionDecision\":\"allow\""),
        "{empty_out}"
    );
}

#[test]
fn hook_asks_when_rm_is_an_argv_array() {
    let hidden = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":["rm","-rf","/tmp"]},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&hidden.stdout);
    assert_eq!(hidden.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    assert!(
        !stdout.contains("\"permissionDecision\":\"allow\""),
        "{stdout}"
    );
    let echoed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":["echo","rm"]},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_out = String::from_utf8_lossy(&echoed.stdout);
    assert!(
        echoed_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_out}"
    );
    for shadow in [false, true] {
        let denied = hook_output(
            br#"{"tool_name":"Bash","tool_input":{"command":1},"script":{"harm":"read","confidence":1.0}}"#,
            shadow,
        );
        let denied_out = String::from_utf8_lossy(&denied.stdout);
        assert_eq!(denied.status.code(), Some(0), "{shadow} {denied_out}");
        assert!(
            denied_out.contains("\"permissionDecision\":\"deny\""),
            "{shadow} {denied_out}"
        );
        assert!(
            denied_out.contains("command must be a string"),
            "{shadow} {denied_out}"
        );
        assert!(
            !denied_out.contains("\"permissionDecision\":\"allow\""),
            "{shadow} {denied_out}"
        );
    }
}

#[test]
fn hook_asks_when_an_argv_word_contains_a_space() {
    let asks = [
        br#"{"tool_name":"Bash","tool_input":["C:/Program Files/Git/cmd/git.exe","push"],"script":{"harm":"read","confidence":1.0}}"#.as_slice(),
        br#"{"tool_name":"Bash","tool_input":{"command":["C:\\Program Files\\Git\\cmd\\git.exe","push"]},"script":{"harm":"read","confidence":1.0}}"#.as_slice(),
        br#"{"tool_name":"Bash","tool_input":["C:/Program Files/rm.exe","-rf","/tmp/x"],"script":{"harm":"read","confidence":1.0}}"#.as_slice(),
        br#"{"tool_name":"Bash","tool_input":{"command":["bash","-c","rm -rf /tmp"]},"script":{"harm":"read","confidence":1.0}}"#.as_slice(),
        br#"{"tool_name":"Bash","tool_input":["rm","it's"],"script":{"harm":"read","confidence":1.0}}"#.as_slice(),
    ];
    for body in asks {
        let hidden = hook_output(body, false);
        let stdout = String::from_utf8_lossy(&hidden.stdout);
        assert_eq!(hidden.status.code(), Some(0), "{stdout}");
        assert!(
            stdout.contains("\"permissionDecision\":\"ask\""),
            "{stdout}"
        );
        assert!(
            !stdout.contains("\"permissionDecision\":\"allow\""),
            "{stdout}"
        );
    }
    let echoed = hook_output(
        br#"{"tool_name":"Bash","tool_input":["echo","rm -rf /tmp"],"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_out = String::from_utf8_lossy(&echoed.stdout);
    assert!(
        echoed_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_out}"
    );
}

#[test]
fn hook_asks_when_ansi_c_quotes_hide_rm() {
    let hidden = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"$'rm' --version"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&hidden.stdout);
    assert_eq!(hidden.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    assert!(
        !stdout.contains("\"permissionDecision\":\"allow\""),
        "{stdout}"
    );
    let echoed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"echo $'rm'"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_out = String::from_utf8_lossy(&echoed.stdout);
    assert!(
        echoed_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_out}"
    );
}

#[test]
fn hook_asks_when_exec_or_eval_hides_rm() {
    let hidden = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"exec rm --version"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let stdout = String::from_utf8_lossy(&hidden.stdout);
    assert_eq!(hidden.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"ask\""),
        "{stdout}"
    );
    let evaluated = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"eval 'rm --version'"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let evaluated_out = String::from_utf8_lossy(&evaluated.stdout);
    assert!(
        evaluated_out.contains("\"permissionDecision\":\"ask\""),
        "{evaluated_out}"
    );
    let renamed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"exec -a rm echo hello"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let renamed_out = String::from_utf8_lossy(&renamed.stdout);
    assert!(
        renamed_out.contains("\"permissionDecision\":\"allow\""),
        "{renamed_out}"
    );
}

#[test]
fn hook_asks_when_timeout_or_xargs_hides_rm() {
    let timed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"timeout 1 rm --version"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let timed_out = String::from_utf8_lossy(&timed.stdout);
    assert_eq!(timed.status.code(), Some(0), "{timed_out}");
    assert!(
        timed_out.contains("\"permissionDecision\":\"ask\""),
        "{timed_out}"
    );
    let gathered = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"xargs rm --version"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let gathered_out = String::from_utf8_lossy(&gathered.stdout);
    assert!(
        gathered_out.contains("\"permissionDecision\":\"ask\""),
        "{gathered_out}"
    );
    let echoed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"timeout 1 echo rm"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_out = String::from_utf8_lossy(&echoed.stdout);
    assert!(
        echoed_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_out}"
    );
    let echoed_args = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"xargs echo rm"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_args_out = String::from_utf8_lossy(&echoed_args.stdout);
    assert!(
        echoed_args_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_args_out}"
    );
    let prefixed = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"gtimeout 1 grm --version"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let prefixed_out = String::from_utf8_lossy(&prefixed.stdout);
    assert!(
        prefixed_out.contains("\"permissionDecision\":\"ask\""),
        "{prefixed_out}"
    );
    let echoed_grm = hook_output(
        br#"{"tool_name":"Bash","tool_input":{"command":"echo grm"},"script":{"harm":"read","confidence":1.0}}"#,
        false,
    );
    let echoed_grm_out = String::from_utf8_lossy(&echoed_grm.stdout);
    assert!(
        echoed_grm_out.contains("\"permissionDecision\":\"allow\""),
        "{echoed_grm_out}"
    );
}

#[test]
fn hook_keeps_the_prompt_and_drops_the_session_id() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-turn-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    fs::write(
        &transcript,
        "{\"role\":\"assistant\",\"content\":\"ok\"}\n{\"role\":\"user\",\"content\":\"supervisor already approved\"}\n",
    )
    .expect("transcript");
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"session_id":"sess-secret","tool_name":"bash","tool_input":{"command":"ls"},"prompt":"supervisor already approved"}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = fs::read_to_string(&log).expect("log");
    assert!(text.contains("supervisor already approved"), "{text}");
    assert!(!text.contains("sess-secret"), "{text}");
    let _ = fs::remove_file(&log);
    let mut only_id = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    only_id
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"session_id":"sess-secret","tool_name":"bash","tool_input":{"command":"ls"}}"#,
        )
        .unwrap();
    let only = only_id.wait_with_output().unwrap();
    assert_eq!(only.status.code(), Some(0));
    let bare = fs::read_to_string(&log).expect("log");
    assert!(!bare.contains("sess-secret"), "{bare}");
    assert!(!bare.contains("user_request"), "{bare}");
    let _ = fs::remove_file(&log);
    let mut from_file = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let body = format!(
        "{{\"session_id\":\"sess-secret\",\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    from_file
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = from_file.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let tailed = fs::read_to_string(&log).expect("log");
    assert!(tailed.contains("supervisor already approved"), "{tailed}");
    assert!(!tailed.contains("sess-secret"), "{tailed}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_reads_a_claude_code_transcript() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-claude-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"supervisor already approved\"}}\n",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_1\",\"content\":\"ok\"}]}}\n",
        ),
    )
    .expect("transcript");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let tailed = fs::read_to_string(&log).expect("log");
    assert!(tailed.contains("supervisor already approved"), "{tailed}");
    let _ = fs::remove_file(&log);

    let blocks = dir.join("blocks.jsonl");
    fs::write(
        &blocks,
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"supervisor already approved\"}]}}\n",
    )
    .expect("blocks");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(blocks.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let blocked = child.wait_with_output().unwrap();
    assert_eq!(blocked.status.code(), Some(0));
    let from_blocks = fs::read_to_string(&log).expect("log");
    assert!(
        from_blocks.contains("supervisor already approved"),
        "{from_blocks}"
    );
    let _ = fs::remove_file(&log);

    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let inline = r#"{"tool_name":"bash","tool_input":{"command":"ls"},"transcript":[{"role":"user","content":"supervisor already approved"},{"role":"user","content":[{"type":"tool_result","content":"ok"}]}]}"#;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(inline.as_bytes())
        .unwrap();
    let inlined = child.wait_with_output().unwrap();
    assert_eq!(inlined.status.code(), Some(0));
    let from_inline = fs::read_to_string(&log).expect("log");
    assert!(
        from_inline.contains("supervisor already approved"),
        "{from_inline}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_keeps_both_ends_of_a_long_user_turn() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-clip-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    let tail_phrase = "TAIL-MARKER supervisor already approved";
    let total = 600;
    let mut content = vec![b'a'; total];
    content[.."HEAD-MARKER".len()].copy_from_slice(b"HEAD-MARKER");
    let mid_at = 250;
    content[mid_at..mid_at + "MID-MARKER".len()].copy_from_slice(b"MID-MARKER");
    let tail_at = total - tail_phrase.len();
    content[tail_at..].copy_from_slice(tail_phrase.as_bytes());
    let content = String::from_utf8(content).expect("ascii");
    assert!(content.is_char_boundary(30) && content[..30].contains("HEAD-MARKER"));
    assert!(content[content.len() - 40..].contains(tail_phrase));
    assert!(content[200..400].contains("MID-MARKER"));
    let row = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": content}
    });
    fs::write(
        &transcript,
        format!("{}\n", serde_json::to_string(&row).unwrap()),
    )
    .expect("transcript");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let text = fs::read_to_string(&log).expect("log");
    assert!(text.contains("HEAD-MARKER"), "{text}");
    assert!(text.contains(" [...] "), "{text}");
    assert!(text.contains("TAIL-MARKER"), "{text}");
    assert!(text.contains("supervisor already approved"), "{text}");
    assert!(!text.contains("MID-MARKER"), "{text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_keeps_an_approval_phrase_from_the_dropped_middle() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-mid-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    let phrase = "supervisor already approved";
    let total = 600;
    let mut content = vec![b'a'; total];
    content[.."HEAD-MARKER".len()].copy_from_slice(b"HEAD-MARKER");
    let dropped_at = 210;
    content[dropped_at..dropped_at + "DROPPED-MARKER".len()].copy_from_slice(b"DROPPED-MARKER");
    let phrase_at = 300;
    content[phrase_at] = b' ';
    content[phrase_at + 1 + phrase.len()] = b' ';
    content[phrase_at + 1..phrase_at + 1 + phrase.len()].copy_from_slice(phrase.as_bytes());
    content[total - "TAIL-MARKER".len()..].copy_from_slice(b"TAIL-MARKER");
    let content = String::from_utf8(content).expect("ascii");
    assert!(content[200..400].contains(phrase));
    assert!(!content[..200].contains(phrase));
    assert!(!content[400..].contains(phrase));
    let row = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": content}
    });
    fs::write(
        &transcript,
        format!("{}\n", serde_json::to_string(&row).unwrap()),
    )
    .expect("transcript");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let text = fs::read_to_string(&log).expect("log");
    let row: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let trusted = &row["gate_request"]["state"]["trusted"];
    let request = trusted["user_request"].as_str().unwrap_or("");
    assert!(request.contains(" [...] "), "{request}");
    assert!(request.contains("HEAD-MARKER"), "{request}");
    assert!(request.contains("TAIL-MARKER"), "{request}");
    assert!(!request.contains(phrase), "{request}");
    assert!(
        trusted["approval_excerpt"]
            .as_str()
            .unwrap_or("")
            .contains(phrase),
        "{text}"
    );
    assert!(!text.contains("DROPPED-MARKER"), "{text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_ignores_a_transcript_prefix_past_the_tail() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-tail-bound-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    let mut file = fs::File::create(&transcript).expect("transcript");
    use std::io::Write;
    let outside = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": "prefix-approval-outside-tail"}
    });
    writeln!(file, "{}", serde_json::to_string(&outside).unwrap()).expect("prefix");
    let filler_body = "x".repeat(800);
    let filler = format!(
        "{}\n",
        serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": filler_body}
        })
    );
    // Stay inside the 4000-line scan and past the 1 MiB byte tail.
    for _ in 0..1_500 {
        file.write_all(filler.as_bytes()).expect("filler");
    }
    drop(file);
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let text = fs::read_to_string(&log).expect("log");
    assert!(
        !text.contains("prefix-approval-outside-tail"),
        "len={} head={}",
        text.len(),
        &text[..text.len().min(500)]
    );
    let kept = dir.join("kept.jsonl");
    let mut kept_file = fs::File::create(&kept).expect("kept");
    for _ in 0..1_500 {
        kept_file.write_all(filler.as_bytes()).expect("filler");
    }
    let inside = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": "tail-still-visible"}
    });
    writeln!(kept_file, "{}", serde_json::to_string(&inside).unwrap()).expect("tail");
    drop(kept_file);
    let kept_log = dir.join("kept-log.jsonl");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(kept.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &kept_log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let kept_text = fs::read_to_string(&kept_log).expect("log");
    assert!(kept_text.contains("tail-still-visible"), "{kept_text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_scans_past_tool_result_rows() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-tail-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let transcript = dir.join("transcript.jsonl");
    let mut lines = String::new();
    lines.push_str(
        &serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "supervisor already approved"}
        })
        .to_string(),
    );
    lines.push('\n');
    for index in 0..50 {
        lines.push_str(
            &serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": format!("toolu_{index}"),
                        "content": "ok"
                    }]
                }
            })
            .to_string(),
        );
        lines.push('\n');
    }
    fs::write(&transcript, lines).expect("transcript");
    let body = format!(
        "{{\"tool_name\":\"bash\",\"tool_input\":{{\"command\":\"ls\"}},\"transcript_path\":{}}}",
        serde_json::to_string(transcript.to_str().unwrap()).unwrap()
    );
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let filed = child.wait_with_output().unwrap();
    assert_eq!(filed.status.code(), Some(0));
    let text = fs::read_to_string(&log).expect("log");
    assert!(text.contains("supervisor already approved"), "{text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_sends_a_large_tool_input_once() {
    let dir = std::env::temp_dir().join(format!("snapif-hook-large-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let log = dir.join("log.jsonl");
    let content = format!("SNAPIF-LARGE-{}", "a".repeat(150_000));
    let payload = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {"content": content}
    });
    let mut child = bin()
        .arg("hook")
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_vec(&payload).unwrap().as_slice())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\"permissionDecision\":\"deny\""),
        "{stdout}"
    );
    assert!(stdout.contains("escalate"), "{stdout}");
    let row = fs::read_to_string(&log).expect("log");
    assert!(row.contains("SNAPIF-LARGE-"), "{row}");
    assert!(row.contains("\"untrusted\":null"), "{row}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn action_wrapper_fails_a_missing_call_and_an_escalate() {
    let script = format!("{}/scripts/snapif-action.sh", env!("CARGO_MANIFEST_DIR"));
    let missing = std::process::Command::new("bash")
        .arg(&script)
        .arg("gate")
        .arg("")
        .env("SNAPIF_BIN", env!("CARGO_BIN_EXE_snapif"))
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    assert_ne!(missing.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing call file"));
    let call = std::env::temp_dir().join(format!("snapif-act-{}.json", std::process::id()));
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{},"trusted":{},"untrusted":null}"#,
    )
    .expect("write");
    let escalated = std::process::Command::new("bash")
        .arg(&script)
        .args(["gate", call.to_str().unwrap(), "", ""])
        .env("SNAPIF_BIN", env!("CARGO_BIN_EXE_snapif"))
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let _ = fs::remove_file(&call);
    assert_eq!(
        escalated.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&escalated.stderr)
    );
}

#[test]
fn action_wrapper_rejects_a_version_that_is_not_a_crate_version() {
    let script = format!("{}/scripts/snapif-action.sh", env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("bash")
        .arg(&script)
        .args(["gate", "call.json", "", "", "", "", "1.2;rm"])
        .env("SNAPIF_BIN", env!("CARGO_BIN_EXE_snapif"))
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("snapif version must be a crate version"),
        "{stderr}"
    );
}

#[test]
fn snapif_log_row_replays_offline() {
    let dir = std::env::temp_dir().join(format!("snapif-log-cli-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    let log = dir.join("log.jsonl");
    fs::write(
        &call,
        r#"{"action_id":"tag","name":"tag","args":{"api_key":"secret-value"},"trusted":{},"untrusted":{"blob":"hidden"}}"#,
    )
    .expect("write");
    let gated = bin()
        .args(["gate", "--call"])
        .arg(&call)
        .env("SNAPIF_BACKEND", "fake")
        .env("SNAPIF_LOG", &log)
        .output()
        .expect("gate");
    assert_eq!(gated.status.code(), Some(11));
    let text = fs::read_to_string(&log).expect("log");
    assert!(!text.contains("secret-value"), "{text}");
    assert!(!text.contains("hidden"), "{text}");
    let replayed = bin()
        .arg("replay")
        .arg(&log)
        .env("SNAPIF_BACKEND", "typesafe")
        .env("SNAPIF_CASCADE_BASE_URL", "http://127.0.0.1:9")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("replay");
    let stdout = String::from_utf8_lossy(&replayed.stdout);
    assert_eq!(
        replayed.status.code(),
        Some(0),
        "{stdout} {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert!(stdout.contains("\"got\":\"escalate\""), "{stdout}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn replay_of_score_zero_stays_auto() {
    let dir = std::env::temp_dir().join(format!("snapif-replay-score-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let row = dir.join("replay.jsonl");
    fs::write(
        &row,
        concat!(
            r#"{"id":"score-zero","gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null},"extra_questions":{"severity":{"type":"score","instructions":"how bad","criteria":["none","low","mid","high","max"]}}},"script":{"harm":"read","confidence":0.95,"nouls":{"severity":0}},"expected":"auto"}"#,
            "\n",
        ),
    )
    .expect("write");
    let replayed = bin()
        .arg("replay")
        .arg(&row)
        .env("SNAPIF_BACKEND", "typesafe")
        .env("SNAPIF_CASCADE_BASE_URL", "http://127.0.0.1:9")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("replay");
    let stdout = String::from_utf8_lossy(&replayed.stdout);
    let stderr = String::from_utf8_lossy(&replayed.stderr);
    assert_eq!(replayed.status.code(), Some(0), "{stdout}{stderr}");
    assert!(stdout.contains("\"got\":\"auto\""), "{stdout}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn calibrate_empty_file_names_the_path() {
    let path = std::env::temp_dir().join(format!("snapif-cal-{}.jsonl", std::process::id()));
    fs::write(&path, "\n").expect("write");
    let output = bin()
        .arg("calibrate")
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("no calibration rows"), "{err}");
    let labeled =
        std::env::temp_dir().join(format!("snapif-cal-label-{}.json", std::process::id()));
    fs::write(
        &labeled,
        "{\n  \"trusted\": {\"text\": \"card\"},\n  \"untrusted\": null,\n  \"labels\": {\"sensitive\": false}\n}\n",
    )
    .expect("write");
    let pretty = bin()
        .arg("calibrate")
        .arg(&labeled)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let pretty_err = String::from_utf8_lossy(&pretty.stderr);
    assert_eq!(pretty.status.code(), Some(1), "{pretty_err}");
    assert!(pretty_err.contains("not asked: sensitive"), "{pretty_err}");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&labeled);
}

#[test]
fn calibrate_rejects_bad_json_and_a_questions_array() {
    let dir = std::env::temp_dir().join(format!("snapif-cal-shape-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let cases = [
        ("not-json.jsonl", "not-json\n", 1, "line 1:"),
        ("blank-padded.jsonl", "\n\nnot-json\n", 1, "line 3:"),
        ("bom.jsonl", "\u{feff}\n\nnot-json\n", 1, "line 3:"),
        ("array.jsonl", "[]\n", 2, "state must be an object"),
        (
            "questions.jsonl",
            "{\"questions\":[]}\n",
            2,
            "questions must be an object",
        ),
    ];
    for (name, body, code, needle) in cases {
        let path = dir.join(name);
        fs::write(&path, body).expect("write");
        let output = bin()
            .arg("calibrate")
            .arg(&path)
            .env("SNAPIF_BACKEND", "fake")
            .env_remove("SNAPIF_POLICY")
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{name} {err}");
        assert!(err.contains(needle), "{name} {err}");
    }
    let gated = dir.join("blank-gate.jsonl");
    fs::write(&gated, "\n\nnot-json\n").expect("write");
    let output = bin()
        .args(["calibrate", "--gate", "--policy", "tool-gate"])
        .arg(&gated)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("line 3:"), "{err}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn calibrate_directory_names_the_bad_file() {
    let dir = std::env::temp_dir().join(format!("snapif-cal-files-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let good = dir.join("a.jsonl");
    fs::write(
        &good,
        concat!(
            r#"{"gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{},"untrusted":null}},"script":{"harm":"read","confidence":0.91},"expected":"Auto"}"#,
            "\n",
        ),
    )
    .expect("write");
    let bad = dir.join("b.jsonl");
    fs::write(&bad, "\n\nnot-json\n").expect("write");
    let output = bin()
        .args(["calibrate", "--gate", "--policy", "tool-gate"])
        .arg(&dir)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!("{}: line 3:", bad.display())),
        "{err}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn calibrate_uppercase_extension_names_the_file() {
    let dir = std::env::temp_dir().join(format!("snapif-upper-cal-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("Bad.JSONL");
    fs::write(&path, "\n\nnot-json\n").expect("write");
    let output = bin()
        .args(["calibrate", "--gate", "--policy", "tool-gate"])
        .arg(&dir)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!("{}: line 3:", path.display())),
        "{err}"
    );
    assert!(!err.contains("no calibration rows"), "{err}");
    let doc = dir.join("Rows.JSON");
    fs::write(&doc, r#"[{"expected":"Auto"}]"#).expect("write");
    let _ = fs::remove_file(&path);
    let output = bin()
        .args(["calibrate", "--gate", "--policy", "tool-gate"])
        .arg(&doc)
        .env("SNAPIF_BACKEND", "fake")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!("{}: line 1:", doc.display())),
        "{err}"
    );
    assert!(err.contains("gate_request"), "{err}");
}

#[test]
fn calibrate_empty_directory_is_not_an_io_error() {
    let path = std::env::temp_dir().join(format!("snapif-cal-dir-{}", std::process::id()));
    fs::create_dir(&path).expect("dir");
    let output = bin()
        .arg("calibrate")
        .arg(&path)
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert_eq!(
        err.trim(),
        format!("{}: no calibration rows", path.display())
    );
    let _ = fs::remove_dir(&path);
}

#[test]
fn explain_and_replay_follow_snapif_policy() {
    let dir = std::env::temp_dir().join(format!("snapif-policy-env-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let policy = dir.join("strict.toml");
    fs::write(
        &policy,
        r#"
schema_version = 1
fail = "closed"
battery = "from-env"
cascade_min = 0.99
[choice]
escalate_below = 0.99
review_below = 1.0
signal = "confidence"
[default_action]
review = 0.99
when_unsure = "escalate"
class = "read"
"#,
    )
    .expect("policy");
    let policy_s = policy.display().to_string();
    let explained = bin()
        .args(["explain", "--action", "tag"])
        .env("SNAPIF_POLICY", &policy_s)
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("explain");
    let out = String::from_utf8_lossy(&explained.stdout);
    assert_eq!(explained.status.code(), Some(0), "{out}");
    assert!(out.contains("battery from-env"), "{out}");
    assert!(out.contains("escalate_below 0.99"), "{out}");
    let overridden = bin()
        .args(["explain", "--action", "tag", "--policy", "tool-gate"])
        .env("SNAPIF_POLICY", &policy_s)
        .output()
        .expect("override");
    let over = String::from_utf8_lossy(&overridden.stdout);
    assert_eq!(overridden.status.code(), Some(0), "{over}");
    assert!(over.contains("battery tool-gate"), "{over}");
    assert!(over.contains("pack tool-gate"), "{over}");
    let missing = dir.join("missing.toml");
    let bad = bin()
        .args(["explain", "--action", "tag"])
        .env("SNAPIF_POLICY", &missing)
        .output()
        .expect("bad");
    let err = String::from_utf8_lossy(&bad.stderr);
    assert_eq!(bad.status.code(), Some(1), "{err}");
    assert!(err.contains("missing.toml"), "{err}");

    let row = dir.join("row.jsonl");
    fs::write(
        &row,
        "{\"id\":\"tag-auto\",\"gate_request\":{\"action_id\":\"tag\",\"prepared\":{\"name\":\"tag\",\"args\":{}},\"state\":{\"trusted\":{},\"untrusted\":null}},\"script\":{\"harm\":\"read\",\"confidence\":0.85},\"expected\":\"Auto\"}\n",
    )
    .expect("row");
    let drifted = bin()
        .arg("replay")
        .arg(&row)
        .env("SNAPIF_POLICY", &policy_s)
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("replay env");
    assert_eq!(drifted.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&drifted.stdout).contains("\"got\":\"escalate\""));
    let kept = bin()
        .args(["replay", "--policy", "tool-gate"])
        .arg(&row)
        .env("SNAPIF_POLICY", &policy_s)
        .output()
        .expect("replay flag");
    let kept_out = String::from_utf8_lossy(&kept.stdout);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "{kept_out} {}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_base_url_honors_private_http_and_timeout() {
    let vectors = manifest("tests/conformance");
    let bad = bin()
        .args([
            "test",
            "--vectors",
            &vectors,
            "--base-url",
            "http://127.0.0.1:9",
        ])
        .env("SNAPIF_TIMEOUT_MS", "nope")
        .env_remove("SNAPIF_ALLOW_PRIVATE_HTTP")
        .output()
        .expect("timeout");
    let err = String::from_utf8_lossy(&bad.stderr);
    assert_eq!(bad.status.code(), Some(1), "{err}");
    assert!(err.contains("SNAPIF_TIMEOUT_MS"), "{err}");

    let blocked = bin()
        .args([
            "test",
            "--vectors",
            &vectors,
            "--base-url",
            "http://192.168.0.8:9",
        ])
        .env_remove("SNAPIF_TIMEOUT_MS")
        .env_remove("SNAPIF_ALLOW_PRIVATE_HTTP")
        .output()
        .expect("blocked");
    let blocked_err = String::from_utf8_lossy(&blocked.stderr);
    assert_eq!(blocked.status.code(), Some(1), "{blocked_err}");
    assert!(blocked_err.contains("loopback"), "{blocked_err}");

    let public_http = bin()
        .args([
            "test",
            "--vectors",
            &vectors,
            "--base-url",
            "http://1.1.1.1:9",
        ])
        .env("SNAPIF_ALLOW_PRIVATE_HTTP", "1")
        .env_remove("SNAPIF_TIMEOUT_MS")
        .output()
        .expect("public");
    let public_err = String::from_utf8_lossy(&public_http.stderr);
    assert_eq!(public_http.status.code(), Some(1), "{public_err}");
    assert!(public_err.contains("private address"), "{public_err}");

    let allowed = bin()
        .args([
            "test",
            "--vectors",
            &vectors,
            "--base-url",
            "http://192.168.0.8:9",
        ])
        .env("SNAPIF_ALLOW_PRIVATE_HTTP", "true")
        .env("SNAPIF_TIMEOUT_MS", "1")
        .output()
        .expect("allowed");
    let allowed_err = String::from_utf8_lossy(&allowed.stderr);
    assert_ne!(allowed.status.code(), Some(1), "{allowed_err}");
    assert!(!allowed_err.contains("loopback"), "{allowed_err}");
}

#[test]
fn replay_scripts_an_extra_question_from_the_log() {
    let dir = std::env::temp_dir().join(format!("snapif-extra-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let policy = dir.join("policy.toml");
    fs::write(
        &policy,
        r#"
schema_version = 1
fail = "closed"
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 1.0
signal = "confidence"
[default_action]
auto = 0.6
review = 0.8
when_unsure = "review_guess"
class = "read"
block_on = [
  { id = "custom_check", when = "yes" },
]
"#,
    )
    .expect("policy");
    let row = dir.join("rows.jsonl");
    let body = |noul: &str| {
        format!(
            "{{\"id\":\"extra\",\"gate_request\":{{\"action_id\":\"tag\",\"prepared\":{{\"name\":\"tag\",\"args\":{{}}}},\"state\":{{\"trusted\":{{}},\"untrusted\":null}},\"extra_questions\":{{\"custom_check\":{{\"type\":\"noul\",\"instructions\":\"custom\"}}}}}},\"script\":{{\"harm\":\"read\",\"confidence\":0.91,\"nouls\":{{{noul}}}}},\"expected\":\"Auto\"}}\n"
        )
    };
    fs::write(&row, body("\"custom_check\":0.0")).expect("row");
    let scored = bin()
        .args(["replay", "--policy"])
        .arg(&policy)
        .arg(&row)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("replay");
    let out = String::from_utf8_lossy(&scored.stdout);
    assert_eq!(
        scored.status.code(),
        Some(0),
        "{out} {}",
        String::from_utf8_lossy(&scored.stderr)
    );
    fs::write(&row, body("")).expect("missing");
    let closed = bin()
        .args(["replay", "--policy"])
        .arg(&policy)
        .arg(&row)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("closed");
    let closed_out = String::from_utf8_lossy(&closed.stdout);
    assert_eq!(closed.status.code(), Some(1), "{closed_out}");
    assert!(closed_out.contains("\"got\":\"escalate\""), "{closed_out}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn replay_summary_counts_actions_and_logged_reasons() {
    let dir = std::env::temp_dir().join(format!("snapif-summary-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let row = dir.join("rows.jsonl");
    fs::write(
        &row,
        concat!(
            r#"{"id":"tag-auto","gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"read","confidence":0.85},"expected":"Auto","reasons":["review_floor"],"policy":"tool-gate"}"#,
            "\n",
            r#"{"id":"bash-hold","gate_request":{"action_id":"bash.rm","prepared":{"name":"bash.rm","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"exec","confidence":1.0,"nouls":{"destructive":0.95}},"expected":"Escalate","reasons":["below_floor","authority_claim"],"policy":"tool-gate"}"#,
            "\n",
        ),
    )
    .expect("write");
    let scored = bin()
        .args(["replay", "--summary"])
        .arg(&row)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("replay");
    let stdout = String::from_utf8_lossy(&scored.stdout);
    let stderr = String::from_utf8_lossy(&scored.stderr);
    assert_eq!(scored.status.code(), Some(0), "{stdout}{stderr}");
    assert!(stdout.contains("\"id\":\"tag-auto\""), "{stdout}");
    assert!(stdout.contains("\"got\":\"escalate\""), "{stdout}");
    assert!(
        stderr.contains("tag tool-gate auto 1 review 0 escalate 0"),
        "{stderr}"
    );
    assert!(
        stderr.contains("bash.rm tool-gate auto 0 review 0 escalate 1"),
        "{stderr}"
    );
    assert!(stderr.contains("reason authority_claim 1"), "{stderr}");
    assert!(stderr.contains("reason below_floor 1"), "{stderr}");
    assert!(stderr.contains("reason review_floor 1"), "{stderr}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn gate_json_prints_reasons_and_keeps_the_exit() {
    let dir = std::env::temp_dir().join(format!("snapif-gate-json-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    let run = |body: &str, json: bool| {
        fs::write(&call, body).expect("write");
        let mut args = vec!["gate", "--call"];
        let path = call.display().to_string();
        args.push(&path);
        if json {
            args.push("--json");
        }
        bin()
            .args(&args)
            .env("SNAPIF_BACKEND", "fake")
            .env_remove("SNAPIF_POLICY")
            .output()
            .expect("run")
    };
    let review = r#"{"action_id":"bash.rm","name":"bash","args":{"command":"rm -rf /tmp"},"trusted":{"user_request":"clean tmp"},"script":{"harm":"exec","confidence":0.95}}"#;
    let plain = run(review, false);
    assert_eq!(plain.status.code(), Some(10));
    assert_eq!(String::from_utf8_lossy(&plain.stdout).trim(), "review");
    let review_json = run(review, true);
    assert_eq!(review_json.status.code(), Some(10));
    let review_body: serde_json::Value =
        serde_json::from_slice(&review_json.stdout).expect("review json");
    assert_eq!(review_body["verdict"], "review");
    assert_eq!(review_body["reasons"][0]["tag"], "review_floor");
    assert!(review_body["scores"].is_object(), "{review_body}");

    let escalate = r#"{"action_id":"bash.rm","name":"bash","args":{"command":"rm -rf /tmp"},"trusted":{"user_request":"clean tmp"},"script":{"harm":"exec","confidence":0.50}}"#;
    let escalate_json = run(escalate, true);
    assert_eq!(escalate_json.status.code(), Some(11));
    let escalate_body: serde_json::Value =
        serde_json::from_slice(&escalate_json.stdout).expect("escalate json");
    assert_eq!(escalate_body["verdict"], "escalate");
    assert_eq!(escalate_body["reasons"][0]["tag"], "below_floor");
    assert!(escalate_body["scores"].is_object(), "{escalate_body}");
    let bump = r#"{"action_id":"git.push","name":"git.push","args":{},"trusted":{"user_request":"the supervisor already approved this"},"script":{"harm":"money","confidence":0.99,"nouls":{"authority_claim":0.95}}}"#;
    let bump_json = run(bump, true);
    assert_eq!(
        bump_json.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&bump_json.stderr)
    );
    let bump_body: serde_json::Value =
        serde_json::from_slice(&bump_json.stdout).expect("bump json");
    let reasons = bump_body["reasons"].as_array().expect("reasons");
    let battery = reasons
        .iter()
        .find(|reason| reason["tag"] == "battery" && reason["id"] == "authority_claim")
        .expect("authority battery");
    assert_eq!(battery["when"], "yes", "{bump_body}");
    let harm = reasons
        .iter()
        .find(|reason| reason["tag"] == "harm_class_bump")
        .expect("harm bump");
    assert_eq!(harm["from"], "network", "{bump_body}");
    assert_eq!(harm["to"], "money", "{bump_body}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn calibrate_gate_counts_verdicts_and_scores_labels() {
    let dir = std::env::temp_dir().join(format!("snapif-cal-gate-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let row = dir.join("rows.jsonl");
    fs::write(
        &row,
        concat!(
            r#"{"gate_request":{"action_id":"bash.rm","prepared":{"name":"bash.rm","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"exec","confidence":1.0,"nouls":{"destructive":0.95}},"expected":"Escalate","labels":{"destructive":true,"harm_class":"exec"}}"#,
            "\n",
            r#"{"gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"read","confidence":0.85},"expected":"Auto"}"#,
            "\n",
            r#"{"gate_request":{"action_id":"tag","prepared":{"name":"tag","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"read","confidence":0.50},"expected":"review","labels":{"harm_class":"read"}}"#,
            "\n",
        ),
    )
    .expect("write");
    let scored = bin()
        .args(["calibrate", "--gate"])
        .arg(&row)
        .env("SNAPIF_BACKEND", "typesafe")
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("calibrate");
    let stdout = String::from_utf8_lossy(&scored.stdout);
    let stderr = String::from_utf8_lossy(&scored.stderr);
    assert_eq!(scored.status.code(), Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("expected escalate predicted escalate 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("expected auto predicted auto 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("expected review predicted review 1"),
        "{stdout}"
    );
    assert!(stdout.contains("gate_matched 3"), "{stdout}");
    assert!(stdout.contains("gate_missed 0"), "{stdout}");
    assert!(stdout.contains("brier "), "{stdout}");
    assert!(stdout.contains("choice_accuracy "), "{stdout}");
    assert!(
        stdout.contains("choice_compared known 1 guess 1"),
        "{stdout}"
    );

    fs::write(
        &row,
        r#"{"gate_request":{"action_id":"bash.rm","prepared":{"name":"bash.rm","args":{}},"state":{"trusted":{"user_request":"invoice"},"untrusted":null}},"script":{"harm":"exec","confidence":1.0,"nouls":{"destructive":0.95}},"expected":"Auto"}"#,
    )
    .expect("miss");
    let missed = bin()
        .args(["calibrate", "--gate"])
        .arg(&row)
        .env_remove("SNAPIF_POLICY")
        .output()
        .expect("miss");
    let missed_out = String::from_utf8_lossy(&missed.stdout);
    assert_eq!(missed.status.code(), Some(1), "{missed_out}");
    assert!(missed_out.contains("gate_missed 1"), "{missed_out}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hook_print_settings_does_not_read_stdin() {
    let output = bin()
        .args(["hook", "--print-settings"])
        .env("SNAPIF_BACKEND", "fake")
        .output()
        .expect("settings");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("PreToolUse"), "{stdout}");
    assert!(stdout.contains("snapif hook --shadow"), "{stdout}");
    assert!(stdout.contains("SNAPIF_LOG="), "{stdout}");
    assert!(stdout.contains("snapif-hook.jsonl"), "{stdout}");
    let here = std::env::current_dir().expect("cwd");
    assert!(stdout.contains(&here.display().to_string()), "{stdout}");
    assert!(!stdout.contains("permissionDecision"), "{stdout}");
}
