use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Guard(std::process::Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn config(port: u16, timeout: u64, name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "jarvis-security-{name}-{}.json",
        std::process::id()
    ));
    fs::write(&path, format!(r#"{{"limits":{{"timeout_seconds":{timeout},"max_output_bytes":524288}},"models":[{{"name":"test","tier":"t1","capabilities":["text"],"allowed_data":["sensitive","public"],"trusted_local":true,"port":{port}}}]}}"#)).unwrap();
    path
}

#[test]
fn sensitive_argv_is_rejected() {
    let path = config(1, 5, "argv");
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args([
            "chat",
            "--config",
            path.to_str().unwrap(),
            "--data",
            "sensitive",
            "secret",
        ])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(output.stderr, b"{\"error\":\"sensitive_requires_stdin\"}\n");
}

#[test]
fn sensitive_stdin_reaches_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let path = config(listener.local_addr().unwrap().port(), 5, "stdin");
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        let headers = String::from_utf8(headers).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|s| {
                s.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["prompt"], "secret from stdin");
        socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"response\":\"ok\",\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}\n").unwrap();
    });
    let mut child = Guard(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args([
                "chat",
                "--config",
                path.to_str().unwrap(),
                "--data",
                "sensitive",
                "--stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"secret from stdin")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "stdin request hung");
        thread::sleep(Duration::from_millis(20));
    }
    let mut stdout = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert_eq!(stdout, "ok");
    assert!(child.0.wait().unwrap().success());
    server.join().unwrap();
    fs::remove_file(path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn full_stdout_pipe_respects_sigint_and_timeout() {
    for (signal, timeout) in [(true, 10), (false, 1)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let path = config(
            listener.local_addr().unwrap().port(),
            timeout,
            if signal { "signal" } else { "timeout" },
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .unwrap();
            let line = format!(
                "{{\"response\":\"{}\",\"done\":false}}\n",
                "x".repeat(200000)
            );
            let _ = socket.write_all(line.as_bytes());
            tx.send(()).unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let mut child = Guard(
            Command::new(env!("CARGO_BIN_EXE_jarvis"))
                .args(["chat", "--config", path.to_str().unwrap(), "test"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        rx.recv_timeout(Duration::from_secs(4)).unwrap();
        thread::sleep(Duration::from_millis(300));
        if signal {
            assert!(Command::new("kill")
                .args(["-INT", &child.0.id().to_string()])
                .status()
                .unwrap()
                .success());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "blocked stdout prevented shutdown"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let mut err = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert!(
            err.contains(if signal { "cancelled" } else { "timeout" }),
            "{err}"
        );
        server.join().unwrap();
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn length_finish_reports_usage_and_budget_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let path = config(listener.local_addr().unwrap().port(), 5, "length");
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let _ = socket.read(&mut buf);
        socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"response\":\"partial\",\"done\":true,\"done_reason\":\"length\",\"prompt_eval_count\":2,\"eval_count\":3}\n").unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config", path.to_str().unwrap(), "hello"])
        .output()
        .unwrap();
    server.join().unwrap();
    fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"partial");
    assert_eq!(
        output.stderr,
        b"{\"input_tokens\":2,\"output_tokens\":3}\n{\"error\":\"budget_exceeded\"}\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn inherited_stdout_flags_remain_unchanged() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let path = config(listener.local_addr().unwrap().port(), 5, "shared-flags");
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let _ = socket.read(&mut buf);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
            .unwrap();
        let line = format!(
            "{{\"response\":\"{}\",\"done\":false}}\n",
            "x".repeat(200000)
        );
        let _ = socket.write_all(line.as_bytes());
        thread::sleep(Duration::from_secs(2));
    });
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let _reader = unsafe { fs::File::from_raw_fd(fds[0]) };
    let writer = unsafe { fs::File::from_raw_fd(fds[1]) };
    let fd = writer.as_raw_fd();
    let mut child = Guard(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args(["chat", "--config", path.to_str().unwrap(), "test"])
            .stdout(Stdio::from(writer.try_clone().unwrap()))
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let mut available: libc::c_int = 0;
        assert_eq!(
            unsafe { libc::ioctl(_reader.as_raw_fd(), libc::FIONREAD, &mut available) },
            0
        );
        if available > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "stream did not reach stdout");
        thread::sleep(Duration::from_millis(10));
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_eq!(
        flags & libc::O_NONBLOCK,
        0,
        "shared stdout became nonblocking"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
        0
    );
    server.join().unwrap();
    fs::remove_file(path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn stdin_without_eof_respects_timeout_and_signal() {
    for (signal, timeout) in [(false, 1), (true, 10)] {
        let path = config(
            1,
            timeout,
            if signal {
                "stdin-signal"
            } else {
                "stdin-timeout"
            },
        );
        let mut child = Guard(
            Command::new(env!("CARGO_BIN_EXE_jarvis"))
                .args([
                    "chat",
                    "--config",
                    path.to_str().unwrap(),
                    "--data",
                    "sensitive",
                    "--stdin",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        thread::sleep(Duration::from_millis(300));
        if signal {
            assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGINT) }, 0);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "stdin without EOF hung");
            thread::sleep(Duration::from_millis(20));
        }
        let mut err = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert!(
            err.contains(if signal { "cancelled" } else { "timeout" }),
            "{err}"
        );
        fs::remove_file(path).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn sensitive_terminal_input_is_rejected() {
    use std::os::fd::FromRawFd;
    let path = config(1, 5, "tty");
    let (mut master, mut slave) = (0, 0);
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
    let _master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let mut child = Guard(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .args([
                "chat",
                "--config",
                path.to_str().unwrap(),
                "--data",
                "sensitive",
                "--stdin",
            ])
            .stdin(Stdio::from(slave))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "sensitive terminal input was not rejected"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let mut err = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(err.contains("sensitive_tty"), "{err}");
    fs::remove_file(path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn stdout_pipe_without_reader_fails() {
    use std::os::fd::FromRawFd;
    let path = config(1, 2, "no-reader");
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    unsafe {
        libc::close(fds[0]);
    }
    let writer = unsafe { fs::File::from_raw_fd(fds[1]) };
    let output = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(["chat", "--config", path.to_str().unwrap(), "test"])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("output_failed"),
        "{:?}",
        output
    );
    fs::remove_file(path).unwrap();
}
