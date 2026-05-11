Build a rate limiter service project in Rust with two algorithms exposed via HTTP API.

Layout:

```
src/
  main.rs           - axum service binary
  lib.rs            - public API
  algo/mod.rs       - RateLimiter trait
  algo/token_bucket.rs   - token bucket impl
  algo/sliding_window.rs - sliding window log impl
  store.rs          - per-key state (DashMap)
  api.rs            - HTTP handlers
tests/
  algos.rs          - unit tests for each algorithm
  service.rs        - integration HTTP tests
```

`Cargo.toml` deps:
- `axum = "0.8"`
- `tokio = { version = "1", features = ["full"] }`
- `serde = { version = "1", features = ["derive"] }`
- `serde_json = "1"`
- `dashmap = "6"`
- `clap = { version = "4", features = ["derive"] }`
- `thiserror = "1"`
- `tracing = "0.1"`

[dev-dependencies]:
- `tokio-test = "0.4"`
- `reqwest = "0.12"`

`RateLimiter` trait:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision { Allow, Deny { retry_after_ms: u64 } }

pub trait RateLimiter: Send + Sync {
    fn check(&self, key: &str) -> Decision;
}
```

Token bucket parameters (per-key state):
- `capacity` (max tokens)
- `refill_rate_per_sec`
- `now: Instant`-based; tokens accrue at refill_rate between checks

Sliding window log:
- `window_secs: u64`
- `max_requests: u64`
- store: `Vec<Instant>` per key; evict timestamps older than window on check

HTTP API:
- `POST /limit?key=<key>&algo=<token_bucket|sliding_window>`
  → 200 if Allow: `{"decision":"allow"}`
  → 429 if Deny: `{"decision":"deny","retry_after_ms":N}`
- `GET /healthz`
- `GET /metrics` — Prometheus-style text: `allow_total`, `deny_total`, per-algo counters

CLI: `--bind 127.0.0.1:8080`, `--bucket-capacity`, `--bucket-refill`, `--window-secs`, `--window-max`.

Tests:
- `test_token_bucket_allows_below_capacity`
- `test_token_bucket_denies_when_empty`
- `test_token_bucket_refills_over_time`
- `test_sliding_window_allows_below_max`
- `test_sliding_window_denies_at_max`
- `test_sliding_window_evicts_old_entries` — wait past window, check resets
- `test_per_key_isolation` — different keys have independent counters
- `test_http_returns_429_when_denied`
- `test_metrics_endpoint_reports_counters`

`cargo check` clean, `cargo test` all pass.
