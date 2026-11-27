use std::{fs, process::Command};

#[test]
fn configured_chat_rejects_vision_for_text_only_transport() {
    let path = std::env::temp_dir().join(format!("jarvis-vision-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text","vision"],"allowed_data":["public"],"port":1}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--require",
            "vision",
            "hello",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.stderr, b"{\"error\":\"capability_unavailable\"}\n");
}

#[test]
fn task_prefix_rules_select_tiers() {
    use jarvis::inference::{classify, Tier};
    assert_eq!(classify("hello"), Tier::T1);
    assert_eq!(classify("code: fix it"), Tier::T2);
    assert_eq!(classify(" complex: review it"), Tier::T3);
}

#[test]
fn configured_cli_streams_and_forwards_limits() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        process::Stdio,
        thread,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["model"], "coder");
        assert_eq!(body["options"]["num_predict"], 7);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{\"response\":\"first\\n\",\"done\":false}\n").unwrap();
        socket.flush().unwrap();
        thread::sleep(Duration::from_millis(700));
        socket
            .write_all(
                b"{\"response\":\"last\",\"done\":true,\"prompt_eval_count\":2,\"eval_count\":3}\n",
            )
            .unwrap();
    });
    let path = std::env::temp_dir().join(format!("jarvis-stream-{}.json", std::process::id()));
    fs::write(&path, format!(r#"{{"limits":{{"max_tokens":7,"timeout_seconds":5,"max_output_bytes":100}},"models":[{{"name":"coder","tier":"t2","capabilities":["text"],"allowed_data":["public"],"port":{port}}}]}}"#)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config", path.to_str().unwrap(), "code: hi"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        tx.send(first).unwrap();
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        rest
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "first\n"
    );
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(reader.join().unwrap(), "last");
    assert_eq!(output.stderr, b"{\"input_tokens\":2,\"output_tokens\":3}\n");
    server.join().unwrap();
    fs::remove_file(path).unwrap();
}
