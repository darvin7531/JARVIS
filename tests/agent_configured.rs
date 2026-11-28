use serde_json::json;
use std::process::Command;

#[test]
fn configured_agent_reaches_shared_config_validation() {
    let path = std::env::temp_dir().join(format!("agent-configured-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let owner = json!({"store":path, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"value", "expected":"value", "expires_unix":4102444800_u64, "mode":"write", "context":"test"});
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
        let owner = json!({"store":root, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"approved", "expected":"approved", "expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "context":"produce a JSON proposal"});
        let config = json!({"models":[{"name":"model", "tier":"t1", "capabilities":["text"], "allowed_data":["public"], "trusted_no_truncation":true, "port":port}], "limits":{"max_tokens":100}, "budget":{"workspace":root, "limit_tokens":1000, "max_input_tokens":200}});
        let owner_path = root.join("owner.json");
        let config_path = root.join("config.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(&config_path, config.to_string()).unwrap();
        let proposal = json!({"kind":"tool", "request":request("write", proposed)}).to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        let payload: serde_json::Value =
                            serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                        assert!(
                            payload["format"].is_object(),
                            "agent must request a proposal schema"
                        );
                        break;
                    }
                }
            }
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
    let owner = json!({"store":root,"task":"task","revision":1,"approval":"write","read_approval":"read","resource":"mock/item","data_class":"sensitive","permitted_text":"x","expected":"x","expires_unix":4102444800_u64,"mode":"write", "context":"secret"});
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

#[test]
fn configured_rejection_finishes_only_proven_pre_dispatch_steps() {
    use jarvis::storage::{SqliteStore, Task};
    use std::{io::Read, net::TcpListener};

    for case in ["policy", "budget", "unknown"] {
        let root =
            std::env::temp_dir().join(format!("agent-rejection-{}-{case}", std::process::id()));
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
        let owner = json!({"store":root,"task":"task","revision":1,"approval":"write","read_approval":"read","resource":"mock/item","data_class":if case == "policy" {"sensitive"} else {"public"},"permitted_text":"x","expected":"x","expires_unix":4102444800_u64,"mode":"write", "context":"produce a proposal"});
        let config = json!({"models":[{"name":"model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":listener.local_addr().unwrap().port()}],"limits":{"max_tokens":100},"budget":{"workspace":root,"limit_tokens":if case == "budget" {0} else {1000},"max_input_tokens":200}});
        let owner_path = root.join("owner.json");
        let config_path = root.join("config.json");
        std::fs::write(&owner_path, owner.to_string()).unwrap();
        std::fs::write(&config_path, config.to_string()).unwrap();
        let server = if case == "unknown" {
            let server = listener.try_clone().unwrap();
            Some(std::thread::spawn(move || {
                server.set_nonblocking(false).unwrap();
                let (mut stream, _) = server.accept().unwrap();
                let mut buf = [0; 4096];
                assert!(stream.read(&mut buf).unwrap() > 0);
                // Close without a terminal usage frame: dispatch and usage are uncertain.
            }))
        } else {
            None
        };
        let run = || {
            Command::new(env!("CARGO_BIN_EXE_jarvis"))
                .args(["agent", "--owner"])
                .arg(&owner_path)
                .arg("--config")
                .arg(&config_path)
                .output()
                .unwrap()
        };
        let output = run();
        if let Some(server) = server {
            server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
        }
        assert!(!output.status.success());
        if case != "unknown" {
            assert!(listener.accept().is_err(), "unexpected provider connection");
        }
        let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
        let state: (i64, i64, i64, i64, i64) = db.query_row("SELECT steps, failures, pending, (SELECT count(*) FROM tool_executions), (SELECT count(*) FROM mock_state) FROM agent_runtime WHERE task_id='task'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap();
        assert_eq!(
            state,
            if case == "unknown" {
                (1, 0, 1, 0, 0)
            } else {
                (1, 1, 0, 0, 0)
            },
            "{case}"
        );
        if case == "unknown" {
            let pending: i64 = db
                .query_row(
                    "SELECT count(*) FROM inference_budget WHERE pending=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pending, 1);
            assert!(!run().status.success());
            assert!(listener.accept().is_err(), "uncertain inference replayed");
        }
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "requires the pinned Ollama smoke model on GitHub Actions"]
fn real_ollama_configured_agent_approved_tool() {
    use jarvis::storage::{SqliteStore, Task};
    use jarvis::tool_policy::{DataClass, ToolRequest};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    let root = std::env::temp_dir().join(format!("agent-ollama-{}", std::process::id()));
    let mut store = SqliteStore::create(&root).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "synthetic".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(2).unwrap();
    let expiry = SystemTime::now() + Duration::from_secs(900);
    let request = |tool: &str, text: &str| ToolRequest {
        tool: tool.into(),
        action: tool.into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":text}),
    };
    let write = request("write", "accepted");
    store
        .issue_tool_approval("write", "task", 1, &write, expiry)
        .unwrap();
    store
        .issue_tool_approval("read", "task", 1, &request("read", ""), expiry)
        .unwrap();
    drop(store);
    let proposal = json!({"kind":"tool", "request":write});
    let owner = json!({"store":root, "task":"task", "revision":1, "approval":"write", "read_approval":"read", "resource":"mock/item", "data_class":"public", "permitted_text":"accepted", "expected":"accepted", "expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(), "mode":"write", "max_steps":1, "max_failures":1, "context":format!("Output only this JSON object, with no explanation or markdown: {proposal}")});
    let config = json!({"models":[{"name":"jarvis-smoke", "tier":"t1", "capabilities":["text"], "allowed_data":["public"], "trusted_no_truncation":true, "port":11434}], "limits":{"max_tokens":512}, "budget":{"workspace":root, "limit_tokens":2560, "max_input_tokens":2048}});
    let owner_path = root.join("owner.json");
    let config_path = root.join("config.json");
    std::fs::write(&owner_path, owner.to_string()).unwrap();
    std::fs::write(&config_path, config.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "--owner"])
        .arg(&owner_path)
        .arg("--config")
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "configured agent failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "verified");
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let state: (String, String, i64, i64, i64, i64) = db.query_row("SELECT value, data_class, (SELECT count(*) FROM tool_approvals WHERE id IN ('write','read') AND consumed=1), (SELECT count(*) FROM tool_executions WHERE approval_id IN ('write','read') AND pending=0), (SELECT used FROM tool_budget), (SELECT count(*) FROM inference_budget WHERE pending=0 AND tokens>0) FROM mock_state WHERE resource='mock/item'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    assert_eq!(state, ("accepted".into(), "public".into(), 2, 2, 2, 1));
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
