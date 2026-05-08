# ADR-003: Verification-First Pivot

**Date:** 2026-05-08
**Status:** ACCEPTED
**Confidence:** 78% (Mission ruling 2026-05-07)

---

## Context

ADR-002 (never written as a file, referenced in memory) spec'd CORTEX as a Rust CLI daemon with:
- Phase 3: MCP server
- Phase 4: MCP client
- Phase 5: Kairos (scheduling/orchestration layer)

**The claw-code problem:** claw-code (Rust clean-room rewrite of Claude Code, 190K stars, 109K forks, ~48K LOC as of 2026-05-07) covers the same ground as ADR-002 Phases 3–5: Rust daemon, MCP server, MCP client, permissions system, tool-trait architecture. Building those phases produces a worse-funded clone with zero distribution. That path is dead.

**The surviving thesis:** claw-code's `green_contract`, `FilesystemIsolationMode`, `bash_validation`, and `policy_engine` gate *access* (sandbox, permissions, isolation) — NOT *output correctness*. No pre-apply test/typecheck/reject pipeline exists in claw-code or any other incumbent. The METR 44% acceptance-rate finding (now revised toward null in Feb 2026 author update, n=16) motivated the original thesis, but the empirical gap remains: no tool verifies AI output before applying it.

---

## Decision

**CORTEX pivots to a verification-first daemon.** The product is a pre-apply gate that wraps any coding AI (claw-code, Claude Code, Cursor, raw API) and rejects changes that fail compile, test, type-check, or behavioral diff — before they touch the filesystem.

### What changes

| Component | Old trajectory (ADR-002) | New trajectory (ADR-003) |
|---|---|---|
| Phase 3: MCP server | Generic MCP exposure | Thin verification-policy MCP server — exposes `verify_edit`, `apply_if_clean` tools only |
| Phase 4: MCP client | Connect to arbitrary MCP servers | Removed — not CORTEX's job |
| Phase 5: Kairos | Scheduling layer | Removed — premature |
| Core differentiator | Feature parity with claw-code | Verification gate that no incumbent has |

### What stays

- Phase 0: bug fixes (net-positive regardless)
- Phase 1: Tool trait refactor (net-positive regardless)
- Phase 2: per-project config — **reassess after prototype results**

---

## Four Differentiators (spec-level)

### 1. Verification-First Pre-Apply Gate (BUILD FIRST)

Before any edit reaches the filesystem:

```
proposed_diff → sandbox apply → [compile | typecheck | lint | test subset] → ACCEPT / REJECT + reason
```

**Spec:**
- Sandbox: apply diff to temp directory (copy-on-write or `tempfile`)
- Verifiers (language-gated, run in parallel):
  - Rust: `cargo check` (fast, ~1s) + `cargo test --test-subset` (affected tests only)
  - TypeScript: `tsc --noEmit`
  - Python: `ruff check` + `pyright --outputjson`
- Output: `VerificationResult { accepted: bool, reason: String, elapsed_ms: u64, verifier: String }`
- Timeout: 10s hard cap per verifier (configurable)
- Prototype target: ~200 LOC in `crates/cortex-gate/`

**Kill switch:** Run on 20 real edits from a live claw-code session. If `cargo check` catch rate < 10% → kill CORTEX. If > 30% → ship aggressively.

### 2. Language-Calibrated Blast Radius (BUILD SECOND)

Static analysis is less reliable on dynamic languages. Gate aggressiveness must reflect this.

| Language | Verifier accuracy | Gate behavior |
|---|---|---|
| Rust | ~95% (strict type system) | HARD REJECT on any failure |
| TypeScript (strict) | ~85% | HARD REJECT on type errors, WARN on lint |
| Python | ~60% (dynamic, call graphs unreliable) | ADVISE only — log, never block |
| Other | Unknown | PASS-THROUGH with warning |

Implementation: `BlastRadius` enum in `cortex-core`, consumed by gate logic.

### 3. WRITER/BREAKER/ARBITER Adversarial Loop (BUILD THIRD)

For multi-file changes, three agents run in sequence:
- **WRITER**: generates the change (any LLM)
- **BREAKER**: attempts to find a test that fails given the change (adversarial, different model or temperature)
- **ARBITER**: if BREAKER finds a failure, arbitrates: accept WRITER's change anyway, request revision, or reject

Distinction from cooperative agents (AgentCoder et al.): BREAKER's goal is explicitly to break WRITER's output, not to help. Adversarial architecture.

Routing:
- WRITER + ARBITER: Qwen3.6-27B (local Ollama, fast, 77.2% SWE-bench)
- BREAKER: Claude Sonnet 4.6 API (different model = different failure modes, higher adversarial value)
- Fallback (offline): both WRITER and BREAKER on Qwen3.6-27B with different system prompts

### 4. Git-as-Memory with Causal Edges (BUILD FOURTH)

Index commit history with semantic embeddings (nomic-embed-text via Ollama) and causal edge types:

```
commit_A --[fix-for]--> commit_B
commit_C --[broke-by]--> commit_D
commit_E --[related-to]--> commit_F
```

Storage: SQLite (already in workspace via `rusqlite`). Schema:

```sql
CREATE TABLE commit_nodes (
    sha TEXT PRIMARY KEY,
    message TEXT,
    author TEXT,
    timestamp INTEGER,
    embedding BLOB  -- nomic-embed-text FP16
);

CREATE TABLE causal_edges (
    from_sha TEXT,
    to_sha TEXT,
    edge_type TEXT CHECK(edge_type IN ('fix-for', 'broke-by', 'related-to')),
    confidence REAL,
    PRIMARY KEY (from_sha, to_sha, edge_type)
);
```

Edge inference: heuristic first (regex on commit messages: "fix #123", "fixes", "broke", "revert"), then embedding similarity for `related-to`.

---

## Model Routing

| Task | Model | Reason |
|---|---|---|
| Verification (compile/typecheck) | Deterministic toolchain — no LLM | Ground truth only |
| WRITER (local/offline) | `qwen3.6:27b` via Ollama | Best dense coding model, 77.2% SWE-bench, fits 4090 |
| BREAKER | `claude-sonnet-4-6` API | Different failure modes from WRITER |
| ARBITER | `qwen3.6:27b` via Ollama | Cost-efficient for arbitration |
| Git-memory embeddings | `nomic-embed-text` via Ollama | ~274MB, leaves headroom on 4090 |
| Blast radius fallback (no API) | `qwen3.6:27b` for BREAKER | Degrade gracefully offline |

---

## Implementation Order

```
[NOW]   Prototype pre-apply gate (~200 LOC) → empirical kill switch test
[THEN]  crates/cortex-gate/ crate: full VerificationResult pipeline
[THEN]  BlastRadius enum + language-gated verifier dispatch
[THEN]  Thin MCP server: exposes verify_edit + apply_if_clean only
[LATER] WRITER/BREAKER/ARBITER loop
[LATER] Git-as-memory causal edges
```

---

## Rejected Alternatives

| Alternative | Reason rejected |
|---|---|
| Continue ADR-002 Phases 3–5 as planned | Produces claw-code clone with 5× fewer LOC and 0 distribution |
| Build on top of claw-code as a plugin | claw-code architecture unclear, fork risk, loses Rust ownership |
| Pure MCP verification server, no daemon | Viable — reassess if pre-apply prototype catch rate is high |
| Kill CORTEX entirely | Premature: verification gap is real, no incumbent solves it, prototype will resolve the bet |

---

## Open Questions

1. Does `cargo check` catch enough pre-apply errors to justify the gate? (Answered by kill-switch prototype.)
2. Should the MCP server be the primary interface, or CLI-first? (Defer to after prototype.)
3. Is METR's revised null finding fatal to the thesis? (No — the empirical gap in incumbents is independent of METR's population-level finding.)
