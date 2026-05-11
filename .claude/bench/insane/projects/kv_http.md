Build a complete HTTP key-value server project in Rust.

Layout:

```
src/
  main.rs           - CLI entry point: parses --port, --db-path, --bind
  lib.rs            - re-exports public API
  server.rs         - axum router + handlers
  store.rs          - storage abstraction trait + sled-backed impl
  config.rs         - Config struct + load_from_env
tests/
  integration.rs    - end-to-end HTTP tests using axum::test or reqwest
```

`Cargo.toml` deps:
- `axum = "0.8"`
- `tokio = { version = "1", features = ["full"] }`
- `serde = { version = "1", features = ["derive"] }`
- `serde_json = "1"`
- `sled = "0.34"`
- `clap = { version = "4", features = ["derive"] }`
- `anyhow = "1"`
- `thiserror = "1"`
- `tracing = "0.1"`
- `tracing-subscriber = "0.3"`

[dev-dependencies]:
- `tempfile = "3"`
- `reqwest = { version = "0.12", features = ["json"] }`

HTTP API (all endpoints return JSON):

- `GET    /kv/:key`           → 200 `{"key":"...","value":"..."}` or 404
- `PUT    /kv/:key`           → body: `{"value":"..."}` → 201 `{"key":"..."}`
- `DELETE /kv/:key`           → 204 if deleted, 404 if absent
- `GET    /kv?prefix=<p>`     → 200 `[{"key":"...","value":"..."}, ...]`
- `GET    /healthz`           → 200 `{"status":"ok","entries":<usize>}`

Required behaviors:
- Errors return JSON: `{"error":"...","code":<u16>}` with correct HTTP status
- All `tracing::info!` requests are logged with method + path + status + duration_ms
- Graceful shutdown on SIGTERM / Ctrl-C (drop sled DB cleanly)
- CLI defaults: --bind 127.0.0.1:8080, --db-path ./kv.db

Tests (must pass):
- `test_put_then_get_returns_value`
- `test_get_missing_returns_404`
- `test_delete_then_get_returns_404`
- `test_prefix_scan_returns_matching_keys`
- `test_healthz_includes_entry_count`
- `test_put_replaces_existing_value`
- `test_concurrent_writes_no_data_loss` (10 parallel PUTs, count == 10)

`cargo check` clean, `cargo test` all pass. No `unwrap()` in library code.
