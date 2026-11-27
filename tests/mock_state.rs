use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use serde_json::json;
use std::time::{Duration, SystemTime};

fn request(tool: &str, action: &str, text: &str) -> ToolRequest {
    ToolRequest {
        tool: tool.into(),
        action: action.into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text": text}),
    }
}
fn contract(tool: &str, action: &str, scope: Scope, text: &str) -> TaskContract {
    TaskContract {
        scopes: vec![scope],
        tool: tool.into(),
        action: action.into(),
        resource: "mock/item".into(),
        denied_resources: vec![],
        data_class: DataClass::Public,
        permitted_text: text.into(),
        expires_at: SystemTime::now() + Duration::from_secs(120),
    }
}
#[test]
fn approved_mock_write_persists_and_denials_do_not_mutate() {
    let path = std::env::temp_dir().join(format!("jarvis-mock-state-{}", std::process::id()));
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
    store.initialize_tool_budget(10).unwrap();
    let write = request("write", "write", "first");
    store
        .issue_tool_approval(
            "write",
            "task",
            1,
            &write,
            SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
    let mut no_approval = Broker::new(contract("write", "write", Scope::Write, "first"));
    assert_eq!(no_approval.call(&write), Err(BrokerError::Denied));
    let mut mismatch = Broker::with_approval(
        contract("write", "write", Scope::Write, "forged"),
        store,
        "write".into(),
        "task".into(),
        1,
    );
    assert_eq!(
        mismatch.call(&request("write", "write", "forged")),
        Err(BrokerError::Denied)
    );
    assert_eq!(mismatch.executions(), 0);
    drop(mismatch);
    let mut store = SqliteStore::open(&path).unwrap();
    let no_scope = contract("write", "write", Scope::Read, "first");
    let mut denied_resource = contract("write", "write", Scope::Write, "first");
    denied_resource.denied_resources.push("mock/item".into());
    let mut expired = contract("write", "write", Scope::Write, "first");
    expired.expires_at = SystemTime::now() - Duration::from_secs(1);
    for denied in [no_scope, denied_resource, expired] {
        let mut denied_broker =
            Broker::with_approval(denied, store, "write".into(), "task".into(), 1);
        assert_eq!(denied_broker.call(&write), Err(BrokerError::Denied));
        assert_eq!(denied_broker.executions(), 0);
        drop(denied_broker);
        store = SqliteStore::open(&path).unwrap();
    }
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    let mut broker = Broker::with_approval(
        contract("write", "write", Scope::Write, "first"),
        store,
        "write".into(),
        "task".into(),
        1,
    );
    let mut forged = request("write", "write", "forged");
    assert_eq!(broker.call(&forged), Err(BrokerError::Denied));
    forged.resource = "mock/other".into();
    assert_eq!(broker.call(&forged), Err(BrokerError::Denied));
    let mut wrong_class = request("write", "write", "first");
    wrong_class.data_class = DataClass::Sensitive;
    assert_eq!(broker.call(&wrong_class), Err(BrokerError::Denied));
    assert_eq!(broker.executions(), 0);
    assert_eq!(broker.call(&write), Ok("first".into()));
    assert_eq!(broker.call(&write), Err(BrokerError::Denied));
    drop(broker);
    let mut store = SqliteStore::open(&path).unwrap();
    let read = request("read", "read", "");
    store
        .issue_tool_approval(
            "read",
            "task",
            1,
            &read,
            SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
    let mut broker = Broker::with_approval(
        contract("read", "read", Scope::Read, ""),
        store,
        "read".into(),
        "task".into(),
        1,
    );
    assert_eq!(broker.call(&read), Ok("first".into()));
}

fn sensitive_then_public(tool: &str) {
    let path = std::env::temp_dir().join(format!("jarvis-class-{tool}-{}", std::process::id()));
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
    store.initialize_tool_budget(3).unwrap();
    let mut secret = request("write", "write", "secret");
    secret.data_class = DataClass::Sensitive;
    let mut secret_contract = contract("write", "write", Scope::Write, "secret");
    secret_contract.data_class = DataClass::Sensitive;
    store
        .issue_tool_approval(
            "secret",
            "task",
            1,
            &secret,
            SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
    let mut broker =
        Broker::with_approval(secret_contract, store, "secret".into(), "task".into(), 1);
    assert_eq!(broker.call(&secret), Ok("secret".into()));
    drop(broker);
    let mut store = SqliteStore::open(&path).unwrap();
    let (scope, text) = if tool == "read" {
        (Scope::Read, "")
    } else {
        (Scope::Write, "replacement")
    };
    let public = request(tool, tool, text);
    store
        .issue_tool_approval(
            "public",
            "task",
            1,
            &public,
            SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
    let mut broker = Broker::with_approval(
        contract(tool, tool, scope, text),
        store,
        "public".into(),
        "task".into(),
        1,
    );
    assert!(
        broker.call(&public).is_err(),
        "public {tool} must be denied"
    );
    drop(broker);
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let value: String = db
        .query_row(
            "SELECT value FROM mock_state WHERE resource = 'mock/item'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "secret");
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn sensitive_value_cannot_be_read_as_public() {
    sensitive_then_public("read");
}

#[test]
fn sensitive_value_cannot_be_overwritten_as_public() {
    sensitive_then_public("write");
}
