use jarvis::storage::SqliteStore;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::fd::{FromRawFd, OwnedFd},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn setup(label: &str) -> (std::path::PathBuf, TcpListener) {
    let root = std::env::temp_dir().join(format!("jarvis-review-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    drop(SqliteStore::create(&root).unwrap());
    (root, TcpListener::bind("127.0.0.1:0").unwrap())
}
fn config(root: &std::path::Path, port: u16, models: serde_json::Value) -> std::path::PathBuf {
    let path = root.join("chat.json");
    fs::write(&path, serde_json::json!({"models": models, "limits":{"timeout_seconds":1, "max_tokens":5}, "budget":{"workspace":root,"limit_tokens":100,"max_input_tokens":10}}).to_string()).unwrap();
    let _ = port;
    path
}
fn model(name: &str, port: u16, trusted: bool) -> serde_json::Value {
    serde_json::json!({"name":name,"tier":"t1","capabilities":["text"],"allowed_data":["public"],"trusted_no_truncation":trusted,"port":port})
}
fn serve(listener: TcpListener, pieces: Vec<&'static str>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        assert!(s.read(&mut buf).unwrap() > 0);
        let mut body = String::new();
        for piece in pieces {
            body.push_str(&format!(
                "{}\n",
                serde_json::json!({"response":piece,"done":false})
            ));
        }
        body.push_str("{\"response\":\"\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":2,\"eval_count\":1}\n");
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    })
}
#[test]
fn eligible_runtime_is_selected() {
    let (root, listener) = setup("route");
    let port = listener.local_addr().unwrap().port();
    let path = config(
        &root,
        port,
        serde_json::json!([model("bad", port, false), model("good", port, true)]),
    );
    let server = serve(listener, vec!["ok"]);
    let out = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(path)
        .arg("hello")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"ok");
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn deadline_after_budget_lock_never_connects() {
    let (root, listener) = setup("deadline");
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let path = config(&root, port, serde_json::json!([model("good", port, true)]));
    let lock = rusqlite::Connection::open(root.join("jarvis.sqlite3")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config"])
        .arg(path)
        .arg("hello")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(40));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGSTOP);
    }
    thread::sleep(Duration::from_millis(1300));
    lock.execute_batch("ROLLBACK").unwrap();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGCONT);
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        listener.accept().is_err(),
        "provider contacted after deadline"
    );
    assert_eq!(out.stderr, b"{\"error\":\"timeout\"}\n");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn terminal_escapes_controls_but_pipe_stays_raw() {
    for tty in [true, false] {
        let (root, listener) = setup(if tty { "tty" } else { "pipe" });
        let port = listener.local_addr().unwrap().port();
        let path = config(&root, port, serde_json::json!([model("good", port, true)]));
        let server = serve(listener, vec!["hello\x1b", "]52;c;evil\x07 world\n"]);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jarvis"));
        cmd.args(["chat", "--config"])
            .arg(path)
            .arg("hello")
            .stderr(Stdio::piped());
        let output = if tty {
            let mut master = -1;
            let mut slave = -1;
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            let master = unsafe { OwnedFd::from_raw_fd(master) };
            let slave = unsafe { OwnedFd::from_raw_fd(slave) };
            let child = cmd.stdout(Stdio::from(slave)).spawn().unwrap();
            drop(cmd);
            let out = child.wait_with_output().unwrap();
            let mut text = String::new();
            let _ = std::fs::File::from(master).read_to_string(&mut text);
            (out, text.into_bytes())
        } else {
            let out = cmd.output().unwrap();
            let text = out.stdout.clone();
            (out, text)
        };
        assert!(
            output.0.status.success(),
            "{}",
            String::from_utf8_lossy(&output.0.stderr)
        );
        if tty {
            assert!(
                !output.1.contains(&0x1b) && !output.1.contains(&0x07),
                "unsafe TTY bytes: {:?}",
                output.1
            );
            assert!(String::from_utf8_lossy(&output.1).contains("52;c;evil"));
        } else {
            assert_eq!(output.1, b"hello\x1b]52;c;evil\x07 world\n");
        }
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
