//! Small, deterministic routing primitives; no model is granted tool authority.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Text,
    Vision,
    Tools,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClass {
    Public,
    Internal,
    Sensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    T0,
    T1,
    T2,
    T3,
}

/// Rules are lexical hints, not a semantic complexity assessment.
/// ponytail: a small keyword list misses synonyms; extend it from observed tasks.
pub fn classify(prompt: &str) -> Tier {
    if prompt == "ping" {
        return Tier::T0;
    }
    let prompt = prompt.trim_start();
    let heavy = prompt
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| {
            [
                "architecture",
                "incident",
                "critical",
                "архитектур",
                "инцидент",
                "критическ",
            ]
            .iter()
            .any(|hint| word.starts_with(hint))
        });
    if prompt.starts_with("complex:") || heavy {
        Tier::T3
    } else if prompt.starts_with("code:") {
        Tier::T2
    } else {
        Tier::T1
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    CapabilityUnavailable,
    DataNotAllowed,
    ModelUnavailable,
}

pub struct ModelProfile<'a> {
    pub name: &'a str,
    pub tier: Tier,
    pub capabilities: &'a [Capability],
    pub allowed_data: &'a [DataClass],
}

pub fn route<'a>(
    models: &'a [ModelProfile<'a>],
    tier: Tier,
    manual: Option<&str>,
    capability: Capability,
    data: DataClass,
) -> Result<&'a ModelProfile<'a>, RouteError> {
    if tier == Tier::T0
        || manual.is_some_and(|name| models.iter().filter(|model| model.name == name).count() != 1)
    {
        return Err(RouteError::ModelUnavailable);
    }
    let eligible = |model: &&ModelProfile<'_>| {
        model.tier != Tier::T0 && manual.map_or(model.tier == tier, |name| model.name == name)
    };
    if !models.iter().any(|model| eligible(&model)) {
        return Err(RouteError::ModelUnavailable);
    }
    if !models
        .iter()
        .filter(eligible)
        .any(|m| m.capabilities.contains(&capability))
    {
        return Err(RouteError::CapabilityUnavailable);
    }
    models
        .iter()
        .filter(eligible)
        .filter(|m| m.capabilities.contains(&capability))
        .find(|m| m.allowed_data.contains(&data))
        .ok_or(RouteError::DataNotAllowed)
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamError {
    Truncated,
    InvalidEvent,
    TooLarge,
    Failed,
}

/// Incrementally decodes Responses SSE frames. The maximum is per frame, in bytes.
pub struct ResponseStream {
    buffer: Vec<u8>,
    max_frame: usize,
    completed: bool,
}

impl ResponseStream {
    pub fn new(max_frame: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_frame,
            completed: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8], text: &mut String) -> Result<(), StreamError> {
        for byte in bytes {
            if self.completed {
                return Err(StreamError::InvalidEvent);
            }
            self.buffer.push(*byte);
            if self.buffer.len() > self.max_frame {
                return Err(StreamError::TooLarge);
            }
            if self.buffer.ends_with(b"\n\n") || self.buffer.ends_with(b"\r\n\r\n") {
                let frame =
                    std::str::from_utf8(&self.buffer).map_err(|_| StreamError::InvalidEvent)?;
                let mut data = Vec::new();
                let mut event = None;
                for line in frame.lines() {
                    if let Some(value) = line.strip_prefix("data:") {
                        data.push(value.strip_prefix(' ').unwrap_or(value));
                    } else if let Some(value) = line.strip_prefix("event:") {
                        event = Some(value.strip_prefix(' ').unwrap_or(value));
                    }
                }
                if !data.is_empty() {
                    let value: serde_json::Value = serde_json::from_str(&data.join("\n"))
                        .map_err(|_| StreamError::InvalidEvent)?;
                    let kind = value
                        .get("type")
                        .and_then(|v| v.as_str())
                        .ok_or(StreamError::InvalidEvent)?;
                    if event.is_some_and(|e| e != kind) {
                        return Err(StreamError::InvalidEvent);
                    }
                    match kind {
                        "response.output_text.delta" => text.push_str(
                            value
                                .get("delta")
                                .and_then(|v| v.as_str())
                                .ok_or(StreamError::InvalidEvent)?,
                        ),
                        "response.completed" => {
                            if value.pointer("/response/status").and_then(|v| v.as_str())
                                != Some("completed")
                            {
                                return Err(StreamError::InvalidEvent);
                            }
                            self.completed = true;
                        }
                        "error" | "response.failed" | "response.incomplete" => {
                            return Err(StreamError::Failed)
                        }
                        _ => {}
                    }
                }
                self.buffer.clear();
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<(), StreamError> {
        if self.completed && self.buffer.is_empty() {
            Ok(())
        } else {
            Err(StreamError::Truncated)
        }
    }
}
