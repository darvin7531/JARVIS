use std::{fs, process::Command};

#[test]
fn t0_ping_is_a_local_rule_without_any_models() {
    let path = std::env::temp_dir().join(format!("jarvis-t0-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--tier",
            "t0",
            "ping",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"pong\n");
}
