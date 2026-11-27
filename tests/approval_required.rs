use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use serde_json::json;
use std::time::{Duration, SystemTime};

#[test]
fn contract_alone_does_not_authorize_execution() {
    let contract = TaskContract {
        scopes: vec![Scope::Read],
        tool: "inert".into(),
        action: "echo".into(),
        resource: "task/item".into(),
        denied_resources: vec![],
        data_class: DataClass::Public,
        permitted_text: "hello".into(),
        expires_at: SystemTime::now() + Duration::from_secs(60),
    };
    let request: ToolRequest = serde_json::from_value(json!({"tool":"inert", "action":"echo", "resource":"task/item", "data_class":"public", "arguments":{"text":"hello"}})).unwrap();
    let mut broker = Broker::new(contract);
    assert_eq!(broker.call(&request), Err(BrokerError::Denied));
    assert_eq!(broker.executions(), 0);
}
