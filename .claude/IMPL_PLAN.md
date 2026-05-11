# Cortex Phase 0 + Phase 1 Implementation Plan

**Date**: 2026-05-11
**Driver**: scan DEEP findings (`/.claude/scan-reports/2026-05-11-0522-DEEP.md`)
**Context**: Rust-only pivot ruled. Critic reverted. Python tree removed.

---

## PHASE 0 — Cleanup (controller, ~10 min)

Sequential single-actor work. No subagent.

1. `git rm -r scheduler/ rag/ main.py`
2. `rm -rf __pycache__ .pytest_cache .mypy_cache .ruff_cache`
3. `rm crates/cortex-daemon/src/critic.rs` (untracked)
4. Edit `crates/cortex-daemon/src/lib.rs` — remove `pub mod critic;`
5. Edit `crates/cortex-daemon/src/apply.rs` — remove:
   - `use crate::critic::run_local_tri_critic;`
   - `with_critic: bool` parameter on `run_apply_loop`
   - Any critic-invocation block inside the loop
6. Edit `crates/cortex-daemon/src/server.rs` — delete duplicate `run_local_tri_critic` + `run_single_critic` at lines 1315-1420. Remove debate-handler calls to them.
7. Trim `.gitignore` — drop Python lines (`__pycache__/`, `*.pyc`, `*.pyo`, `.pytest_cache/`)
8. `cargo check --workspace` → must pass
9. `git commit -m "cleanup: remove Python prototype + critic.rs (Rust-only pivot)"`

---

## PHASE 1 — Security hardening (4 parallel Sonnet agents)

Each agent gets ONE scope. Returns: files modified + tests added + `cargo check` proof. Controller commits each fix atomically after verification.

### Agent SHARP-A — C1 search.rs command injection
**Scope**: `crates/cortex-tools/src/tools/search.rs` only
**Fix**: Replace `Command::new("sh").arg("-c").arg(format!("find {} -name '{}' ...", search_path, pattern))` with direct argv:
- `Command::new("find").arg("--").arg(search_path).args(["-name", pattern, "-type", "f"])`
- Reject `search_path` starting with `-` (find treats as option)
- Reject `\0` in either input
- Cap output in Rust (already done via `lines.truncate(100)`)
**Test**: add `test_glob_rejects_shell_metachars` — verify pattern `x'; touch /tmp/cortex-rce; #` does NOT create that file.

### Agent SHARP-B — C3 cloud exfil default + URL allowlist
**Scope**: `crates/cortex-core/src/config.rs` + `crates/cortex-daemon/src/ollama.rs`
**Fix**:
- `config.rs`: `cloud_enabled` default → `false`
- `config.rs`: add `allowed_ollama_hosts: Vec<String>` default `["127.0.0.1", "localhost", "::1"]`
- `ollama.rs`: validate `base_url` parses as `http://` (not `https://` if it's localhost-only), host is in allowlist OR explicit override flag set. Refuse on mismatch with an error mentioning the env var name.
- Log INFO every outbound URL host on first call
- Add unit test: malicious `CORTEX_OLLAMA_URL=http://attacker.example/` is rejected unless allowlist override
**Test**: 2 tests — allowed host accepted, disallowed host rejected.

### Agent SHARP-C — C4 gate fails CLOSED, not OPEN
**Scope**: `crates/cortex-core/src/gate.rs` only
**Fix**:
- Currently: `run_with_timeout` returns `None` on timeout → `Skipped` → `accepted=true`. CHANGE: distinguish `Timeout` from `SpawnFailed`. On timeout → `VerificationResult { accepted: false, reason: "verification timed out after Ns", ... }`. On spawn-failed (cargo not found) → `accepted: false, reason: "cargo not found"`.
- Add `cargo check --offline --frozen --locked` arguments
- Set `CARGO_TARGET_DIR` env var to a shared cache dir to eliminate cold compile (path: `<workspace_root>/.cortex-target-cache` OR `$XDG_CACHE_HOME/cortex/target/<workspace_hash>`)
- Make the 30s timeout configurable via `CortexConfig::verify_timeout_secs` (default 60s)
**Test**: existing kill_switch test should still pass. Add new test: simulated timeout → `accepted=false`.

### Agent SHARP-D — C2/FN-1 + H7 + H4 (apply.rs + verification.rs + new WorkspaceGuard)
**Scope**: `crates/cortex-core/src/workspace_guard.rs` (NEW) + `crates/cortex-daemon/src/apply.rs` + `crates/cortex-mcp/src/verification.rs` + small touches to `crates/cortex-tools/src/tools/file_ops.rs` if needed.

**Fix C2/FN-1 — WorkspaceGuard newtype**:
- New module `cortex-core::workspace_guard`. Public type `WorkspacePath` (newtype around `PathBuf`, only constructible via `WorkspaceGuard::resolve(&self, untrusted: &str) -> Result<WorkspacePath, GuardError>`).
- `WorkspaceGuard::new(root: &Path)` canonicalizes root, stores it.
- `resolve()`:
  1. Reject `\0` in input
  2. Reject if input is absolute
  3. Reject if any component is `ParentDir`
  4. Join with root, canonicalize the **parent** (must exist), assert `canonical_parent.starts_with(&self.root_canonical)`
  5. Check `symlink_metadata` for every ancestor in the path — reject if any is a symlink
- Returns `WorkspacePath` (a wrapper) that exposes only `.as_path() -> &Path`.
- Add unit tests: traversal `..`, symlink-bait, absolute path, NUL byte, valid path — covers all 5 cases.

**Apply C2 in apply.rs**:
- Replace `validate_relative_path` with `WorkspaceGuard::resolve()`
- `run_apply_loop` accepts `&WorkspaceGuard`, not `&Path`

**Apply FN-1 in verification.rs**:
- `handle_apply_if_clean` must build a `WorkspaceGuard` for its workspace and use `.resolve(file_path)` before any `fs::write`. Currently writes to caller-supplied absolute path with NO check — fix this.

**Fix H7 — Rust system prompt (Rust-only pivot)**:
- In `apply.rs::build_system_prompt`, replace `"Always produce valid, runnable Python. No placeholders or TODOs."` with a Rust-aware prompt referencing the workspace's edition (read from `Cargo.toml` if present) and Rust idiom rules:
  - "Always produce valid, compiling Rust. Match the workspace edition. No `unwrap()`/`expect()` in lib code. No `unsafe` without a `// SAFETY:` comment. Prefer `?` propagation."

**Fix H4 — atomic fs::write**:
- Replace `std::fs::write(&abs_path, new_content)?` with temp-then-rename:
  1. Build temp path: `abs_path.with_extension(format!("cortex-tmp-{}", random_suffix))`
  2. Open with `OpenOptions::new().write(true).create_new(true)`
  3. Write content, `sync_data()`
  4. `std::fs::rename(temp_path, abs_path)?`
- Atomic on same filesystem. Crash before rename leaves no corruption.

**Tests**: 5 unit tests for WorkspaceGuard rejection cases + 1 integration test for atomic write (write, kill process mid-write, verify file is either old-or-new but never partial).

---

## PHASE 1 finalization

Controller, after all 4 agents return:
1. `cargo check --workspace` — must pass
2. `cargo clippy --workspace --no-deps -- -W clippy::all` — note new warnings
3. `cargo test --workspace --no-run` — must compile
4. `cargo test --workspace -- --skip kill_switch` — fast-path tests must pass (kill_switch needs cargo toolchain, skip in this gate)
5. Commit each fix atomically with conventional commits:
   - `fix(search): remove sh -c command injection (CVE-class)`
   - `fix(config): cloud_enabled=false default + ollama URL allowlist`
   - `fix(gate): timeout fails closed, not open`
   - `feat(core): WorkspaceGuard newtype for path validation`
   - `fix(apply): use WorkspaceGuard + atomic temp-rename writes`
   - `fix(mcp): apply_if_clean uses WorkspaceGuard`
   - `fix(apply): Rust-only system prompt (Rust-only pivot)`

---

## Out of scope for this session

- ADR-005 write (Phase 2)
- gate.rs strip to Rust-only (Phase 2)
- server.rs split (Phase 3)
- Dev infra floor (Phase 4)
- 208 unwrap sweep (Phase 5)
- Real test coverage (Phase 6)
