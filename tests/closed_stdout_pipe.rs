use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn closed_pipe_reports_failure_on_shutdown() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut started = String::new();
    stdout.read_line(&mut started).unwrap();
    assert_eq!(started, "{\"event\":\"started\"}\n");
    drop(stdout);
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("process did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success(), "closed stdout was reported as success");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert_eq!(stderr, "{\"error\":\"output_failed\"}\n");
}
