
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
