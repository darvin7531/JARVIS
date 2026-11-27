# JARVIS foundation

Linux, Rust 1.89.0. No credentials or LLM needed. Run `cargo run --locked`; press Enter, send SIGINT or SIGTERM to stop. EOF alone does not stop the service; stdin errors are ignored, so service managers may use `/dev/null`.

Optional `--config path.json` accepts an empty JSON object (`{}`); there are currently no configuration options. Config files are limited to 64 KiB; unknown fields, malformed JSON, oversized and unreadable files are rejected without echoing input or paths. Do not put secrets in config.

Development checks (also run in CI):

```sh
cargo build --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Service logs are JSON lifecycle events or a fixed error code. Chat has a separate local inference interface described below; there is no task engine or secret manager.

Storage workspaces are private directories (no group or other permissions); opening an existing workspace with broader permissions fails without changing them. The database must be a regular file, not a symlink. Schema objects and SQLite integrity and foreign keys are checked before migration. These checks assume the workspace is not concurrently modified by untrusted processes; they do not protect against another process with the same user privileges or pre-existing hard links and ACLs.

Backups retry SQLite steps for approximately three seconds with 100 ms SQLite busy timeouts; this is not a hard wall-clock deadline: individual SQLite calls, filesystem I/O, scheduling, and publication can take longer. A backup is published only after completion and never replaces an existing destination. New evidence provenance and classification must be nonblank strings of at most 1024 bytes; they are not authenticated security labels. Observed time, when present, must be a valid UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` or `YYYY-MM-DDTHH:MM:SS.sssZ` format. Existing legacy metadata is not retroactively validated.

Configured chat connects to a loopback Ollama-compatible server and requires `curl` on PATH. Example `chat.json` (no secrets):

```json
{"models":[{"name":"smollm2:135m-instruct-q4_0","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":11434}]}
```

Loopback is not a privacy guarantee: Ollama can forward requests to cloud models, and a local server can be malicious. By default, only public data can be sent, even if `allowed_data` lists other classes. To permit internal or sensitive data, the operator must explicitly set `"trusted_local":true` on each model profile, after verifying and controlling the server and its model, disabling cloud features (e.g. start Ollama with `OLLAMA_NO_CLOUD=1`), and applying appropriate network isolation. This flag is an operator assertion, not attestation; JARVIS cannot verify that the server or model is private. Direct library callers must enforce their own data policy.

Sensitive prompts must use `--data sensitive --stdin` as the final option, with UTF-8 prompt bytes supplied on stdin until EOF (maximum 1 MiB); argv prompts marked sensitive are rejected, but rejection cannot erase argv exposure that already occurred. Avoid shell history or process arguments when supplying sensitive text; protect the pipe and output too. `--stdin` also works for other data classes. Sensitive terminal stdin is rejected to prevent echo; supply input via a protected pipe or file. Stdin reading is subject to the request timeout and SIGINT/SIGTERM; close it to start inference. For example, `jarvis chat --config chat.json --data sensitive --stdin < private-prompt.txt`. Pipe stdin and stdout use independently reopened nonblocking descriptors on Linux (/proc must be mounted); socket stdin and stdout are rejected. Regular files and devices may still block in the kernel, so their deadlines are not guaranteed.

Run `jarvis chat --config chat.json --tier t1 --data public 'Hello'`. Optional `--model NAME` selects a configured model, `--tier` accepts `t1` through `t3`, `--require` accepts `text`, `vision`, or `tools`, and `--data` accepts `public`, `internal`, or `sensitive`. Configuration labels are operator assertions, not verified model capabilities or security guarantees; this interface sends only text and grants no tool execution. Output streams to stdout; token usage is JSON on stderr. Requests default to 128 output tokens, 64 KiB output and 120 seconds; set `"limits":{"max_tokens":128,"max_output_bytes":65536,"timeout_seconds":120}` in the JSON to change them. Without `--tier`, deterministic prefixes `code:` and `complex:` select T2 and T3; all other prompts select T1. Explicit `--tier` overrides classification and `--model` selects a named profile across tiers, still subject to data and capability filters. This CLI accepts text only: `--require vision` and `--require tools` are rejected even when a model profile claims them. SIGINT/SIGTERM cancel active generation. On failure after generation begins, stderr reports `{"usage":null}` when final usage is unavailable, followed by an error; a token-length termination instead reports available usage and `budget_exceeded`; partial stdout must not be treated as a completed response. No provider fallback occurs. Each CLI invocation issues one request; a shared cross-process concurrency limit is not implemented. `--openai` remains a nonfunctional experimental placeholder.

The supported backend is an Ollama-compatible loopback server; there is no paid backend or monetary budget enforcement. The output byte limit is enforced during streaming; the token limit is requested from Ollama and checked against its final reported usage, not independently counted during streaming. Prompts are limited to 1 MiB and config files to 64 KiB. The provider API requires callbacks to return promptly; unlike the CLI output path, arbitrary callbacks cannot be interrupted by timeout or cancellation.
