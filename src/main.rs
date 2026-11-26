use serde::{Deserialize, Serialize};
use std::{env, fs, io, process::ExitCode};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Lifecycle {
    Started,
    Stopped,
}

#[derive(Serialize)]
struct Event {
    event: Lifecycle,
}

#[derive(Debug)]
enum Error {
    InvalidArguments,
    InvalidConfig,
    StdinReadFailed,
}

impl Error {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidConfig => "invalid_config",
            Self::StdinReadFailed => "stdin_read_failed",
        }
    }
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1);
    let _config: Config = match args.next() {
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
    println!(
        "{}",
        serde_json::to_string(&Event {
            event: Lifecycle::Started
        })
        .unwrap()
    );
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|_| Error::StdinReadFailed)?;
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
