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
fn approved(contract: TaskContract, request: ToolRequest) -> Broker {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "jarvis-approval-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut store = jarvis::storage::SqliteStore::create(&path).unwrap();
    store
        .issue_tool_approval(
            "one",
            "task",
            1,
            &request,
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
    Broker::with_approval(contract, store, "one".into(), "task".into(), 1)
}
#[test]
fn forged_request_grant_cannot_authorize_another_resource() {
    let mut broker = approved(contract(), request());
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
    let mut broker = approved(contract(), request());
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
        approved(denied, request()).call(&request()),
        Err(BrokerError::Denied)
    );
    let mut no_scope = contract();
    no_scope.scopes = vec![Scope::Write];
    assert_eq!(
        approved(no_scope, request()).call(&request()),
        Err(BrokerError::Denied)
    );
    let mut expired = contract();
    expired.expires_at = SystemTime::now() - Duration::from_secs(1);
    assert_eq!(
        approved(expired, request()).call(&request()),
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
        "approval",
    ] {
        let mut payload = json!({"tool":"inert", "action":"echo", "resource":"task/item", "data_class":"public", "arguments":{"text":"hello"}});
        payload[field] = json!({});
        assert!(serde_json::from_value::<ToolRequest>(payload).is_err());
    }
    let installed = contract();
    let mut outside = installed.resource.clone();
    let mut broker = approved(installed, request());
    outside.push_str("/changed");
    let mut changed = request();
    changed.resource = outside;
    assert_eq!(broker.call(&changed), Err(BrokerError::Denied));
    assert_eq!(broker.call(&request()), Ok("hello".into()));
}

#[test]
fn approval_is_exact_and_single_use() {
    let mut different = request();
    different.arguments = json!({"text":"different"});
    let mut mismatch = approved(contract(), different);
    assert_eq!(mismatch.call(&request()), Err(BrokerError::Denied));
    assert_eq!(mismatch.executions(), 0);

    let mut broker = approved(contract(), request());
    assert_eq!(broker.call(&request()), Ok("hello".into()));
    assert_eq!(broker.call(&request()), Err(BrokerError::Denied));
    assert_eq!(broker.executions(), 1);
}

#[test]
fn approval_cannot_be_replayed_after_broker_recreation() {
    let path = std::env::temp_dir().join(format!("jarvis-approval-reopen-{}", std::process::id()));
    let mut store = jarvis::storage::SqliteStore::create(&path).unwrap();
    store
        .issue_tool_approval(
            "one",
            "task",
            1,
            &request(),
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
    let mut first = Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
    assert_eq!(first.call(&request()), Ok("hello".into()));
    drop(first);
    let store = jarvis::storage::SqliteStore::open(&path).unwrap();
    let mut reopened = Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
    assert_eq!(reopened.call(&request()), Err(BrokerError::Denied));
}

#[test]
fn approval_race_and_identity_are_enforced() {
    use std::sync::{Arc, Barrier};
    let path = std::env::temp_dir().join(format!("jarvis-approval-race-{}", std::process::id()));
    let mut store = jarvis::storage::SqliteStore::create(&path).unwrap();
    store
        .issue_tool_approval(
            "one",
            "task",
            1,
            &request(),
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
    drop(store);
    for (task, revision) in [("other", 1), ("task", 2)] {
        let store = jarvis::storage::SqliteStore::open(&path).unwrap();
        let mut broker =
            Broker::with_approval(contract(), store, "one".into(), task.into(), revision);
        assert_eq!(broker.call(&request()), Err(BrokerError::Denied));
    }
    let stores: Vec<_> = (0..2)
        .map(|_| jarvis::storage::SqliteStore::open(&path).unwrap())
        .collect();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = stores
        .into_iter()
        .map(|store| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut broker =
                    Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
                barrier.wait();
                broker.call(&request()).is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
}
