use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Started,
    Stopped,
}

#[derive(Serialize)]
pub struct Event {
    pub event: Lifecycle,
}

pub mod inference;
/// Real inference is only exposed through the policy- and budget-checked CLI.
///
/// ```compile_fail
/// use jarvis::provider::{Ollama, Provider};
/// ```
pub mod storage;
