use jarvis::storage::{SqliteStore, Task};
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    os::fd::FromRawFd,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn setup(name: &str, seconds: u64) -> (std::path::PathBuf, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("jarvis-approval-{name}-{}", std::process::id()));
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
        + seconds;
    let input = path.join("approval.json");
    std::fs::write(&input, json!({"store":path,"task":"task","revision":1,"approval":"one","expires_unix":expires,"request":{"tool":"write","action":"write","resource":"mock/item","data_class":"public","arguments":{"text":"hello"}}}).to_string()).unwrap();
    (path, input)
}

fn finish(mut child: Child, path: &std::path::Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("approval process did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{:?}",
        output
    );
    let db = rusqlite::Connection::open(path.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM tool_approvals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn cancellation_while_decision_pipe_is_held_open() {
    let (path, input) = setup("cancel-input", 60);
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "approve", "--input"])
        .arg(input)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("Type approve"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    finish(child, &path, "cancelled");
}

#[test]
fn full_output_pipe_expires_and_cancels() {
    for (name, seconds, signal, expected) in [
        ("expiry-output", 2, false, "timeout"),
        ("cancel-output", 60, true, "cancelled"),
    ] {
        let (path, input) = setup(name, seconds);
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
            0
        );
        let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        use std::os::fd::AsRawFd;
        let data = [b'x'; 4096];
        while unsafe { libc::write(writer.as_raw_fd(), data.as_ptr().cast(), data.len()) } > 0 {}
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["agent", "approve", "--input"])
            .arg(input)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if signal {
            std::thread::sleep(Duration::from_millis(500));
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        }
        finish(child, &path, expected);
        drop(reader);
    }
}

#[test]
fn unsupported_character_device_is_rejected() {
    let (path, input) = setup("device", 60);
    let child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["agent", "approve", "--input"])
        .arg(input)
        .stdin(Stdio::from(std::fs::File::open("/dev/zero").unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    finish(child, &path, "agent_io");
}
