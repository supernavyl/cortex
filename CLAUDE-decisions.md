
## 2026-05-08 — WRITER + Retry Loop for Method::Apply (ADR-004)

**Problem**: Implement Method::Apply stub. Three options: full W/B/A adversarial loop, WRITER + retry, single-shot.
**Decision**: Option B — WRITER (qwen3.6:27b) + SandboxGate::verify() retry loop. Max 3 rounds. No BREAKER, no ARBITER, no Anthropic client.
**Key finding**: PHANTOM proved BREAKER tests are decorative under `cargo check` — they compile but never execute. Zero signal added over existing gate.
**Confidence**: 72%
**Key trade-off**: Safety signal (4/10 vs 6/10 vs 8/10 for C/B/A) sacrificed for time-to-ship (9/10 vs 7/10 vs 2/10 for C/B/A).
**Rejected**: Option A — BREAKER decorative; Option C — wastes WRITER context on trivial compile errors.
**ADR**: docs/adr/ADR-004-writer-retry-loop.md
**Agents**: NEXUS, FORGE, PHANTOM, VERDICT

---

## 2026-05-11 — Rust-Only Verification Scope (ADR-005)

**Problem**: scan-DEEP audit revealed (1) apply.rs hardcoded a Python system prompt while the gate ran cargo check — multi-language story was unmaintained; (2) check_python loaded arbitrary mypy plugins (RCE class); (3) check_typescript used `npx --yes` (npm RCE); (4) only Rust has empirical kill-switch data — TS/Python paths shipped unvalidated.
**Decision**: Cortex narrows to a Rust-only verification daemon. Single verifier: `cargo check --offline`. Language enum collapses to `Rust | Other`. BlastRadius collapses to `HardReject`. check_typescript, check_python, has_py_files deleted. Non-Rust workspaces return SpawnFailed → accepted=false.
**Key finding**: ADR-003's language-agnostic framing was theatrical — Python verifier was 60% catch rate (advise-only), TypeScript path shipped with zero kill-switch data. Cortex's flagship feature (verification-first) is sharpest where verification accuracy is highest (Rust ~95%).
**Confidence**: 78%
**Trade-off**: Smaller TAM (~3-5% of devs) and CEO's own Python/TS work outside scope, traded for sharper positioning vs claw-code + 4 scan findings closed by design.
**Rejected**: status-quo multi-language (drift evidence); Python-first (cortex itself is Rust); language-specialist critics (multiplies VRAM contention).
**Escalation trigger** (revisit when ALL three hold): ≥10 users demand it + viable verifier with ≥75% catch rate + Rust path through Phase 6.
**Supersedes (partial)**: ADR-003's language-agnostic framing.
**ADR**: docs/adr/ADR-005-rust-only-verification-scope.md
**Agents**: SENTINEL, SURVEYOR, MEDIC, BENCH, DEPS, VERDICT (scan-DEEP); CEO ruled.

---
