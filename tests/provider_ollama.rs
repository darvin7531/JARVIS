mod common;
use jarvis::provider::{Error, Ollama, Provider, Request};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};

fn request() -> Request<'static> {
    Request {
        model: "test",
        prompt: "private prompt",
        max_tokens: 8,
        max_output_bytes: 64,
        timeout: Duration::from_secs(2),
    }
}
fn serve(body: &'static str, delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        common::accept_version(&listener);
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let _ = stream.read(&mut buf);
        thread::sleep(delay);
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        for chunk in body.as_bytes().chunks(3) {
            let _ = stream.write_all(chunk);
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(1));
        }
    });
    port
}
#[test]
fn fragmented_ndjson_and_usage() {
    let port = serve(
        "{\"response\":\"Hi\",\"done\":false}\n{\"response\":\"!\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":2,\"eval_count\":3}\n",
        Duration::ZERO,
    );
    let mut text = String::new();
    let usage = Ollama { port }
        .generate(&request(), &AtomicBool::new(false), &mut |s| {
            text.push_str(s)
        })
        .unwrap();
    assert_eq!(text, "Hi!");
    assert_eq!((usage.input_tokens, usage.output_tokens), (2, 3));
}
#[test]
fn truncated_budget_timeout_cancel_and_redacted_errors() {
    let mut emit = |_: &str| {};
    let port = serve(
        "{\"response\":\"private prompt\",\"done\":false}\n",
        Duration::ZERO,
    );
    let err = Ollama { port }
        .generate(&request(), &AtomicBool::new(false), &mut emit)
        .unwrap_err();
    assert_eq!(err, Error::Truncated);
    assert!(!format!("{err:?}").contains("private prompt"));
    let port = serve("{\"response\":\"long\",\"done\":false}\n", Duration::ZERO);
    let mut r = request();
    r.max_output_bytes = 2;
    assert_eq!(
        Ollama { port }.generate(&r, &AtomicBool::new(false), &mut emit),
        Err(Error::Budget)
    );
    let port = serve("", Duration::from_secs(1));
    let mut r = request();
    r.timeout = Duration::from_millis(80);
    assert_eq!(
        Ollama { port }.generate(&r, &AtomicBool::new(false), &mut emit),
        Err(Error::Timeout)
    );
    assert_eq!(
        Ollama { port }.generate(&r, &AtomicBool::new(true), &mut emit),
        Err(Error::Cancelled)
    );
    let port = serve("", Duration::from_secs(1));
    let cancel = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(Duration::from_millis(80));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(
            Ollama { port }.generate(&request(), &cancel, &mut emit),
            Err(Error::Cancelled)
        );
    });
}
#[test]
fn actual_ollama_smoke_when_enabled() {
    if std::env::var_os("JARVIS_OLLAMA_SMOKE").is_none() {
        return;
    }
    let r = Request {
        model: "smollm2:135m-instruct-q4_0",
        prompt: "Say hello in one word.",
        max_tokens: 16,
        max_output_bytes: 4096,
        timeout: Duration::from_secs(120),
    };
    let mut text = String::new();
    let usage = Ollama { port: 11434 }
        .generate(&r, &AtomicBool::new(false), &mut |s| text.push_str(s))
        .unwrap();
    assert!(!text.trim().is_empty());
    assert!(usage.input_tokens > 0 && usage.output_tokens > 0 && usage.output_tokens <= 16);
}

#[test]
fn terminal_text_is_not_emitted_when_usage_is_invalid() {
    let port = serve(
        "{\"response\":\"unsafe\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":1,\"eval_count\":9}\n",
        Duration::ZERO,
    );
    let mut text = String::new();
    let result = Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |s| {
        text.push_str(s)
    });
    assert!(result.is_err());
    assert!(text.is_empty(), "invalid terminal text was emitted");
}

#[test]
fn callback_panic_closes_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        common::accept_version(&listener);
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0; 4096];
        assert!(stream.read(&mut buf).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"response\":\"x\",\"done\":false}\n").unwrap();
        matches!(stream.read(&mut buf), Ok(0))
    });
    let result = std::panic::catch_unwind(|| {
        let _ = Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |_| {
            panic!("callback")
        });
    });
    assert!(result.is_err());
    assert!(
        server.join().unwrap(),
        "curl kept the connection open after panic"
    );
}

#[test]
fn incomplete_http_body_is_transport_error_even_after_done() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        common::accept_version(&listener);
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let _ = stream.read(&mut buf);
        let body = "{\"response\":\"ok\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":1,\"eval_count\":1}\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len() + 1,
            body
        )
        .unwrap();
    });
    assert_eq!(
        Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |_| {}),
        Err(Error::Transport)
    );
}

#[test]
fn missing_usage_rejects_completion_after_provisional_chunk() {
    let port = serve(
        "{\"response\":\"first\",\"done\":false}\n{\"response\":\"last\",\"done\":true,\"done_reason\":\"stop\"}\n",
        Duration::ZERO,
    );
    let mut text = String::new();
    let result = Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |s| {
        text.push_str(s)
    });
    assert_eq!(result, Err(Error::InvalidStream));
    assert_eq!(text, "first");
}

#[test]
fn sends_requested_token_limit_and_handles_fragmented_unicode() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        common::accept_version(&listener);
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 512];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse().ok())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    let payload: serde_json::Value =
                        serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    assert_eq!(payload["options"]["num_predict"], 8);
                    assert_eq!(payload["truncate"], false);
                    assert_eq!(payload["shift"], false);
                    break;
                }
            }
        }
        let body = "{\"response\":\"é\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":1,\"eval_count\":1}\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        for byte in body.as_bytes() {
            stream.write_all(&[*byte]).unwrap();
        }
    });
    let mut text = String::new();
    Ollama { port }
        .generate(&request(), &AtomicBool::new(false), &mut |s| {
            text.push_str(s)
        })
        .unwrap();
    server.join().unwrap();
    assert_eq!(text, "é");
}

#[test]
fn length_finish_is_incomplete_and_preserves_usage() {
    let port = serve(
        "{\"response\":\"partial\",\"done\":true,\"done_reason\":\"length\",\"prompt_eval_count\":2,\"eval_count\":3}\n",
        Duration::ZERO,
    );
    let mut text = String::new();
    let result = Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |s| {
        text.push_str(s)
    });
    assert_eq!(text, "partial");
    let error = result.expect_err("length must not be reported as complete");
    let debug = format!("{error:?}");
    assert!(
        debug.contains("input_tokens: 2") && debug.contains("output_tokens: 3"),
        "{debug}"
    );
}

#[test]
fn terminal_finish_reason_must_be_recognized() {
    for reason in [
        "",
        ",\"done_reason\":null",
        ",\"done_reason\":42",
        ",\"done_reason\":\"unknown\"",
    ] {
        let body = format!(
            "{{\"response\":\"last\",\"done\":true{reason},\"prompt_eval_count\":2,\"eval_count\":3}}\n"
        );
        let port = serve(Box::leak(body.into_boxed_str()), Duration::ZERO);
        let mut text = String::new();
        assert_eq!(
            Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |s| text
                .push_str(s)),
            Err(Error::InvalidStream)
        );
        assert!(text.is_empty());
    }
}

#[test]
fn unsupported_ollama_is_rejected_before_prompt_is_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        let was_version = request.starts_with("GET /api/version ");
        let leaked = request.contains("private prompt");
        let body = r#"{"version":"0.6.8"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        (was_version, leaked)
    });
    assert!(Ollama { port }
        .generate(&request(), &AtomicBool::new(false), &mut |_| {})
        .is_err());
    let (was_version, leaked) = server.join().unwrap();
    assert!(
        was_version && !leaked,
        "must check version before sending prompt"
    );
}

#[test]
fn malformed_and_unverified_versions_fail_closed() {
    for body in [
        "{}",
        r#"{"version":"0.13.6"}"#,
        r#"{"version":"0.13.5-rc1"}"#,
        r#"{"version":13}"#,
        r#"{"version":"0.13.5","version":"0.13.5"}"#,
        "not json",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).unwrap();
            assert!(buf[..n].starts_with(b"GET /api/version "));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        assert_eq!(
            Ollama { port }.generate(&request(), &AtomicBool::new(false), &mut |_| {}),
            Err(Error::Transport)
        );
        server.join().unwrap();
    }
}
