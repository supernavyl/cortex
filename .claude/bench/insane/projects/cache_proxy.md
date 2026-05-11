Build a caching HTTP reverse proxy project in Rust.

Layout:

```
src/
  main.rs           - CLI: bind addr, upstream URL, cache size
  lib.rs            - public API
  proxy.rs          - request forwarding + response rewriting
  cache.rs          - LRU + TTL cache (in-memory)
  hop_headers.rs    - hop-by-hop header stripping
  config.rs         - config struct
tests/
  proxy.rs          - integration: spin up upstream + proxy, hit proxy, verify
                      cache hits and TTL expiry
```

`Cargo.toml` deps:
- `tokio = { version = "1", features = ["full"] }`
- `hyper = { version = "1", features = ["full"] }`
- `hyper-util = { version = "0.1", features = ["full"] }`
- `http-body-util = "0.1"`
- `bytes = "1"`
- `parking_lot = "0.12"`
- `clap = { version = "4", features = ["derive"] }`
- `thiserror = "1"`
- `tracing = "0.1"`

[dev-dependencies]:
- `reqwest = "0.12"`
- `tempfile = "3"`

Behavior:
- Forwards all incoming HTTP requests to a configured upstream URL
- Caches GET responses with status 200..=299 by request URL + Vary headers
- Cache entry TTL is the Cache-Control max-age (or 60s if absent)
- Cache eviction: LRU with configurable max bytes
- Hop-by-hop headers are stripped per RFC 7230 §6.1 (Connection, Keep-Alive, ...)
- Cache hits add an `X-Cache: HIT` response header; misses add `X-Cache: MISS`
- POST/PUT/DELETE are forwarded but never cached

Public API:

```rust
pub use cache::Cache;
pub use config::ProxyConfig;
pub use proxy::Proxy;

impl Proxy {
    pub fn new(upstream: hyper::Uri, cache: Cache) -> Self;
    pub async fn serve(self, listen_addr: SocketAddr) -> Result<(), ProxyError>;
}

impl Cache {
    pub fn new(max_bytes: usize) -> Self;
    pub fn len(&self) -> usize;
    pub fn current_bytes(&self) -> usize;
}
```

Tests:
- `test_cache_hit_sets_header` — first request: X-Cache: MISS; second: X-Cache: HIT
- `test_cache_respects_max_age` — set max-age=1s, wait 1.5s, second request is MISS
- `test_cache_respects_no_cache` — Cache-Control: no-store responses never cached
- `test_cache_lru_eviction_when_full` — fill cache past max_bytes; oldest entry
  evicted; current_bytes() ≤ max_bytes
- `test_post_not_cached` — POST same URL twice; both hit upstream (use a counter
  in the upstream mock)
- `test_hop_by_hop_headers_stripped` — upstream sends `Connection: close`, proxy
  response does not contain it
- `test_streaming_response_body_forwards` — upstream returns 100 KB body in chunks;
  proxy forwards same bytes

`cargo check` clean, `cargo test` all pass.
