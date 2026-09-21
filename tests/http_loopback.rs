#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use snapif::backend::Backend;
use snapif::backends::http::HttpBackend;
use snapif::error::BackendError;
use snapif::wire::{WireAnswer, WireQuestion, WireRequest};
use url::Url;

fn choice_request() -> WireRequest {
    let mut criteria = indexmap::IndexMap::new();
    criteria.insert("billing".to_string(), serde_json::json!("Payments"));
    let mut questions = indexmap::IndexMap::new();
    questions.insert(
        "department".to_string(),
        WireQuestion::Choice {
            instructions: serde_json::json!("dept"),
            criteria,
        },
    );
    WireRequest {
        model: "jev-latest".to_string(),
        state: serde_json::json!({}),
        questions,
    }
}

fn choice_body() -> &'static str {
    r#"{"model":"loop","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":1.0},"confidence":0.91}},"usage":{"input_tokens":1,"output_tokens":2}}"#
}

fn http_response(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

struct Hit {
    request: String,
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
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
                if buf.len() > 65_536 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn serve(
    listener: TcpListener,
    replies: Vec<Vec<u8>>,
) -> (Arc<AtomicUsize>, thread::JoinHandle<Vec<Hit>>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let handle = thread::spawn(move || {
        let mut hits = Vec::new();
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let request = read_request(&mut stream);
            let _ = stream.write_all(&reply);
            hits.push(Hit { request });
            seen.fetch_add(1, Ordering::SeqCst);
        }
        hits
    });
    (count, handle)
}

fn block<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

#[tokio::test]
async fn loopback_post_decodes_choice_and_optional_bearer() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_count, handle) = serve(listener, vec![http_response("200 OK", "", choice_body())]);
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        Some("snapif-key".to_string()),
    )
    .expect("client");
    let evaluated = backend
        .evaluate(choice_request(), Instant::now() + Duration::from_secs(2))
        .await
        .expect("evaluate");
    let hits = handle.join().expect("server");
    let request = &hits[0].request;
    assert!(request.starts_with("POST /v1/systemone "));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer snapif-key")
    );
    assert!(matches!(
        evaluated.wire.answers.get("department"),
        Some(WireAnswer::Choice { choice, confidence, .. })
            if choice == "billing" && (*confidence - 0.91).abs() < 1e-9
    ));
    assert_eq!(evaluated.wire.usage.input_tokens, 1);
    assert_eq!(evaluated.backend_id, "compatible");
}

#[tokio::test]
async fn loopback_without_key_omits_authorization() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_count, handle) = serve(listener, vec![http_response("200 OK", "", choice_body())]);
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        None,
    )
    .expect("client");
    backend
        .evaluate(choice_request(), Instant::now() + Duration::from_secs(2))
        .await
        .expect("evaluate");
    let hits = handle.join().expect("server");
    assert!(
        !hits[0]
            .request
            .to_ascii_lowercase()
            .contains("authorization:")
    );
}

#[tokio::test]
async fn retry_after_zero_then_ok_posts_twice() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_count, handle) = serve(
        listener,
        vec![
            http_response("429 Too Many Requests", "Retry-After: 0\r\n", ""),
            http_response("200 OK", "", choice_body()),
        ],
    );
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        None,
    )
    .expect("client");
    let evaluated = backend
        .evaluate(choice_request(), Instant::now() + Duration::from_secs(2))
        .await
        .expect("retry");
    let hits = handle.join().expect("server");
    assert_eq!(hits.len(), 2);
    assert!(matches!(
        evaluated.wire.answers.get("department"),
        Some(WireAnswer::Choice { .. })
    ));
}

#[tokio::test]
async fn rejected_422_returns_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_count, handle) = serve(
        listener,
        vec![http_response("422 Unprocessable Entity", "", "too big")],
    );
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        None,
    )
    .expect("client");
    let err = backend
        .evaluate(choice_request(), Instant::now() + Duration::from_secs(2))
        .await
        .expect_err("422");
    let _ = handle.join();
    assert!(matches!(
        err,
        BackendError::Rejected { status: 422, ref body } if body == "too big"
    ));
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let redirect_to = TcpListener::bind("127.0.0.1:0").expect("bind");
    let dest_port = redirect_to.local_addr().expect("addr").port();
    redirect_to.set_nonblocking(true).expect("nonblocking");
    let first = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = first.local_addr().expect("addr").port();
    let (_count, handle) = serve(
        first,
        vec![http_response(
            "302 Found",
            &format!("Location: http://127.0.0.1:{dest_port}/v1/systemone\r\n"),
            "",
        )],
    );
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        None,
    )
    .expect("client");
    let err = backend
        .evaluate(choice_request(), Instant::now() + Duration::from_secs(2))
        .await
        .expect_err("redirect");
    let _ = handle.join();
    assert!(matches!(err, BackendError::Transport(_)));
    thread::sleep(Duration::from_millis(50));
    assert!(
        redirect_to.accept().is_err(),
        "redirect target accepted a connection"
    );
}

#[test]
fn past_deadline_does_not_connect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    listener.set_nonblocking(true).expect("nonblocking");
    let backend = HttpBackend::compatible(
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("url"),
        None,
    )
    .expect("client");
    let err = block(backend.evaluate(choice_request(), Instant::now() - Duration::from_secs(1)))
        .expect_err("deadline");
    assert!(matches!(err, BackendError::Timeout));
    thread::sleep(Duration::from_millis(50));
    assert!(listener.accept().is_err(), "deadline still connected");
}
