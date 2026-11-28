use jarvis::storage::{SqliteStore, Task};
use jarvis::tool_policy::{DataClass, ToolRequest};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn unknown_observation_reaches_inference_and_is_independently_verified() {
    for (case, context, value, model) in [
        (
            "adversarial",
            "Report the value",
            "critical incident architecture",
            "t1-model",
        ),
        (
            "owner-heavy",
            "Investigate a critical incident",
            "ordinary value",
            "t3-model",
        ),
        (
            "owner-code",
            "code: report the value",
            "architecture",
            "t2-model",
        ),
        ("ping", "ping", "critical", "t1-model"),
    ] {
        check_observation(case, context, value, model);
    }
}

fn check_observation(case: &str, context: &str, value: &str, model: &str) {
    let root =
        std::env::temp_dir().join(format!("agent-observation-{}-{case}", std::process::id()));
    let mut store = SqliteStore::create(&root).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "diagnose".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.initialize_tool_budget(2).unwrap();
    let expiry = SystemTime::now() + Duration::from_secs(300);
    let request = ToolRequest {
        tool: "read".into(),
        action: "read".into(),
        resource: "mock/item".into(),
        data_class: DataClass::Public,
        arguments: json!({"text":""}),
    };
    store
        .issue_tool_approval("first", "task", 1, &request, expiry)
        .unwrap();
    store
        .issue_tool_approval("verify", "task", 1, &request, expiry)
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO mock_state(resource, value, data_class) VALUES ('mock/item', ?1, 'public')",
        [value],
    )
    .unwrap();
    drop(db);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let value_owned = value.to_owned();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return None,
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let claim = json!({"kind":"claim", "value":value_owned}).to_string();
        let body = format!(
            "{}\n{}\n",
            json!({"response":claim,"done":false}),
            json!({"response":"","done":true,"done_reason":"stop","prompt_eval_count":20,"eval_count":10})
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        Some(String::from_utf8(request).unwrap())
    });
    let owner = json!({"store":root,"task":"task","revision":1,"approval":"first","read_approval":"verify","resource":"mock/item","data_class":"public","permitted_text":"","expected":"","expires_unix":expiry.duration_since(UNIX_EPOCH).unwrap().as_secs(),"mode":"observe","context":context});
    let config = json!({"models":[{"name":"t1-model","tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":port},{"name":"t2-model","tier":"t2","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":port},{"name":"t3-model","tier":"t3","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":true,"port":port}],"limits":{"max_tokens":100},"budget":{"workspace":root,"limit_tokens":1000,"max_input_tokens":512}});
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
    let prompt = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = prompt.unwrap();
    let body = request.split_once("\r\n\r\n").unwrap().1;
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["model"], model, "{case}");
    assert!(body["prompt"].as_str().unwrap().contains(value));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["status"],
        "verified"
    );
    let db = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    let counts: (i64, i64, i64) = db.query_row("SELECT (SELECT count(*) FROM tool_approvals WHERE id IN ('first','verify') AND consumed=1), (SELECT count(*) FROM tool_executions WHERE approval_id IN ('first','verify') AND pending=0), (SELECT used FROM tool_budget)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(counts, (2, 2, 2));
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
