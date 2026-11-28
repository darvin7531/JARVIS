use jarvis::storage::SqliteStore;
use std::process::Command;

#[test]
fn worker_accepts_explicit_owner_configuration_when_queue_is_empty() {
    let path = std::env::temp_dir().join(format!("jarvis-worker-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    drop(SqliteStore::create(&path).unwrap());
    let config = path.join("worker.json");
    std::fs::write(
        &config,
        serde_json::json!({"store": path, "owners": []}).to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["worker", "--once", "--config"])
        .arg(config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(path).unwrap();
}
