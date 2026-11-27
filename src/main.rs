use jarvis::{Event, Lifecycle};
use serde::Deserialize;
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    env,
    fs::File,
    io::{self, Read},
    process::ExitCode,
    sync::mpsc,
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
    SignalSetupFailed,
}

impl Error {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidConfig => "invalid_config",
            Self::MissingApiKey => "missing_api_key",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::SignalSetupFailed => "signal_setup_failed",
        }
    }
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == "chat") {
        args.next();
        let backend = args.next().ok_or(Error::InvalidArguments)?;
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
        let mut prompt = args.next().ok_or(Error::InvalidArguments)?;
        if prompt == "--require" {
            use jarvis::inference::{route, Capability, DataClass, ModelProfile, Tier};
            let requested = args.next().ok_or(Error::InvalidArguments)?;
            let capability = match requested.to_str() {
                Some("text") => Capability::Text,
                Some("vision") => Capability::Vision,
                Some("tools") => Capability::Tools,
                _ => return Err(Error::InvalidArguments),
            };
            let models = [ModelProfile {
                name: "mock",
                tier: Tier::T1,
                capabilities: &[Capability::Text],
                allowed_data: &[DataClass::Public],
            }];
            route(&models, Tier::T1, None, capability, DataClass::Public)
                .map_err(|_| Error::CapabilityUnavailable)?;
            prompt = args.next().ok_or(Error::InvalidArguments)?;
        }
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
            File::open(path)
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
    println!(
        "{}",
        serde_json::to_string(&Event {
            event: Lifecycle::Started
        })
        .unwrap()
    );
    let _ = receiver.recv();
    println!(
        "{}",
        serde_json::to_string(&Event {
            event: Lifecycle::Stopped
        })
        .unwrap()
    );
    Ok(())
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
