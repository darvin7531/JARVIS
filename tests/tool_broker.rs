use jarvis::tool_policy::{Broker, BrokerError, Grant, ToolRequest};
use serde_json::json;

fn request(tool: &str, resource: &str, arguments: serde_json::Value) -> ToolRequest {
    ToolRequest {
        tool: tool.into(),
        resource: resource.into(),
        arguments,
    }
}

#[test]
fn real_broker_enforces_registry_grants_and_schema_before_execution() {
    let mut broker = Broker::default();
    let grants = [Grant {
        tool: "inert".into(),
        resource: "task/item".into(),
    }];
    let args = json!({"text": "hello"});
    assert_eq!(
        broker.call(&request("inert", "task/item", args.clone()), &grants),
        Ok("hello".into())
    );
    assert_eq!(
        broker.call(&request("shell", "task/item", args.clone()), &grants),
        Err(BrokerError::UnknownTool)
    );
    assert_eq!(
        broker.call(&request("inert", "task/other", args.clone()), &grants),
        Err(BrokerError::Denied)
    );
    assert_eq!(
        broker.call(&request("inert", "", args.clone()), &grants),
        Err(BrokerError::InvalidResource)
    );
    assert_eq!(
        broker.call(&request("inert", "task/item", json!({"text": 2})), &grants),
        Err(BrokerError::InvalidArguments)
    );
    assert_eq!(
        broker.call(
            &request("inert", "task/item", json!({"text": "x", "extra": 1})),
            &grants
        ),
        Err(BrokerError::InvalidArguments)
    );
    assert_eq!(broker.executions(), 1);
    assert!(serde_json::from_value::<ToolRequest>(json!({
        "tool": "inert", "resource": "task/item", "arguments": {}, "extra": true
    }))
    .is_err());
}
