use jarvis::storage::{SqliteStore, Task};
use serde_json::json;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn held_open_decision_expires_without_approval() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-expiry-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 2;
    let input = path.join("approval.json");
    std::fs::write(&input, json!({"store":path,"task":"task","revision":1,"approval":"one","expires_unix":expires,"request":{"tool":"write","action":"write","resource":"mock/item","data_class":"public","arguments":{"text":"hello"}}}).to_string()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "approve", "--input"])
        .arg(&input)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(
        status.is_some_and(|s| !s.success()),
        "approval interaction did not expire"
    );
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM tool_approvals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn explicit_owner_decision_issues_only_exact_approval() {
    let path = std::env::temp_dir().join(format!("jarvis-owner-cli-{}", std::process::id()));
    let mut store = SqliteStore::create(&path).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "test".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    let request = json!({"tool":"write","action":"write","resource":"mock/item","data_class":"public","arguments":{"text":"hello\u{001b}[2J"}});
    let input = path.join("approval.json");
    std::fs::write(&input, json!({"store":path,"task":"task","revision":1,"approval":"one","expires_unix":expires,"request":request}).to_string()).unwrap();
    let run = |decision: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["agent", "approve", "--input"])
            .arg(&input)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(decision.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    for decision in ["deny\n", "", "yes\n"] {
        let out = run(decision);
        assert!(!out.status.success());
        let count: i64 = db
            .query_row("SELECT count(*) FROM tool_approvals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let out = run("approve\n");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let display = String::from_utf8(out.stdout).unwrap();
    for expected in [
        "task",
        "revision",
        "one",
        "expires_unix",
        "write",
        "mock/item",
        "public",
        "hello",
        "\\u001b",
    ] {
        assert!(display.contains(expected), "missing {expected}: {display}");
    }
    assert!(!display.contains('\u{1b}'));
    let (stored, consumed): (String, i64) = db.query_row("SELECT request, consumed FROM tool_approvals WHERE id='one' AND task_id='task' AND revision=1", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
        request
    );
    assert_eq!(consumed, 0);
    let executed: i64 = db
        .query_row("SELECT count(*) FROM tool_executions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(executed, 0);
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}
