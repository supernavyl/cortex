Build a cron-style job scheduler library project in Rust.

Layout:

```
src/
  lib.rs            - public API
  expr.rs           - cron expression parser
  schedule.rs       - next-fire-time calculator
  job.rs            - Job trait + JobHandle
  runner.rs         - async scheduler loop
  store.rs          - persistent state (sled-backed)
tests/
  scheduling.rs     - integration tests covering scheduling, persistence, restart
```

`Cargo.toml` deps:
- `tokio = { version = "1", features = ["full"] }`
- `chrono = { version = "0.4", features = ["serde"] }`
- `serde = { version = "1", features = ["derive"] }`
- `sled = "0.34"`
- `thiserror = "1"`
- `tracing = "0.1"`
- `async-trait = "0.1"`

Public API (`lib.rs`):

```rust
pub use expr::{CronExpr, ParseError};
pub use job::{Job, JobId, JobHandle};
pub use schedule::Schedule;
pub use runner::Scheduler;
```

`CronExpr` supports the full 5-field syntax: `minute hour day-of-month month day-of-week`,
with `*`, `*/N`, `A,B,C`, and `A-B` ranges. (No predefined `@hourly` etc.)

`Scheduler`:
- `Scheduler::new(state_dir: impl AsRef<Path>) -> Result<Self>` — opens sled DB
- `schedule_job(name: &str, cron: CronExpr, job: Arc<dyn Job>) -> JobId` — non-async
- `run(&self, shutdown: CancellationToken)` — async loop, ticks every second
- Persists last-run-time per job; on restart, missed-run policy = "fire once" (catch-up at most one)

`Job` trait:
```rust
#[async_trait]
pub trait Job: Send + Sync {
    async fn run(&self) -> Result<(), JobError>;
    fn name(&self) -> &str;
}
```

Required tests:
- `test_parse_cron_every_minute` — `* * * * *` parses ok
- `test_parse_cron_complex_ranges` — `0,15,30,45 9-17 * * 1-5` parses ok
- `test_parse_invalid_field_errors`
- `test_next_fire_time_every_minute` — Schedule::next_after returns now+1min
- `test_next_fire_time_skips_weekends` — `0 9 * * 1-5` on Saturday returns Monday
- `test_scheduler_fires_job_on_time` — schedule a job at the next minute boundary
  using a counter-incrementing impl; assert counter > 0 after 90s timeout
- `test_persistence_across_restart` — schedule a job, drop scheduler, reopen with
  same state_dir; assert last_run_time is preserved (read via public accessor)

`cargo check` clean, `cargo test` passes (use `tokio::time::pause()` / `advance()` for
deterministic time in unit tests; the 90s integration test is fine to actually wait).

No `unwrap()` in library code.
