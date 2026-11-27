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
    store.initialize_tool_budget(4).unwrap();
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
    let success = run("approved");
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(result["status"], "verified");
    let mut store = SqliteStore::open(&path).unwrap();
    store
        .issue_tool_approval(
            "write-again",
            "owner-task",
            1,
            &req("write", "approved"),
            expires,
        )
        .unwrap();
    store
        .issue_tool_approval("read-again", "owner-task", 1, &req("read", ""), expires)
        .unwrap();
    drop(store);
    let mut reapproved = owner.clone();
    reapproved["approval"] = json!("write-again");
    reapproved["read_approval"] = json!("read-again");
    std::fs::write(&owner_path, reapproved.to_string()).unwrap();
    assert!(!run("approved").status.success());
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db.query_row("SELECT count(*) FROM events WHERE event LIKE '%tool_execution_outcome%' AND event LIKE '%write%'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
    drop(db);
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
    store.initialize_tool_budget(4).unwrap();
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
fn missing_read_approval_records_failed_verification_without_replay() {
    let path =
        std::env::temp_dir().join(format!("jarvis-owner-missing-read-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(4).unwrap();
    let expires = SystemTime::now() + Duration::from_secs(300);
    let req = ToolRequest {
        tool: "write".into(),
        action: "write".into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":"value"}),
    };
    store
        .issue_tool_approval("write", "task", 1, &req, expires)
        .unwrap();
    drop(store);
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"missing", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test"});
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&proposal_path, serde_json::to_string(&req).unwrap()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("agent")
            .arg("--owner")
            .arg(&owner_path)
            .arg("--mock-response")
            .arg(&proposal_path)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(!output.status.success());
    let store = SqliteStore::open(&path).unwrap();
    let events = store.events("task").unwrap();
    assert!(
        events.iter().any(|e| e.contains("verification_failed")),
        "{events:?}"
    );
    drop(store);
    assert!(!run().status.success());
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db.query_row("SELECT count(*) FROM events WHERE event LIKE '%tool_execution_outcome%' AND event LIKE '%write%'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn step_limit_survives_rerun() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-rerun-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let expires = SystemTime::now() + Duration::from_secs(300);
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test", "max_steps":1});
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&proposal_path, json!({"kind":"continue"}).to_string()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("agent")
            .arg("--owner")
            .arg(&owner_path)
            .arg("--mock-response")
            .arg(&proposal_path)
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    assert!(!run().status.success());
    let store = SqliteStore::open(&path).unwrap();
    let checkpoints = store
        .events("task")
        .unwrap()
        .into_iter()
        .filter(|e| e.contains("thinking"))
        .count();
    assert_eq!(checkpoints, 1, "rerun consumed an extra step");
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn stale_revision_cannot_checkpoint_newer_task() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-stale-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    db.execute("UPDATE tasks SET version = 2 WHERE id = 'task'", [])
        .unwrap();
    drop(db);
    let expires = SystemTime::now() + Duration::from_secs(300);
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test"});
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&proposal_path, json!({"kind":"continue"}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .arg("agent")
        .arg("--owner")
        .arg(&owner_path)
        .arg("--mock-response")
        .arg(&proposal_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let store = SqliteStore::open(&path).unwrap();
    assert!(!store
        .events("task")
        .unwrap()
        .iter()
        .any(|e| e.contains("checkpoint")));
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn concurrent_admission_and_limit_changes_are_denied() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-concurrent-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&path).unwrap();
                barrier.wait();
                store.agent_step("task", 1, 1, 1).unwrap()
            })
        })
        .collect();
    barrier.wait();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|v| *v)
            .count(),
        1
    );
    let mut store = SqliteStore::open(&path).unwrap();
    assert!(!store.agent_step("task", 1, 32, 32).unwrap());
    store.finish_agent_step("task", 1, false, false).unwrap();
    assert!(!store.agent_step("task", 1, 32, 32).unwrap());
    assert!(!store.agent_step("task", 1, 1, 1).unwrap());
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
