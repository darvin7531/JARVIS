use std::{fs, process::Command};

#[test]
fn exhausted_shared_budget_rejects_before_connecting() {
    let root = std::env::temp_dir().join(format!("jarvis-budget-red-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let config = root.join("chat.json");
    fs::write(&config, format!(r#"{{"models":[{{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":1}}],"budget":{{"workspace":"{}","limit_tokens":0,"max_input_tokens":1}}}}"#, root.display())).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis")).args(["chat", "--config"]).arg(&config).arg("hello").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stderr), "{\"error\":\"budget_exceeded\"}\n");
    fs::remove_dir_all(root).unwrap();
}
