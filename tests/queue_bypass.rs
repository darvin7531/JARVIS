use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use std::time::{Duration, SystemTime};

#[test]
fn direct_broker_cannot_use_another_workers_claim() {
    let path = std::env::temp_dir().join(format!("jarvis-queue-bypass-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "task".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(1).unwrap();
    let request: ToolRequest = serde_json::from_value(serde_json::json!({"tool":"write", "action":"write", "resource":"mock/item", "data_class":"public", "arguments":{"text":"changed"}})).unwrap();
    let expires = SystemTime::now() + Duration::from_secs(600);
    store
        .issue_tool_approval("approval", "task", 1, &request, expires)
        .unwrap();
    store.enqueue_task("task", "mock/item").unwrap();
    assert!(store.claim_task("task").unwrap());
    assert!(!store.agent_step("task", 1, 2, 2).unwrap());
    assert!(store.recover_task("task").unwrap());
    assert!(store.claim_task("task").unwrap());
    assert!(!store.agent_step("task", 1, 2, 2).unwrap());
    drop(store);
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    db.execute("INSERT INTO mock_state(resource,value,data_class) VALUES ('mock/item','original','public')", []).unwrap();
    drop(db);
    let contract = TaskContract {
        scopes: vec![Scope::Write],
        tool: "write".into(),
        action: "write".into(),
        resource: "mock/item".into(),
        denied_resources: vec![],
        data_class: DataClass::Public,
        permitted_text: "changed".into(),
        expires_at: expires,
    };
    let store = SqliteStore::open(&path).unwrap();
    let mut broker = Broker::with_approval(contract, store, "approval".into(), "task".into(), 1);
    let result = broker.call(&request);
    drop(broker);
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let state: (String, i64, i64, i64) = db.query_row("SELECT (SELECT value FROM mock_state WHERE resource='mock/item'), (SELECT consumed FROM tool_approvals WHERE id='approval'), (SELECT used FROM tool_budget), (SELECT count(*) FROM tool_executions)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!(state, ("original".into(), 0, 0, 0));
    assert_eq!(result, Err(BrokerError::Denied));
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}
