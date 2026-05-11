# Insane bench — historical results

## algo-smoke-1-claude.tsv + algo-smoke-2-cortex-deepseek.tsv (2026-05-11 07:10-07:19)

First head-to-head smoke on the 3 algorithmic tasks (rope, bloom_filter, lru_threadsafe).

**Finding**: cortex+deepseek-v4-pro produced 0/3 actual code (model returned text
instead of calling propose_edit tool). Claude Code: 3/3 with passing tests.

Algo-1 also recorded cortex failures from a stale daemon socket (pre-fix).
Algo-2 is the cortex re-run after the daemon helper fix — proves the apply path
is functional but the deepseek model doesn't engage Ollama's tool-call format
through cortex's apply loop.

Pivoting subsequent runs to qwen3-coder:480b-cloud (cortex's intended flagship
per ADR-003) + project-scale tasks.
