use serde::Deserialize;
use serde_json::Value;

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    pub tool: String,
    pub resource: String,
    pub arguments: Value,
}

#[derive(Debug)]
pub struct Grant {
    pub tool: String,
    pub resource: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrokerError {
    UnknownTool,
    InvalidResource,
    Denied,
    InvalidArguments,
}

#[derive(Clone, Copy)]
enum RegisteredTool {
    Inert,
}

impl RegisteredTool {
    fn lookup(name: &str) -> Option<Self> {
        match name {
            "inert" => Some(Self::Inert),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InertArguments {
    text: String,
}

// This registry contains only a fixed, side-effect-free test executor.
// Grants are caller-supplied; this does not authenticate the caller.
#[derive(Default)]
pub struct Broker {
    executions: usize,
}

impl Broker {
    pub fn executions(&self) -> usize {
        self.executions
    }

    pub fn call(&mut self, request: &ToolRequest, grants: &[Grant]) -> Result<String, BrokerError> {
        let tool = RegisteredTool::lookup(&request.tool).ok_or(BrokerError::UnknownTool)?;
        if request.resource.is_empty() || request.resource.len() > 128 {
            return Err(BrokerError::InvalidResource);
        }
        if !grants
            .iter()
            .any(|grant| grant.tool == request.tool && grant.resource == request.resource)
        {
            return Err(BrokerError::Denied);
        }
        match tool {
            RegisteredTool::Inert => {
                let args: InertArguments = serde_json::from_value(request.arguments.clone())
                    .map_err(|_| BrokerError::InvalidArguments)?;
                if args.text.len() > 1024 {
                    return Err(BrokerError::InvalidArguments);
                }
                self.executions += 1;
                Ok(args.text)
            }
        }
    }
}
