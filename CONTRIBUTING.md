# Contributing to cortex

Cortex is a Rust verification-first daemon. Single contributor today. This doc exists so future contributors (including future-me) ship without regressing what's been hardened.

## Before you commit

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo check --workspace --all-targets` passes
- [ ] `cargo test --workspace --lib` passes (kill_switch and other slow tests run in CI)
- [ ] No `unwrap()`/`expect()` added to library code — use `?` propagation. (Phase 5 will enforce via clippy lint.)
- [ ] No `unsafe` without a `// SAFETY:` comment explaining the invariant.
- [ ] No `Command::new("sh").arg("-c")` with interpolated input. Argv only.
- [ ] No `std::Mutex.lock().unwrap()` in async paths — use `tokio::sync::Mutex` or `parking_lot::Mutex` with poison handling.
- [ ] No `std::fs::write` for non-atomic edits — use the temp + fsync + rename pattern in `apply.rs` / `verification.rs`.
- [ ] Untrusted paths go through `cortex_core::workspace_guard::WorkspaceGuard::resolve` — never raw `&Path` at trust boundaries.

## Scope rules

- Cortex is **Rust-only** per ADR-005. Don't add Python/TypeScript verifier paths without amending the ADR. See its escalation criteria.
- The pre-apply gate must **fail closed**. Any new `GateResult` variant maps to `accepted = false` unless documented otherwise.
- `server.rs` and its sibling handler files (`server/*.rs`) are capped at ~600 LOC each. If a handler grows past that, extract a helper module — don't accrete.

## Architecture decisions

Every non-trivial decision lands in `docs/adr/ADR-NNN-*.md` AND `CLAUDE-decisions.md`. Format follows ADR-003.

## Commit messages

Conventional Commits format:

```
fix(scope): one-line summary

Body explaining the why, not the what (the diff is the what).

Refs: docs/adr/ADR-NNN-*.md, .claude/scan-reports/...
```

Common scopes: `apply`, `gate`, `mcp`, `daemon`, `cli`, `bench`, `config`, `search`, `core`.

## Running the scan

`/scan DEEP` in `.claude/` orchestrates multi-agent codebase audits. Reports land under `.claude/scan-reports/`. Re-run after major surgery.

## Phase roadmap

See `.claude/IMPL_PLAN.md` for the live remediation plan. Phases 0-4 shipped. Phases 5-6 still open.
