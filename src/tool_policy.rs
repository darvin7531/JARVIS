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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InertArguments {
    text: String,
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
        if request.tool != "inert" {
            return Err(BrokerError::UnknownTool);
        }
        if request.resource.is_empty() || request.resource.len() > 128 {
            return Err(BrokerError::InvalidResource);
        }
        let contract = &self.contract;
        if SystemTime::now() >= contract.expires_at
            || !contract.scopes.contains(&Scope::Read)
            || request.tool != contract.tool
            || request.action != "echo"
            || request.action != contract.action
            || request.resource != contract.resource
            || contract.denied_resources.contains(&request.resource)
            || request.data_class != contract.data_class
        {
            return Err(BrokerError::Denied);
        }
        let args: InertArguments = serde_json::from_value(request.arguments.clone())
            .map_err(|_| BrokerError::InvalidArguments)?;
        if args.text.len() > 1024 {
            return Err(BrokerError::InvalidArguments);
        }
        if args.text != contract.permitted_text {
            return Err(BrokerError::Denied);
        }
        let (store, id, task, revision) = self.approval.as_mut().ok_or(BrokerError::Denied)?;
        if !store
            .consume_tool_approval(id, task, *revision, request)
            .unwrap_or(false)
        {
            return Err(BrokerError::Denied);
        }
        self.executions += 1;
        Ok(args.text)
    }
}
