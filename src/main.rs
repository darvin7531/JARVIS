use jarvis::{Event, Lifecycle};
use serde::Deserialize;
use std::{
    env, fs, io,
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    shutdown_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shutdown_timeout_secs: 1,
        }
    }
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_: i32) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
fn install_signals() -> Result<(), Error> {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    // Linux SIGINT and SIGTERM; handler only performs a lock-free atomic store.
    for sig in [2, 15] {
        if unsafe { signal(sig, handle_signal as usize) } == usize::MAX {
            return Err(Error::SignalSetupFailed);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
compile_error!("Signal handling currently supports Linux only");

#[derive(Debug)]
enum Error {
    InvalidArguments,
    InvalidConfig,
    StdinReadFailed,
    SignalSetupFailed,
}

impl Error {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidConfig => "invalid_config",
            Self::StdinReadFailed => "stdin_read_failed",
            Self::SignalSetupFailed => "signal_setup_failed",
        }
    }
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1);
    let config: Config = match args.next() {
        None => Config::default(),
        Some(flag) if flag == "--config" => {
            let path = args.next().ok_or(Error::InvalidArguments)?;
            if args.next().is_some() {
                return Err(Error::InvalidArguments);
            }
            let data = fs::read(path).map_err(|_| Error::InvalidConfig)?;
            serde_json::from_slice(&data).map_err(|_| Error::InvalidConfig)?
        }
        Some(_) => return Err(Error::InvalidArguments),
    };
    if !(1..=30).contains(&config.shutdown_timeout_secs) {
        return Err(Error::InvalidConfig);
    }
    install_signals()?;
    println!(
        "{}",
        serde_json::to_string(&Event {
            event: Lifecycle::Started
        })
        .unwrap()
    );
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = sender.send(io::stdin().read_line(&mut line));
    });
    while !SHUTDOWN.load(Ordering::Relaxed) {
        match receiver.recv_timeout(Duration::from_secs(config.shutdown_timeout_secs)) {
            Ok(result) => {
                result.map_err(|_| Error::StdinReadFailed)?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::StdinReadFailed),
        }
    }
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
