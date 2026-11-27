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
}
