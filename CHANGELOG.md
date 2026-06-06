# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2025-05-19

### Added
- **Release packaging**: musl-static binaries (`x86_64`, `aarch64`), AUR PKGBUILD, systemd user unit, install docs.
- **Benchmark harness** (`cortex-bench`): stdlib-only Rust tasks, multi-model support (local + cloud), metrics + report generation.
- **`cargo check --all-targets` gate**: sandbox verification now runs `cargo check --offline --frozen` on all targets, not just the default package.
- **De-personalized public release**: scrubbed personal paths, generic author metadata, ready for open-source distribution.

### Changed
- Default benchmark model set to `qwen3.6:27b` (local) for Phase 0 reproducibility.

### Fixed
- Phase 5 unwrap sweep: 17 library + binary sites converted to `?` or `LockExt::lock_panic_on_poison()`.
- WRITER prompt drift in ADR-006 edge cases.
- `verify_edit` path traversal hardening + symbol byzantine fallback.
- Retry loop now handles 503 Service Unavailable in addition to 429.
- `reqwest` timeout bumped to 1800s for long cloud-model generations.
- Bench child processes escalated to SIGKILL 30s after SIGTERM.

## [0.1.0] — 2025-05-11

### Added
- Initial release: verification-first coding-AI gate for Rust workspaces.
- Sandbox + `cargo check` pre-apply verification.
- WRITER retry loop (up to 6 rounds) with compiler feedback.
- WorkspaceGuard with canonicalize + per-component symlink check.
- `cortex-daemon` Unix-socket daemon: `apply`, `ask`, `debate`, `implement`, `research`.
- `cortex-cli` binary: `cortex apply "…"` etc.
- `cortex-mcp` MCP server exposing `verify_edit` + `apply_if_clean`.
- `cortex-context` SQLite + FTS5 symbol/session store.
- Configuration via `~/.config/cortex/config.toml` with safe defaults (`cloud_enabled = false`).
- ADR-003 through ADR-007 ratified and documented.

[0.2.0]: https://github.com/supernavyl/cortex/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/supernavyl/cortex/releases/tag/v0.1.0
