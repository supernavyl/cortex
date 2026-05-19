# ADR-007: Reject Multi-Language Override As-Stated; Build Empirical Prerequisites Instead

**Date:** 2026-05-17
**Status:** PROPOSED
**Refines:** ADR-005 (verification floor still binding), ADR-006 (federation remains deferred; 0/5 escalation triggers unfired)
**Supersedes:** none
**Confidence:** 74%

---

## Context

On 2026-05-17, CEO supernovyl issued a one-sentence override of ADR-006 (federation deferred, 87% confidence, ratified 2026-05-16, less than 48 hours prior). The override: *"make CORTEX most sophisticated with all languages so it can actually be useful and build anything."*

ADR-006's five escalation criteria (six-month review 2026-11-16; second adopter request; WASI 0.3 stable; Rust hygiene P0 closed; verification-first multi-language proven in another project) have 0/5 fired.

Phase 1 agents (NEXUS, FORGE, ORACLE) proposed architectures ranging from in-process trait dispatch to "don't build it." Phase 2 red team (PHANTOM) produced grep-verified empirical findings that invalidated multiple Phase 1 LOC estimates by 2-6x and exposed three latent defects (sandbox does not exist, bench-daemon schizophrenia, three Language enums require breaking on-disk migration).

NOESIS retrieval was down during the session (Qdrant 400 errors on search_corpus / cite_check; embedder + reranker reported OK; corpus 447,515 chunks but dense-query payload malformed at column ~53000). All external numerical citations in upstream analyses are **ESTIMATE not FACT**.

## Decision

**Phase 0 (mandatory, 4-6 calendar weeks at solo-CEO velocity): Build the empirical prerequisites that any multi-language ADR-008 would need.**

1. Labeled bench dataset for Rust (≥50 tasks, held-out, ground-truth labels).
2. Labeled bench dataset for Python (≥50 tasks, held-out, ground-truth labels). Python ships as *measured target only*, not as supported language.
3. Linux sandbox (bubblewrap + seccomp + namespaces + tmpfs overlay + `--unshare-net` mandatory + ≥30 tests + ≥5 escape-attempt tests).
4. CanonicalLanguage migration (collapse three enums + on-disk schema versioning + `cortex migrate` command + zero-loss re-index of existing symbol table).
5. CI catch-rate gate enforcing ADR-005's 75% floor with hard-fail action.

**Phase 1 (post-Phase-0 only): Open ADR-008 with empirical inputs.** ADR-008 will rule on whether to admit any additional language verifier. ADR-008 has access to real catch-rate data on a real labeled set inside a real sandbox. Today no such artifact exists; ADR-008 cannot be resolved on evidence today.

**Forbidden until Phase 0 complete**:
- Adding any non-Rust verifier to the production VerifierRegistry.
- Modifying WRITER prompts to be language-aware (reopens 680d57f drift fix).
- Any `--unverified` / `--force` / `--yolo` / soft-mode CLI flag.
- Reopening `BlastRadius::Advise`, `BlastRadius::Warn`, `BlastRadius::PassThrough` in any form.
- Shipping pyright, tsc, go vet, or clang inside the current `fs::copy`-as-sandbox.

## Decision Matrix

| Criterion | Weight | H: Honor Full | I: Honor Narrowed (R+Go) | J: Hold + Document | K: Hold + Pre-Work |
|-----------|--------|---------------|--------------------------|---------------------|---------------------|
| Verification confidence preserved | 10 | 3 | 6 | 9 | 10 |
| Fail-closed posture (HardReject only) | 10 | 4 | 7 | 10 | 10 |
| Focus / identity preservation | 9 | 2 | 5 | 9 | 9 |
| Time-to-ship measurable value | 7 | 2 | 5 | 7 | 6 |
| Maintainability for solo-CEO | 9 | 2 | 5 | 9 | 8 |
| Reversibility | 7 | 3 | 6 | 10 | 9 |
| Complexity cost (lower=better, inv) | 8 | 1 | 5 | 10 | 8 |
| Risk of RCE / theater verification | 9 | 2 | 6 | 10 | 9 |
| Performance headroom | 5 | 4 | 7 | 9 | 8 |
| **Empirical-evidence basis** | **10** | **2** | **5** | **8** | **10** |
| **Normalized score (/100)** | | **26.2** | **58.5** | **89.8** | **90.1** |

**K wins by 0.3pp over J.** Tiebreaker: K produces *empirical infrastructure* the CEO can use to make the next override evidence-driven instead of intuition-driven.

## Consequences

### Easier
- Future multi-language decisions resolvable on evidence not intuition.
- Rust verification path strengthened by real sandbox (eliminates ADR-005 latent risk).
- ADR-005 75% floor becomes enforceable via CI (today it is decoration).
- n=1 user's on-disk symbol store migrates to canonical schema, eliminating 71+17+22 = 110+ grep-hit divergence across three enums.
- WRITER prompt 680d57f fix protected against regression by test.

### Harder
- CEO does not receive multi-language CORTEX in the timeframe implied by the override.
- 4-6 weeks of work delivers no user-visible new language support.
- CEO override pattern (high-confidence ADR overridden in <48h by one-sentence directive) is not preserved; this ADR rejects the override pattern itself.
- If CEO rejects this ADR and orders Option H, this ADR becomes evidence in the post-mortem of why H failed.

## Rejected Alternatives

- **H: Honor Full Override (26.2/100)** — Build NEXUS architecture as proposed (2200-3000 LOC, 12-20 weeks, 8+ languages, multi-OS sandbox, conformance suite). Rejected: ships ADR-005 theater; PHANTOM Flaws #1/#2/#6/#8/#9/#10/#11 all unmitigated; LOC realistically 2200-3000 at solo-CEO velocity = 12-20 calendar weeks; Claude Code beats CORTEX on n=1 user's first Python comparison; verification-first identity collapses.
- **I: Honor Narrowed (R+Go) (58.5/100)** — Build Rust + Go only, require Phase 0 labeled-bench, kill-switch via CI, HardReject only. Rejected: even narrowed I requires Phase 0 prerequisites (sandbox does not exist); I is K with a premature Go commitment that should follow Phase 0 evidence; FORGE Phase 2 confidence collapse (-31pp, withdrew Go 85-90% catch-rate claim) is dispositive of "we don't know enough to commit to Go specifically yet."
- **J: Hold + Document Only (89.8/100)** — Document override as non-architectural; do nothing. Rejected by 0.3pp: J leaves Phase 0 hygiene debt unfixed (sandbox missing, three enums, bench schizophrenic); K is J plus the work that strengthens Rust path AND prepares evidence-based reopening.

## Acceptance Criteria for Phase 0 (Measurable Predicates for "Done")

K is "done" and ADR-008 may be opened when ALL hold:

1. **Labeled bench exists**: `crates/cortex-bench/datasets/rust-gold.jsonl` and `python-gold.jsonl`, each ≥50 tasks, each labeled `{has_bug: bool, bug_category: enum, ground_truth_fix?: diff}`, hash committed, dataset NOT authored by the verifier author for the language in question.
2. **CI catch-rate gate enforces ADR-005 75% floor**: `cargo bench --bench verifier-catch-rate -- --hard-fail-below 0.75` runs in CI; merge blocked if catch rate on held-out set drops below 0.75 for any registered verifier.
3. **Bubblewrap sandbox class shipped on Linux**: `crates/cortex-sandbox/src/linux.rs` with namespace isolation (user, mount, pid, net, ipc, uts), `--unshare-net` mandatory for all non-Rust verifiers, tmpfs overlay for writes, per-workspace read-only bind mounts, seccomp filter, ≥30 unit tests, ≥5 escape-attempt tests.
4. **CanonicalLanguage migration completed**: Three Language enums collapsed to one; on-disk symbol table schema versioned; `cortex migrate` command re-indexes existing user data with zero loss; migration tested on CEO's actual `~/projects/cortex/` symbol store.
5. **WRITER per-language coupling EXPLICITLY rejected in code**: A test asserts WRITER prompt is language-agnostic; future regression toward per-language templates fails CI. Protects the 680d57f fix.
6. **No soft modes**: `grep -r "unverified\|--force\|--yolo\|BlastRadius::Advise\|BlastRadius::Warn\|BlastRadius::PassThrough" crates/` returns zero results. CI gate enforces.
7. **Documented Phase 0 retrospective**: Written artifact comparing Phase 0 actual LOC to PHANTOM's estimates. If PHANTOM was right, this becomes evidence for K's empiricism in future overrides; if wrong, evidence for the next override.

## Escalation Criteria (What Would Reopen This Ruling)

1. A working bubblewrap-based Rust sandbox library exists elsewhere and can be vendored in <200 LOC.
2. A second adopter (n≥2) requests CORTEX for non-Rust verification with written commitment to use it over Claude Code.
3. WASI 0.3 stabilizes (reopens ADR-006 Option F — WASM plugins — at potentially viable score).
4. A pre-existing labeled multi-language bench dataset is found that PHANTOM cannot impeach.
5. Phase 0 completes ahead of schedule with all 7 acceptance criteria met (triggers ADR-008 opening on schedule).
6. CEO produces empirical evidence contradicting PHANTOM Flaws #1, #2, #6, #8, #9, #10, or #11. **Restatement of preference is not evidence.**

What would NOT change this ruling:
- CEO restatement of the override with more emphasis. Repetition is not evidence.
- A Phase 1 agent revising confidence upward without new empirical data.
- "Claude Code does this so we can too." Claude Code does generation, not verification-gated apply. Different physics.

## Anti-Patterns Forbidden

Preserved from ADR-005/006 and extended by this ADR:

1. **No soft modes**: `BlastRadius::Advise`, `Warn`, `PassThrough` remain killed. `gate.rs:245` invariant binding.
2. **No CLI escape hatches**: `--unverified`, `--force`, `--yolo`, `--skip-verify`, `--trust-me` forbidden.
3. **No federation**: sibling daemons per language (ADR-006 Option A) remains rejected.
4. **No WASM plugins** until WASI 0.3 stable AND escalation criterion #3 fires.
5. **No theater verification**: any verifier with measured catch-rate <75% on held-out labeled set is removed from VerifierRegistry by CI, not by code review.
6. **No bench-daemon schizophrenia**: any commit that adds a language to `cortex-bench` MUST also add it to the production VerifierRegistry or be rejected; any commit that removes a language from production MUST also remove it from bench.
7. **No per-language WRITER prompts**: WRITER remains language-agnostic. Per-language hints belong in templates loaded by the verifier, not in the WRITER system prompt. Test enforces.
8. **No on-disk schema changes without migration command**: any change to `store.rs` serialization MUST ship with `cortex migrate` step and zero-loss test.
9. **No npx-class plugin loading**: pyright with plugins enabled, eslint with plugins, prettier with plugins, etc. — any verifier that loads user-controlled code at runtime is forbidden until sandbox class B (network-denied + plugin-denied) exists.
10. **No CEO override executed without ADR**: this ADR establishes precedent that CEO overrides of ratified ADRs trigger a new multi-agent debate cycle before execution, not direct implementation.

## Key Empirical Findings of Record

1. `crates/cortex-core/src/gate.rs:552-585` — sandbox is `std::env::temp_dir().join() + fs::copy`, no isolation.
2. `crates/cortex-bench/src/tasks.rs:16-137` — every task `language: "python"`, zero Rust held-out set.
3. Commit `50134f4` — Python deleted from prod same commit Python added to bench.
4. Commit `680d57f` — WRITER prompt drift fix closed <24h before override; Phase 1 plans reopen this file.
5. `crates/cortex-core/src/gate.rs:245` — *"any softer mode (Warn/Advise/PassThrough) was a silent fail-open and has been removed."*
6. `crates/cortex-core/src/store.rs:430` — symbol table serializes lowercase language strings; enum rename = breaking on-disk migration not designed by any Phase 1 agent.
7. `crates/cortex-core/src/workspace.rs:78-85` — workspace detection priority-orders markers, returns ONE language; polyglot dispatch absent. `~/p/` (Django + SvelteKit) verified polyglot.
8. `pacman -Ql pyright` — 37MB node_modules, plugin loading default-enabled, Node runtime required. FORGE's "single-file Node binary" claim is false.
9. Re-measured on CEO's box: tsc 104ms cold (FORGE claimed 5ms, 20x off), pyright 363ms cold (claimed 136ms, 2.7x off), go 1655ms cold (claimed 19ms — hot-cache reading, 87x off).
10. Go vet catches 10-20% of runtime bugs empirically (misses nil deref, div-by-zero, slice index; catches printf misuse, copylocks). FORGE's 85-90% was wishful intuition. Withdrawn in Phase 2.
11. NOESIS was offline — all SWE-bench / dprint / LSP / WASI external citations are ESTIMATE not FACT.

## Audit Trail

- **NEXUS Phase 1**: 68% conf, in-process trait dispatch + VerifierRegistry, 2000 LOC estimate.
- **NEXUS Phase 2 revised**: **42% conf (-26pp)** accepting polyglot critique, 75%-floor non-enforceability, 12-16 week realistic timeline, conformance-suite-as-moat rejection.
- **FORGE Phase 1**: 58% conf, R+Go in 5-8 days at 520 LOC.
- **FORGE Phase 2 revised**: **27% conf (-31pp)** — withdrawing `--unverified`, conceding pyright is not single-file, tsc 5ms claim false (real 485ms), go catch-rate 85-90% wishful (real 10-60%), LOC understated 4-6x, held-out python set is 7 .md specs not a labeled dataset.
- **ORACLE Phase 1**: 72% conf, CEO override pattern-matches IBM Watson/Quibi/Fleet (scope expansion under weak per-unit capability), recommends hold ADR-005/006. Endorsed by PHANTOM.
- **PHANTOM Phase 2**: 84% conf, threat CRITICAL. 11 grep-verified empirical findings (above).
- **VERDICT (this ADR)**: 74% conf (Opus); 26pp discount from theoretical ceiling for: NOESIS down (8pp), PHANTOM catch-rate testing anecdotal not statistical (6pp), solo-CEO velocity has no resolved Brier track record at this LOC scale (5pp), unknown CEO response to K (4pp), ORACLE historical analogies interpretive (3pp).

**Status**: PROPOSED. Awaiting CEO ratification or counter-override. If CEO counter-overrides, this ADR must be amended to record the counter-override and the engineering objections preserved here, so that future post-mortems have evidence of what was known at decision time.

---

*The verification floor holds. The override is not honored as stated. The override is honored as a directive to build the apparatus that would resolve it on evidence within 6 weeks. The CEO retains decision authority; VERDICT removes only the ability to decide without evidence.*
