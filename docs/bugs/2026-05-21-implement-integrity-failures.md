---
id: BUG-001
title: "`cortex implement` reports step success when no artifacts were written; cloud planner returns 0 steps"
status: Open
date: 2026-05-21
component: cortex-daemon (implement orchestrator), cortex-daemon (planner)
severity: HIGH
reporter: supernovyl
binary: cortex-cli 0.2.0 (target/release, commit pre-tag)
---

## Summary

Two independent integrity failures in `cortex implement`, observed in a single
session against the same prompt (a multi-file Rust CLI scaffold spec, ~1.6 KB,
fully concrete).

1. **Local writer (qwen3.6:27b)** reports `step N: implemented and verified
   with cargo check` for steps whose artifacts do not exist on disk after the
   run completes. The retry-loop exit condition is checking the wrong
   invariant.
2. **Cloud planner (qwen3-coder-next:cloud)** returns `0 steps` and aborts
   with `task may be too vague` for the same prompt that the local planner
   decomposed into 6 steps.

Both are reproducible.

## Reproduction

```bash
mkdir -p /tmp/cortex-bug && cd /tmp/cortex-bug && cargo init --bin --name adr-cli
cortex-cli implement "$(cat <<'EOF'
Build a Rust CLI named adr-cli for managing Architecture Decision Records.

Single binary crate. Use modules in separate files:
- src/main.rs: clap-derive CLI entry point with subcommands.
- src/parser.rs: parse a markdown ADR with YAML frontmatter into a typed Adr struct.
- src/store.rs: walk configured root directories and load all ADRs.
- src/config.rs: load roots from ~/.config/adr-cli/config.toml with a sensible default.
- src/error.rs: anyhow-based Error/Result type aliases.

[... full schema and acceptance criteria, see ~/projects/adr-cli session log ...]
EOF
)"
# Then with --cloud against the same dir for bug #2.
```

## Bug 1 — Hollow-shell `[OK]` reports

### Observed (local, qwen3.6:27b)

Daemon log fragments (`/tmp/claude-1000/.../bdcfiwaj2.output`):

```
[EXECUTE 3/6] Create src/parser.rs containing the Adr struct, Status enum...
[DEBATE WRITER] edit_file [FAIL]
[DEBATE WRITER] edit_file [FAIL]
[DEBATE WRITER] edit_file [FAIL]
[DEBATE WRITER] write_file [FAIL]
[DEBATE WRITER] bash [FAIL]
  [OK] step 3: implemented and verified with `cargo check && cargo test`

[EXECUTE 4/6] Create src/store.rs using walkdir to recursively scan...
[DEBATE WRITER] bash [FAIL]          (x9 across the step)
[DEBATE WRITER] glob [OK]            (x4 — reads only)
[DEBATE WRITER] read_file [OK]       (x7 — reads only)
[DEBATE WRITER] edit_file [FAIL]
[DEBATE WRITER] write_file [FAIL]
max iterations reached, stopping
  [OK] step 4: implemented and verified with `cargo check`
```

Final filesystem state after `[INTEGRATE] checking 6 files... cargo check: PASS`:

```
src/
├── config.rs    (145 lines, 4 unit tests pass — cortex actually wrote this)
├── error.rs     (8 lines — cortex actually wrote this)
└── main.rs      (11 lines, the cargo-init stub, untouched)
```

**`src/parser.rs` and `src/store.rs` do not exist.** Steps 3 and 4 reported
`[OK]` regardless. The integrate-phase `cargo check: PASS` is vacuous because
the unchanged tree compiles trivially.

### Likely root cause

In `crates/cortex-daemon/src/server/implement.rs`, the step-success criterion
appears to be:

- `cargo check` returns 0 on the workspace as-it-stands, AND
- the writer's tool-call loop terminated (either by producing a final message
  or by hitting `max iterations reached`).

Neither condition implies the writer actually produced the artifact named in
the step description. When all `write_file` / `edit_file` tool calls fail and
the writer exhausts iterations on reads, the workspace is unchanged → `cargo
check` still passes → step is reported `[OK]`.

### Suggested fix

Step success must require **at least one successful `write_file` or
`edit_file` tool invocation within the step**, in addition to `cargo check`
passing. Tracked at the orchestrator level, not delegated to the writer's
self-report. A reasonable extra check: snapshot the file-set + content-hashes
before the step, require a strict superset / hash delta after.

Bonus: the final `[INTEGRATE] checking N files...` line currently reads the
*planned* file count from the step descriptions, not the actual on-disk file
count. Replace with `walkdir` of `src/` and report the real number.

## Bug 2 — Cloud planner returns 0 steps

### Observed (cloud, qwen3-coder-next:cloud, `--cloud`)

```
[CLOUD IMPLEMENT] planning → execute → integrate → report
[PLAN] decomposing task with qwen3-coder-next:cloud...
[PLAN] 0 steps identified:
error: planner produced no steps — task may be too vague
```

The prompt is identical to the local run, where qwen3.6:27b produced a
6-step plan. The prompt is ~1.6 KB, names six specific files, lists
seven dependencies, specifies an exact YAML schema, and lists four CLI
subcommands. It is the opposite of vague.

### Likely root cause

One of:
- The cloud planner prompt template strips or truncates content that the
  local template preserves
- The JSON-extraction step parsing the cloud model's response is fragile to
  formatting (markdown fences, leading prose, etc.) and silently produces
  `[]`
- The cloud model returns a refusal-style "I need more information" response
  that the parser treats as 0 steps without surfacing the underlying text

### Suggested fix

When the planner returns 0 steps, dump the raw model response into the log
(at trace or warn level) before erroring out, so the failure mode is
diagnosable in the field. Currently the model's response is lost.

## Severity rationale

- HIGH for bug 1: the entire value-prop of cortex ("verification gate before
  edits land") is silently inverted when the gate evaluates the wrong tree.
  The user receives a "PARTIAL_SUCCESS" report claiming 5/6 steps green
  while two key modules were never written and one CLI module is a stub.
- HIGH for bug 2: the cloud path is the recommended escape hatch when local
  models stall. If it 0-steps on a concrete prompt, the escape hatch is
  unusable without prompt-engineering trial-and-error.

## Workaround

Pending fix:
- After every `cortex implement` run, `find src -name '*.rs' | xargs wc -l`
  and compare against the planned files before trusting the report.
- For cloud runs, decompose the task manually into smaller steps and use
  `cortex apply` per step rather than `cortex implement`.

## Test artifacts

- `~/projects/adr-cli/` — local-model partial-success output
- `~/projects/adr-cli-cloud/` — cloud-model 0-step abort
- Session logs preserved at `/tmp/claude-1000/-home-supernovyl/da540a86-4f36-433f-a2a7-dda6d273cbc2/tasks/{bdcfiwaj2,bgggri36m}.output`
