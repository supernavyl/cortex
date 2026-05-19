# cortex — the Rust pre-apply gate

A verification-first coding-AI daemon for Rust workspaces.

Cortex wraps any LLM-driven editor (Claude Code, Cursor, raw API) with a **sandbox + `cargo check` gate** that runs *before* edits touch your filesystem. Edits that don't compile are rejected; only verified diffs reach disk.

**Status:** v0.2.0. The Rust-only scope (ADR-005) is closed: gate, sandbox, retry loop, MCP server, daemon, CLI all shipped. Multi-language is explicitly out of scope (ADR-006, ADR-007).

## Why

Every coding AI ships hallucinated diffs. The 2024–2026 incumbents (Cursor, Aider, Claude Code, claw-code, etc.) gate **access** — sandbox, permissions, isolation — but not **output correctness**. No tool verifies AI output compiles before it lands.

Cortex closes that gap, **for Rust only** (per [ADR-005](docs/adr/ADR-005-rust-only-verification-scope.md)).

- `cargo check` has ~95% catch rate on regressions (per ADR-003 kill-switch).
- Python and TypeScript verifiers are too weak to make the same claim. Cortex doesn't pretend.

## Install

Three paths, easiest first.

### 1. Prebuilt binary (musl-static, no runtime deps)

```bash
# Pick your arch — x86_64 or aarch64.
VERSION=0.2.0
TARGET=x86_64-unknown-linux-musl   # or aarch64-unknown-linux-musl
curl -fsSL "https://github.com/supernavyl/cortex/releases/download/v${VERSION}/cortex-${VERSION}-${TARGET}.tar.gz" \
  | tar -xz -C /tmp
sudo install -Dm755 "/tmp/cortex-${VERSION}-${TARGET}/cortex"        /usr/local/bin/cortex
sudo install -Dm755 "/tmp/cortex-${VERSION}-${TARGET}/cortex-daemon" /usr/local/bin/cortex-daemon
cortex --version    # cortex 0.2.0
```

Every release publishes a `*.sha256` next to the tarball — verify before installing if you didn't build from source.

### 2. Arch / AUR

```bash
# Once the AUR package is up:
yay -S cortex-bin

# Or build manually from the in-tree PKGBUILD:
cd packaging/aur && makepkg -si
```

### 3. From source

```bash
# Rust 1.93+ pinned in rust-toolchain.toml.
git clone https://github.com/supernavyl/cortex && cd cortex
cargo install --path crates/cortex-cli
cargo install --path crates/cortex-daemon
```

### Optional: systemd user unit

```bash
install -Dm644 packaging/systemd/cortex-daemon.service \
  ~/.config/systemd/user/cortex-daemon.service
systemctl --user daemon-reload
systemctl --user enable --now cortex-daemon
```

## Quickstart

```bash
# 1. Start the daemon (or use the systemd user unit above).
cortex-daemon &

# 2. Apply a verified change. The gate runs `cargo check` in a sandbox copy
#    of the workspace BEFORE the diff lands on disk.
cd ~/my-rust-project
cortex apply "add a unit test for parse_plan covering the empty-input case"
```

If the WRITER's first attempt doesn't compile, cortex feeds the compiler output back and retries up to 6 rounds. Either the edit compiles and lands atomically, or nothing changes.

## How it works

1. WRITER model (Qwen3.6:27B local by default) proposes file edits via the `propose_edit` tool.
2. Each proposed edit is applied to a sandbox copy of the workspace under `$XDG_CACHE_HOME/cortex/target/<hash>`.
3. `cargo check --offline --frozen` runs in the sandbox.
4. **Pass** → edit is written atomically (temp + fsync + rename) to the real workspace.
5. **Fail** → compiler output is fed back to WRITER, up to 6 retry rounds.
6. **Timeout / spawn failure** → rejected (fail-closed per ADR-005).

Path validation uses [`WorkspaceGuard`](crates/cortex-core/src/workspace_guard.rs) — canonicalize + per-component symlink check + NUL/`..`/absolute rejection.

## Architecture

```
crates/
├── cortex-core      — sandbox gate, workspace guard, language detect, router
├── cortex-tools     — tool trait, permissions, glob/grep/edit, sandbox executor
├── cortex-daemon    — Unix-socket daemon: apply, ask, debate, implement, research
├── cortex-cli       — `cortex apply "…"` etc.
├── cortex-mcp       — thin MCP server exposing `verify_edit` + `apply_if_clean`
├── cortex-context   — SQLite + FTS5 symbol/session store
└── cortex-bench     — multi-model benchmark harness (stdlib-only Rust tasks)
```

## Configuration

`~/.config/cortex/config.toml`. Defaults are safe:
- `cloud_enabled = false`
- `allowed_ollama_hosts = ["127.0.0.1", "localhost", "::1"]`
- `allow_remote_ollama = false`

Override Ollama endpoint:

```bash
CORTEX_OLLAMA_URL=http://127.0.0.1:11434 cortex-daemon
# Remote (opt-in only):
CORTEX_ALLOW_REMOTE_OLLAMA=1 CORTEX_ALLOWED_OLLAMA_HOSTS=myhost.example cortex-daemon
```

## MCP integration (Claude Code, Cursor, etc.)

Cortex ships an MCP server exposing two tools: `verify_edit` (dry-run a diff through the gate) and `apply_if_clean` (verify-then-write atomically).

Add to your MCP client config:

```json
{
  "mcpServers": {
    "cortex": {
      "command": "cortex",
      "args": ["mcp-server"]
    }
  }
}
```

## Scope

| Language | Status |
|---|---|
| Rust | supported |
| Anything else | rejected at the gate (per ADR-005) |

Multi-language re-expansion is gated by five escalation criteria — see [ADR-007](docs/adr/ADR-007-multi-language-override-rejected-phase-0-prework.md). 0/5 fired as of v0.2.0.

## Architecture decisions

- [ADR-003](docs/adr/ADR-003-verification-first-pivot.md) — Verification-first pivot
- [ADR-004](docs/adr/ADR-004-writer-retry-loop.md) — WRITER + retry loop (no critic)
- [ADR-005](docs/adr/ADR-005-rust-only-verification-scope.md) — Rust-only scope
- [ADR-006](docs/adr/ADR-006-federation-deferred-pending-escalation.md) — Federation deferred
- [ADR-007](docs/adr/ADR-007-multi-language-override-rejected-phase-0-prework.md) — Multi-language override rejected, Phase 0 pre-work

## License

[MIT](LICENSE).
