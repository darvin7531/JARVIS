use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, BrokerError, DataClass, Scope, TaskContract, ToolRequest};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::provider::{MockProvider, Provider, Request};
use serde::Deserialize;
use std::{
    ffi::OsStr,
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

// This file is trusted owner input, not model output. The owner must separately
// provision the task, budget and exact one-shot approvals in the store.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    store: PathBuf,
    task: String,
    revision: i64,
    approval: String,
    read_approval: String,
    resource: String,
    data_class: DataClass,
    permitted_text: String,
    expected: String,
    expires_unix: u64,
    context: String,
    #[serde(default = "one")]
    max_steps: u32,
    #[serde(default = "one")]
    max_failures: u32,
}

fn one() -> u32 {
    1
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Proposal {
    Continue,
    Tool { request: ToolRequest },
}

fn bounded_file(path: &OsStr) -> Result<String, &'static str> {
    let mut data = String::new();
    crate::open_config(path)
        .map_err(|_| "agent_input")?
        .take(crate::MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut data)
        .map_err(|_| "agent_input")?;
    if data.len() as u64 > crate::MAX_CONFIG_BYTES {
        return Err("agent_input");
    }
    Ok(data)
}

pub fn run_owner(owner_file: &OsStr, response_file: &OsStr) -> Result<(), &'static str> {
    let owner: Owner =
        serde_json::from_str(&bounded_file(owner_file)?).map_err(|_| "agent_input")?;
    let expires = UNIX_EPOCH
        .checked_add(Duration::from_secs(owner.expires_unix))
        .ok_or("agent_input")?;
    if !(1..=32).contains(&owner.max_steps) || !(1..=32).contains(&owner.max_failures) {
        return Err("agent_input");
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let int = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancelled))
        .map_err(|_| "agent_signal")?;
    let term = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&cancelled))
        .map_err(|_| "agent_signal")?;
    // The process exits after this command; cancellation cannot retract a dispatched call.
    let _signals = (int, term);
    let response = bounded_file(response_file)?;
    let scripted: serde_json::Value =
        serde_json::from_str(&response).map_err(|_| "agent_inference")?;
    let responses = match scripted {
        serde_json::Value::Array(items) => items,
        item => vec![item],
    };
    let mut proposal = None;
    let mut failures = 0;
    for item in responses {
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled");
        }
        let mut store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
        if !store
            .agent_step(
                &owner.task,
                owner.revision,
                owner.max_steps,
                owner.max_failures,
            )
            .map_err(|_| "agent_storage")?
        {
            return Err("agent_steps");
        }
        let text = item.to_string();
        let provider = MockProvider::new(&text);
        let request = Request {
            model: "mock",
            prompt: &owner.context,
            max_tokens: 1024,
            max_output_bytes: 65536,
            deadline: Instant::now() + Duration::from_secs(10),
        };
        let mut output = String::new();
        provider
            .generate(&request, &cancelled, &mut |chunk| output.push_str(chunk))
            .map_err(|_| {
                if cancelled.load(Ordering::Relaxed) {
                    "cancelled"
                } else {
                    "agent_inference"
                }
            })?;
        // Legacy single-request input remains supported but never supplies authority.
        let parsed = serde_json::from_str::<serde_json::Value>(&output)
            .map_err(|_| ())
            .and_then(|value| {
                if value.get("kind").and_then(|v| v.as_str()) == Some("continue")
                    && value.as_object().is_none_or(|object| object.len() != 1)
                {
                    return Err(());
                }
                serde_json::from_value::<Proposal>(value.clone())
                    .or_else(|_| {
                        serde_json::from_value::<ToolRequest>(value)
                            .map(|request| Proposal::Tool { request })
                    })
                    .map_err(|_| ())
            });
        store
            .finish_agent_step(
                &owner.task,
                owner.revision,
                parsed.is_err(),
                matches!(&parsed, Ok(Proposal::Tool { .. })),
            )
            .map_err(|_| "agent_storage")?;
        match parsed {
            Ok(Proposal::Tool { request }) => {
                proposal = Some(request);
                break;
            }
            Ok(Proposal::Continue) => {}
            Err(_) => {
                failures += 1;
                if failures >= owner.max_failures {
                    return Err("agent_failures");
                }
            }
        }
    }
    let proposal = proposal.ok_or("agent_steps")?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled");
    }
    let contract = TaskContract {
        scopes: vec![Scope::Write],
        tool: "write".into(),
        action: "write".into(),
        resource: owner.resource.clone(),
        denied_resources: vec![],
        data_class: owner.data_class,
        permitted_text: owner.permitted_text,
        expires_at: expires,
    };
    let store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
    let mut broker = Broker::with_approval(
        contract,
        store,
        owner.approval,
        owner.task.clone(),
        owner.revision,
    );
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled");
    }
    // The test installs a narrow SQLite fault only after schema validation.
    #[cfg(test)]
    let fault_db = if FAIL_OUTCOME.with(|flag| flag.get()) {
        let db = rusqlite::Connection::open(owner.store.join("jarvis.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER test_reject_outcome BEFORE INSERT ON events WHEN NEW.event LIKE '%tool_execution_outcome%' BEGIN SELECT RAISE(ABORT, 'test outcome failure'); END;").unwrap();
        Some(db)
    } else {
        None
    };
    let outcome = broker.call_cancellable(&proposal, &cancelled);
    drop(broker);
    #[cfg(test)]
    if let Some(db) = fault_db {
        db.execute_batch("DROP TRIGGER test_reject_outcome")
            .unwrap();
    }
    if outcome == Err(BrokerError::OutcomeUncertain) {
        return Err("agent_outcome_uncertain");
    }
    if outcome == Err(BrokerError::ReconciliationRequired) {
        return Err("agent_reconciliation_required");
    }
    if outcome == Err(BrokerError::Cancelled) {
        return Err("cancelled");
    }
    if outcome.is_err() {
        let mut store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
        store
            .append_event_at_revision(
                &owner.task,
                owner.revision,
                r#"{"type":"checkpoint","status":"denied_or_failed"}"#,
            )
            .map_err(|_| "agent_storage")?;
        return Err("agent_tool");
    }
    // Independent broker read, with a separate owner-issued approval. The write
    // result is not used as verification and the store is reopened first.
    let read = ToolRequest {
        tool: "read".into(),
        action: "read".into(),
        resource: owner.resource.clone(),
        data_class: owner.data_class,
        arguments: serde_json::json!({"text":""}),
    };
    let read_contract = TaskContract {
        scopes: vec![Scope::Read],
        tool: "read".into(),
        action: "read".into(),
        resource: owner.resource,
        denied_resources: vec![],
        data_class: owner.data_class,
        permitted_text: String::new(),
        expires_at: expires,
    };
    let store = SqliteStore::open(&owner.store).map_err(|_| "agent_verification_incomplete")?;
    let mut broker = Broker::with_approval(
        read_contract,
        store,
        owner.read_approval,
        owner.task.clone(),
        owner.revision,
    );
    #[cfg(test)]
    if FAIL_VERIFICATION.with(|flag| flag.get()) {
        let db = rusqlite::Connection::open(owner.store.join("jarvis.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER test_reject_read_outcome BEFORE INSERT ON events WHEN NEW.event LIKE '%tool_execution_outcome%' BEGIN SELECT RAISE(ABORT, 'test outcome failure'); END; CREATE TRIGGER test_reject_checkpoint BEFORE INSERT ON events WHEN NEW.event LIKE '%checkpoint%' BEGIN SELECT RAISE(ABORT, 'test checkpoint failure'); END;").unwrap();
    }
    let observed = broker.call_cancellable(&read, &cancelled);
    drop(broker);
    let status = match observed {
        Ok(value) if value == owner.expected => "verified",
        Ok(_) => "verification_failed",
        Err(BrokerError::OutcomeUncertain | BrokerError::ReconciliationRequired) => {
            "verification_uncertain"
        }
        Err(_) => "verification_incomplete",
    };
    let storage_error = if status == "verification_uncertain" {
        "agent_verification_uncertain"
    } else {
        "agent_verification_incomplete"
    };
    let mut store = SqliteStore::open(&owner.store).map_err(|_| storage_error)?;
    store
        .append_event_at_revision(
            &owner.task,
            owner.revision,
            &serde_json::json!({"type":"checkpoint", "status":status}).to_string(),
        )
        .map_err(|_| storage_error)?;
    if status == "verification_uncertain" {
        return Err("agent_verification_uncertain");
    }
    if status == "verification_incomplete" {
        return Err("agent_verification_incomplete");
    }
    if status != "verified" {
        return Err("agent_verify");
    }
    println!("{}", serde_json::json!({"status":status}));
    Ok(())
}

// This tracer is deliberately limited to an empty, isolated mock resource.
// ponytail: one read-only scenario; add owner-supplied contracts for other tasks.
pub fn run() -> Result<(), &'static str> {
    let now = SystemTime::now();
    let path = std::env::temp_dir().join(format!(
        "jarvis-agent-{}-{}",
        std::process::id(),
        now.duration_since(UNIX_EPOCH)
            .map_err(|_| "agent_clock")?
            .as_nanos()
    ));
    let mut store = SqliteStore::create(&path).map_err(|_| "agent_storage")?;
    store
        .create_task(
            &Task {
                id: "diagnose".into(),
                version: 1,
                title: "read-only diagnosis".into(),
            },
            "created",
            &[],
        )
        .map_err(|_| "agent_storage")?;
    store
        .initialize_tool_budget(1)
        .map_err(|_| "agent_storage")?;
    let expires = now + Duration::from_secs(60);
    let contract = TaskContract {
        scopes: vec![Scope::Read],
        tool: "read".into(),
        action: "read".into(),
        resource: "mock/item".into(),
        denied_resources: vec![],
        data_class: DataClass::Public,
        permitted_text: String::new(),
        expires_at: expires,
    };
    // Owner authorizes this exact request, independently of the inference output.
    let authorized: ToolRequest = serde_json::from_str(r#"{"tool":"read","action":"read","resource":"mock/item","data_class":"public","arguments":{"text":""}}"#).map_err(|_| "agent_internal")?;
    store
        .issue_tool_approval("read", "diagnose", 1, &authorized, expires)
        .map_err(|_| "agent_storage")?;
    // Deterministic structured mock inference; it cannot create or widen authority.
    let proposal: ToolRequest = serde_json::from_str(r#"{"tool":"read","action":"read","resource":"mock/item","data_class":"public","arguments":{"text":""}}"#).map_err(|_| "agent_inference")?;
    let mut broker = Broker::with_approval(contract, store, "read".into(), "diagnose".into(), 1);
    let observed = broker.call(&proposal).map_err(|_| "agent_tool")?;
    drop(broker);
    // T0 reads the backing state independently; never trust the tool's success alone.
    let db = rusqlite::Connection::open_with_flags(
        path.join("jarvis.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "agent_verify")?;
    let count: i64 = db
        .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
        .map_err(|_| "agent_verify")?;
    if count != 0 || !observed.is_empty() {
        return Err("agent_verify");
    }
    drop(db);
    let mut store = SqliteStore::open(&path).map_err(|_| "agent_storage")?;
    store
        .append_event(
            "diagnose",
            r#"{"type":"checkpoint","status":"verified","changes":0}"#,
        )
        .map_err(|_| "agent_storage")?;
    println!(
        "{}",
        serde_json::json!({"status":"verified","changes":0,"checkpoint":path.to_string_lossy()})
    );
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_OUTCOME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_VERIFICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verification_uncertainty_survives_checkpoint_store_failure() {
        let path =
            std::env::temp_dir().join(format!("agent-read-uncertain-{}", std::process::id()));
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
        let request = |tool: &str, text: &str| ToolRequest {
            tool: tool.into(),
            action: tool.into(),
            resource: "mock/item".into(),
            data_class: DataClass::Public,
            arguments: json!({"text":text}),
        };
        store
            .issue_tool_approval("write", "task", 1, &request("write", "value"), expires)
            .unwrap();
        store
            .issue_tool_approval("read", "task", 1, &request("read", ""), expires)
            .unwrap();
        drop(store);
        let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test"});
        let owner_path = path.join("owner.json");
        let proposal_path = path.join("proposal.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(
            &proposal_path,
            serde_json::to_string(&request("write", "value")).unwrap(),
        )
        .unwrap();
        FAIL_VERIFICATION.with(|flag| flag.set(true));
        let result = run_owner(owner_path.as_os_str(), proposal_path.as_os_str());
        FAIL_VERIFICATION.with(|flag| flag.set(false));
        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        let state: (String, i64, i64, i64) = db.query_row("SELECT (SELECT value FROM mock_state WHERE resource = 'mock/item'), (SELECT pending FROM tool_executions WHERE approval_id = 'read'), (SELECT consumed FROM tool_approvals WHERE id = 'read'), (SELECT used FROM tool_budget)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(state, ("value".into(), 1, 1, 2));
        assert_eq!(result, Err("agent_verification_uncertain"));
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn existing_pending_tool_requires_agent_reconciliation() {
        let path =
            std::env::temp_dir().join(format!("agent-existing-pending-{}", std::process::id()));
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
        let request = ToolRequest {
            tool: "write".into(),
            action: "write".into(),
            resource: "mock/item".into(),
            data_class: DataClass::Public,
            arguments: json!({"text":"value"}),
        };
        for id in ["pending", "write"] {
            store
                .issue_tool_approval(id, "task", 1, &request, expires)
                .unwrap();
        }
        drop(store);
        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        db.execute_batch("UPDATE tool_approvals SET consumed = 1 WHERE id = 'pending'; UPDATE tool_budget SET used = 1; INSERT INTO tool_executions (approval_id, pending) VALUES ('pending', 1);").unwrap();
        drop(db);
        let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"missing", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test"});
        let owner_path = path.join("owner.json");
        let proposal_path = path.join("proposal.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(&proposal_path, serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(
            run_owner(owner_path.as_os_str(), proposal_path.as_os_str()),
            Err("agent_reconciliation_required")
        );
        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        let state: (i64, i64, i64, i64) = db.query_row("SELECT (SELECT pending FROM tool_executions WHERE approval_id = 'pending'), (SELECT consumed FROM tool_approvals WHERE id = 'write'), (SELECT used FROM tool_budget), (SELECT count(*) FROM mock_state)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(state, (1, 0, 1, 0));
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn post_dispatch_outcome_failure_is_uncertain_and_never_replayed() {
        let path = std::env::temp_dir().join(format!("agent-post-dispatch-{}", std::process::id()));
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
        let request = ToolRequest {
            tool: "write".into(),
            action: "write".into(),
            resource: "mock/item".into(),
            data_class: DataClass::Public,
            arguments: json!({"text":"value"}),
        };
        store
            .issue_tool_approval("write", "task", 1, &request, expires)
            .unwrap();
        drop(store);
        let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"missing", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":expires.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"test"});
        let owner_path = path.join("owner.json");
        let proposal_path = path.join("proposal.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(&proposal_path, serde_json::to_string(&request).unwrap()).unwrap();
        FAIL_OUTCOME.with(|flag| flag.set(true));
        let result = run_owner(owner_path.as_os_str(), proposal_path.as_os_str());
        FAIL_OUTCOME.with(|flag| flag.set(false));
        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        let value: String = db
            .query_row(
                "SELECT value FROM mock_state WHERE resource = 'mock/item'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, "value");
        let pending: i64 = db
            .query_row(
                "SELECT pending FROM tool_executions WHERE approval_id = 'write'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
        assert_eq!(result, Err("agent_outcome_uncertain"));
        drop(db);
        assert!(run_owner(owner_path.as_os_str(), proposal_path.as_os_str()).is_err());
        let store = SqliteStore::open(&path).unwrap();
        let events = store.events("task").unwrap();
        assert!(!events
            .iter()
            .any(|e| e.contains("denied_or_failed") || e.contains("verified")));
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
