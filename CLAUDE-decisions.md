
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

## 2026-05-16 — Federation Deferred; Pre-Federation Hygiene Required (ADR-006)

**Problem**: User requested federation of language-specific verification daemons (cortex-py-daemon, cortex-ts-daemon, ...) federated via MCP. Four-agent /architect debate evaluated 7 options (A-G).
**Decision**: Option G — defer all federation work; honor ADR-005 escalation criteria as the sole gate. Fix 3 P0 defects this week: (1) WRITER prompt drift at server.rs:484-505 (CORTEX coaches Python/TS/Go output the gate then rejects — present on 28,748 user .py files); (2) verify_edit::extract_edit path-traversal (STRIDE-I info disclosure, `/etc/passwd` works); (3) Byzantine `_ => Language::Rust` fallback in store.rs::row_to_symbol. Total 95-130 LOC, 1 day.
**Key findings**:
- PHANTOM exposed 3 grep-verifiable present-tense defects in the Rust path.
- FORGE Phase 1 invented pyright `--co` flag (does not exist — that's pytest), undercounted BlastRadius::Advise re-introduction by 5-6× (21 grep hits → 150-190 LOC, not 30), and recommended a change ADR-005's rejected-alternatives section explicitly closed. FORGE retracted, conf -17pp.
- ORACLE mischaracterized dprint (WASM in-process plugins, not subprocess invocation) and cited a Brooks recommendation Brooks himself retracted in 1995.
- All three Phase-1 agents designed past ADR-005's escalation criteria (0/3 hold today). Scope violation corrected by Phase 2.
- Python verifier at ~60% catch rate = verification-theater, not verification. Shipping cortex-py-daemon = strictly worse than Claude Code direct on Python.
**Confidence**: 87%
**Key trade-off**: User's Python/TS workflows (NOESIS 26.5k, LIS 1.9k, SIP 262 .py files) keep routing through Claude Code rather than CORTEX. CORTEX identity (verification-first Rust daemon) preserved over breadth.
**Rejected**: A (Federation — premature), B (Monolith — reverses ADR-005), C (Plugin .so — RCE class), D (Multi-repo — drift), E (Don't build — runner-up at 88.2%), F (dprint — mischaracterized analog).
**Escalation criteria**: Re-litigate on any of: ≥10 distinct users + Python verifier ≥75% catch rate empirically + Rust path through Phase 6 + 30 days stable + organic 2nd adopter + WASM-component-model maturity.
**Six-month review**: 2026-11-16.
**ADR**: docs/adr/ADR-006-federation-deferred-pending-escalation.md
**Agents**: NEXUS (Opus), FORGE (Sonnet), ORACLE (Opus), PHANTOM (Sonnet), VERDICT (Opus).

---
