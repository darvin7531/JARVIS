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
