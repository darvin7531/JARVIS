mod agent;
mod chat;
#[allow(dead_code)]
mod provider;

use jarvis::{Event, Lifecycle};
use serde::Deserialize;
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    env,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    process::ExitCode,
    sync::mpsc,
    time::Duration,
};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {}

#[derive(Debug)]
enum Error {
    InvalidArguments,
    InvalidConfig,
    MissingApiKey,
    CapabilityUnavailable,
    DataNotAllowed,
    SignalSetupFailed,
    OutputFailed,
    Chat(&'static str),
}

impl Error {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidConfig => "invalid_config",
            Self::MissingApiKey => "missing_api_key",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::DataNotAllowed => "data_not_allowed",
            Self::SignalSetupFailed => "signal_setup_failed",
            Self::OutputFailed => "output_failed",
            Self::Chat(code) => code,
        }
    }
}

fn open_config(path: impl AsRef<std::path::Path>) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == "agent") {
        args.next();
        let mode = args.next().ok_or(Error::InvalidArguments)?;
        if mode == "--owner" {
            let owner = args.next().ok_or(Error::InvalidArguments)?;
            if args.next().is_none_or(|arg| arg != "--mock-response") {
                return Err(Error::InvalidArguments);
            }
            let response = args.next().ok_or(Error::InvalidArguments)?;
            if args.next().is_some() {
                return Err(Error::InvalidArguments);
            }
            return agent::run_owner(&owner, &response).map_err(Error::Chat);
        }
        if mode != "--mock"
            || args.next().is_none_or(|arg| arg != "diagnose")
            || args.next().is_some()
        {
            return Err(Error::InvalidArguments);
        }
        return agent::run().map_err(Error::Chat);
    }
    if args.peek().is_some_and(|arg| arg == "chat") {
        args.next();
        let backend = args.next().ok_or(Error::InvalidArguments)?;
        if backend == "--config" {
            return chat::run(args).map_err(Error::Chat);
        }
        if backend == "--openai" {
            if args.next().is_none_or(|arg| arg != "--model")
                || args.next().is_none()
                || args.next().is_none()
                || args.next().is_some()
            {
                return Err(Error::InvalidArguments);
            }
            if !env::var("OPENAI_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
                return Err(Error::MissingApiKey);
            }
            return Err(Error::InvalidArguments);
        }
        if backend != "--mock" {
            return Err(Error::InvalidArguments);
        }
        use jarvis::inference::{route, Capability, DataClass, ModelProfile, RouteError, Tier};
        let mut capability = Capability::Text;
        let mut data = DataClass::Public;
        let mut seen_capability = false;
        let mut seen_data = false;
        let mut prompt = args.next().ok_or(Error::InvalidArguments)?;
        while prompt == "--require" || prompt == "--data" {
            let value = args.next().ok_or(Error::InvalidArguments)?;
            if prompt == "--require" {
                if seen_capability {
                    return Err(Error::InvalidArguments);
                }
                seen_capability = true;
                capability = match value.to_str() {
                    Some("text") => Capability::Text,
                    Some("vision") => Capability::Vision,
                    Some("tools") => Capability::Tools,
                    _ => return Err(Error::InvalidArguments),
                };
            } else {
                if seen_data {
                    return Err(Error::InvalidArguments);
                }
                seen_data = true;
                data = match value.to_str() {
                    Some("public") => DataClass::Public,
                    Some("internal") => DataClass::Internal,
                    Some("sensitive") => DataClass::Sensitive,
                    _ => return Err(Error::InvalidArguments),
                };
            }
            prompt = args.next().ok_or(Error::InvalidArguments)?;
        }
        let models = [ModelProfile {
            name: "mock",
            tier: Tier::T1,
            capabilities: &[Capability::Text],
            allowed_data: &[DataClass::Public],
        }];
        route(&models, Tier::T1, None, capability, data).map_err(|err| match err {
            RouteError::CapabilityUnavailable => Error::CapabilityUnavailable,
            RouteError::DataNotAllowed => Error::DataNotAllowed,
            RouteError::ModelUnavailable => Error::InvalidArguments,
        })?;
        if args.next().is_some() {
            return Err(Error::InvalidArguments);
        }
        println!("{}", prompt.to_string_lossy());
        return Ok(());
    }
    let _config: Config = match args.next() {
        None => Config::default(),
        Some(flag) if flag == "--config" => {
            let path = args.next().ok_or(Error::InvalidArguments)?;
            if args.next().is_some() {
                return Err(Error::InvalidArguments);
            }
            let mut data = Vec::new();
            open_config(path)
                .map_err(|_| Error::InvalidConfig)?
                .take(MAX_CONFIG_BYTES + 1)
                .read_to_end(&mut data)
                .map_err(|_| Error::InvalidConfig)?;
            if data.len() as u64 > MAX_CONFIG_BYTES {
                return Err(Error::InvalidConfig);
            }
            serde_json::from_slice(&data).map_err(|_| Error::InvalidConfig)?
        }
        Some(_) => return Err(Error::InvalidArguments),
    };
    let mut signals = Signals::new([SIGINT, SIGTERM]).map_err(|_| Error::SignalSetupFailed)?;
    let (sender, receiver) = mpsc::channel();
    let signal_sender = sender.clone();
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = signal_sender.send(());
        }
    });
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for byte in stdin.lock().bytes() {
            match byte {
                Ok(b'\n') => {
                    let _ = sender.send(());
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (done_sender, done_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            writeln!(
                io::stdout(),
                "{}",
                serde_json::to_string(&Event {
                    event: Lifecycle::Started
                })?
            )?;
            let _ = stop_receiver.recv();
            writeln!(
                io::stdout(),
                "{}",
                serde_json::to_string(&Event {
                    event: Lifecycle::Stopped
                })?
            )
        })();
        let _ = done_sender.send(result);
    });
    let _ = receiver.recv();
    let _ = stop_sender.send(());
    // ponytail: blocked stdout may lose lifecycle events; use cancellable I/O if delivery is required.
    match done_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(Ok(())) | Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
        Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::OutputFailed),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{{\"error\":\"{}\"}}", error.code());
            ExitCode::FAILURE
        }
    }
}
