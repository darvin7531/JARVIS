use jarvis::inference::{classify, Tier};
use std::{fs, process::Command};

#[test]
fn exact_ping_is_local_but_other_prompts_are_not() {
    assert_eq!(classify("ping"), Tier::T0);
    assert_ne!(classify("ping please"), Tier::T0);
}

#[test]
fn russian_critical_incident_routes_heavy() {
    assert_eq!(classify("Разбери критический инцидент"), Tier::T3);
}

#[test]
fn configured_inference_requires_budget() {
    let path = std::env::temp_dir().join(format!("jarvis-no-budget-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":1}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config", path.to_str().unwrap(), "hello"])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.stderr, b"{\"error\":\"invalid_config\"}\n");
}
