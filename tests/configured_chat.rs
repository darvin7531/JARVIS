use std::{fs, process::Command};

#[test]
fn configured_chat_rejects_disallowed_data_before_connecting() {
    let path = std::env::temp_dir().join(format!("jarvis-chat-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":1}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--data",
            "sensitive",
            "--stdin",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"{\"error\":\"data_not_allowed\"}\n");
}
