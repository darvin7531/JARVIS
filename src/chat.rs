use jarvis::inference::{route, Capability, DataClass, ModelProfile, RouteError, Tier};
use jarvis::provider::{Ollama, Provider, Request};
use serde::Deserialize;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    models: Vec<Model>,
    #[serde(default)]
    limits: Limits,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Limits {
    max_tokens: u32,
    max_output_bytes: usize,
    timeout_seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_tokens: 128,
            max_output_bytes: 65536,
            timeout_seconds: 120,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    name: String,
    tier: String,
    capabilities: Vec<String>,
    allowed_data: Vec<String>,
    port: u16,
}
fn tier(s: &str) -> Option<Tier> {
    match s {
        "t1" => Some(Tier::T1),
        "t2" => Some(Tier::T2),
        "t3" => Some(Tier::T3),
        _ => None,
    }
}
fn capability(s: &str) -> Option<Capability> {
    match s {
        "text" => Some(Capability::Text),
        "vision" => Some(Capability::Vision),
        "tools" => Some(Capability::Tools),
        _ => None,
    }
}
fn data(s: &str) -> Option<DataClass> {
    match s {
        "public" => Some(DataClass::Public),
        "internal" => Some(DataClass::Internal),
        "sensitive" => Some(DataClass::Sensitive),
        _ => None,
    }
}

pub fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), &'static str> {
    let path = args.next().ok_or("invalid_arguments")?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "invalid_config")?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_config")?;
    if bytes.len() > 65536 {
        return Err("invalid_config");
    }
    let config: Config = serde_json::from_slice(&bytes).map_err(|_| "invalid_config")?;
    if config.limits.max_tokens == 0
        || config.limits.max_output_bytes == 0
        || config.limits.timeout_seconds == 0
    {
        return Err("invalid_config");
    }
    let mut selected_tier = None;
    let mut manual = None;
    let mut required = Capability::Text;
    let mut class = DataClass::Public;
    let mut seen = Vec::new();
    let prompt = loop {
        let arg = args
            .next()
            .ok_or("invalid_arguments")?
            .into_string()
            .map_err(|_| "invalid_arguments")?;
        if !arg.starts_with("--") {
            break arg;
        }
        if seen.contains(&arg) {
            return Err("invalid_arguments");
        }
        seen.push(arg.clone());
        let value = args
            .next()
            .ok_or("invalid_arguments")?
            .into_string()
            .map_err(|_| "invalid_arguments")?;
        match arg.as_str() {
            "--tier" => selected_tier = Some(tier(&value).ok_or("invalid_arguments")?),
            "--model" => manual = Some(value),
            "--require" => required = capability(&value).ok_or("invalid_arguments")?,
            "--data" => class = data(&value).ok_or("invalid_arguments")?,
            _ => return Err("invalid_arguments"),
        }
    };
    if args.next().is_some() {
        return Err("invalid_arguments");
    }
    // This transport accepts text prompts only; profile labels cannot add modalities.
    if required != Capability::Text {
        return Err("capability_unavailable");
    }
    let selected_tier = selected_tier.unwrap_or_else(|| jarvis::inference::classify(&prompt));
    let mut caps = Vec::new();
    let mut classes = Vec::new();
    let mut tiers = Vec::new();
    for m in &config.models {
        if m.name.is_empty() || m.port == 0 {
            return Err("invalid_config");
        }
        tiers.push(tier(&m.tier).ok_or("invalid_config")?);
        caps.push(
            m.capabilities
                .iter()
                .map(|s| capability(s).ok_or("invalid_config"))
                .collect::<Result<Vec<_>, _>>()?,
        );
        classes.push(
            m.allowed_data
                .iter()
                .map(|s| data(s).ok_or("invalid_config"))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    let profiles: Vec<_> = config
        .models
        .iter()
        .enumerate()
        .map(|(i, m)| ModelProfile {
            name: &m.name,
            tier: tiers[i],
            capabilities: &caps[i],
            allowed_data: &classes[i],
        })
        .collect();
    let selected = route(&profiles, selected_tier, manual.as_deref(), required, class).map_err(
        |e| match e {
            RouteError::ModelUnavailable => "model_unavailable",
            RouteError::CapabilityUnavailable => "capability_unavailable",
            RouteError::DataNotAllowed => "data_not_allowed",
        },
    )?;
    let index = profiles
        .iter()
        .position(|p| std::ptr::eq(p, selected))
        .ok_or("invalid_config")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGINT, Arc::clone(&cancelled))
        .map_err(|_| "signal_setup_failed")?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&cancelled))
        .map_err(|_| "signal_setup_failed")?;
    let req = Request {
        model: selected.name,
        prompt: &prompt,
        max_tokens: config.limits.max_tokens,
        max_output_bytes: config.limits.max_output_bytes,
        timeout: Duration::from_secs(config.limits.timeout_seconds),
    };
    let mut failed = false;
    let usage = Ollama {
        port: config.models[index].port,
    }
    .generate(&req, &cancelled, &mut |text| {
        let mut out = std::io::stdout().lock();
        if out
            .write_all(text.as_bytes())
            .and_then(|_| out.flush())
            .is_err()
        {
            failed = true;
            cancelled.store(true, Ordering::Relaxed);
        }
    })
    .map_err(|error| {
        eprintln!("{{\"usage\":null}}");
        if failed {
            return "output_failed";
        }
        match error {
            jarvis::provider::Error::Cancelled => "cancelled",
            jarvis::provider::Error::Timeout => "timeout",
            jarvis::provider::Error::Budget => "budget_exceeded",
            _ => "inference_failed",
        }
    })?;
    if failed {
        return Err("output_failed");
    }
    eprintln!(
        "{{\"input_tokens\":{},\"output_tokens\":{}}}",
        usage.input_tokens, usage.output_tokens
    );
    Ok(())
}
