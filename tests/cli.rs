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
fn explain_git_push_has_no_auto_and_names_a_missing_policy() {
    let output = bin()
        .args(["explain", "--action", "git.push"])
        .env_remove("SNAPIF_BACKEND")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("auto none"), "{stdout}");
    assert!(stdout.contains("exfil:yes"), "{stdout}");
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
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid json"));
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
