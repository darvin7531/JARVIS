use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn starts_without_credentials_and_shuts_down_on_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("started"), "{line}");
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("stopped"), "{line}");
    assert!(child.wait().unwrap().success());
}
