use std::process::Command;

#[test]
fn self_authorized_mock_diagnosis_is_not_available() {
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--mock", "diagnose"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}
