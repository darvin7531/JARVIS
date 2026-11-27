use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{DataClass, ToolRequest};
use serde_json::json;
use std::{
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn owner_approval_is_independent_of_model_and_write_is_verified() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "owner-task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(2).unwrap();
    let expires = SystemTime::now() + Duration::from_secs(300);
    let req = |tool: &str, text: &str| ToolRequest {
        tool: tool.into(),
        action: tool.into(),
        resource: "mock/owner".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":text}),
    };
    store
        .issue_tool_approval(
            "write-owner",
            "owner-task",
            1,
            &req("write", "approved"),
            expires,
        )
        .unwrap();
    store
        .issue_tool_approval("read-owner", "owner-task", 1, &req("read", ""), expires)
        .unwrap();
    drop(store);
    let owner = json!({"store":path, "task":"owner-task", "revision":1, "approval":"write-owner", "read_approval":"read-owner", "resource":"mock/owner", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"write approved value"});
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    let run = |text: &str| {
        std::fs::write(
            &proposal_path,
            serde_json::to_string(&req("write", text)).unwrap(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("agent")
            .arg("--owner")
            .arg(&owner_path)
            .arg("--mock-response")
            .arg(&proposal_path)
            .output()
            .unwrap()
    };
    let denied = run("forged");
    assert!(!denied.status.success());
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    let success = run("approved");
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(result["status"], "verified");
    let store = SqliteStore::open(&path).unwrap();
    let events = store.events("owner-task").unwrap();
    assert!(events.iter().any(|event| event.contains("checkpoint")));
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn owner_loop_skips_non_action_and_stops_after_verified_write() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-loop-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "owner-task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(2).unwrap();
    let expires = SystemTime::now() + Duration::from_secs(300);
    let req = |tool: &str, text: &str| ToolRequest {
        tool: tool.into(),
        action: tool.into(),
        resource: "mock/owner".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":text}),
    };
    store
        .issue_tool_approval(
            "write-owner",
            "owner-task",
            1,
            &req("write", "approved"),
            expires,
        )
        .unwrap();
    store
        .issue_tool_approval("read-owner", "owner-task", 1, &req("read", ""), expires)
        .unwrap();
    drop(store);
    let owner = json!({"store":path, "task":"owner-task", "revision":1, "approval":"write-owner", "read_approval":"read-owner", "resource":"mock/owner", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"write approved value", "max_steps":3, "max_failures":2});
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    let run = |text: &str| {
        std::fs::write(
            &proposal_path,
            json!([{ "kind":"continue" }, { "kind":"tool", "request":req("write", text) }, { "kind":"tool", "request":req("write", "forged") }]).to_string(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("agent")
            .arg("--owner")
            .arg(&owner_path)
            .arg("--mock-response")
            .arg(&proposal_path)
            .output()
            .unwrap()
    };
    let mut limited = owner.clone();
    limited["max_steps"] = json!(1);
    std::fs::write(&owner_path, limited.to_string()).unwrap();
    let exhausted = run("approved");
    assert!(!exhausted.status.success());
    assert!(String::from_utf8_lossy(&exhausted.stderr).contains("agent_steps"));
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    let denied = run("forged");
    assert!(!denied.status.success());
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    let success = run("approved");
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(result["status"], "verified");
    let store = SqliteStore::open(&path).unwrap();
    let events = store.events("owner-task").unwrap();
    assert!(events.iter().any(|event| event.contains("checkpoint")));
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
