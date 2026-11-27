use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, DataClass, Scope, TaskContract, ToolRequest};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::provider::{MockProvider, Provider, Request};
use serde::Deserialize;
use std::{
    ffi::OsStr,
    io::Read,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc},
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
    let scripted: serde_json::Value = serde_json::from_str(&response).map_err(|_| "agent_inference")?;
    let responses = match scripted {
        serde_json::Value::Array(items) => items,
        item => vec![item],
    };
    let mut proposal = None;
    let mut failures = 0;
    for (index, item) in responses.into_iter().enumerate() {
        if index >= owner.max_steps as usize {
            break;
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled");
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
            .map_err(|_| if cancelled.load(Ordering::Relaxed) { "cancelled" } else { "agent_inference" })?;
        // Legacy single-request input remains supported but never supplies authority.
        let parsed = serde_json::from_str::<Proposal>(&output).or_else(|_| {
            serde_json::from_str::<ToolRequest>(&output).map(|request| Proposal::Tool { request })
        });
        match parsed {
            Ok(Proposal::Tool { request }) => {
                proposal = Some(request);
                break;
            }
            Ok(Proposal::Continue) => {},
            Err(_) => {
                failures += 1;
                if failures >= owner.max_failures {
                    return Err("agent_failures");
                }
            }
        }
        let mut store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
        store.append_event(&owner.task, &serde_json::json!({"type":"checkpoint", "status":"thinking", "steps":index + 1, "failures":failures}).to_string()).map_err(|_| "agent_storage")?;
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
    let outcome = broker.call(&proposal);
    drop(broker);
    if outcome.is_err() {
        let mut store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
        store
            .append_event(
                &owner.task,
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
    let store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
    let mut broker = Broker::with_approval(
        read_contract,
        store,
        owner.read_approval,
        owner.task.clone(),
        owner.revision,
    );
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled");
    }
    let observed = broker.call(&read).map_err(|_| "agent_verify")?;
    drop(broker);
    let mut store = SqliteStore::open(&owner.store).map_err(|_| "agent_storage")?;
    let status = if observed == owner.expected {
        "verified"
    } else {
        "verification_failed"
    };
    store
        .append_event(
            &owner.task,
            &serde_json::json!({"type":"checkpoint", "status":status}).to_string(),
        )
        .map_err(|_| "agent_storage")?;
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
