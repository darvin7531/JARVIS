use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use serde_json::json;
use std::time::{Duration, SystemTime};

fn request(tool: &str, action: &str, text: &str) -> ToolRequest {
    ToolRequest { tool: tool.into(), action: action.into(), resource: "mock/item".into(), data_class: DataClass::Public, arguments: json!({"text": text}) }
}
fn contract(tool: &str, action: &str, scope: Scope, text: &str) -> TaskContract {
    TaskContract { scopes: vec![scope], tool: tool.into(), action: action.into(), resource: "mock/item".into(), denied_resources: vec![], data_class: DataClass::Public, permitted_text: text.into(), expires_at: SystemTime::now() + Duration::from_secs(120) }
}
#[test]
fn approved_mock_write_persists_and_denials_do_not_mutate() {
    let path = std::env::temp_dir().join(format!("jarvis-mock-state-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store.create_task(&Task { id: "task".into(), version: 1, title: "test".into() }, "created", &[]).unwrap();
    store.initialize_tool_budget(10).unwrap();
    let write = request("write", "write", "first");
    store.issue_tool_approval("write", "task", 1, &write, SystemTime::now() + Duration::from_secs(120)).unwrap();
    let mut broker = Broker::with_approval(contract("write", "write", Scope::Write, "first"), store, "write".into(), "task".into(), 1);
    let mut forged = request("write", "write", "forged");
    assert_eq!(broker.call(&forged), Err(BrokerError::Denied));
    forged.resource = "mock/other".into();
    assert_eq!(broker.call(&forged), Err(BrokerError::Denied));
    assert_eq!(broker.executions(), 0);
    assert_eq!(broker.call(&write), Ok("first".into()));
    assert_eq!(broker.call(&write), Err(BrokerError::Denied));
    drop(broker);
    let mut store = SqliteStore::open(&path).unwrap();
    let read = request("read", "read", "");
    store.issue_tool_approval("read", "task", 1, &read, SystemTime::now() + Duration::from_secs(120)).unwrap();
    let mut broker = Broker::with_approval(contract("read", "read", Scope::Read, ""), store, "read".into(), "task".into(), 1);
    assert_eq!(broker.call(&read), Ok("first".into()));
}
