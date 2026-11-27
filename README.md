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

Logs are JSON lifecycle events or a fixed error code. This is a foundation, not a provider, task engine or secret manager.

Storage workspaces are private directories (no group or other permissions); opening an existing workspace with broader permissions fails without changing them. The database must be a regular file, not a symlink. Schema objects and SQLite integrity and foreign keys are checked before migration. These checks assume the workspace is not concurrently modified by untrusted processes; they do not protect against another process with the same user privileges or pre-existing hard links and ACLs.

Backups retry SQLite steps for approximately three seconds with 100 ms SQLite busy timeouts; this is not a hard wall-clock deadline: individual SQLite calls, filesystem I/O, scheduling, and publication can take longer. A backup is published only after completion and never replaces an existing destination. New evidence provenance and classification must be nonblank strings of at most 1024 bytes; they are not authenticated security labels. Observed time, when present, must be a valid UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` or `YYYY-MM-DDTHH:MM:SS.sssZ` format. Existing legacy metadata is not retroactively validated.

R03 is incomplete: `chat --mock` only echoes input (not a streaming provider). The routing and event decoder are experimental; no real backend, token accounting, cancellation, or timeout exists yet. `--openai` does not send requests, even if an API key is set. No live chat readiness is claimed.
