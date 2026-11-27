use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{Broker, DataClass, Scope, TaskContract, ToolRequest};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
