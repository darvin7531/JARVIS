use serde_json::json;
use std::process::Command;

#[test]
fn configured_agent_reaches_shared_config_validation() {
    let path = std::env::temp_dir().join(format!("agent-configured-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":4102444800_u64, "context":"test"});
    let owner_path = path.join("owner.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--owner"])
        .arg(&owner_path)
        .arg("--config")
        .arg(path.join("missing.json"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_config"), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::remove_dir_all(path).unwrap();
}
