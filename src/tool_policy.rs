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
        if !store
            .consume_tool_approval(id, task, *revision, request, contract.expires_at)
            .unwrap_or(false)
        {
            return Err(BrokerError::Denied);
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
            RegisteredTool::Inert { text } => text,
            RegisteredTool::Read { .. } => store
                .mock_read(&request.resource, request.data_class)
                .map_err(|_| BrokerError::OutcomeUncertain)?,
            RegisteredTool::Write { text } => {
                store
                    .mock_write(&request.resource, &text, request.data_class)
                    .map_err(|_| BrokerError::OutcomeUncertain)?;
                text
            }
        };
        store
            .record_tool_outcome(id, task, *revision)
            .map_err(|_| BrokerError::OutcomeUncertain)?;
        Ok(result)
    }
}
