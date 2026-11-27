use std::{fs, process::Command};

#[test]
fn t0_ping_is_a_local_rule_without_any_models() {
    let path = std::env::temp_dir().join(format!("jarvis-t0-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--tier",
            "t0",
            "ping",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"pong\n");
}

#[test]
fn t0_full_stdout_respects_timeout() {
    use std::{
        os::fd::FromRawFd,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let path = std::env::temp_dir().join(format!("jarvis-t0-blocked-{}.json", std::process::id()));
    fs::write(&path, r#"{"models":[],"limits":{"timeout_seconds":1}}"#).unwrap();
    let mut fds = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) },
        0
    );
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    let bytes = [0u8; 4096];
    while unsafe { libc::write(fds[1], bytes.as_ptr().cast(), bytes.len()) } > 0 {}
    assert_eq!(unsafe { libc::fcntl(fds[1], libc::F_SETFL, 0) }, 0);
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--tier",
            "t0",
            "ping",
        ])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let exited = child.try_wait().unwrap().is_some();
    if !exited {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    drop(reader);
    fs::remove_file(path).unwrap();
    assert!(exited, "T0 blocked on a full stdout pipe beyond timeout");
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"{\"error\":\"timeout\"}\n");
}
