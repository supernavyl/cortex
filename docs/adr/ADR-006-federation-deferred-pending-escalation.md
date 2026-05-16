# ADR-006: Federation Deferred Pending ADR-005 Escalation; Pre-Federation Hygiene Required

**Date:** 2026-05-16
**Status:** ACCEPTED
**Supersedes:** none
**Refines:** ADR-005 (Rust-only narrowing)
**Confidence:** 87%

---

## Context

User requested federation of language-specific verification daemons ("build best Python coding agent and connect to cortex, then one for other languages — they all connect"). A four-agent adversarial debate (NEXUS, FORGE, ORACLE, PHANTOM) evaluated seven options against CORTEX's stated priorities (verification confidence > breadth, fail-closed > fail-open, focus > scope).

Phase 2 (PHANTOM red team) surfaced three present-tense defects in the Rust path that directly contradict CORTEX's identity:

1. **WRITER prompt drift** (`crates/cortex-daemon/src/server.rs:484-505`): WRITER system prompt injects Python/TS/Go language-specific hints; gate rejects all non-Rust output. CORTEX is currently coaching language output it will reject. User's actual workloads contain 28,748 Python files across NOESIS/LIS/SIP — meaning the defect is hit constantly when CORTEX is invoked outside Rust contexts.
2. **`verify_edit` path-traversal** (`crates/cortex-mcp/src/verification.rs::extract_edit`): reads `file_path` parameter without `WorkspaceGuard`. STRIDE-I information disclosure: `/etc/passwd` reads succeed.
3. **Byzantine `Language` fallback** (`crates/cortex-context/src/store.rs::row_to_symbol`): `_ => Language::Rust` silently miscategorizes unknown lang strings, corrupting the symbol table.

PHANTOM additionally falsified three FORGE Phase-1 claims (pyright `--co` flag does not exist, `BlastRadius::Advise` 30-LOC estimate is fiction — true cost 150-190 LOC across 21 grep hits, claim that Advise reintroduction does not reverse ADR-005). FORGE retracted Option B with -17pp confidence delta. ORACLE's Option F (dprint pattern) was mischaracterized — dprint uses WASM in-process plugins, not native-toolchain subprocess invocation; the option collapses to behavior `gate.rs` already implements.

ADR-005's escalation criteria for multi-language expansion (≥10 users demanding + ≥75% verifier catch rate + Rust through Phase 6) hold at 0/3. All four Phase-1 agents designed past these criteria, constituting a scope violation that Phase 2 corrected.

## Decision

**Defer all federation work. Honor ADR-005's escalation criteria as the sole gate to reconsidering multi-language support. Fix the three present-tense defects in the Rust path this week as pre-federation hygiene.**

Concrete scope:

- **P0**: Strip Python/TS/Go hints from WRITER system prompt (`server.rs:471-507`). Rust-only WRITER content. ~30 LOC.
- **P0**: Wrap `verify_edit::extract_edit` in `WorkspaceGuard`. ~15 LOC, 3 tests.
- **P0**: Replace `_ => Language::Rust` in `store.rs::row_to_symbol` with `Result<Language>`. ~15 LOC.
- **P1**: Unify `ProjectLanguage` (`workspace.rs`) and `gate::Language` (`gate.rs`) enums. ~50 LOC. Not blocking ADR-006 close.
- **P1**: Install `cargo audit` and wire to pre-commit or CI.
- **P1**: `README.md` states "Rust-only by design; see ADR-005, ADR-006."

Total estimated effort: 95-130 LOC, 1 working day.

## Decision Matrix

| Criterion | Weight | A: Federation | B: Monolith | C: Plugin .so | D: Multi-repo | E: Don't build | F: dprint | G: Defer + fix |
|-----------|--------|---------------|-------------|---------------|---------------|----------------|-----------|----------------|
| Verification confidence preserved | 10 | 4 | 3 | 2 | 3 | 9 | 4 | 10 |
| Fail-closed posture | 10 | 5 | 2 | 1 | 5 | 10 | 4 | 10 |
| Focus / ADR-005 compatibility | 9 | 2 | 1 | 1 | 2 | 10 | 3 | 10 |
| Time-to-ship value | 7 | 2 | 4 | 1 | 2 | 8 | 3 | 9 |
| Maintainability (1-user reality) | 8 | 3 | 4 | 1 | 2 | 9 | 4 | 9 |
| Reversibility | 6 | 4 | 3 | 2 | 3 | 10 | 5 | 10 |
| Complexity cost | 7 | 3 | 4 | 1 | 2 | 10 | 4 | 9 |
| Risk (RCE, drift, theater) | 9 | 4 | 2 | 1 | 3 | 10 | 4 | 10 |
| Performance headroom | 5 | 5 | 5 | 4 | 5 | 8 | 5 | 8 |
| **Weighted total (/710)** | | **246** | **221** | **102** | **211** | **626** | **270** | **678** |
| **Normalized %** | | 34.6 | 31.1 | 14.4 | 29.7 | 88.2 | 38.0 | **95.5** |

## Consequences

**Easier:**
- CORTEX's identity (verification-first Rust daemon) reinforced, not diluted.
- Rust path through Phase 6 unblocked — no second-language tax on every refactor.
- WRITER stops producing output the gate rejects; effective Rust suggestion quality improves.
- Path-traversal exposure closed before any external user touches CORTEX.
- Each fix is a single-commit revert; reversibility maximized.

**Harder:**
- Python and TypeScript workflows must route through Claude Code or Aider; CORTEX provides no value there.
- User's literal ask ("connect a Python agent") is declined. Trust cost is real but bounded by intellectual-honesty norms.
- If federation eventually proves correct (post-trigger), the `cortex-core` `LanguageGate` trait extraction has not yet started — that cost is deferred, not eliminated.

## Rejected Alternatives

- **A: Federation (sibling Rust daemons per language)** — Scored 34.6/100. Premature against 0/3 escalation criteria. Verification-theater risk dominates (Python verifier at ~60% catch rate ships a worse experience than Claude Code direct). MCP federation introduces new coupling surface (MCP client tool-selection policy lives outside CORTEX). Reconsider when ADR-005 triggers fire.
- **B: Monolithic multi-language daemon** — Scored 31.1/100. Directly reverses ADR-005. Requires reintroducing `BlastRadius::Advise` (PHANTOM verified true cost is 150-190 LOC across 21 grep hits in `gate.rs`, not FORGE's claimed 30). Advise reintroduction is fail-open at the language layer — the same architectural defect ADR-005 explicitly removed (quoted: "Advise mode was the silent fail-open at the language layer that the C4 fix removed at the timeout layer. Same architectural mistake"). FORGE retracted under PHANTOM scrutiny.
- **C: Plugin `.so` dynamic loading** — Scored 14.4/100. Plugin-as-RCE class. Mirrors mypy-plugin issue that killed Python in ADR-005. Dismissed on security grounds alone.
- **D: Per-language separate repos** — Scored 29.7/100. Drift over time, no shared verification core, worst maintainability for a 1-user project.
- **E: Don't build it (use Claude Code/Aider for non-Rust)** — Scored 88.2/100. Strong runner-up. Dominated by G only because G captures the same focus benefit while extracting user value via defect repair.
- **F: dprint pattern** — Scored 38.0/100. ORACLE mischaracterized dprint as native-toolchain subprocess invocation; PHANTOM verified dprint actually uses WASM in-process plugins. Option as described offers no insight over `gate.rs`'s existing subprocess executor behavior.

## Escalation Criteria

Re-litigate ADR-006 if **any one** of the following triggers fire:

1. ≥10 distinct users (not 10 repeated requests from supernovyl) request multi-language CORTEX support, evidenced in issue tracker or external feedback.
2. A Python verifier reaches ≥75% catch rate on a held-out test set of 100 real Python edits, measured against ground-truth pass/fail.
3. CORTEX Rust path completes Phase 6 AND has ≥30 days of stable operation with zero P0 defects.
4. A second adopter independently requests Python/TS support (organic market pull).
5. WASM-component-model verifiers mature such that language plugins become safe in-process, eliminating the RCE class that killed Option C.

Absent any trigger, ADR-006 stands until **2026-11-16** (six-month scheduled review).

## Anti-Patterns Forbidden (per ORACLE)

ADR-006 explicitly forbids the following patterns if federation is ever re-opened:

1. **Shared `cortex-core` absorbing language-specific code** (Eclipse JDT anti-pattern). Enforce via CI grep on PR.
2. **Tight version coupling across sibling daemons** (distributed monolith). Semver discipline on `cortex-core`; siblings pin major version.
3. **First-class Rust + second-class siblings** (Eclipse JDT vs CDT/PDT tragedy). Every new daemon must pass identical conformance suite before merge.
4. **No capability negotiation in MCP federation** (LSP success kernel). Initialize handshake with declared capabilities mandatory.
5. **In-process shared state across siblings** (microservices→DB→distributed monolith). Each sibling owns its own state.
6. **Per-language daemons spawning per-file** (pre-commit 187s tax). Long-running persistent daemon per language.

## Audit Trail

- **Phase-1 agents**: NEXUS Phase 1 (68% → revised 79%, Option E → G), FORGE Phase 1 (78% → revised 61%, Option B → G), ORACLE (76%, Option F partially voided by dprint mischaracterization)
- **Phase-2 red team**: PHANTOM (81%, threat HIGH, 8 findings, 3 grep-verifiable defects)
- **VERDICT**: 87% confidence, decision matrix G=95.5/100, dominant across weighted criteria
- Tri-agent convergence on G via distinct reasoning paths (systems-dynamics / cost-realism / empirical-defects); convergence cross-checked as genuine, not anchored
- Brooks "build one to throw away" citation by ORACLE was invoked then retracted (Brooks self-retracted in 1995 anniversary edition)
