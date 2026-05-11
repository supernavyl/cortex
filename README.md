# cortex — the Rust pre-apply gate

A verification-first coding-AI daemon for Rust workspaces.

Cortex wraps any LLM-driven editor (Claude Code, Cursor, raw API) with a **sandbox + `cargo check` gate** that runs *before* edits touch your filesystem. Edits that don't compile are rejected; only verified diffs reach disk.

**Status:** alpha. Single-developer machine. Not yet packaged for distribution.

## Why

Every coding AI ships hallucinated diffs. The 2024-2025 incumbents (Cursor, Aider, Claude Code, claw-code, etc.) gate **access** — sandbox, permissions, isolation — but not **output correctness**. No tool verifies AI output compiles before it lands.

Cortex closes that gap, **for Rust only** (per [ADR-005](docs/adr/ADR-005-rust-only-verification-scope.md)).

- `cargo check` has ~95% catch rate on regressions (per ADR-003 kill-switch).
- Python and TypeScript verifiers are too weak to make the same claim. Cortex doesn't pretend.

## Architecture

```
crates/
├── cortex-core      — sandbox gate, workspace guard, language detect, router
├── cortex-tools     — tool trait, permissions, glob/grep/edit, sandbox executor
├── cortex-daemon    — Unix-socket daemon: apply, ask, debate, implement, research
├── cortex-cli       — `cortex apply "…"` etc.
├── cortex-mcp       — thin MCP server exposing `verify_edit` + `apply_if_clean`
├── cortex-context   — SQLite + FTS5 symbol/session store
└── cortex-bench     — multi-model benchmark harness
```

## How it works

1. WRITER model (Qwen3.6:27B local by default) proposes file edits via the `propose_edit` tool.
2. Each proposed edit is applied to a sandbox copy of the workspace under `$XDG_CACHE_HOME/cortex/target/<hash>`.
3. `cargo check --offline --frozen` runs in the sandbox.
4. **Pass** → edit is written atomically (temp + fsync + rename) to the real workspace.
5. **Fail** → compiler output is fed back to WRITER, up to 6 retry rounds.
6. **Timeout / spawn failure** → rejected (fail-closed per ADR-005).

Path validation uses [`WorkspaceGuard`](crates/cortex-core/src/workspace_guard.rs) — canonicalize + per-component symlink check + NUL/`..`/absolute rejection.

## Build

```bash
# Rust 1.93+ (pinned in rust-toolchain.toml)
cargo build --workspace
```

## Run

```bash
# Start daemon
cargo run --bin cortex-daemon

# In another terminal:
cargo run --bin cortex-cli -- apply "add a unit test for parse_plan"
```

## Configuration

`~/.config/cortex/config.toml`. Defaults are safe:
- `cloud_enabled = false`
- `allowed_ollama_hosts = []` (localhost-only)
- `allow_remote_ollama = false`

Override Ollama endpoint:

```bash
CORTEX_OLLAMA_URL=http://127.0.0.1:11434 cortex-daemon
# Remote (opt-in only):
CORTEX_ALLOW_REMOTE_OLLAMA=1 CORTEX_ALLOWED_OLLAMA_HOSTS=myhost.example cortex-daemon
```

## Status of scope

| Language | Status |
|---|---|
| Rust | supported |
| Anything else | rejected at the gate (per ADR-005) |

Multi-language re-expansion is gated by three conditions — see ADR-005.

## Architecture decisions

- [ADR-003](docs/adr/ADR-003-verification-first-pivot.md) — Verification-first pivot
- [ADR-004](docs/adr/ADR-004-writer-retry-loop.md) — WRITER + retry loop (no critic)
- [ADR-005](docs/adr/ADR-005-rust-only-verification-scope.md) — Rust-only scope

## License

MIT
