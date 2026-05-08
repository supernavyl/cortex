# CORTEX RAG Wiring + Auto-Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the built-but-dead FTS5 RAG search into the Ask handler, and auto-index the daemon's workspace on startup when no watch dirs are configured.

**Architecture:** `search_chunks` (BM25-ranked FTS5) exists in `store.rs` but `build_symbol_context` in `server.rs` never calls it — it only does prefix match on symbol names. We add a `search_chunks` call inside `build_symbol_context`, merge chunk results into the context string, and cap total output. For auto-index: `main.rs` currently logs "context engine idle" when `watch_dirs` is empty; we add a workspace-detection fallback from `std::env::current_dir()` and index that directory before starting the server loop.

**Tech Stack:** Rust 2021, tokio, rusqlite (FTS5), tree-sitter, tracing

---

## File Structure

| File | Responsibility |
|------|---------------|
| `crates/cortex-daemon/src/server.rs` | `build_symbol_context` — adds `search_chunks` call and formats chunk results into context string. Also houses the unit test. |
| `crates/cortex-daemon/src/main.rs` | Daemon startup — adds workspace auto-detection and indexing when `watch_dirs` is empty. |
| `crates/cortex-context/src/store.rs` | Already has `search_chunks` and `ChunkResult` — no changes needed. |
| `crates/cortex-core/src/workspace.rs` | Already has `detect` — no changes needed. |

---

### Task 1: Wire FTS5 `search_chunks` into `build_symbol_context`

**Files:**
- Modify: `crates/cortex-daemon/src/server.rs:621-664`
- Test: `crates/cortex-daemon/src/server.rs` (add `#[cfg(test)]` block at bottom)

- [ ] **Step 1: Write the failing test**

Add a test block at the bottom of `server.rs` (after line 707, inside a new `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use cortex_context::store::SymbolStore;
    use super::build_symbol_context;

    #[test]
    fn test_build_symbol_context_includes_chunk_search() {
        let store = SymbolStore::in_memory().unwrap();

        // Index a fake file with a known function and chunk content
        store
            .upsert_file(
                "src/search.rs",
                1000,
                "hash1",
                cortex_context::symbol::Language::Rust,
                &[
                    cortex_context::symbol::Symbol {
                        file_path: "src/search.rs".to_string(),
                        name: "find_user".to_string(),
                        kind: cortex_context::symbol::SymbolKind::Function,
                        language: cortex_context::symbol::Language::Rust,
                        start_line: 0,
                        end_line: 5,
                        start_col: 0,
                        end_col: 1,
                        parent_name: None,
                        signature: Some("fn find_user(id: u64) -> User".to_string()),
                    },
                ],
            )
            .unwrap();

        // Upsert a code chunk that contains the word "database"
        store
            .upsert_chunks(
                "src/db.rs",
                "fn connect_db() -> Connection {\n    // opens the database\n}",
            )
            .unwrap();

        let symbols = Arc::new(Mutex::new(store));
        let ctx = build_symbol_context(&symbols, "how does the database connection work");

        // Should contain symbol match (find_user is not about database, but db.rs chunk is)
        assert!(
            ctx.contains("db.rs"),
            "chunk search should find db.rs via 'database' keyword; got:\n{ctx}"
        );
        assert!(
            ctx.contains("opens the database"),
            "chunk content should appear in context; got:\n{ctx}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon test_build_symbol_context_includes_chunk_search -- --nocapture
```

Expected: **FAIL** with assertion error — `ctx` does not contain `db.rs` because `build_symbol_context` currently only queries symbols by name, not chunks.

- [ ] **Step 3: Modify `build_symbol_context` to call `search_chunks`**

Replace the body of `build_symbol_context` in `server.rs:621-664` with this merged implementation. Keep the function signature exactly as-is:

```rust
fn build_symbol_context(symbols: &Arc<Mutex<SymbolStore>>, prompt: &str) -> String {
    let store = match symbols.lock() {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    // Extract potential symbol names from the prompt
    let keywords: Vec<&str> = prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3 && !is_common_word(w))
        .collect();

    let mut context_lines = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // ── Symbol name search (prefix match) ───────────────────────────────
    for keyword in &keywords {
        if let Ok(matches) = store.query_by_name(keyword) {
            for sym in matches.iter().take(5) {
                let key = format!("{}:{}", sym.file_path, sym.name);
                if seen.insert(key) {
                    let sig = sym.signature.as_deref().unwrap_or(&sym.name);
                    let parent = sym
                        .parent_name
                        .as_deref()
                        .map(|p| format!(" (in {p})"))
                        .unwrap_or_default();
                    context_lines.push(format!(
                        "{} {} `{}`{} at {}:{}",
                        sym.kind.as_str(),
                        sym.language.as_str(),
                        sig,
                        parent,
                        sym.file_path,
                        sym.start_line + 1,
                    ));
                }
            }
        }
    }

    // ── FTS5 chunk search (semantic/BM25 over code content) ─────────────
    // Build an FTS5 query from the top 5 longest keywords (longer = more specific)
    let mut fts5_keywords: Vec<&str> = keywords.clone();
    fts5_keywords.sort_by_key(|w| w.len());
    fts5_keywords.reverse();
    let fts5_query = fts5_keywords.into_iter().take(5).collect::<Vec<_>>().join(" ");

    if !fts5_query.is_empty() {
        if let Ok(chunks) = store.search_chunks(&fts5_query, 10) {
            for chunk in chunks {
                let key = format!("chunk:{}:{}-{}", chunk.file_path, chunk.start_line, chunk.end_line);
                if seen.insert(key) {
                    // Truncate chunk content to avoid blowing up the prompt
                    let content_preview = if chunk.content.len() > 800 {
                        format!("{}…", &chunk.content[..800])
                    } else {
                        chunk.content.clone()
                    };
                    context_lines.push(format!(
                        "--- {}:{}-{} ---\n{}",
                        chunk.file_path,
                        chunk.start_line,
                        chunk.end_line,
                        content_preview,
                    ));
                }
            }
        }
    }

    // Cap total context lines to avoid blowing up the prompt
    context_lines.truncate(25);
    context_lines.join("\n")
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon test_build_symbol_context_includes_chunk_search -- --nocapture
```

Expected: **PASS**

- [ ] **Step 5: Run all daemon tests to check for regressions**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon
```

Expected: all tests green.

- [ ] **Step 6: Commit**

```bash
cd ~/projects/cortex && git add crates/cortex-daemon/src/server.rs && git commit -m "feat(daemon): wire FTS5 chunk search into Ask context builder

search_chunks() existed in store.rs but was never called.
build_symbol_context now queries both symbol names (prefix match)
and code_chunks (BM25 FTS5) when building prompt context.

- Merges symbol + chunk results with deduplication
- Caps chunk preview at 800 chars and total lines at 25
- Adds unit test verifying chunk content appears in context"
```

---

### Task 2: Auto-index workspace on daemon startup

**Files:**
- Modify: `crates/cortex-daemon/src/main.rs:63-79`
- Test: `crates/cortex-daemon/src/main.rs` (add `#[cfg(test)]` block at bottom)

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod tests` block at the bottom of `main.rs` (after the last line):

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// Helper: simulate the auto-index decision logic.
    /// Returns the directories that would be indexed given empty watch_dirs.
    fn resolve_auto_index_dirs(
        watch_dirs: &[PathBuf],
        cwd: &std::path::Path,
    ) -> Vec<PathBuf> {
        if !watch_dirs.is_empty() {
            return watch_dirs.to_vec();
        }
        if let Some(ws) = cortex_core::workspace::detect(cwd) {
            vec![ws.root]
        } else {
            vec![]
        }
    }

    #[test]
    fn test_auto_index_finds_rust_workspace() {
        let tmp = std::env::temp_dir().join("cortex-auto-index-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();

        let dirs = resolve_auto_index_dirs(&[], &tmp);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], tmp);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auto_index_skips_when_watch_dirs_present() {
        let explicit = vec![PathBuf::from("/some/project")];
        let dirs = resolve_auto_index_dirs(&explicit, std::path::Path::new("/home/user"));
        assert_eq!(dirs, explicit);
    }

    #[test]
    fn test_auto_index_empty_when_no_workspace() {
        let tmp = std::env::temp_dir().join("cortex-no-workspace-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let dirs = resolve_auto_index_dirs(&[], &tmp);
        assert!(dirs.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon test_auto_index_finds_rust_workspace -- --nocapture
```

Expected: **FAIL** — the test function `resolve_auto_index_dirs` does not exist yet in `main.rs` (you are writing it in the next step). Actually, since `resolve_auto_index_dirs` is defined in the test block, it will compile and pass as long as the logic is correct. Wait — the skill says "write the failing test". But if I define the helper in the test block and call it, the test passes immediately because the helper already implements the logic.

Let me reconsider. The TDD approach for this task should be:
1. Write the test with the helper function that calls the *actual* production code
2. The production code doesn't have `resolve_auto_index_dirs` yet, so it won't compile
3. Step 3 is to add the production function

So I should define the test to call a function that will be added to `main.rs` as a regular (non-test) function. Let me restructure:

The test block should import/call a function like `maybe_auto_index_dirs` that we'll add to `main.rs`. The test will fail to compile because the function doesn't exist yet.

Let me revise the plan:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use super::maybe_auto_index_dirs;

    #[test]
    fn test_auto_index_finds_rust_workspace() {
        let tmp = std::env::temp_dir().join("cortex-auto-index-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();

        let dirs = maybe_auto_index_dirs(&[], &tmp);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], tmp);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_auto_index_skips_when_watch_dirs_present() {
        let explicit = vec![PathBuf::from("/some/project")];
        let dirs = maybe_auto_index_dirs(&explicit, std::path::Path::new("/home/user"));
        assert_eq!(dirs, explicit);
    }

    #[test]
    fn test_auto_index_empty_when_no_workspace() {
        let tmp = std::env::temp_dir().join("cortex-no-workspace-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let dirs = maybe_auto_index_dirs(&[], &tmp);
        assert!(dirs.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

This won't compile because `maybe_auto_index_dirs` doesn't exist yet. Good, that's the failing test.

- [ ] **Step 3: Add `maybe_auto_index_dirs` and wire it into startup**

Add this function to `main.rs`, right before `#[tokio::main]` (i.e., after the `use` statements and before `main`):

```rust
/// Resolve directories to index.
///
/// If `watch_dirs` is non-empty, returns them verbatim.
/// Otherwise, attempts workspace detection from `cwd` and returns
/// the workspace root if found.
pub fn maybe_auto_index_dirs(
    watch_dirs: &[std::path::PathBuf],
    cwd: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    if !watch_dirs.is_empty() {
        return watch_dirs.to_vec();
    }
    if let Some(ws) = cortex_core::workspace::detect(cwd) {
        vec![ws.root]
    } else {
        vec![]
    }
}
```

Then replace lines 63-79 in `main.rs`:

**OLD:**
```rust
    // Index configured directories
    if !config.context.watch_dirs.is_empty() {
        let stats = cortex_context::indexer::index_directories(
            &symbol_store,
            &config.context.watch_dirs,
            &config.context.extensions,
            config.context.max_file_size,
        )?;
        tracing::info!(
            files = stats.files_indexed,
            symbols = stats.symbols_total,
            elapsed_ms = stats.elapsed_ms,
            "initial indexing complete"
        );
    } else {
        tracing::info!("no watch directories configured — context engine idle");
    }
```

**NEW:**
```rust
    // Resolve directories to index (explicit config or auto-detected workspace)
    let dirs_to_index = maybe_auto_index_dirs(
        &config.context.watch_dirs,
        &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
    );

    if !dirs_to_index.is_empty() {
        let stats = cortex_context::indexer::index_directories(
            &symbol_store,
            &dirs_to_index,
            &config.context.extensions,
            config.context.max_file_size,
        )?;
        tracing::info!(
            files = stats.files_indexed,
            symbols = stats.symbols_total,
            elapsed_ms = stats.elapsed_ms,
            "initial indexing complete"
        );
    } else {
        tracing::info!("no watch directories configured and no workspace detected — context engine idle");
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon test_auto_index -- --nocapture
```

Expected: all three auto-index tests **PASS**.

- [ ] **Step 5: Run full daemon test suite**

```bash
cd ~/projects/cortex && cargo test -p cortex-daemon
```

Expected: all tests green.

- [ ] **Step 6: Verify clean build**

```bash
cd ~/projects/cortex && cargo check -p cortex-daemon
```

Expected: no warnings or errors.

- [ ] **Step 7: Commit**

```bash
cd ~/projects/cortex && git add crates/cortex-daemon/src/main.rs && git commit -m "feat(daemon): auto-index workspace on startup when watch_dirs is empty

Previously the daemon logged 'context engine idle' whenever
watch_dirs was empty. Now it detects the workspace from the
daemon's current working directory and indexes it automatically.

- Adds maybe_auto_index_dirs helper with workspace fallback
- Updates startup indexing logic to use resolved dirs
- Adds 3 unit tests for watch_dir / auto-detect / no-workspace cases"
```

---

## Self-Review

**1. Spec coverage:**
- Wire FTS5 into Ask handler → Task 1, steps 1-6
- Auto-index workspace on startup → Task 2, steps 1-7

**2. Placeholder scan:**
- No "TBD", "TODO", "implement later"
- No vague "add error handling" — exact code provided
- No "Similar to Task N" — each task is self-contained
- Every step has exact file paths, exact commands, expected output

**3. Type consistency:**
- `build_symbol_context` signature unchanged
- `SymbolStore`, `ChunkResult`, `Language`, `SymbolKind` used consistently with existing crate exports
- `maybe_auto_index_dirs` return type `Vec<PathBuf>` matches `index_directories` parameter type
- `cortex_core::workspace::detect` already returns `Option<Workspace>` with `root: PathBuf`

No issues found. Plan is complete.
