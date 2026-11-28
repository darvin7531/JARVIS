use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{DataClass, ToolRequest};
use serde_json::json;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn setup(name: &str) -> (std::path::PathBuf, ToolRequest) {
    let root = std::env::temp_dir().join(format!("agent-e2e-{name}-{}", std::process::id()));
    let mut store = SqliteStore::create(&root).unwrap();
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
    store.initialize_tool_budget(2).unwrap();
    let request = ToolRequest {
        tool: "write".into(),
        action: "write".into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":"new"}),
    };
    let expiry = SystemTime::now() + Duration::from_secs(300);
    store
        .issue_tool_approval("write", "task", 1, &request, expiry)
        .unwrap();
    drop(store);
    let owner = json!({"store":root,"task":"task","revision":1,"approval":"write","read_approval":"read","resource":"mock/item","data_class":"public","permitted_text":"new","expected":"new","expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(),"mode":"write","context":"write new"});
    std::fs::write(root.join("owner.json"), owner.to_string()).unwrap();
    (root, request)
}

fn command(root: &std::path::Path, option: &str, input: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jarvis"));
    cmd.args(["agent", "--owner"])
        .arg(root.join("owner.json"))
        .arg(option)
        .arg(root.join(input));
    cmd
}

#[test]
fn class_mismatch_is_denied_without_execution_or_replay() {
    let (root, request) = setup("failure");
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO mock_state VALUES ('mock/item', 'original', 'sensitive')",
        [],
    )
    .unwrap();
    drop(db);
    std::fs::write(
        root.join("proposal.json"),
        serde_json::to_string(&request).unwrap(),
    )
    .unwrap();
    let output = command(&root, "--mock-response", "proposal.json")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{:?}", output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("agent_tool"),
        "{:?}",
        output
    );
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let state: (String, String, i64, i64, i64, i64) = db.query_row("SELECT value, data_class, (SELECT consumed FROM tool_approvals WHERE id='write'), (SELECT count(*) FROM tool_executions), (SELECT used FROM tool_budget), (SELECT pending FROM agent_runtime WHERE task_id='task') FROM mock_state WHERE resource='mock/item'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
    assert_eq!(state, ("original".into(), "sensitive".into(), 0, 0, 0, 0));
    drop(db);
    let retry = command(&root, "--mock-response", "proposal.json")
        .output()
        .unwrap();
    assert!(!retry.status.success());
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let state: (i64, i64, String) = db.query_row("SELECT (SELECT count(*) FROM tool_executions), (SELECT used FROM tool_budget), (SELECT value FROM mock_state WHERE resource='mock/item')", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(state, (0, 0, "original".into()));
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sigterm_during_inference_never_dispatches_tool_or_replays() {
    use std::net::TcpListener;
    let (root, _) = setup("cancel");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = json!({"models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],"limits":{"max_tokens":100},"budget":{"workspace":root,"limit_tokens":4096,"max_input_tokens":2048}});
    std::fs::write(root.join("config.json"), config.to_string()).unwrap();
    let mut child = command(&root, "--config", "config.json")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            other => {
                let _ = child.kill();
                panic!("provider not reached: {other:?}");
            }
        }
    };
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("cancel did not stop agent");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    drop(stream);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cancelled"),
        "{:?}",
        output
    );
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let state: (i64, i64, i64, i64, i64, i64) = db.query_row("SELECT (SELECT consumed FROM tool_approvals WHERE id='write'), (SELECT count(*) FROM tool_executions), (SELECT used FROM tool_budget), (SELECT count(*) FROM mock_state), (SELECT pending FROM agent_runtime WHERE task_id='task'), (SELECT count(*) FROM inference_budget WHERE pending=1)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
    assert_eq!(state, (0, 0, 0, 0, 1, 1));
    drop(db);
    assert!(!command(&root, "--config", "config.json")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::remove_dir_all(root).unwrap();
}
