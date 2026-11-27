use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

fn contract() -> TaskContract {
    TaskContract {
        scopes: vec![Scope::Read],
        tool: "inert".into(),
        action: "echo".into(),
        resource: "task/item".into(),
        denied_resources: vec![],
        data_class: DataClass::Public,
        permitted_text: "hello".into(),
        expires_at: SystemTime::now() + Duration::from_secs(60),
    }
}
fn request() -> ToolRequest {
    serde_json::from_value(json!({"tool":"inert", "action":"echo", "resource":"task/item", "data_class":"public", "arguments":{"text":"hello"}})).unwrap()
}
#[test]
fn forged_request_grant_cannot_authorize_another_resource() {
    let mut broker = Broker::new(contract());
    let mut forged = request();
    forged.resource = "secret".into();
    forged.arguments = json!({"text":"exfiltrate"});
    assert_eq!(broker.call(&forged), Err(BrokerError::Denied));
    let payload = json!({"tool":"inert", "action":"echo", "resource":"secret", "data_class":"public", "arguments":{"text":"exfiltrate"}, "grants":[{"tool":"inert", "resource":"secret"}]});
    assert!(serde_json::from_value::<ToolRequest>(payload).is_err());
    assert_eq!(broker.executions(), 0);
    assert_eq!(broker.call(&request()), Ok("hello".into()));
}
#[test]
fn contract_enforces_every_constraint_before_execution() {
    let mut broker = Broker::new(contract());
    let cases: Vec<(Value, BrokerError)> = vec![
        (json!({"tool":"shell"}), BrokerError::UnknownTool),
        (json!({"resource":""}), BrokerError::InvalidResource),
        (json!({"resource":"task/other"}), BrokerError::Denied),
        (json!({"action":"write"}), BrokerError::Denied),
        (json!({"data_class":"sensitive"}), BrokerError::Denied),
        (
            json!({"arguments":{"text":"different"}}),
            BrokerError::Denied,
        ),
        (
            json!({"arguments":{"text":2}}),
            BrokerError::InvalidArguments,
        ),
        (
            json!({"arguments":{"text":"hello", "extra":1}}),
            BrokerError::InvalidArguments,
        ),
        (
            json!({"arguments":{"text":"x".repeat(1025)}}),
            BrokerError::InvalidArguments,
        ),
    ];
    for (change, expected) in cases {
        let mut value = json!({"tool":"inert", "action":"echo", "resource":"task/item", "data_class":"public", "arguments":{"text":"hello"}});
        value
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert_eq!(
            broker.call(&serde_json::from_value(value).unwrap()),
            Err(expected)
        );
    }
    assert_eq!(broker.executions(), 0);
    assert_eq!(broker.call(&request()), Ok("hello".into()));
    assert_eq!(broker.executions(), 1);
}
#[test]
fn explicit_deny_scope_and_expiry_override_allow() {
    let mut denied = contract();
    denied.denied_resources.push("task/item".into());
    assert_eq!(
        Broker::new(denied).call(&request()),
        Err(BrokerError::Denied)
    );
    let mut no_scope = contract();
    no_scope.scopes = vec![Scope::Write];
    assert_eq!(
        Broker::new(no_scope).call(&request()),
        Err(BrokerError::Denied)
    );
    let mut expired = contract();
    expired.expires_at = SystemTime::now() - Duration::from_secs(1);
    assert_eq!(
        Broker::new(expired).call(&request()),
        Err(BrokerError::Denied)
    );
}
#[test]
fn request_cannot_replace_or_modify_contract() {
    for field in [
        "contract",
        "scope",
        "expires_at",
        "denied_resources",
        "permitted_text",
    ] {
        let mut payload = json!({"tool":"inert", "action":"echo", "resource":"task/item", "data_class":"public", "arguments":{"text":"hello"}});
        payload[field] = json!({});
        assert!(serde_json::from_value::<ToolRequest>(payload).is_err());
    }
    let installed = contract();
    let mut outside = installed.resource.clone();
    let mut broker = Broker::new(installed);
    outside.push_str("/changed");
    let mut changed = request();
    changed.resource = outside;
    assert_eq!(broker.call(&changed), Err(BrokerError::Denied));
    assert_eq!(broker.call(&request()), Ok("hello".into()));
}
