# JARVIS foundation

Linux, Rust 1.85.1. No credentials or LLM needed. Run `cargo run --locked`; press Enter, send SIGINT or SIGTERM to stop. EOF alone does not stop the service; stdin errors are ignored, so service managers may use `/dev/null`.

Optional `--config path.json` accepts JSON such as `{"shutdown_timeout_secs": 1}`. The value is 1–30 seconds (default 1), retained for compatibility but currently unused: shutdown has no asynchronous cleanup to time out. Signals wake the service promptly regardless of this value. Config files are limited to 64 KiB; unknown fields, invalid values, oversized and unreadable files are rejected without echoing input or paths. Do not put secrets in config.

Development checks (also run in CI):

```sh
cargo build --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Logs are JSON lifecycle events or a fixed error code. This is a foundation, not a provider, task engine or secret manager.
