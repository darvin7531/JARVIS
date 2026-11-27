use jarvis::inference::{classify, Tier};
use std::{fs, process::Command};

#[test]
fn exact_ping_is_local_but_other_prompts_are_not() {
    assert_eq!(classify("ping"), Tier::T0);
    assert_ne!(classify("ping please"), Tier::T0);
    assert_ne!(classify(" ping"), Tier::T0);
    let path = std::env::temp_dir().join(format!("jarvis-auto-ping-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config", path.to_str().unwrap(), "ping"])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"pong\n");
}

#[test]
fn russian_critical_incident_routes_heavy() {
    assert_eq!(classify("Разбери критический инцидент"), Tier::T3);
    assert_eq!(classify("КРИТИЧЕСКИЙ ИНЦИДЕНТ"), Tier::T3);
}
