use std::process::Command;

#[test]
fn mock_chat_streams_text_without_credentials() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .args(["chat", "--mock", "hello"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello\n");
}

#[test]
fn real_chat_requires_explicit_api_key() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .args(["chat", "--openai", "--model", "example-model", "hello"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"{\"error\":\"missing_api_key\"}\n");
}

#[test]
fn mock_route_rejects_unsupported_capability() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .args(["chat", "--mock", "--require", "vision", "hello"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"{\"error\":\"capability_unavailable\"}\n");
}

#[test]
fn mock_route_rejects_disallowed_data() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .args(["chat", "--mock", "--data", "sensitive", "hello"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"{\"error\":\"data_not_allowed\"}\n");
}
