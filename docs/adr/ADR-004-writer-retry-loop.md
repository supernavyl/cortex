# ADR-004: WRITER + Sandbox Retry Loop for Method::Apply

## Status
Accepted — 2026-05-08

## Context
`Method::Apply` is a stub returning "not yet implemented". CORTEX needs to
take a natural-language edit request, produce a verified code change, write
it to disk, and stream progress.

Three architectures were evaluated via multi-agent design session
(NEXUS 72%, FORGE 82%, PHANTOM 82%, VERDICT 72%):

- **Option A**: Full WRITER/BREAKER/ARBITER adversarial loop with `cargo test`
- **Option B**: WRITER + sandbox retry loop using compiler feedback
- **Option C**: WRITER + single-shot `SandboxGate::verify()`, no loop

PHANTOM's adversarial review demonstrated that BREAKER tests as proposed
never execute, because `SandboxGate::verify()` runs `cargo check` which does
not invoke `#[test]` functions. BREAKER as specified is decorative — it adds
latency and architectural debt for zero additional signal over the existing gate.

The existing `SandboxGate` has a 75% empirical catch rate (kill-switch test,
20 edits). Time-to-ship is the dominant constraint (solo project, 1-2 days
available).

## Decision
Adopt **Option B**: WRITER proposes edits via a single `propose_edit` tool
call (qwen3.6:27b via Ollama), sandbox runs `cargo check`, on failure the
compiler stderr is fed back to WRITER for up to 3 retry rounds. No BREAKER.
No ARBITER. No Anthropic client.

## Architecture

```
Method::Apply → run_apply_loop(req, ctx) in cortex-daemon/src/apply.rs
  ├── WRITER: OllamaModelClient → propose_edit tool call
  │     { workspace_relative_path: string, new_content: string, rationale: string }
  ├── SandboxGate::verify() (existing, untouched)
  │     on ACCEPT → write to disk, return success
  │     on REJECT → feed stderr to WRITER, retry (max 3 rounds)
  └── After 3 failures → return last compiler error to user
```

Concurrency: `Arc<Mutex<()>>` on workspace root in daemon state — serializes
Apply requests to prevent workspace race (PHANTOM flaw #6).

Path validation: `propose_edit.workspace_relative_path` validated as
`Path::new(p).is_relative() && !p.starts_with("..")` (PHANTOM flaw #5).

## Consequences

**Positive:**
- ~280 LOC, 1-2 day ship
- Single model dependency (Ollama qwen3.6:27b)
- Reuses validated `SandboxGate::verify()` unchanged
- Existing `parse_text_tool_calls` fallback covers qwen3 tool-call fragility
- Option A remains reachable as future ADR-005 if real-world usage shows
  logical-error class as top failure mode

**Negative:**
- Logical errors that compile are not caught (mitigated by user review at
  apply boundary)
- 3-round cap may truncate complex edits (mitigated by telemetry retune)
- No semantic test coverage at apply time

## Rejected Alternatives

**Option A rejected:** BREAKER is decorative under `cargo check`; fixing to
`cargo test` costs 3-5 days plus dual-client (Anthropic + Ollama) architecture
debt; qwen3 thinking-token regex fragility compounds across three model
exchanges. PHANTOM confirmed all three issues empirically against the codebase.

**Option C rejected:** ~170 LOC and 1 working day separates B from C, and
the UX of daemon self-healing trivial compile errors justifies the cost.
Single-shot returns compiler noise to the user on first error.

## Escalation Criteria (when to revisit this ADR)

- >25% of real Apply requests fail with logical errors that compile cleanly → Option A
- WRITER convergence p90 > 3 rounds on production fixtures → Option A or raise retry cap
- qwen3.6:27b tool-call reliability < 80% even with fallback → consider Option C as MVP
- Anthropic API becomes free/zero-latency → reconsider BREAKER as cheap insurance

## Done Criteria

- [ ] `Method::Apply` succeeds on 5-fixture integration test suite
  (add_function, fix_typo, refactor_signature, invalid_path, syntax_error_in_proposal)
- [ ] Retry loop demonstrates convergence within 2 rounds on add_function + fix_typo
- [ ] Deterministic error (not panic) on invalid_path + syntax_error after 3 rounds
- [ ] 2 concurrent Apply calls serialize correctly — no interleaved disk writes
- [ ] Progress notifications emitted at each round boundary, observable from CLI
- [ ] Workspace unchanged on rejection (sandbox cleanup verified)
- [ ] No new dependency on anthropic-sdk or equivalent
- [ ] PHANTOM flaws #5 (path ambiguity) and #6 (concurrency) closed with tests

## Confidence: 72%

Decision matrix weighted scores (590 max):
- Option A: 270 — loses on complexity, latency, time-to-ship, model-dependency risk
- Option B: 436 — wins on safety signal vs C; wins on complexity vs A
- Option C: 496 — highest raw score but unacceptable UX for trivial compile errors

What would change this ruling: see Escalation Criteria above.

## Agents Consulted
NEXUS (72%), FORGE (82%), PHANTOM (82%), VERDICT (72%)
