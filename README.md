# JARVIS foundation

Linux, Rust 1.85.1. No credentials or LLM needed. Run `cargo run --locked`; press Enter, send SIGINT or SIGTERM to stop. EOF also stops the process.

Optional `--config path.json` accepts JSON such as `{"shutdown_timeout_secs": 1}`. The value is 1–30 seconds (default 1) and bounds the signal polling delay. Unknown fields, invalid values and unreadable files are rejected without echoing input or paths. Do not put secrets in config.

Development checks (also run in CI):

```sh
cargo build --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Logs are JSON lifecycle events or a fixed error code. This is a foundation, not a provider, task engine or secret manager.
