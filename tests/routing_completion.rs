use std::{fs, process::Command};

#[test]
fn configured_chat_rejects_vision_for_text_only_transport() {
    let path = std::env::temp_dir().join(format!("jarvis-vision-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text","vision"],"allowed_data":["public"],"port":1}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--require",
            "vision",
            "hello",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.stderr, b"{\"error\":\"capability_unavailable\"}\n");
}
