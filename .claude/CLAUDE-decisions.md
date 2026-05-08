# CORTEX Architectural Decisions

## ADR-001: Personalization Architecture — TOML Config, Model Routing, Workspace Detection

**Date**: 2026-04-06
**Status**: ACCEPTED
**Decision by**: VERDICT (mission arbiter)
**Confidence**: 82%

### Context

CORTEX v0.1 hardcodes all configuration via `Config::default()`. No config file loading, no intelligent model routing (just prompt length < 500 chars), no workspace awareness. The user has 13 Ollama models and 39 projects. Personalization requires three capabilities: persistent config, smart model selection, and project-aware permission scoping.

Three agents analyzed this problem:
- NEXUS (87%): Systemic approach with 5 task categories, VRAM warmth checks, `[[projects]]` registry
- FORGE (94%): Pragmatic approach with 2-tier scorer, ~500 LOC estimate
- PHANTOM (91%): Adversarial review found 6 flaws, 2 critical

### Decision

#### 1. TOML Config System

**Add dependencies**: `toml = "0.8"` and `shellexpand = "3"` to `cortex-core`.

**Add `#[serde(default)]` on all config structs**: `Config`, `DaemonConfig`, `ModelConfig`, `ContextConfig`. Without this, partial config files crash serde deserialization. (PHANTOM flaw #1, confirmed against code.)

**New method `Config::load() -> Result<Config>`**:
- Check `$CORTEX_CONFIG` env var, fall back to `~/.config/cortex/config.toml`
- File exists: `toml::from_str()` + expand tildes on all `PathBuf` fields via `shellexpand`
- File missing: return `Config::default()` (preserves current behavior)
- File malformed: return `anyhow::Error` with file path context
- `anthropic_api_key`: fall back to `$ANTHROPIC_API_KEY` env var after TOML load

**Tilde expansion** (PHANTOM flaw #6): `PathBuf::from("~/path")` does NOT resolve. All PathBuf fields from TOML must be post-processed through `shellexpand::tilde()`.

**Change both binaries**: `Config::default()` -> `Config::load()?` in `cortex-daemon/src/main.rs:15` and `cortex-cli/src/main.rs:46`.

#### 2. Model Routing

**Config shape** (replaces single `local_model`):
```toml
[models]
ollama_url = "http://localhost:11434"
fast_model = "huihui_ai/qwen3-abliterated:8b"
default_model = "huihui_ai/qwen3-coder-abliterated:30b"
heavy_model = "huihui_ai/qwq-abliterated:32b"
routing_threshold = 60
```

**New module `cortex-core/src/router.rs`** (~80 LOC):
- `route(prompt, files, tier) -> ModelSelection`
- For `ModelTier::Local`/`Cloud`: direct override (unchanged behavior)
- For `ModelTier::Auto`: compute complexity score:
  - `word_count = prompt.split_whitespace().count()`
  - `file_bonus = files.len() * 10`
  - `keyword_boost`: +15 for "refactor|architect|design|migrate|optimize", +10 for "debug|fix|error|bug"
  - Score >= `routing_threshold` -> `default_model`, else -> `fast_model`
  - `heavy_model` reserved for future escalation (v2: retry on fast failure)
- Log routing decision at `info` level always: model chosen, score, reason

**Rejected alternatives**:
- NEXUS's 5-category task classifier: too complex for v1, keyword matching is fragile
- NEXUS's `/api/ps` VRAM warmth check: adds latency per request, underdocumented
- FORGE's hardcoded threshold without config: threshold IS fabricated, must be tunable

#### 3. Workspace Detection

**Protocol change**: Add `cwd: Option<String>` to `Method::Ask` in `protocol.rs`. Field name `cwd` chosen over `workspace_hint`.

**CLI change**: Auto-populate `cwd` from `std::env::current_dir()` in CLI.

**New module `cortex-core/src/workspace.rs`** (~60 LOC):
- `detect_workspace(cwd: &Path) -> PathBuf`: walk up from cwd looking for markers (`.git`, `Cargo.toml`, `package.json`, `pyproject.toml`, `project.godot`, `go.mod`). Return first match's parent, or cwd itself.

**Permission model** (resolves PHANTOM flaw #2):
- **Keep `PermissionMode::FullAccess`** as the active mode. Changing to `WorkspaceWrite` silently disables bash (spec.rs: bash requires FullAccess). Critical flaw caught by PHANTOM.
- **Set `workspace_root`** from detected project root. `check_file_write()` constrains writes to workspace. Adds file write sandboxing without breaking bash.
- Bash sandboxing deferred to v2.

### Ship Plan

**PR1 — Config** (~45 LOC new):
- `toml` + `shellexpand` deps, `#[serde(default)]` on all structs
- `Config::load()` with env fallback and tilde expansion
- Tests: partial TOML, missing file, malformed file, tilde expansion

**PR2 — Routing + Workspace** (~145 LOC new):
- `router.rs` with complexity scorer
- `workspace.rs` with marker walk-up
- `cwd` field in `Method::Ask`, CLI auto-sends
- Tests: scoring boundaries, workspace detection, permission sandboxing

### Consequences

**Positive**: Config persists, routing adapts to task complexity, file writes sandboxed.
**Negative**: Routing threshold arbitrary until tuned, no bash sandboxing, tilde expansion adds a dep.

### What Would Change This Decision

- Evidence that keyword-based routing is worse than random (50+ real prompts)
- A security incident caused by unsandboxed bash
- Ollama `/api/ps` proving reliable (would add warmth check)
- User requesting project-specific model overrides (would add `[[projects]]` registry)

---

## ADR-002: Evolution Architecture — Tool Trait, MCP, Per-Project Config, Kairos

**Date**: 2026-04-06
**Status**: ACCEPTED
**Decision by**: VERDICT (mission arbiter)
**Confidence**: 81%

### Context

CORTEX is a 5-crate Rust workspace (~4300 LOC, edition 2024, rust-version 1.93) with a working agentic loop, 6 built-in tools, Ollama integration, tree-sitter symbol indexing, and complexity-based model routing. Four capabilities are requested: dynamic tool plugins, MCP client/server, per-project customization, and an autonomous Kairos cycle.

Four agents analyzed this:
- NEXUS (81%): Systemic analysis. 3-method ToolPlugin trait, 2 new crates, failure cascade map.
- FORGE (82%): Pragmatic implementation. 2-method Tool trait, rmcp SDK, LOC estimates.
- ORACLE (82%): Historical precedent. LSP analogy, Brooks' warning, "plugin IS MCP" thesis.
- PHANTOM (87%): Adversarial review. 8 flaws found, 3 HIGH severity in current code.

### Decision Matrix

| Criterion | Weight | NEXUS | FORGE | ORACLE |
|-----------|--------|-------|-------|--------|
| Trait simplicity | 8 | 5 (3 methods, boilerplate) | 9 (2 methods, minimal) | N/A (no trait) |
| MCP integration | 9 | 7 (custom + MCP) | 8 (rmcp SDK) | 6 (MCP-only, no internal trait) |
| Risk of security regression | 10 | 4 (boundary check not threaded) | 5 (same gap identified) | 7 (fewer surfaces) |
| Implementation feasibility | 7 | 6 (high complexity) | 8 (LOC-budgeted) | 5 (strictest sequencing) |
| Historical precedent alignment | 6 | 6 | 7 | 9 (LSP/VS Code analogy) |
| Second-system risk | 8 | 5 (2 new crates + refactor) | 7 (incremental) | 8 (minimalist) |

### Conflict Resolution

#### 1. Tool Trait Design: NEXUS `ToolPlugin` (3 methods) vs FORGE `Tool` (2 methods)

NEXUS proposed `specs()`, `can_handle()`, `execute()`. FORGE proposed `spec()`, `execute()`.

**Ruling**: FORGE's 2-method design wins. `can_handle()` is redundant -- the executor already dispatches by tool name from `spec().name`. Three methods means more boilerplate for every tool implementation with no functional gain. FORGE's `Cow<'static, str>` for ToolSpec.name/description is adopted -- built-in tools use zero-cost `&'static str` via `Cow::Borrowed`, dynamic plugins use `Cow::Owned`.

**Confidence**: 88%

The trait signature:

```rust
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn execute(
        &self,
        input: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + '_>>;
}
```

`async fn` in traits is stable in Rust 1.93 but requires boxing for `dyn Tool` dispatch. PHANTOM confirmed this constraint (AFIT + dyn requires boxing). The return type is explicit rather than using AFIT to keep dynamic dispatch working.

#### 2. ToolSpec Location: cortex-core (NEXUS) vs cortex-tools (FORGE)

**Ruling**: Split. Neither agent proposed the right answer.

- **Move to cortex-core**: `PermissionMode`, `PermissionPolicy`, `PermissionOutcome`. These are security primitives that MCP, Kairos, and any new crate will need.
- **Keep in cortex-tools**: `ToolSpec`, `Tool` trait, `ToolError`, `ToolExecutor`. These are tool-specific and belong with the tool engine.

This avoids scope creep in cortex-core while making the permission system universally available.

**Confidence**: 82%

#### 3. Plugin Protocol: "Plugin IS MCP" (ORACLE) vs In-Process Trait (NEXUS/FORGE)

ORACLE argued that building a custom plugin protocol duplicates MCP's purpose. NEXUS and FORGE both built in-process trait systems.

**Ruling**: Both are needed. ORACLE is wrong on the "plugin IS MCP" claim, but right on the spirit.

The 6 built-in tools (read_file, write_file, edit_file, bash, glob, grep) require direct access to `PermissionPolicy.check_file_write()` and workspace filesystem state. They cannot run as external MCP servers -- they ARE the core tool surface. The in-process `Tool` trait is not a "plugin protocol"; it is the internal tool abstraction.

MCP serves a different purpose: consuming external tool servers and exposing CORTEX's tools to external consumers.

**Design constraint**: External tools (anything that doesn't need PermissionPolicy access) should be MCP servers, not in-process plugins. The in-process `Tool` trait is reserved for tools that need direct CORTEX state access. This prevents the ecosystem-splitting problem ORACLE warned about.

**Confidence**: 91%

#### 4. Phase Ordering: Config First (ORACLE) vs Plugin First (NEXUS/FORGE)

ORACLE: Config -> Plugin -> MCP Server -> MCP Client -> Kairos.
NEXUS: Foundation -> Plugin+MCP -> Config -> Kairos.
FORGE: Plugin||Config -> MCP -> Kairos.

**Ruling**: The existing config system is already built (config.rs, 275 LOC, TOML, env overrides). There is nothing to "build first" for config. Per-project config is an additive feature.

Correct order based on dependency analysis:

1. **Tool trait refactor + bug fixes** (foundation -- everything depends on this)
2. **Per-project config** (additive, ~150 LOC, immediate user value)
3. **MCP server** (expose tools to external consumers)
4. **MCP client** (consume external tool servers)
5. **Kairos** (depends on all above being stable)

ORACLE was wrong to prioritize config (it's done). FORGE's parallel approach risks interface churn.

**Confidence**: 84%

#### 5. Write-Boundary Security Regression (PHANTOM HIGH)

PHANTOM identified that `check_file_write()` is embedded inline in executor.rs match arms (lines 84-106). When the match block is replaced by dynamic `Tool` dispatch, this check vanishes.

**Ruling**: Centralize the check in `ToolExecutor::execute()` as a pre-dispatch policy hook.

```rust
// In ToolExecutor::execute(), after finding the tool and checking permission:
if spec.required_permission >= PermissionMode::WorkspaceWrite {
    if let Some(path) = input.get("file_path").and_then(Value::as_str) {
        let p = std::path::Path::new(path);
        if let PermissionOutcome::Deny { .. } = self.policy.check_file_write(p) {
            return Err(ToolError::permission(
                format!("write to '{}' denied: outside workspace boundary", path)
            ));
        }
    }
}
// Then dispatch to tool.execute(input)
```

This keeps the security check in ONE place. Every tool -- built-in or plugin -- with `WorkspaceWrite` or higher permission goes through the same boundary check. No tool implementation can bypass it.

**Confidence**: 92%

#### 6. std::sync::Mutex on SymbolStore (PHANTOM HIGH)

PHANTOM flagged `Arc<Mutex<SymbolStore>>` in server.rs as a deadlock risk in async context.

**Ruling**: PHANTOM is partially wrong on the deadlock claim. The current code holds `std::sync::Mutex` only during synchronous rusqlite operations, never across await points. This is contention, not deadlock.

However, the fix is still warranted for Kairos: `std::sync::Mutex` -> `std::sync::RwLock`. Multiple concurrent reads for `build_symbol_context()` (called every request), exclusive write for indexing (called rarely). SQLite with WAL mode already supports concurrent reads.

**Concrete change**: `Arc<Mutex<SymbolStore>>` -> `Arc<RwLock<SymbolStore>>`. Read-path callers use `.read().unwrap()`. Write-path callers use `.write().unwrap()`.

**Confidence**: 86%

#### 7. MCP Dual-Role Feedback Loop (PHANTOM MED)

PHANTOM: External MCP client writes file -> Kairos observes change -> triggers turn -> writes more -> loop.

**Ruling**: This is a genuine Kairos-phase risk but irrelevant to MCP implementation. Mitigation belongs in cortex-kairos, not cortex-mcp.

Kairos mitigation: workspace-level write semaphore. When Kairos executes a turn, it holds a write token. File changes during execution are queued. After turn completion, a debounce window (configurable, default 5s) absorbs rapid successive changes. The debounce resets on each new change during the window. This is a state machine with three states: `Idle`, `Executing`, `Debouncing`.

**Confidence**: 72% (theoretical -- nobody has built this yet)

### Immediate Bug Fixes (Pre-Phase, from PHANTOM)

These are defects in the current codebase that must be fixed before any evolution work:

1. **`std::process::exit(0)` on Shutdown** (server.rs:166): Socket file never cleaned. Replace with graceful shutdown via `tokio_util::sync::CancellationToken`. The server loop breaks, drops the listener, then `std::fs::remove_file(socket_path)`.

2. **No model call timeout** (ollama.rs): `reqwest::Client::new()` has no timeout. Hanging Ollama = permanent block. Fix: `Client::builder().timeout(Duration::from_secs(300)).build()`. Wrap agentic loop model calls in `tokio::time::timeout(Duration::from_secs(600), ...)`.

3. **Cloud auto-promote at 2000 chars** (router.rs:48): `prompt.len() > 2000` with API key silently routes to cloud. Under Kairos, context-enriched prompts will trivially exceed this. Fix: raise threshold to 10000 chars, or remove auto-promote entirely and require explicit `--tier cloud`.

4. **all_specs() allocated per execute() call** (executor.rs:64): Vec rebuilt on every tool invocation. Fix: cache tool registry at ToolExecutor construction (naturally solved by the Tool trait refactor -- specs are built once during `register()`).

### Crate Structure

```
cortex/
  crates/
    cortex-core/      # config, protocol, router, workspace, permissions
    cortex-tools/     # Tool trait, ToolExecutor, runtime, built-in tools
    cortex-context/   # tree-sitter indexing, SymbolStore
    cortex-mcp/       # NEW: MCP server + client (dep: rmcp 1.3.0)
    cortex-kairos/    # NEW: file watcher, autonomous cycle engine (dep: notify)
    cortex-daemon/    # server, OllamaClient, integration point
    cortex-cli/       # CLI binary
```

Estimated LOC after evolution: ~5100 (current 4300 + MCP ~600 + Kairos ~700 - ~500 from refactor consolidation).

### Dependency Graph After Evolution

```
cortex-core (permissions, config, protocol, workspace)
    ^           ^            ^
    |           |            |
cortex-tools  cortex-mcp  cortex-kairos
    ^           ^     ^      ^
    |           |     |      |
    +-----------+-----+------+
                |
          cortex-daemon
                ^
                |
          cortex-context
```

### Ship Plan

**Phase 0 -- Bug Fixes** (~100 LOC changed, 1 session):
- Fix `std::process::exit(0)` -> graceful shutdown
- Add reqwest timeout + model call timeout
- Fix cloud auto-promote threshold
- `Mutex` -> `RwLock` on SymbolStore

**Phase 1 -- Tool Trait Refactor** (~300 LOC, 1-2 sessions):
- Move `PermissionMode`, `PermissionPolicy`, `PermissionOutcome` to cortex-core
- Define `Tool` trait in cortex-tools
- Refactor 6 built-in tools to implement `Tool`
- Refactor `ToolExecutor` to `Vec<Box<dyn Tool>>` with centralized write-boundary check
- Cache tool specs at construction
- Tests: permission enforcement survives refactor (regression test for write boundary)

**Phase 2 -- Per-Project Config** (~150 LOC, 1 session):
- `.cortex/SYSTEM.md` loaded from workspace root, prepended to system prompt
- `.cortex/config.toml` for per-project model overrides, permission mode
- Config precedence: per-project > global > defaults

**Phase 3 -- MCP Server** (~350 LOC, 1-2 sessions):
- New crate `cortex-mcp` with `rmcp` dependency
- Expose built-in tools via MCP server protocol (stdio transport)
- MCP consumers get the same permission enforcement as internal callers
- Integration test: external MCP client calls read_file, gets result

**Phase 4 -- MCP Client** (~250 LOC, 1 session):
- Consume external MCP tool servers
- MCP tools appear in `ToolExecutor.available_tools()` alongside built-in tools
- MCP tool invocations routed through permission system (WorkspaceWrite check on file_path)
- Config: `[[mcp_servers]]` section in config.toml with command + args

**Phase 5 -- Kairos** (~700 LOC, 2-3 sessions):
- New crate `cortex-kairos` with `notify` dependency
- File watcher on workspace root
- State machine: Idle -> Executing -> Debouncing
- Workspace write semaphore prevents re-entrant triggers
- User preemption: any CLI `ask` command interrupts Kairos mid-turn
- Model call budget: configurable max turns per Kairos trigger (default: 3)
- Integration test: file change triggers a cycle, second change during cycle is queued

### Acceptance Criteria

Phase 0: All 4 bugs fixed. `cargo test` passes. Daemon restarts cleanly after shutdown.
Phase 1: `ToolExecutor` uses `Vec<Box<dyn Tool>>`. Write to `/etc/passwd` is denied. Existing tests pass without modification. New test: register a custom tool, execute it.
Phase 2: Create `.cortex/SYSTEM.md` in a project. Start CORTEX in that directory. System prompt includes the file contents. Per-project model override works.
Phase 3: Run `cortex mcp-server` as stdio MCP server. External MCP client lists tools, calls `read_file`, gets file contents.
Phase 4: Configure an external MCP server in config.toml. Its tools appear in `cortex status`. Model can invoke them.
Phase 5: Touch a file in a watched workspace. Kairos triggers a turn within 10 seconds. Touch a file during the turn. No re-entrant loop. CLI `ask` preempts Kairos.

### Top 3 Risks and Mitigations

1. **Write-boundary regression during refactor** (probability: 40% without mitigation, 5% with). Mitigation: regression test that asserts write to `/tmp/outside-workspace/file` is denied by a WorkspaceWrite-permission tool. Run this test in CI. This test must exist BEFORE the refactor begins.

2. **rmcp API instability** (probability: 25%). rmcp 1.3.0 has 6.9M downloads but is still pre-2.0. Mitigation: wrap rmcp behind a thin adapter layer in cortex-mcp. If rmcp breaks, only the adapter changes.

3. **Kairos runaway token burn** (probability: 35%). An autonomous agent with no budget burns local compute or API credits. Mitigation: hard budget of max 3 model calls per Kairos trigger, configurable. Kairos disabled by default -- requires explicit `[kairos] enabled = true` in config.

### Consequences

**Positive**: Dynamic tool system. MCP ecosystem access. Per-project customization. Autonomous operation.
**Negative**: 7 crates increases workspace complexity. rmcp dependency ties to external release cycle. Kairos introduces a new failure domain (runaway loops, stale workspace detection).

### What Would Change This Decision

- rmcp proving unsuitable (API churn, bugs) -> fall back to hand-rolled MCP protocol (the spec is simple)
- Evidence that in-process plugins are never needed (all tools work as MCP) -> remove Tool trait, adopt ORACLE's MCP-only position
- Kairos proving fundamentally unsafe in practice -> defer indefinitely or require explicit user approval per trigger
- CORTEX remaining a single-user tool forever -> simplify concurrency model, remove RwLock, keep Mutex

### Dissent Acknowledged

ORACLE's "plugin IS MCP" thesis has intellectual merit. If the MCP ecosystem matures to the point where even built-in tools can be MCP servers with negligible overhead, the in-process Tool trait becomes unnecessary complexity. I rule against it NOW because the permission integration story for MCP-as-plugin is unsolved, but reasonable people could disagree.

---

## ADR-003: Fusion Architecture — Daemonless Core, Gate Wiring, Tier Collapse, Anthropic Backend

**Date**: 2026-05-08
**Status**: ACCEPTED
**Decision by**: VERDICT (Mission 1.8 arbiter, Opus)
**Confidence**: 79%

### Context

CORTEX is 8,791 LOC Rust across 6 crates with a working daemon, context engine, and tool system. ADR-002 defined Phase 1-5 evolution. Pre-apply verification gate (gate.rs, 319 LOC) was designed as CORTEX's unique architectural differentiator — but Phase 2 empirical inspection revealed it is unwired in production: `enable_gate()` has zero callers outside test code. GreenContract graduated verification (4 levels) is 4/4 dead code in the runtime path. Post-edit verification is actually standard across competitors (Cline has real-time LSP monitoring, Aider has `--auto-test` loop) — CORTEX's uniqueness claim was REFUTED by PROBE.

The user wants CORTEX as their daily coding driver with "architecture over model capability."

Five agents analyzed this:
- NEXUS (74%): Morphological analysis. 10-dimension design matrix, verification daemon framing, Kairos autonomy as blue ocean feature.
- FORGE (84%): Premortem. 11 kill shots, survival architecture with daemonless-first principle.
- PHANTOM (82%): Devil's advocate. 8 flaws found, gate characterized as "latency anchor dressed as safety feature."
- PROBE (92% empirical confidence): 8 confirmed, 2 refuted, 1 partial, 4 unverifiable across 2 rounds. 1 protocol violation found.
- VERDICT (79%): Ruled on all 5 conflicts. Held confidence below 85% due to 20.2pp calibration gap.

### Decision

#### 1. Daemonless Single-Binary Default

**Ruling**: FORGE wins. Daemonless mode is the default. `cortex ask "prompt"` works with zero setup. The daemon is strictly optional, launched via `cortex watch` for Kairos file watching only.

**Why**: The daemon is a UX cliff with weak architectural justification. It exists to hold in-memory state (SymbolStore, KairosState) that is already backed by SQLite. Every successful AI coding tool uses zero-startup-friction.

**New function**: Extract model call + tool execution from server.rs into standalone function in cortex-core or cortex-daemon. CLI `ask` subcommand calls it directly without the Unix socket layer.

**Confidence**: 82%

#### 2. Pre-Apply Gate Wiring (Immediate)

**Ruling**: Wire the gate NOW. Change server.rs:411 from `ToolExecutor::new(policy)` to `ToolExecutor::new(policy).enable_gate(PreApplyGate::default())`. Import `PreApplyGate` from `cortex_core::gate`.

**Also fix 3 confirmed gate bugs in same PR**:
1. **No timeout**: Wrap all 3 `Command::output()` calls in `tokio::task::spawn_blocking` + `tokio::time::timeout(30s)`. Currently bare `std::process::Command::output()` blocks the tokio runtime indefinitely.
2. **Polyglot blindness**: `Language::detect()` early-returns on first marker. Change to return `Vec<Language>` and run all detected language checks. Currently Tauri projects (Cargo.toml + tsconfig.json) only get Rust checking.
3. **GreenContract stripped**: Keep enum variants for domain model but only enforce `Package` level in v1. The `GreenContractOutcome` field is populated but never checked by any caller.

**Confidence**: 75% on gate providing measurable value (blocking-vs-advisory distinction is real but unmeasured).

#### 3. Model Tier Collapse: 18 → 4

**Ruling**: 18 `ModelTier` variants collapse to 4:

| New Tier | Local Model | Cloud Model | Use Case |
|----------|------------|-------------|----------|
| FAST | qwen3:4b | claude-haiku-4-5 | Quick answers, simple edits |
| STANDARD | qwen3.6:27b | claude-sonnet-4-6 | Daily coding (default) |
| HEAVY | qwen3.5:35b | claude-opus-4-7 | Complex refactors, architecture |
| AUTO | N/A | N/A | Keyword-based routing to above tiers |

Remove all per-model config fields from `ModelConfig` except the 3 tier models + cloud toggle. Preserve `detect_task_type()` keyword matching (43 keywords) but map to 4 buckets instead of 17.

**Note**: VERDICT fabricated specific Anthropic parameter counts (4B/27B/35B) — PROBE audit flagged this as 1 protocol violation. Anthropic does not disclose model sizes. Counts removed from this ADR.

**Confidence**: 88%

#### 4. Anthropic Backend

**Ruling**: Implement `AnthropicClient` struct implementing the existing `ModelClient` trait (runtime.rs:36). Supports prompt caching, streaming, and tool use in Anthropic format. Reads API key from `claude-vault get ANTHROPIC_API_KEY`. ~300 LOC in new file `cortex-daemon/src/anthropic.rs`.

**Confidence**: 90%

#### 5. Kairos Autonomy Deferred to v3

**Ruling**: FORGE wins on timing. Adding autonomous Kairos while the gate is unwired is building on a nonexistent foundation. The concept is mechanically sound — preserve a hook point as `trait KairosDecide` with a single method `fn evaluate(&self, state: &KairosState) -> Option<KairosAction>`. No implementation in v1/v2. v3 autonomy loop implements this trait.

**Confidence**: 78%

#### 6. LIS Integration: File Export Bridge

**Ruling**: CORTEX writes workspace snapshots (symbol graph, git context, Kairos hot files) to `~/.config/cortex/export/workspace.json` on each `cortex ask` completion. LIS reads on startup. ~50 LOC. Avoids cross-process coupling. Deferred NEXUS's HTTP server proposal for ship speed — not architectural purity.

**Confidence**: 72%

#### 7. VSCode Extension Scaffold

**Ruling**: TypeScript extension spawning `cortex ask` as child process. Displays streaming response + gate results inline. ~2,000 LOC. Built AFTER daemonless mode ships.

### Ship Plan

**Session 1 — Gate Wiring + Bug Fixes** (~150 LOC):
- Wire enable_gate() in server.rs:411 (2 lines)
- Add tokio::time::timeout(30s) on all 3 check methods in gate.rs (~30 lines)
- Fix Language::detect() to return Vec<Language> (~20 lines)
- Strip GreenContract enforcement to Package only (~10 lines)
- Add socket cleanup on startup (not just shutdown) (~5 lines)
- Tests: gate catches known-bad code, timeout fires on hung process, polyglot detection works

**Session 2 — Daemonless Mode** (~200 LOC):
- Extract standalone ask function from server.rs
- CLI ask subcommand uses standalone path by default
- SQLite SymbolStore loaded from disk, not in-memory
- `cortex watch` subcommand starts daemon (existing code path)
- Tests: daemonless ask works without running daemon

**Session 3 — Tier Collapse + Anthropic Backend** (~400 LOC):
- Reduce ModelTier enum to 4 variants in protocol.rs
- Update router.rs mapping
- Remove 14 model fields from ModelConfig
- Implement AnthropicClient in new file
- Tests: routing maps correctly to 4 tiers, AnthropicClient implements ModelClient trait

**Session 4 — Integration + Extension** (~300 LOC + 2,000 TS):
- Add memory_query tool for Memory MCP (~100 LOC)
- Add LIS export bridge (~50 LOC)
- Add KairosDecide trait hook (~30 LOC)
- VSCode extension scaffold (~2,000 LOC TypeScript)

### Consequences

**Positive**: Zero-friction startup. Gate actually runs. Simpler routing. Anthropic model access. LIS awareness.
**Negative**: Daemonless mode requires SQLite reads per invocation (acceptable — currently sub-millisecond). Gate adds 2-30s latency per write-capable tool call. File export bridge less real-time than HTTP. Kairos autonomy delayed.

### What Would Change This Decision

1. **Empirical gate data**: If wired gate blocks fewer than 5% of tool calls in first 100 calls, the gate is cosmetic — shift priority to Kairos autonomy or LIS integration.
2. **User testing of daemonless vs daemon**: If daemonless mode is NOT actually preferred (user reports daemon is fine), reprioritize.
3. **Competitor movement**: If Cline or Aider ships synchronous blocking verification before CORTEX wires its gate, gate value drops to near-zero.
4. **LIS real-time needs**: If LIS needs real-time CORTEX queries (not file export), revisit NEXUS's HTTP server proposal.

### Falsifiable Predictions

1. Gate catches >=1 genuine compilation error within first 50 write-capable tool calls on CORTEX's own codebase (72% conf, deadline 2026-05-22).
2. Daemonless mode `cortex ask "explain main"` completes in under 3s mean wall-clock time across 10 runs (68% conf, deadline 2026-06-07).
