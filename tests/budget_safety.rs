use jarvis::storage::SqliteStore;
use std::{
    fs,
    io::Read,
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[test]
fn killed_process_retains_reservation() {
    interrupted_process_retains_reservation(true);
}

#[test]
fn cancelled_process_retains_reservation() {
    interrupted_process_retains_reservation(false);
}

fn interrupted_process_retains_reservation(kill: bool) {
    let root =
        std::env::temp_dir().join(format!("jarvis-budget-kill-{}-{kill}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = root.join("config.json");
    fs::write(&config, serde_json::json!({
        "models": [{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],
        "limits":{"max_tokens":5},
        "budget":{"workspace":root,"limit_tokens":100,"max_input_tokens":10}
    }).to_string()).unwrap();
    let (tx, rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        assert!(stream.read(&mut buf).unwrap() > 0);
        tx.send(()).unwrap();
        stop_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    let command = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jarvis"));
        cmd.args(["chat", "--config"]).arg(&config).arg("hello");
        cmd
    };
    let mut child = command()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    if kill {
        child.kill().unwrap();
    } else {
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    }
    child.wait().unwrap();
    stop_tx.send(()).unwrap();
    server.join().unwrap();
    let next = command().output().unwrap();
    assert_eq!(next.stderr, b"{\"error\":\"budget_exceeded\"}\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore]
fn real_ollama_budgeted_cli() {
    let root = std::env::temp_dir().join(format!("jarvis-budget-real-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let config = root.join("config.json");
    fs::write(&config, serde_json::json!({
        "models":[{"name":"jarvis-smoke","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":11434}],
        "budget":{"workspace":root,"limit_tokens":2176,"max_input_tokens":2048}
    }).to_string()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(&config)
        .arg("Say hello in one word.")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.stdout.is_empty());
    let usage: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    let input = usage["input_tokens"].as_u64().unwrap();
    let output = usage["output_tokens"].as_u64().unwrap();
    assert!(input > 0 && output > 0 && output <= 128);
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let tokens: i64 = db
        .query_row("SELECT sum(tokens) FROM inference_budget", [], |r| r.get(0))
        .unwrap();
    assert_eq!(u64::try_from(tokens).unwrap(), input + output);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore]
fn create_smoke_workspace() {
    let path = std::env::var_os("JARVIS_SMOKE_WORKSPACE").unwrap();
    drop(jarvis::storage::SqliteStore::create(std::path::Path::new(&path)).unwrap());
}

#[test]
#[ignore]
fn real_ollama_rejects_oversized_prompt() {
    let root = std::env::temp_dir().join(format!("jarvis-oversized-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    let config = root.join("config.json");
    fs::write(&config, serde_json::json!({
        "models":[{"name":"smollm2:135m-instruct-q4_0","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":11434}],
        "budget":{"workspace":root,"limit_tokens":2176,"max_input_tokens":2048}
    }).to_string()).unwrap();
    let prompt = "hello ".repeat(12000);
    let result = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(&config)
        .arg(&prompt)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(
        result.stderr,
        b"{\"usage\":null}\n{\"error\":\"inference_failed\"}\n"
    );
    fs::remove_dir_all(root).unwrap();
}
