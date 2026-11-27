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

Configured local chat uses a loopback Ollama server and requires `curl` on PATH. Example `chat.json` (no secrets):

```json
{"models":[{"name":"smollm2:135m-instruct-q4_0","tier":"t1","capabilities":["text"],"allowed_data":["public"],"port":11434}]}
```

Run `jarvis chat --config chat.json --tier t1 --data public 'Hello'`. Optional `--model NAME` selects a configured model, `--tier` accepts `t1` through `t3`, `--require` accepts `text`, `vision`, or `tools`, and `--data` accepts `public`, `internal`, or `sensitive`. Configuration labels are operator assertions, not verified model capabilities or security guarantees; this interface sends only text and grants no tool execution. Output streams to stdout; token usage is JSON on stderr. Requests default to 128 output tokens, 64 KiB output and 120 seconds; set `"limits":{"max_tokens":128,"max_output_bytes":65536,"timeout_seconds":120}` in the JSON to change them. Without `--tier`, deterministic prefixes `code:` and `complex:` select T2 and T3; all other prompts select T1. Explicit `--tier` overrides classification and `--model` selects a named profile across tiers, still subject to data and capability filters. This CLI accepts text only: `--require vision` and `--require tools` are rejected even when a model profile claims them. SIGINT/SIGTERM cancel active generation. On failure after generation begins, stderr reports `{"usage":null}` because final usage is unavailable, followed by an error; partial stdout must not be treated as a completed response. No provider fallback occurs. Each CLI invocation issues one request; a shared cross-process concurrency limit is not implemented. `--openai` remains a nonfunctional experimental placeholder.
