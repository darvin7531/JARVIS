use std::{fs, process::Command};

#[test]
fn exhausted_shared_budget_rejects_before_connecting() {
    let root = std::env::temp_dir().join(format!("jarvis-budget-red-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let config = root.join("chat.json");
    fs::write(&config, format!(r#"{{"models":[{{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":1}}],"budget":{{"workspace":"{}","limit_tokens":0,"max_input_tokens":1}}}}"#, root.display())).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(&config)
        .arg("hello")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "{\"error\":\"budget_exceeded\"}\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_processes_settle_and_retain_unknown_usage() {
    use jarvis::storage::SqliteStore;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        process::Stdio,
        sync::mpsc,
        time::Duration,
    };

    let root = std::env::temp_dir().join(format!("jarvis-budget-shared-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = root.join("chat.json");
    fs::write(&config, serde_json::json!({
        "models": [{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":port}],
        "limits": {"max_tokens":5},
        "budget": {"workspace":root,"limit_tokens":18,"max_input_tokens":10}
    }).to_string()).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        assert!(stream.read(&mut buf).unwrap() > 0);
        ready_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let body = "{\"response\":\"ok\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":2,\"eval_count\":1}\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        drop(stream);
        let (mut stream, _) = listener.accept().unwrap();
        assert!(stream.read(&mut buf).unwrap() > 0);
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
    });
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["chat", "--config"])
            .arg(&config)
            .arg("hello")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn();
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let denied = spawn().wait_with_output().unwrap();
    assert_eq!(denied.stderr, b"{\"error\":\"budget_exceeded\"}\n");
    release_tx.send(()).unwrap();
    assert!(first.wait_with_output().unwrap().status.success());
    let unknown = spawn().wait_with_output().unwrap();
    assert!(!unknown.status.success());
    server.join().unwrap();
    let denied_after_restart = spawn().wait_with_output().unwrap();
    assert_eq!(
        denied_after_restart.stderr,
        b"{\"error\":\"budget_exceeded\"}\n"
    );
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let tokens: i64 = db
        .query_row("SELECT sum(tokens) FROM inference_budget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tokens, 18);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
