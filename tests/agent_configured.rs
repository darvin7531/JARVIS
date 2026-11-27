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
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid_config"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn configured_proposals_require_complete_stream_and_exact_approval() {
    use jarvis::storage::{SqliteStore, Task};
    use jarvis::tool_policy::{DataClass, ToolRequest};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    for (reason, proposed, succeeds) in [
        ("stop", "approved", true),
        ("length", "approved", false),
        ("stop", "forged", false),
    ] {
        let root = std::env::temp_dir().join(format!(
            "configured-agent-{}-{reason}-{proposed}",
            std::process::id()
        ));
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
        store.initialize_tool_budget(4).unwrap();
        let expiry = SystemTime::now() + Duration::from_secs(300);
        let request = |tool: &str, text: &str| ToolRequest {
            tool: tool.into(),
            action: tool.into(),
            resource: "mock/item".into(),
            data_class: DataClass::Public,
            arguments: json!({"text":text}),
        };
        store
            .issue_tool_approval("write", "task", 1, &request("write", "approved"), expiry)
            .unwrap();
        store
            .issue_tool_approval("read", "task", 1, &request("read", ""), expiry)
            .unwrap();
        drop(store);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let owner = json!({"store":root, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(), "context":"produce a JSON proposal"});
        let config = json!({"models":[{"name":"model", "tier":"t1", "capabilities":["text"], "allowed_data":["public"], "trusted_no_truncation":true, "port":port}], "limits":{"max_tokens":100}, "budget":{"workspace":root, "limit_tokens":1000, "max_input_tokens":200}});
        let owner_path = root.join("owner.json");
        let config_path = root.join("config.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(&config_path, config.to_string()).unwrap();
        let proposal = json!({"kind":"tool", "request":request("write", proposed)}).to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            assert!(stream.read(&mut buf).unwrap() > 0);
            let body = format!(
                "{}\n{}\n",
                json!({"response":proposal,"done":false}),
                json!({"response":"","done":true,"done_reason":reason,"prompt_eval_count":20,"eval_count":10})
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["agent", "--owner"])
            .arg(owner_path)
            .arg("--config")
            .arg(config_path)
            .output()
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM mock_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, i64::from(succeeds));
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn sensitive_owner_never_reaches_untrusted_model() {
    use jarvis::storage::{SqliteStore, Task};
    use std::net::TcpListener;
    let root = std::env::temp_dir().join(format!("configured-sensitive-{}", std::process::id()));
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
    drop(store);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let owner = json!({"store":root,"task":"task","revision":1,"approval":"write","read_approval":"read","resource":"mock/item","data_class":"sensitive","permitted_text":"x","expected":"x","expires_unix":4102444800_u64,"context":"secret"});
    let config = json!({"models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["sensitive"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],"budget":{"workspace":root,"limit_tokens":1000,"max_input_tokens":200}});
    let owner_path = root.join("owner.json");
    let config_path = root.join("config.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&config_path, config.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--owner"])
        .arg(owner_path)
        .arg("--config")
        .arg(config_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("data_not_allowed"));
    assert!(listener.accept().is_err());
    std::fs::remove_dir_all(root).unwrap();
}
