use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn config(name: &str, data: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("jarvis-{name}-{}.json", std::process::id()));
    std::fs::write(&path, data).unwrap();
    path
}

fn spawn(path: Option<&PathBuf>, stdin: Stdio) -> (Process, mpsc::Receiver<String>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jarvis"));
    command.env_clear().stdin(stdin).stdout(Stdio::piped());
    if let Some(path) = path {
        command.arg("--config").arg(path);
    }
    let mut child = Process(command.spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    (child, receiver)
}

fn wait(child: &mut Process) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "process did not exit in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn event(receiver: &mpsc::Receiver<String>, name: &str) {
    let line = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("event timed out");
    assert_eq!(line, format!("{{\"event\":\"{name}\"}}"));
}

#[test]
fn newline_stops_service() {
    let (mut child, output) = spawn(None, Stdio::piped());
    event(&output, "started");
    child.0.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(wait(&mut child).success());
    event(&output, "stopped");
}

fn signal_stops_service(signal: &str, name: &str) {
    let path = config(name, b"{}");
    let (mut child, output) = spawn(Some(&path), Stdio::null());
    event(&output, "started");
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.0.try_wait().unwrap().is_none(), "EOF stopped service");
    assert!(Command::new("/bin/kill")
        .args([signal, &child.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    assert!(wait(&mut child).success());
    event(&output, "stopped");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn sigterm_stops_promptly_after_eof() {
    signal_stops_service("-TERM", "term");
}

#[test]
fn sigint_stops_promptly_after_eof() {
    signal_stops_service("-INT", "int");
}

#[test]
fn invalid_config_is_redacted_and_bounded() {
    let path = config("invalid", b"");
    for data in [
        b"{\"secret\":\"sensitive-value\"}".to_vec(),
        b"{\"shutdown_timeout_secs\":1}".to_vec(),
        b"{\"sensitive-value\":".to_vec(),
        vec![b' '; 65537],
    ] {
        std::fs::write(&path, data).unwrap();
        let mut child = Process(
            Command::new(env!("CARGO_BIN_EXE_jarvis"))
                .arg("--config")
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        assert!(!wait(&mut child).success());
        let mut stderr = String::new();
        std::io::Read::read_to_string(&mut child.0.stderr.take().unwrap(), &mut stderr).unwrap();
        assert_eq!(stderr, "{\"error\":\"invalid_config\"}\n");
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn config_at_size_limit_is_accepted() {
    let mut data = b"{}".to_vec();
    data.resize(65536, b' ');
    let path = config("limit", &data);
    let (mut child, output) = spawn(Some(&path), Stdio::piped());
    event(&output, "started");
    child.0.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(wait(&mut child).success());
    event(&output, "stopped");
    std::fs::remove_file(path).unwrap();
}

#[cfg(unix)]
#[test]
fn fifo_config_is_rejected_without_waiting_for_writer() {
    let path = std::env::temp_dir().join(format!("jarvis-fifo-{}", std::process::id()));
    assert!(Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("--config")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!wait(&mut child).success());
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.0.stderr.take().unwrap(), &mut stderr).unwrap();
    assert_eq!(stderr, "{\"error\":\"invalid_config\"}\n");
    std::fs::remove_file(path).unwrap();
}

#[cfg(unix)]
#[test]
fn sigterm_exits_with_full_stdout_pipe() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let _reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let mut writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    loop {
        match writer.write(&[0; 4096]) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("pipe fill failed: {e}"),
        }
    }
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        0
    );
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.0.try_wait().unwrap().is_none());
    assert!(Command::new("/bin/kill")
        .args(["-TERM", &child.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    assert!(wait(&mut child).success());
}

#[cfg(unix)]
#[test]
fn chat_fifo_config_is_rejected_without_waiting_for_writer() {
    let path = std::env::temp_dir().join(format!("jarvis-chat-fifo-{}", std::process::id()));
    assert!(Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_jarvis"))
            .arg("chat")
            .arg("--config")
            .arg(&path)
            .arg("hello")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!wait(&mut child).success());
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.0.stderr.take().unwrap(), &mut stderr).unwrap();
    assert_eq!(stderr, "{\"error\":\"invalid_config\"}\n");
    std::fs::remove_file(path).unwrap();
}
