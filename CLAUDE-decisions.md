
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

## 2026-05-17 — CEO Override of ADR-006 REJECTED As-Stated; Phase 0 Empirical Pre-Work Ordered (ADR-007)

**Problem**: CEO override of ADR-006 (less than 48 hours after 87%-conf ratification, 0/5 escalation triggers fired): "make CORTEX most sophisticated with all languages so it can actually be useful and build anything." STRATEGIC scope. 5-agent /architect debate (NEXUS, FORGE, ORACLE, PHANTOM, VERDICT). NOESIS retrieval BROKEN during session — external citations are ESTIMATE not FACT.
**Decision**: Option K — HOLD + PRE-WORK. Reject the override as-stated on engineering grounds. Reframe as a 4-6 week Phase 0 directive that builds the empirical apparatus to resolve a future multi-language ADR-008 on evidence rather than intuition.

**Decision Matrix** (normalized, /100):
- H (Honor Full): 26.2 — ships ADR-005 theater; sandbox/bench/enums all unfixed
- I (Honor Narrowed R+Go): 58.5 — even narrowed requires Phase 0 first; premature Go commitment
- J (Hold + Document): 89.8 — leaves hygiene debt; would need separate ADR to authorize same work
- **K (Hold + Pre-Work): 90.1** ← chosen, K beats J by 0.3pp on tiebreaker "produces decision-quality artifacts"

**Phase 0 deliverables (mandatory before any ADR-008 reopening)**:
1. Labeled bench dataset for Rust (≥50 tasks, held-out, ground-truth bug/no-bug labels)
2. Labeled bench dataset for Python (≥50 tasks, held-out) — measured target only, NOT supported language yet
3. Linux sandbox: bubblewrap + seccomp + namespaces + tmpfs overlay + `--unshare-net` mandatory + ≥30 tests + ≥5 escape-attempt tests
4. CanonicalLanguage migration: collapse three Language enums (gate/workspace/symbol) + on-disk schema versioning + `cortex migrate` command + zero-loss test
5. CI catch-rate gate enforcing ADR-005 75% floor with hard-fail action

**Key empirical findings** (grep-verified by PHANTOM):
- **Sandbox does not exist**: `crates/cortex-core/src/gate.rs:552-585` is `temp_dir() + fs::copy`, zero process/namespace/network isolation. NEXUS's "80 LOC sandbox classes" omits 600-900 LOC of greenfield work.
- **Bench-daemon schizophrenia**: every BenchTask in `cortex-bench/src/tasks.rs` has `language: "python"`; commit `50134f4` deleted Python from prod the SAME commit Python was added to bench. There is no held-out Rust kill-switch set.
- **Three Language enums = 71+17+22 grep hits** (Language::, ProjectLanguage::, BlastRadius). On-disk symbol-table serialization (`store.rs:430`) uses lowercase strings — schema change invalidates n=1 user's existing index. Honest LOC: 180-250 + migration command.
- **Pyright is NOT a single-file binary**: `pacman -Ql pyright` = symlink into 37MB node_modules tree, Node runtime required, plugin loading default-enabled = npx-class RCE.
- **Re-measured timings on CEO's box**: tsc 104ms cold (FORGE claimed 5ms, off 20x), pyright 363ms cold (claimed 136ms, off 2.7x), go 1655ms cold (claimed 19ms — hot-cache reading, off 87x).
- **Go vet catches ~10-20% of runtime bugs**, not 85-90% (empirical: misses nil deref, div-by-zero, slice index; catches printf misuse, copylocks).
- **TS strict permits semantic bugs** (structural typing, branded-type erasure, any-poisoning) — honest 45-65%, not 75-80%.
- **WRITER prompt drift fix in commit 680d57f** (ADR-006 P0) is <24h old; NEXUS's "templates couple to verifiers" REOPENS that file to re-emit per-language hints.
- **Polyglot workspaces have no dispatch**: `workspace.rs:78-85` priority-orders markers; `gate.rs::detect_all` returns ONE language; `verify_batch (gate.rs:453)` is single-verifier. `~/p/` (Django+SvelteKit) verified polyglot. Multi-language verify_batch = 200-300 LOC rewrite.
- **`--unverified` flag = BlastRadius::Advise resurrected**: `gate.rs:245` comment is binding — *"any softer mode (Warn/Advise/PassThrough) was a silent fail-open and has been removed."* FORGE withdrew the flag.

**Honest LOC re-estimate (grep-based)**: 2200-3000 LOC before first non-Rust verifier ships (FORGE Phase 1's 520 was understated 4-6x; NEXUS's 2000 understated ~1.5x and excluded sandbox entirely).
**Honest timeline**: 12-20 calendar weeks at solo-CEO 4-day velocity (not 6-8 weeks).

**Anti-patterns forbidden (extending ADR-005/006)**:
- No `--unverified` / `--force` / `--yolo` / soft-mode CLI flag (ADR-005 Advise stays killed)
- No per-language WRITER prompts (protects 680d57f fix; test enforces)
- No bench-daemon schizophrenia (any commit adding language to bench MUST add to VerifierRegistry, or vice-versa)
- No pyright/tsc/go-vet inside `fs::copy`-as-sandbox (must wait for Phase 0 bubblewrap)
- No npx-class plugin loading (pyright with plugins enabled, eslint with plugins, etc.)
- No on-disk schema changes without `cortex migrate` + zero-loss test
- No CEO override executed without multi-agent debate cycle (this ADR establishes precedent)

**Agent confidences**:
- NEXUS: 68% Phase 1 → 42% Phase 2 revised (-26pp)
- FORGE: 58% Phase 1 → 27% Phase 2 revised (-31pp, the largest single-agent collapse in this debate)
- ORACLE: 72% (endorsed by PHANTOM; CEO override = IBM Watson/Quibi/Fleet pattern, NOT Jobs/Bezos)
- PHANTOM: 84% (threat CRITICAL, 11 grep-verified findings)
- VERDICT: 74% (Opus; 26pp discount from theoretical ceiling for NOESIS-down, anecdotal catch-rate evidence, solo-CEO velocity uncertainty, unknown CEO response to K)

**Trade-off**: CEO does not receive multi-language CORTEX in the timeframe implied by the override. CEO receives capability to decide multi-language on evidence in 6 weeks instead of commitment to ship multi-language theater in 12-20 weeks.

**Escalation criteria (what would reopen this ruling)**: working bubblewrap-based Rust sandbox vendored <200 LOC; second adopter (n≥2) with written commitment to use CORTEX over Claude Code for non-Rust; WASI 0.3 stabilizes; pre-existing labeled multi-language bench dataset PHANTOM cannot impeach; Phase 0 completes ahead of schedule (triggers ADR-008 opening); CEO produces empirical evidence contradicting PHANTOM Flaws #1, #2, #6, #8, #9, #10, #11.

**What would NOT change this ruling**: CEO restatement of preference (not evidence); Phase 1 agent revising confidence without new data; "Claude Code does this so we can too" (Claude Code does generation, not verification-gated apply — different physics).

**ADR**: docs/adr/ADR-007-multi-language-override-rejected-phase-0-prework.md (to be written)
**Status**: PROPOSED. Awaiting CEO ratification or counter-override. If counter-overridden, this ADR must record the counter-override with the engineering objections preserved so future post-mortems have evidence of what was known at decision time.

---
