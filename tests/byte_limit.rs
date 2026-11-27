use jarvis::storage::SqliteStore;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
};

#[test]
fn t0_enforces_output_byte_boundary() {
    let path = std::env::temp_dir().join(format!("jarvis-t0-bytes-{}.json", std::process::id()));
    for limit in [1, 4, 5] {
        fs::write(
            &path,
            format!(r#"{{"models":[],"limits":{{"max_output_bytes":{limit}}}}}"#),
        )
        .unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["chat", "--config"])
            .arg(&path)
            .args(["--tier", "t0", "ping"])
            .output()
            .unwrap();
        if limit < 5 {
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
            assert_eq!(result.stderr, b"{\"error\":\"budget_exceeded\"}\n");
        } else {
            assert!(result.status.success());
            assert_eq!(result.stdout, b"pong\n");
        }
    }
    fs::remove_file(path).unwrap();
}

#[test]
fn byte_limit_accounts_terminal_usage_but_retains_unknown_reservation() {
    for terminal in [true, false] {
        let root = std::env::temp_dir().join(format!(
            "jarvis-byte-budget-{}-{terminal}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        drop(SqliteStore::create(&root).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = root.join("config.json");
        fs::write(&config, serde_json::json!({
            "models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":listener.local_addr().unwrap().port()}],
            "limits":{"max_tokens":5,"max_output_bytes":1},
            "budget":{"workspace":root,"limit_tokens":20,"max_input_tokens":10}
        }).to_string()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            assert!(stream.read(&mut buf).unwrap() > 0);
            let body = if terminal {
                "{\"response\":\"unsafe\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":11,\"eval_count\":9}\n"
            } else {
                "{\"response\":\"unsafe\",\"done\":false}\n"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let result = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["chat", "--config"])
            .arg(&config)
            .arg("hello")
            .output()
            .unwrap();
        server.join().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
        let tokens: i64 = db
            .query_row("SELECT sum(tokens) FROM inference_budget", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, if terminal { 20 } else { 15 });
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
}
