//! Streaming local Ollama provider. Prompts and server errors are never logged.
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    Cancelled,
    Timeout,
    Budget,
    Incomplete(Usage),
    Transport,
    InvalidStream,
    Truncated,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
pub struct Request<'a> {
    pub model: &'a str,
    pub prompt: &'a str,
    pub max_tokens: u32,
    pub max_output_bytes: usize,
    pub timeout: Duration,
}
/// Emitted text is provisional until `generate` returns success with usage.
/// Callbacks must return promptly: timeout and cancellation cannot interrupt them.
pub trait Provider {
    fn generate(
        &self,
        request: &Request<'_>,
        cancelled: &AtomicBool,
        emit: &mut dyn FnMut(&str),
    ) -> Result<Usage, Error>;
}
fn validate(r: &Request<'_>, cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if r.model.is_empty()
        || r.prompt.len() > 1024 * 1024
        || r.max_tokens == 0
        || r.max_output_bytes == 0
        || r.timeout.is_zero()
    {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
pub struct MockProvider {
    text: String,
}
impl MockProvider {
    pub fn new(text: &str) -> Self {
        Self { text: text.into() }
    }
}
impl Provider for MockProvider {
    fn generate(
        &self,
        r: &Request<'_>,
        cancelled: &AtomicBool,
        emit: &mut dyn FnMut(&str),
    ) -> Result<Usage, Error> {
        validate(r, cancelled)?;
        if self.text.len() > r.max_output_bytes {
            return Err(Error::Budget);
        }
        emit(&self.text);
        Ok(Usage {
            input_tokens: 1,
            output_tokens: 1,
        })
    }
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Uses curl for HTTP framing. Loopback does not guarantee the server handles data locally.
pub struct Ollama {
    pub port: u16,
}
impl Provider for Ollama {
    fn generate(
        &self,
        r: &Request<'_>,
        cancelled: &AtomicBool,
        emit: &mut dyn FnMut(&str),
    ) -> Result<Usage, Error> {
        validate(r, cancelled)?;
        let payload = serde_json::to_vec(&serde_json::json!({"model":r.model,"prompt":r.prompt,"stream":true,"options":{"num_predict":r.max_tokens}})).map_err(|_| Error::InvalidRequest)?;
        let mut child = ChildGuard(
            Command::new("curl")
                .args([
                    "--disable",
                    "--silent",
                    "--fail",
                    "--no-buffer",
                    "--noproxy",
                    "*",
                    "--header",
                    "Content-Type: application/json",
                    "--data-binary",
                    "@-",
                    &format!("http://127.0.0.1:{}/api/generate", self.port),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| Error::Transport)?,
        );
        let mut input = child.0.stdin.take().ok_or(Error::Transport)?;
        thread::spawn(move || {
            let _ = input.write_all(&payload);
        });
        let mut output = child.0.stdout.take().ok_or(Error::Transport)?;
        let (tx, rx) = mpsc::sync_channel(2);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match output.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(Err(()));
                        break;
                    }
                }
            }
        });
        let started = Instant::now();
        let mut line = Vec::new();
        let mut output_bytes = 0usize;
        let mut usage = None;
        let mut incomplete = false;
        let result = loop {
            if cancelled.load(Ordering::Relaxed) {
                break Err(Error::Cancelled);
            }
            if started.elapsed() >= r.timeout {
                break Err(Error::Timeout);
            }
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(bytes)) => {
                    let mut error = None;
                    for byte in bytes {
                        if usage.is_some() {
                            error = Some(Error::InvalidStream);
                            break;
                        }
                        if byte != b'\n' {
                            line.push(byte);
                            if line.len() > 1024 * 1024 {
                                error = Some(Error::Budget);
                                break;
                            }
                            continue;
                        }
                        let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&line);
                        line.clear();
                        let value = match parsed {
                            Ok(v) => v,
                            Err(_) => {
                                error = Some(Error::InvalidStream);
                                break;
                            }
                        };
                        let Some(done) = value.get("done").and_then(|v| v.as_bool()) else {
                            error = Some(Error::InvalidStream);
                            break;
                        };
                        let Some(text) = value.get("response").and_then(|v| v.as_str()) else {
                            error = Some(Error::InvalidStream);
                            break;
                        };
                        output_bytes = match output_bytes.checked_add(text.len()) {
                            Some(n) if n <= r.max_output_bytes => n,
                            _ => {
                                error = Some(Error::Budget);
                                break;
                            }
                        };
                        if done {
                            match value.get("done_reason") {
                                Some(reason) if reason.as_str() == Some("stop") => {}
                                Some(reason) if reason.as_str() == Some("length") => {
                                    incomplete = true
                                }
                                _ => {
                                    error = Some(Error::InvalidStream);
                                    break;
                                }
                            }
                            match (
                                value.get("prompt_eval_count").and_then(|v| v.as_u64()),
                                value.get("eval_count").and_then(|v| v.as_u64()),
                            ) {
                                (Some(input_tokens), Some(output_tokens))
                                    if output_tokens <= u64::from(r.max_tokens) =>
                                {
                                    usage = Some(Usage {
                                        input_tokens,
                                        output_tokens,
                                    })
                                }
                                _ => {
                                    error = Some(Error::InvalidStream);
                                    break;
                                }
                            }
                        }
                        emit(text);
                    }
                    if let Some(e) = error {
                        break Err(e);
                    }
                }
                Ok(Err(())) => break Err(Error::Transport),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = child.0.wait().map_err(|_| Error::Transport)?;
                    break if !status.success() {
                        Err(Error::Transport)
                    } else if !line.is_empty() {
                        Err(Error::Truncated)
                    } else {
                        match usage {
                            Some(usage) if incomplete => Err(Error::Incomplete(usage)),
                            Some(usage) => Ok(usage),
                            None => Err(Error::Truncated),
                        }
                    };
                }
            }
        };
        result
    }
}
