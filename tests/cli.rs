use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn starts_without_credentials_and_shuts_down_on_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("started"), "{line}");
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("stopped"), "{line}");
    assert!(child.wait().unwrap().success());
}

#[test]
fn invalid_config_is_rejected_without_disclosing_contents() {
    let path = std::env::temp_dir().join(format!("jarvis-invalid-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"secret":"sensitive-value"}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .arg("--config")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid_config"), "{stderr}");
    assert!(!stderr.contains("sensitive-value"));
}

#[test]
fn valid_config_starts_and_handles_sigterm() {
    let path = std::env::temp_dir().join(format!("jarvis-valid-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"shutdown_timeout_secs":5}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .arg("--config")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("started"), "{line}");
    assert!(Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("stopped"), "{line}");
    assert!(child.wait().unwrap().success());
    std::fs::remove_file(path).unwrap();
}
