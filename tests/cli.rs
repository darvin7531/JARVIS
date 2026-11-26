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

#[test]
fn invalid_values_and_malformed_input_are_redacted() {
    let path =
        std::env::temp_dir().join(format!("jarvis-sensitive-path-{}.json", std::process::id()));
    for input in [
        r#"{"shutdown_timeout_secs":0}"#,
        r#"{"shutdown_timeout_secs":31}"#,
        r#"{"shutdown_timeout_secs":"sensitive-value"}"#,
        r#"{"sensitive-value": "#,
    ] {
        std::fs::write(&path, input).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("--config")
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"{\"error\":\"invalid_config\"}\n");
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn sigterm_is_prompt_even_with_long_timeout() {
    let path = std::env::temp_dir().join(format!("jarvis-prompt-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"shutdown_timeout_secs":30}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .arg("--config")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    std::fs::remove_file(path).unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "shutdown timed out or failed"
    );
}
