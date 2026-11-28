use jarvis::storage::{ReconciliationOutcome, SqliteStore, Task};
use jarvis::tool_policy::{DataClass, ToolRequest};
use serde_json::json;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn killed_observation_blocks_concurrent_and_restart_until_reconciliation() {
    let root = std::env::temp_dir().join(format!("agent-observation-crash-{}", std::process::id()));
    let mut store = SqliteStore::create(&root).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "diagnose".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(2).unwrap();
    let expiry = SystemTime::now() + Duration::from_secs(300);
    let request = ToolRequest {
        tool: "read".into(),
        action: "read".into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":""}),
    };
    store
        .issue_tool_approval("first", "task", 1, &request, expiry)
        .unwrap();
    store
        .issue_tool_approval("verify", "task", 1, &request, expiry)
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    db.execute("INSERT INTO mock_state(resource, value, data_class) VALUES ('mock/item', 'unknown seeded value', 'public')", []).unwrap();
    drop(db);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let owner = json!({"store":root,"task":"task","revision":1,"approval":"first","read_approval":"verify","resource":"mock/item","data_class":"public","permitted_text":"","expected":"","expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(),"mode":"observe","context":"Report the observed value as a claim."});
    let config = json!({"models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":port}],"limits":{"max_tokens":100},"budget":{"workspace":root,"limit_tokens":1000,"max_input_tokens":512}});
    let owner_path = root.join("owner.json");
    let config_path = root.join("config.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&config_path, config.to_string()).unwrap();
    let mut first = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--owner"])
        .arg(&owner_path)
        .arg("--config")
        .arg(&config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("inference was not dispatched: {e}"),
        }
    };
    let run_again = || {
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["agent", "--owner"])
            .arg(&owner_path)
            .arg("--config")
            .arg(&config_path)
            .output()
            .unwrap()
    };
    assert!(!run_again().status.success());
    first.kill().unwrap();
    assert!(!first.wait().unwrap().success());
    drop(stream);
    assert!(!run_again().status.success());
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let state: (i64, i64, i64, i64, i64, i64) = db.query_row("SELECT (SELECT pending FROM agent_runtime WHERE task_id='task'), (SELECT terminal FROM agent_runtime WHERE task_id='task'), (SELECT consumed FROM tool_approvals WHERE id='first'), (SELECT consumed FROM tool_approvals WHERE id='verify'), (SELECT used FROM tool_budget), (SELECT count(*) FROM inference_budget WHERE pending=1)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
    assert_eq!(state, (1, 0, 1, 0, 1, 1));
    drop(db);
    let mut store = SqliteStore::open(&root).unwrap();
    store
        .reconcile_agent_runtime(
            "task",
            1,
            ReconciliationOutcome::Unknown,
            "operator investigated crash",
        )
        .unwrap();
    drop(store);
    assert!(!run_again().status.success());
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let closed: i64 = db
        .query_row(
            "SELECT closed FROM agent_runtime WHERE task_id='task'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(closed, 1);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
