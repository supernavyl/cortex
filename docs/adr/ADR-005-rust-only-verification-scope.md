# ADR-005: Rust-Only Verification Scope

**Date:** 2026-05-11
**Status:** ACCEPTED
**Supersedes (partial):** ADR-003's "language-agnostic verification daemon" framing and language-gated `BlastRadius` dispatch.
**Confidence:** 78%

---

## Context

ADR-003 (2026-05-08) defined cortex as a verification-first daemon with multi-language verifier dispatch:

| Language | Verifier | ADR-003 accuracy | Gate behavior |
|---|---|---|---|
| Rust | `cargo check` | ~95% | HardReject |
| TypeScript (strict) | `tsc --noEmit` | ~85% | HardReject on type errors, WARN on lint |
| Python | `ruff` + `pyright` | ~60% | ADVISE only |
| Other | none | n/a | PASS-THROUGH |

The `/scan DEEP` audit (2026-05-11, `.claude/scan-reports/2026-05-11-0522-DEEP.md`) surfaced four converging signals that invalidate the multi-language framing:

1. **Drift evidence**: `apply.rs::build_system_prompt` hardcoded `"Always produce valid, runnable Python"` — incompatible with `cargo check`. The system prompt and the gate were targeting different languages without anyone noticing for weeks. (Finding H7, confirmed.)
2. **Security surface in non-Rust paths**: `check_python` invoked `mypy` which loads arbitrary plugins (SCAN-S012, RCE class). `check_typescript` invoked `npx --yes tsc --noEmit` which downloads and executes arbitrary npm code on first run (SCAN-S013). Both are CVE-class. Removing these paths is a net security gain even before considering scope.
3. **Empirical signal asymmetry**: ADR-004's kill-switch found `SandboxGate::verify()` had a 75% catch rate on Rust workspaces (20 edits). No comparable kill-switch was ever run on TypeScript or Python — those paths shipped with zero empirical validation of their verifier accuracy.
4. **Competitive positioning**: claw-code (190K stars) is a generic clean-room Rust Claude-Code clone. Differentiating cortex on Rust depth gives a defensible wedge ("the Rust pre-apply gate"). Going head-to-head as a generic multi-language coding daemon competes with claw-code on its strongest surface.

The internal critic.rs (deleted in commit `50134f4` per the same scan) was tri-model: deepseek-r1:14b + phi4-reasoning:14b + deepseek-coder-v2:16b. None are Rust-specialists. The mismatch between the codebase's actual usage (Rust) and the prompt/model surface (generalist Python-tilted) was the same drift signal as #1, expressed at the model layer.

---

## Decision

**Cortex narrows to a Rust-only verification daemon.**

- The pre-apply gate runs exactly one verifier: `cargo check --offline` (`--frozen` when `Cargo.lock` exists).
- `Language` enum collapses to `Rust` and `Other`. `Other` always maps to `GateResult::SpawnFailed { reason: "cortex Rust-only per ADR-005; non-Rust workspace not supported" }` → `accepted: false`.
- `BlastRadius::Advise` and `BlastRadius::PassThrough` variants are removed. Only `HardReject` remains. (Future: if multi-language support returns under the escalation criteria below, the variants come back.)
- `check_typescript`, `check_python`, `check_mypy` functions are deleted entirely. No `tsc`, no `mypy`, no `ruff`, no `npx`.
- The WRITER system prompt in `apply.rs` is Rust-aware (workspace edition, rust-style.md idioms) — already landed in commit `25cadd1`.
- The README (when written, Phase 4) positions cortex as "the Rust pre-apply gate," not as a general AI coding assistant.

---

## Consequences

### Positive

- **Sharper positioning** versus claw-code. Generic clones cannot differentiate; cortex can.
- **Highest possible verifier accuracy** (~95% per ADR-003) — the verification-first thesis is strongest where verification works.
- **Smaller attack surface**: deleting `mypy` and `npx --yes` paths closes SCAN-S012 + SCAN-S013 by removal, not by patching.
- **Smaller code surface**: gate.rs drops ~150 LOC of non-Rust dispatch.
- **Honest scope**: no theatrical verification of unverifiable languages. Python advise-only at 60% was conceding the gate doesn't work for that language — that concession should be at the project boundary, not buried in `BlastRadius::Advise`.
- **Removes 4+ scan findings by design**: H7 (prompt drift), S012 (mypy RCE), S013 (npx RCE), A007 (Python prototype tree — already archived Phase 0).

### Negative

- **TAM contraction**: Rust developers are ~3-5% of all developers (per StackOverflow surveys 2024-2025). Going Rust-only is a smaller market.
- **CEO's own coverage**: supernovyl's active workload is heavily Python (SwissImmigrationPro backend, LIS, Emily) and TypeScript (SIP frontend, Tauri apps). Rust-only cortex covers <30% of CEO's daily edits.
- **Optionality cost**: re-adding Python/TS later requires restoring the deleted code paths from git history (`git revert` or manual re-implementation). Costs are recoverable but non-zero.
- **Tri-critic dropped**: since the critic models weren't Rust-specialists, this is a wash, but it removes a quality-improvement lever until a Rust-specialist critic is introduced (qwen3-coder:30b candidate, deferred).

---

## Rejected alternatives

| Alternative | Reason rejected |
|---|---|
| **Status-quo multi-language** | Drift evidence (apply.rs:93 Python prompt + Rust workspace + cargo check gate) proves the multi-language story was not being maintained. Continuing it ships a lie. |
| **Python-first** | Cortex itself is Rust. Eating own dogfood requires Rust. Python verifier accuracy is fundamentally lower (60% per ADR-003) — the verification-first thesis is weakest there. Pivoting to a weak signal undermines the project's core claim. |
| **Add language-specialist critics per language** | Multiplies model load (already 30-39GB VRAM contention on tri-critic, P002). Doesn't address the underlying low-accuracy problem for dynamic languages. |
| **Keep `BlastRadius::Advise` for Python pass-through** | Advise mode was the silent fail-open at the language layer that the C4 fix removed at the timeout layer. Same architectural mistake — verification-first means fail-closed, not "best-effort with caveats." |

---

## Escalation criteria (when to revisit Rust-only)

Revisit if **all three** hold:

1. **Demand signal**: ≥10 production users (or ≥3 paying customers if cortex monetizes) ask for Python or TypeScript support specifically — not "would be nice" but "blocks adoption."
2. **Empirical verifier**: A specific Python or TypeScript verifier with ≥75% catch rate on real-world edits (ruff+pyright currently 60% per ADR-003; the gap must close).
3. **Capacity**: cortex's Rust path has shipped through Phase 4 (dev infra floor) and Phase 6 (real test coverage) — multi-language re-expansion happens only after the Rust path is solid.

Any **two** of the three is insufficient. All three closes the case for ADR-006: Multi-Language Re-Expansion.

---

## Implementation

Tracked under `.claude/IMPL_PLAN.md` Phase 2:

- **Phase 2A (this ADR + builder)**: gate.rs strip — `Language` collapses, `BlastRadius` collapses, `check_typescript` and `check_python` deleted, dispatch returns SpawnFailed for Other, tests updated.
- **Phase 2B (deferred)**: README and external positioning copy reflects Rust-only.

---

## Open questions

1. Does cortex's daemon need to reject non-Rust workspaces at *startup* (refuse to bind socket) or only at *apply time* (current decision: apply-time SpawnFailed)? Defer until first user complaint.
2. Should `cortex apply` print a friendly message when invoked on a non-Rust workspace, suggesting it's the wrong tool? Yes — Phase 4 README pass.
3. Does the `kairos.rs` module reference non-Rust scheduling tasks? Audit during Phase 3 server.rs decomposition.

---

## Agents consulted (asynchronous)

- SENTINEL (security-auditor, Sonnet) — supplied SCAN-S012 + SCAN-S013 evidence
- SURVEYOR (code-architect, Sonnet) — supplied ADR-003 drift evidence
- MEDIC, BENCH, DEPS (Sonnet) — supplied empirical asymmetry data
- VERDICT (Opus) — ranked Rust-only as Recommendation B with 72% confidence pre-decision
- CEO (supernovyl) — confirmed Rust-only direction; this ADR is the implementation pact
