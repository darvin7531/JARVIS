use std::{fs, process::Command};

#[test]
fn configured_chat_rejects_disallowed_data_before_connecting() {
    let path = std::env::temp_dir().join(format!("jarvis-chat-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":1}]}"#).unwrap();
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

#[test]
fn untrusted_loopback_rejects_nonpublic_data_before_connecting() {
    for class in ["internal", "sensitive"] {
        let path = std::env::temp_dir().join(format!(
            "jarvis-untrusted-{class}-{}.json",
            std::process::id()
        ));
        fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["internal","sensitive"],"trusted_no_truncation":true,"port":1}]}"#).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args([
                "chat",
                "--config",
                path.to_str().unwrap(),
                "--data",
                class,
                "--stdin",
            ])
            .output()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(output.stderr, b"{\"error\":\"data_not_allowed\"}\n");
    }
}

#[test]
fn missing_or_false_runtime_assertion_fails_closed() {
    for flag in ["", ",\"trusted_no_truncation\":false"] {
        let path = std::env::temp_dir().join(format!(
            "jarvis-runtime-{}-{}.json",
            std::process::id(),
            flag.len()
        ));
        fs::write(&path, format!(r#"{{"models":[{{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":1{flag}}}]}}"#)).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["chat", "--config"])
            .arg(&path)
            .arg("hello")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stderr, b"{\"error\":\"unsupported_runtime\"}\n");
        fs::remove_file(path).unwrap();
    }
}
