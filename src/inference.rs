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
    let eligible =
        |model: &&ModelProfile<'_>| manual.map_or(model.tier == tier, |name| model.name == name);
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
