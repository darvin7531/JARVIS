use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Read,
    Write,
}

// Legacy scope check; it is not the broker's tool registry.
pub fn permits(tool: &str, allowed: &[Scope]) -> bool {
    match tool {
        "read" | "inert" => allowed.contains(&Scope::Read),
        "write" => allowed.contains(&Scope::Write),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Public,
    Sensitive,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    pub tool: String,
    pub action: String,
    pub resource: String,
    pub data_class: DataClass,
    pub arguments: Value,
}

// Constructed and installed by the trusted owner, never deserialized from a request.
// Ownership is moved into the broker; there is no contract mutation API.
pub struct TaskContract {
    pub scopes: Vec<Scope>,
    pub tool: String,
    pub action: String,
    pub resource: String,
    pub denied_resources: Vec<String>,
    pub data_class: DataClass,
    pub permitted_text: String,
    pub expires_at: SystemTime,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrokerError {
    UnknownTool,
    InvalidResource,
    Denied,
    InvalidArguments,
    Cancelled,
    ReconciliationRequired,
    OutcomeUncertain,
}

const INERT_TEXT_MAX_BYTES: usize = 1024;

/// Argument schema; maxUtf8Bytes counts UTF-8 bytes, not Unicode characters.
pub fn tool_schema(name: &str) -> Option<Value> {
    match name {
        "inert" | "read" | "write" => Some(serde_json::json!({
            "type": "object",
            "required": ["text"],
            "additionalProperties": false,
            "properties": {"text": {"type": "string", "maxUtf8Bytes": if name == "read" { 0 } else { INERT_TEXT_MAX_BYTES }}}
        })),
        _ => None,
    }
}

fn inert_text(arguments: &Value) -> Option<&str> {
    let object = arguments.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let text = object.get("text")?.as_str()?;
    (text.len() <= INERT_TEXT_MAX_BYTES).then_some(text)
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisteredTool {
    Inert { text: String },
    Read { text: String },
    Write { text: String },
}

pub fn validate_tool(name: &str, arguments: &Value) -> Result<RegisteredTool, BrokerError> {
    match name {
        "inert" => Ok(RegisteredTool::Inert {
            text: inert_text(arguments)
                .ok_or(BrokerError::InvalidArguments)?
                .to_owned(),
        }),
        "read" => match inert_text(arguments) {
            Some("") => Ok(RegisteredTool::Read {
                text: String::new(),
            }),
            _ => Err(BrokerError::InvalidArguments),
        },
        "write" => Ok(RegisteredTool::Write {
            text: inert_text(arguments)
                .ok_or(BrokerError::InvalidArguments)?
                .to_owned(),
        }),
        _ => Err(BrokerError::UnknownTool),
    }
}

pub(crate) fn valid_action(tool: &str, action: &str) -> bool {
    matches!(
        (tool, action),
        ("inert", "echo") | ("read", "read") | ("write", "write")
    )
}

// The owner must keep broker construction outside the untrusted request path.
// This in-process boundary does not authenticate callers or isolate processes.
pub struct Broker {
    contract: TaskContract,
    approval: Option<(crate::storage::SqliteStore, String, String, i64)>,
    executions: usize,
}

impl Broker {
    pub fn new(contract: TaskContract) -> Self {
        Self {
            contract,
            approval: None,
            executions: 0,
        }
    }

    // The owner installs the trusted task identity and selects an issued approval.
    pub fn with_approval(
        contract: TaskContract,
        store: crate::storage::SqliteStore,
        id: String,
        task: String,
        revision: i64,
    ) -> Self {
        Self {
            contract,
            approval: Some((store, id, task, revision)),
            executions: 0,
        }
    }

    pub fn executions(&self) -> usize {
        self.executions
    }

    pub fn call(&mut self, request: &ToolRequest) -> Result<String, BrokerError> {
        self.call_cancellable(request, &std::sync::atomic::AtomicBool::new(false))
    }

    pub fn call_cancellable(
        &mut self,
        request: &ToolRequest,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<String, BrokerError> {
        self.call_inner(request, cancelled)
            .map(Option::unwrap_or_default)
    }

    /// A read preserves absence separately from an existing empty value.
    pub fn read_cancellable(
        &mut self,
        request: &ToolRequest,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<String>, BrokerError> {
        if request.tool != "read" {
            return Err(BrokerError::Denied);
        }
        self.call_inner(request, cancelled)
    }

    fn call_inner(
        &mut self,
        request: &ToolRequest,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<String>, BrokerError> {
        use std::sync::atomic::Ordering;
        if cancelled.load(Ordering::Relaxed) {
            return Err(BrokerError::Cancelled);
        }
        let tool = validate_tool(&request.tool, &request.arguments)?;
        if request.resource.is_empty() || request.resource.len() > 128 {
            return Err(BrokerError::InvalidResource);
        }
        let contract = &self.contract;
        if SystemTime::now() >= contract.expires_at
            || !contract.scopes.contains(&if request.tool == "write" {
                Scope::Write
            } else {
                Scope::Read
            })
            || request.tool != contract.tool
            || !valid_action(&request.tool, &request.action)
            || request.action != contract.action
            || request.resource != contract.resource
            || contract.denied_resources.contains(&request.resource)
            || request.data_class != contract.data_class
        {
            return Err(BrokerError::Denied);
        }
        let text = match &tool {
            RegisteredTool::Inert { text }
            | RegisteredTool::Read { text }
            | RegisteredTool::Write { text } => text,
        };
        if text != &contract.permitted_text {
            return Err(BrokerError::Denied);
        }
        let (store, id, task, revision) = self.approval.as_mut().ok_or(BrokerError::Denied)?;
        match store.consume_tool_approval(
            id,
            task,
            *revision,
            request,
            contract.expires_at,
            cancelled,
        ) {
            Ok(Some(true)) => {}
            Ok(None) => return Err(BrokerError::ReconciliationRequired),
            Ok(Some(false)) if cancelled.load(Ordering::Relaxed) => {
                return Err(BrokerError::Cancelled)
            }
            _ => return Err(BrokerError::Denied),
        }
        // A consumed approval remains pending on a failed freshness check; the
        // owner must reconcile the durable intent before any further call.
        if !store.tool_approval_is_fresh(id).unwrap_or(false)
            || SystemTime::now() >= contract.expires_at
            || cancelled.load(Ordering::Relaxed)
        {
            return Err(BrokerError::ReconciliationRequired);
        }
        self.executions += 1;
        let result = match tool {
            RegisteredTool::Inert { text } => Some(text),
            RegisteredTool::Read { .. } => store
                .mock_read(&request.resource, request.data_class)
                .map_err(|_| BrokerError::OutcomeUncertain)?,
            RegisteredTool::Write { text } => {
                store
                    .mock_write(&request.resource, &text, request.data_class)
                    .map_err(|_| BrokerError::OutcomeUncertain)?;
                Some(text)
            }
        };
        #[cfg(test)]
        tests::pause_after_write(cancelled);
        store
            .record_tool_outcome(id, task, *revision)
            .map_err(|_| BrokerError::OutcomeUncertain)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ReconciliationOutcome, SqliteStore, Task};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    pub(super) fn pause_after_write(cancelled: &std::sync::atomic::AtomicBool) {
        if std::env::var_os("JARVIS_CRASH_CHILD").is_some()
            || std::env::var_os("JARVIS_CANCEL_CHILD").is_some()
        {
            std::io::stderr().write_all(b"!").unwrap();
            std::io::stderr().flush().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            if std::env::var_os("JARVIS_CANCEL_CHILD").is_some() {
                cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    fn request() -> ToolRequest {
        serde_json::from_value(serde_json::json!({
            "tool": "write", "action": "write", "resource": "mock/item",
            "data_class": "public", "arguments": {"text": "written"}
        }))
        .unwrap()
    }

    fn contract() -> TaskContract {
        TaskContract {
            scopes: vec![Scope::Write],
            tool: "write".into(),
            action: "write".into(),
            resource: "mock/item".into(),
            denied_resources: vec![],
            data_class: DataClass::Public,
            permitted_text: "written".into(),
            expires_at: SystemTime::now() + Duration::from_secs(600),
        }
    }

    #[test]
    fn cancellation_after_write_records_outcome_and_prevents_replay() {
        use std::sync::atomic::AtomicBool;
        if let Some(path) = std::env::var_os("JARVIS_CANCEL_CHILD") {
            let store = SqliteStore::open(std::path::Path::new(&path)).unwrap();
            let mut broker =
                Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
            let cancelled = AtomicBool::new(false);
            assert_eq!(
                broker.call_cancellable(&request(), &cancelled),
                Ok("written".into())
            );
            assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
            return;
        }
        let path = std::env::temp_dir().join(format!("jarvis-cancel-write-{}", std::process::id()));
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
        store.initialize_tool_budget(2).unwrap();
        store
            .issue_tool_approval(
                "one",
                "task",
                1,
                &request(),
                SystemTime::now() + Duration::from_secs(600),
            )
            .unwrap();
        drop(store);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tool_policy::tests::cancellation_after_write_records_outcome_and_prevents_replay",
                "--nocapture",
            ])
            .env("JARVIS_CANCEL_CHILD", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut poll = libc::pollfd {
            fd: stderr.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 30_000) } != 1 {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not reach post-write boundary");
        }
        let mut marker = [0];
        stderr.read_exact(&mut marker).unwrap();
        assert_eq!(&marker, b"!");
        child.stdin.take().unwrap().write_all(b"!").unwrap();
        assert!(child.wait().unwrap().success());
        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        let state: (String, i64, i64, i64, i64) = db.query_row("SELECT (SELECT value FROM mock_state WHERE resource = 'mock/item'), (SELECT consumed FROM tool_approvals WHERE id = 'one'), (SELECT used FROM tool_budget), (SELECT pending FROM tool_executions WHERE approval_id = 'one'), (SELECT count(*) FROM events WHERE event LIKE '%tool_execution_outcome%')", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap();
        assert_eq!(state, ("written".into(), 1, 1, 0, 1));
        let store = SqliteStore::open(&path).unwrap();
        let mut broker = Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
        assert_eq!(broker.call(&request()), Err(BrokerError::Denied));
        assert_eq!(broker.executions(), 0);
        drop(broker);
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn killed_after_write_requires_owner_reconciliation() {
        let path = std::env::temp_dir().join(format!("jarvis-killed-write-{}", std::process::id()));
        if let Some(child_path) = std::env::var_os("JARVIS_CRASH_CHILD") {
            let store = SqliteStore::open(std::path::Path::new(&child_path)).unwrap();
            let mut broker =
                Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
            let _ = broker.call(&request());
            panic!("child was not killed at the post-write boundary");
        }
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
        store.initialize_tool_budget(2).unwrap();
        for id in ["one", "two"] {
            store
                .issue_tool_approval(
                    id,
                    "task",
                    1,
                    &request(),
                    SystemTime::now() + Duration::from_secs(600),
                )
                .unwrap();
        }
        drop(store);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tool_policy::tests::killed_after_write_requires_owner_reconciliation",
                "--nocapture",
            ])
            .env("JARVIS_CRASH_CHILD", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut poll = libc::pollfd {
            fd: stderr.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 30_000) };
        if ready != 1 {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not reach post-write boundary: {ready}");
        }
        let mut signal = [0];
        stderr.read_exact(&mut signal).unwrap();
        assert_eq!(&signal, b"!");
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());

        let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
        let snapshot = || -> (String, i64, i64, i64, i64, i64) {
            db.query_row("SELECT (SELECT value FROM mock_state WHERE resource = 'mock/item'), (SELECT consumed FROM tool_approvals WHERE id = 'one'), (SELECT consumed FROM tool_approvals WHERE id = 'two'), (SELECT used FROM tool_budget), (SELECT pending FROM tool_executions WHERE approval_id = 'one'), (SELECT count(*) FROM events WHERE event LIKE '%tool_execution_outcome%')", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap()
        };
        assert_eq!(snapshot(), ("written".into(), 1, 0, 1, 1, 0));
        let store = SqliteStore::open(&path).unwrap();
        let mut broker = Broker::with_approval(contract(), store, "two".into(), "task".into(), 1);
        assert_eq!(
            broker.call(&request()),
            Err(BrokerError::ReconciliationRequired)
        );
        assert_eq!(broker.executions(), 0);
        drop(broker);
        assert_eq!(snapshot(), ("written".into(), 1, 0, 1, 1, 0));
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .reconcile_tool_execution("one", ReconciliationOutcome::Succeeded)
            .unwrap();
        assert!(store
            .reconcile_tool_execution("one", ReconciliationOutcome::Succeeded)
            .is_err());
        drop(store);
        assert_eq!(snapshot(), ("written".into(), 1, 0, 1, 0, 0));
        let events: i64 = db.query_row("SELECT count(*) FROM events WHERE task_id = 'task' AND revision = 1 AND CASE WHEN json_valid(event) THEN json_extract(event, '$.type') = 'tool_execution_reconciliation' AND json_extract(event, '$.approval_id') = 'one' AND json_extract(event, '$.outcome') = 'succeeded' ELSE 0 END", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 1);
        let store = SqliteStore::open(&path).unwrap();
        let mut broker = Broker::with_approval(contract(), store, "one".into(), "task".into(), 1);
        assert_eq!(broker.call(&request()), Err(BrokerError::Denied));
        assert_eq!(broker.executions(), 0);
        drop(broker);
        assert_eq!(snapshot(), ("written".into(), 1, 0, 1, 0, 0));
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }
}
