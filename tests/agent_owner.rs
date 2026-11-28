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
    let owner = json!({"store":path, "task":"owner-task", "revision":1, "approval":"write-owner", "read_approval":"read-owner", "resource":"mock/owner", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"write approved value"});
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
    let mut store = SqliteStore::open(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "forged-task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store
        .issue_tool_approval(
            "forged-write",
            "forged-task",
            1,
            &req("write", "approved"),
            expires,
        )
        .unwrap();
    drop(store);
    let mut forged_owner = owner.clone();
    forged_owner["task"] = json!("forged-task");
    forged_owner["approval"] = json!("forged-write");
    std::fs::write(&owner_path, forged_owner.to_string()).unwrap();
    assert!(!run("forged").status.success());
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    std::fs::write(&owner_path, owner.to_string()).unwrap();
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
    let owner = json!({"store":path, "task":"owner-task", "revision":1, "approval":"write-owner", "read_approval":"read-owner", "resource":"mock/owner", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"write approved value", "max_steps":3, "max_failures":2});
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
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"missing", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"test"});
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
        events.iter().any(|e| e.contains("verification_incomplete")),
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
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"test", "max_steps":1});
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
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"test"});
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
fn admission_lock_timeout_fails_closed() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-lock-{}", std::process::id()));
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
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        store.agent_step("task", 1, 1, 1),
        Err(jarvis::storage::StorageError::Busy)
    ));
    db.execute_batch("ROLLBACK").unwrap();
    db.execute_batch("CREATE TRIGGER reject_admission BEFORE INSERT ON agent_runtime BEGIN SELECT RAISE(ABORT, 'secret'); END;").unwrap();
    assert!(matches!(
        store.agent_step("task", 1, 1, 1),
        Err(jarvis::storage::StorageError::OperationFailed)
    ));
    db.execute_batch("DROP TRIGGER reject_admission").unwrap();
    assert!(store.agent_step("task", 1, 1, 1).unwrap());
    assert!(!store.agent_step("task", 1, 1, 1).unwrap());
    drop(store);
    drop(db);
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
                match store.agent_step("task", 1, 1, 1) {
                    Ok(admitted) => admitted,
                    Err(jarvis::storage::StorageError::Busy) => false,
                    Err(error) => panic!("unexpected admission error: {error}"),
                }
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

#[test]
fn restored_backup_blocks_agent_before_inference() {
    let root = std::env::temp_dir().join(format!("jarvis-agent-restore-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("source");
    let snapshot = root.join("snapshot");
    let mut store = SqliteStore::create(&source).unwrap();
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
    store.backup(&snapshot).unwrap();
    drop(store);
    let mut restored = SqliteStore::open(&snapshot).unwrap();
    assert!(!restored.agent_step("task", 1, 1, 1).unwrap());
    drop(restored);
    let owner = json!({"store":snapshot, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/owner", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":(SystemTime::now() + Duration::from_secs(300)).duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"test"});
    let owner_path = root.join("owner.json");
    let response_path = root.join("response.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&response_path, r#"{"kind":"continue"}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--owner"])
        .arg(owner_path)
        .arg("--mock-response")
        .arg(response_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_steps"));
    let db = rusqlite::Connection::open(snapshot.join("jarvis.sqlite3")).unwrap();
    let state: (i64, i64) = db.query_row("SELECT (SELECT count(*) FROM agent_runtime), (SELECT count(*) FROM events WHERE event LIKE '%checkpoint%')", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(state, (0, 0));
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn owner_read_only_diagnosis_rejects_write_and_observes_seeded_state() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-diagnosis-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    for id in ["diagnose", "denied"] {
        store
            .create_task(
                &Task {
                    id: id.into(),
                    version: 1,
                    title: "diagnose".into(),
                },
                "created",
                &[],
            )
            .unwrap();
    }
    store.initialize_tool_budget(4).unwrap();
    let expiry = SystemTime::now() + Duration::from_secs(300);
    let read = ToolRequest {
        tool: "read".into(),
        action: "read".into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":""}),
    };
    for id in ["diagnose", "denied"] {
        store.issue_tool_approval(id, id, 1, &read, expiry).unwrap();
    }
    drop(store);
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    db.execute("INSERT INTO mock_state(resource,value,data_class) VALUES('mock/item','seeded','public'),('mock/other','untouched','sensitive')", []).unwrap();
    let snapshot = || {
        let mut q = db
            .prepare("SELECT resource,value,data_class FROM mock_state ORDER BY resource")
            .unwrap();
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = snapshot();
    let owner_path = path.join("owner.json");
    let proposal_path = path.join("proposal.json");
    let run = |task: &str, request: serde_json::Value| {
        let owner = json!({"store":path,"task":task,"revision":1,"approval":task,"read_approval":"unused","resource":"mock/item","data_class":"public","permitted_text":"","expected":"seeded","expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(),"context":"diagnose","mode":"read_only","max_steps":2});
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(
            &proposal_path,
            json!([{"kind":"continue"},{"kind":"tool","request":request}]).to_string(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["agent", "--owner"])
            .arg(&owner_path)
            .arg("--mock-response")
            .arg(&proposal_path)
            .output()
            .unwrap()
    };
    let denied = run(
        "denied",
        json!({"tool":"write","action":"write","resource":"mock/item","data_class":"public","arguments":{"text":"changed"}}),
    );
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("agent_tool"));
    assert_eq!(snapshot(), before);
    let success = run("diagnose", serde_json::to_value(&read).unwrap());
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&success.stdout).unwrap()["status"],
        "verified"
    );
    assert_eq!(snapshot(), before);
    drop(db);
    let store = SqliteStore::open(&path).unwrap();
    let events = store.events("diagnose").unwrap();
    assert!(events.iter().any(|e| e.contains("tool_execution_outcome")));
    assert!(events
        .iter()
        .any(|e| e.contains("checkpoint") && e.contains("verified")));
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
