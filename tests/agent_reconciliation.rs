use jarvis::storage::{ReconciliationOutcome, SqliteStore, Task};

#[test]
fn owner_closes_interrupted_runtime_without_losing_history() {
    let dir = std::env::temp_dir().join(format!("jarvis-reconcile-agent-{}", std::process::id()));
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "agent".into(),
        version: 1,
        title: "first".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    assert!(store.agent_step("agent", 1, 3, 2).unwrap());
    let token = store.reserve_tokens(100, 20).unwrap().unwrap();
    assert!(store
        .reconcile_agent_runtime("agent", 0, ReconciliationOutcome::Unknown, "owner record")
        .is_err());
    assert!(store
        .reconcile_agent_runtime("agent", 1, ReconciliationOutcome::Unknown, "")
        .is_err());
    store
        .reconcile_agent_runtime("agent", 1, ReconciliationOutcome::Unknown, "owner record")
        .unwrap();
    assert!(store
        .reconcile_agent_runtime("agent", 1, ReconciliationOutcome::Unknown, "owner record")
        .is_err());
    assert!(!store.agent_step("agent", 1, 3, 2).unwrap());
    assert!(store.finish_agent_step("agent", 1, false, false).is_err());
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let row: (i64, i64, i64, i64) = db.query_row("SELECT steps, failures, pending, closed FROM agent_runtime WHERE task_id = 'agent' AND revision = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!(row, (1, 0, 1, 1));
    let charge: (i64, i64) = db
        .query_row(
            "SELECT tokens, pending FROM inference_budget WHERE id = ?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(charge, (20, 1));
    let event: String = db.query_row("SELECT event FROM events WHERE task_id = 'agent' AND revision = 1 AND event LIKE '%agent_runtime_reconciliation%'", [], |r| r.get(0)).unwrap();
    let event: serde_json::Value = serde_json::from_str(&event).unwrap();
    assert_eq!(event["outcome"], "unknown");
    assert_eq!(event["evidence"], "owner record");
    let next = Task { version: 2, ..task };
    store.update_task(&next, "revised").unwrap();
    assert!(store.agent_step("agent", 2, 3, 2).unwrap());
    assert!(!store.agent_step("agent", 1, 3, 2).unwrap());
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
