#[allow(dead_code)]
#[path = "../src/provider.rs"]
mod provider;

use std::{fs, process::Command};

#[test]
fn missing_budget_rejects_before_connecting() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let path = std::env::temp_dir().join(format!("jarvis-no-budget-{}.json", std::process::id()));
    fs::write(&path, serde_json::json!({
        "models": [{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],
        "limits": {"timeout_seconds":1}
    }).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(&path)
        .arg("hello")
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.stderr, b"{\"error\":\"invalid_config\"}\n");
    assert!(listener.accept().is_err(), "connected without a budget");
}

#[test]
fn exhausted_shared_budget_rejects_before_connecting() {
    let root = std::env::temp_dir().join(format!("jarvis-budget-red-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let config = root.join("chat.json");
    fs::write(&config, format!(r#"{{"models":[{{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":1}}],"budget":{{"workspace":"{}","limit_tokens":0,"max_input_tokens":1}}}}"#, root.display())).unwrap();
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
        "models": [{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":port}],
        "limits": {"max_tokens":5},
        "budget": {"workspace":root,"limit_tokens":100,"max_input_tokens":10}
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

#[test]
fn budget_limit_is_persistent_and_cannot_be_raised_by_another_caller() {
    use jarvis::storage::SqliteStore;
    let root = std::env::temp_dir().join(format!("jarvis-limit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut store = SqliteStore::create(&root).unwrap();
    assert!(store.reserve_tokens(100, 90).unwrap().is_some());
    drop(store);
    let mut store = SqliteStore::open(&root).unwrap();
    assert!(store.reserve_tokens(1000, 20).is_err());
    assert!(store.reserve_tokens(100, 20).unwrap().is_none());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminal_overusage_is_retained_for_settlement() {
    use crate::provider::{Error, Ollama, Request};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::AtomicBool,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        assert!(stream.read(&mut buf).unwrap() > 0);
        let body = "{\"response\":\"unsafe\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":11,\"eval_count\":9}\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let req = Request {
        model: "test",
        prompt: "x",
        max_tokens: 8,
        max_output_bytes: 64,
        deadline: std::time::Instant::now() + Duration::from_secs(5),
    };
    let mut text = String::new();
    let result = Ollama {
        port,
        trusted_no_truncation: true,
    }
    .generate_bounded(
        &req,
        &std::sync::Arc::new(AtomicBool::new(false)),
        &mut |s| text.push_str(s),
        Some(10),
    );
    server.join().unwrap();
    match result {
        Err(Error::Incomplete(usage)) => {
            assert_eq!((usage.input_tokens, usage.output_tokens), (11, 9))
        }
        other => panic!("expected usage with rejection, got {other:?}"),
    }
    assert!(text.is_empty());
}

#[test]
fn cli_settles_overusage_and_blocks_next_request() {
    use jarvis::storage::SqliteStore;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    for (input, output) in [(2, 20), (20, 2)] {
        let root =
            std::env::temp_dir().join(format!("jarvis-overusage-{}-{input}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        drop(SqliteStore::create(&root).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = root.join("config.json");
        fs::write(&config, serde_json::json!({
            "models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],
            "limits":{"max_tokens":5},
            "budget":{"workspace":root,"limit_tokens":20,"max_input_tokens":10}
        }).to_string()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            assert!(stream.read(&mut buf).unwrap() > 0);
            let body = format!("{{\"response\":\"unsafe\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":{input},\"eval_count\":{output}}}\n");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let run = || {
            Command::new(env!("CARGO_BIN_EXE_jarvis"))
                .args(["chat", "--config"])
                .arg(&config)
                .arg("hello")
                .output()
                .unwrap()
        };
        let result = run();
        server.join().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
        let tokens: i64 = db
            .query_row("SELECT sum(tokens) FROM inference_budget", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, input + output);
        assert_eq!(run().stderr, b"{\"error\":\"budget_exceeded\"}\n");
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn competing_first_limits_cannot_both_succeed() {
    use jarvis::storage::SqliteStore;
    use std::sync::{Arc, Barrier};
    let root = std::env::temp_dir().join(format!("jarvis-limit-race-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [100, 1000]
        .into_iter()
        .map(|limit| {
            let root = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&root).unwrap();
                barrier.wait();
                store.reserve_tokens(limit, 1).is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|v| *v)
            .count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_settlement_blocks_later_requests_after_writer_unlocks() {
    use jarvis::storage::SqliteStore;
    let root = std::env::temp_dir().join(format!("jarvis-settle-lock-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut store = SqliteStore::create(&root).unwrap();
    let id = store.reserve_tokens(100, 15).unwrap().unwrap();
    let lock = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(store.settle_tokens(id, 120).is_err());
    lock.execute_batch("ROLLBACK").unwrap();
    drop(store);
    let mut reopened = SqliteStore::open(&root).unwrap();
    assert!(reopened.reserve_tokens(100, 15).unwrap().is_none());
    reopened.settle_tokens(id, 120).unwrap();
    assert!(reopened.settle_tokens(id, 0).is_err());
    assert!(reopened.reserve_tokens(100, 15).unwrap().is_none());
    drop(reopened);
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_reservations_migrate_as_unresolved_without_losing_charges() {
    use jarvis::storage::SqliteStore;
    let root = std::env::temp_dir().join(format!("jarvis-budget-migrate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("DROP TABLE tool_budget; DROP TABLE tool_approvals; ALTER TABLE inference_budget DROP COLUMN pending; INSERT INTO inference_budget (id, tokens) VALUES (1, 15), (2, 20); INSERT INTO inference_budget_limit VALUES (1, 100); PRAGMA user_version = 6;").unwrap();
    drop(db);
    let mut store = SqliteStore::open(&root).unwrap();
    assert!(store.reserve_tokens(100, 1).unwrap().is_none());
    drop(store);
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let (tokens, pending): (i64, i64) = db
        .query_row(
            "SELECT sum(tokens), sum(pending) FROM inference_budget",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((tokens, pending), (35, 2));
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
