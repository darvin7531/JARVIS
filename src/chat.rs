use crate::provider::{Ollama, Request};
use jarvis::inference::{route, Capability, DataClass, ModelProfile, RouteError, Tier};
use serde::Deserialize;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    models: Vec<Model>,
    #[serde(default)]
    limits: Limits,
    budget: Option<Budget>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    workspace: std::path::PathBuf,
    limit_tokens: u64,
    max_input_tokens: u32,
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
    #[serde(default)]
    trusted_local: bool,
    #[serde(default)]
    trusted_no_truncation: bool,
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
    crate::open_config(path)
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
    let mut stdin_prompt = false;
    let prompt = loop {
        let arg = args
            .next()
            .ok_or("invalid_arguments")?
            .into_string()
            .map_err(|_| "invalid_arguments")?;
        if arg == "--stdin" {
            stdin_prompt = true;
            break String::new();
        }
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
            "--tier" => {
                selected_tier = Some(if value == "t0" {
                    Tier::T0
                } else {
                    tier(&value).ok_or("invalid_arguments")?
                })
            }
            "--model" => manual = Some(value),
            "--require" => required = capability(&value).ok_or("invalid_arguments")?,
            "--data" => class = data(&value).ok_or("invalid_arguments")?,
            _ => return Err("invalid_arguments"),
        }
    };
    if args.next().is_some() {
        return Err("invalid_arguments");
    }
    if class == DataClass::Sensitive && !stdin_prompt {
        return Err("sensitive_requires_stdin");
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGINT, Arc::clone(&cancelled))
        .map_err(|_| "signal_setup_failed")?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&cancelled))
        .map_err(|_| "signal_setup_failed")?;
    let started = Instant::now();
    let timeout = Duration::from_secs(config.limits.timeout_seconds);
    let prompt = if stdin_prompt {
        if class == DataClass::Sensitive && unsafe { libc::isatty(0) } == 1 {
            return Err("sensitive_tty");
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(0, stat.as_mut_ptr()) } != 0 {
            return Err("invalid_prompt");
        }
        let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
        if mode == libc::S_IFSOCK {
            return Err("invalid_prompt");
        }
        let input = if mode == libc::S_IFIFO {
            Some(
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open("/proc/self/fd/0")
                    .map_err(|_| "invalid_prompt")?,
            )
        } else {
            None
        };
        let fd = input.as_ref().map_or(0, AsRawFd::as_raw_fd);
        let mut bytes = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("cancelled");
            }
            if started.elapsed() >= timeout {
                return Err("timeout");
            }
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pollfd, 1, 50) };
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err("invalid_prompt");
            }
            if ready == 0 {
                continue;
            }
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n == 0 {
                break;
            }
            if n < 0 {
                let kind = std::io::Error::last_os_error().kind();
                if kind == std::io::ErrorKind::Interrupted || kind == std::io::ErrorKind::WouldBlock
                {
                    continue;
                }
                return Err("invalid_prompt");
            }
            bytes.extend_from_slice(&buf[..n as usize]);
            if bytes.len() > 1024 * 1024 {
                return Err("invalid_prompt");
            }
        }
        if bytes.len() > 1024 * 1024 {
            return Err("invalid_prompt");
        }
        String::from_utf8(bytes).map_err(|_| "invalid_prompt")?
    } else {
        prompt
    };
    // This transport accepts text prompts only; profile labels cannot add modalities.
    if required != Capability::Text {
        return Err("capability_unavailable");
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(1, stat.as_mut_ptr()) } != 0 {
        return Err("output_failed");
    }
    let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
    if mode == libc::S_IFSOCK {
        return Err("output_failed");
    }
    let output = if mode == libc::S_IFIFO {
        Some(
            OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open("/proc/self/fd/1")
                .map_err(|_| "output_failed")?,
        )
    } else {
        None
    };
    let fd = output.as_ref().map_or(1, AsRawFd::as_raw_fd);
    let selected_tier = selected_tier.unwrap_or_else(|| jarvis::inference::classify(&prompt));
    if selected_tier == Tier::T0 {
        if manual.is_some() || prompt != "ping" {
            return Err("invalid_arguments");
        }
        if b"pong\n".len() > config.limits.max_output_bytes {
            return Err("budget_exceeded");
        }
        return write_output(fd, b"pong\n", &cancelled, started, timeout);
    }
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
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|class| m.trusted_local || *class == DataClass::Public)
                .collect::<Vec<_>>(),
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
    if !config.models[index].trusted_no_truncation {
        return Err("unsupported_runtime");
    }
    let req = Request {
        model: selected.name,
        prompt: &prompt,
        max_tokens: config.limits.max_tokens,
        max_output_bytes: config.limits.max_output_bytes,
        timeout: timeout.checked_sub(started.elapsed()).ok_or("timeout")?,
    };
    let budget = config.budget.as_ref().ok_or("invalid_config")?;
    let mut reservation = {
        let amount = u64::from(budget.max_input_tokens) + u64::from(config.limits.max_tokens);
        if amount > budget.limit_tokens {
            return Err("budget_exceeded");
        }
        if budget.max_input_tokens == 0 || budget.limit_tokens > i64::MAX as u64 {
            return Err("invalid_config");
        }
        let mut store = jarvis::storage::SqliteStore::open(&budget.workspace)
            .map_err(|_| "budget_storage_failed")?;
        let id = store
            .reserve_tokens(budget.limit_tokens, amount)
            .map_err(|_| "budget_storage_failed")?
            .ok_or("budget_exceeded")?;
        (store, id)
    };
    let mut failure = None;
    let result = Ollama {
        port: config.models[index].port,
        trusted_no_truncation: config.models[index].trusted_no_truncation,
    }
    .generate_bounded(
        &req,
        &cancelled,
        &mut |text| {
            if let Err(code) = write_output(fd, text.as_bytes(), &cancelled, started, timeout) {
                failure = Some(code);
                cancelled.store(true, Ordering::Relaxed);
            }
        },
        Some(budget.max_input_tokens),
    );
    {
        let (store, id) = &mut reservation;
        let known = match &result {
            Ok(usage) | Err(crate::provider::Error::Incomplete(usage)) => Some(usage),
            _ => None,
        };
        if let Some(usage) = known {
            let tokens = usage
                .input_tokens
                .checked_add(usage.output_tokens)
                .ok_or("budget_storage_failed")?;
            store
                .settle_tokens(*id, tokens)
                .map_err(|_| "budget_storage_failed")?;
        }
    }
    let usage = result.map_err(|error| {
        if let crate::provider::Error::Incomplete(ref usage) = error {
            eprintln!(
                "{{\"input_tokens\":{},\"output_tokens\":{}}}",
                usage.input_tokens, usage.output_tokens
            );
        } else {
            eprintln!("{{\"usage\":null}}");
        }
        if let Some(code) = failure {
            return code;
        }
        match error {
            crate::provider::Error::Cancelled => "cancelled",
            crate::provider::Error::Timeout => "timeout",
            crate::provider::Error::Budget | crate::provider::Error::Incomplete(_) => {
                "budget_exceeded"
            }
            _ => "inference_failed",
        }
    })?;
    if let Some(code) = failure {
        return Err(code);
    }
    if cancelled.load(Ordering::Relaxed) {
        eprintln!("{{\"usage\":null}}");
        return Err("cancelled");
    }
    eprintln!(
        "{{\"input_tokens\":{},\"output_tokens\":{}}}",
        usage.input_tokens, usage.output_tokens
    );
    Ok(())
}

fn write_output(
    fd: i32,
    mut bytes: &[u8],
    cancelled: &AtomicBool,
    started: Instant,
    timeout: Duration,
) -> Result<(), &'static str> {
    while !bytes.is_empty() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled");
        }
        if started.elapsed() >= timeout {
            return Err("timeout");
        }
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
        } else if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                std::thread::sleep(Duration::from_millis(10));
            } else if error.kind() != std::io::ErrorKind::Interrupted {
                return Err("output_failed");
            }
        } else {
            return Err("output_failed");
        }
    }
    Ok(())
}
