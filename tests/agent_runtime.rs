use std::process::Command;

#[test]
fn read_only_diagnosis_runs_through_the_agent_cli() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--mock", "diagnose"])
        .output()
        .expect("start agent CLI");
    assert!(
        output.status.success(),
        "agent CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON result");
    assert_eq!(result["status"], "verified");
    assert_eq!(result["changes"], 0);
    let path = std::path::Path::new(result["checkpoint"].as_str().unwrap());
    let store = jarvis::storage::SqliteStore::open(path).unwrap();
    let events = store.events("diagnose").unwrap();
    assert!(events.iter().any(|event| event.contains("tool_execution_outcome")));
    assert!(events.iter().any(|event| event.contains("checkpoint")));
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
