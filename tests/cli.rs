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
fn replay_matches_fixtures_without_network() {
    let output = bin()
        .arg("replay")
        .arg(manifest("tests/fixtures/actions.jsonl"))
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
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
fn gate_auto_exits_zero() {
    let dir = std::env::temp_dir().join(format!("snapif-call-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let call = dir.join("call.json");
    fs::write(
        &call,
        r#"{"action_id":"tag","harm":"read","confidence":0.91,"trusted":{"user_request":"list"}}"#,
    )
    .expect("write");
    let output = bin()
        .args(["gate", "--policy", "tool-gate", "--call"])
        .arg(&call)
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "auto");
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
    assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
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
