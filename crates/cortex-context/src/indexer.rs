//! Directory walker and incremental indexer.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::parser;
use crate::store::SymbolStore;
use crate::symbol::Language;

/// Stats from an indexing run.
#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_scanned: u64,
    pub files_indexed: u64,
    pub files_skipped: u64,
    pub files_errored: u64,
    pub symbols_total: u64,
    pub elapsed_ms: u64,
}

/// Index all supported files in the given directories.
pub fn index_directories(
    store: &SymbolStore,
    directories: &[PathBuf],
    extensions: &[String],
    max_file_size: u64,
) -> Result<IndexStats> {
    let start = Instant::now();
    let mut stats = IndexStats::default();

    for dir in directories {
        if !dir.exists() {
            tracing::warn!(path = %dir.display(), "watch directory does not exist, skipping");
            continue;
        }
        index_directory(store, dir, extensions, max_file_size, &mut stats)?;
    }

    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    stats.symbols_total = store.symbol_count().unwrap_or(0);

    tracing::info!(
        files_scanned = stats.files_scanned,
        files_indexed = stats.files_indexed,
        files_skipped = stats.files_skipped,
        files_errored = stats.files_errored,
        symbols = stats.symbols_total,
        elapsed_ms = stats.elapsed_ms,
        "indexing complete"
    );

    Ok(stats)
}

fn index_directory(
    store: &SymbolStore,
    dir: &Path,
    extensions: &[String],
    max_file_size: u64,
    stats: &mut IndexStats,
) -> Result<()> {
    let walker = walkdir(dir, extensions, max_file_size);

    for entry in walker {
        stats.files_scanned += 1;

        let (path, language) = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(error = %e, "skipping file");
                stats.files_errored += 1;
                continue;
            }
        };

        if let Err(e) = index_file(store, &path, language, stats) {
            tracing::debug!(path = %path.display(), error = %e, "failed to index file");
            stats.files_errored += 1;
        }
    }

    Ok(())
}

/// Index a specific list of files (for incremental updates).
///
/// Files that no longer exist are removed from the store.
/// Files with unchanged content hashes are skipped.
pub fn index_files(
    store: &SymbolStore,
    files: &[PathBuf],
    max_file_size: u64,
) -> Result<IndexStats> {
    let start = Instant::now();
    let mut stats = IndexStats::default();

    for path in files {
        stats.files_scanned += 1;

        if !path.exists() {
            // File was deleted — remove from store
            let path_str = path.to_string_lossy();
            if let Err(e) = store.remove_file(&path_str) {
                tracing::debug!(path = %path_str, error = %e, "failed to remove deleted file");
            }
            continue;
        }

        // Check file size
        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.len() > max_file_size {
                stats.files_skipped += 1;
                continue;
            }
        }

        // Detect language from extension
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_string(),
            None => {
                stats.files_skipped += 1;
                continue;
            }
        };
        let language = match Language::from_extension(&ext) {
            Some(l) => l,
            None => {
                stats.files_skipped += 1;
                continue;
            }
        };

        if let Err(e) = index_file(store, path, language, &mut stats) {
            tracing::debug!(path = %path.display(), error = %e, "failed to index file");
            stats.files_errored += 1;
        }
    }

    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    stats.symbols_total = store.symbol_count().unwrap_or(0);
    Ok(stats)
}

fn index_file(
    store: &SymbolStore,
    path: &Path,
    language: Language,
    stats: &mut IndexStats,
) -> Result<()> {
    let content = std::fs::read(path).context("failed to read file")?;
    let content_hash = hex_sha256(&content);

    let path_str = path.to_string_lossy();

    // Skip if unchanged
    if !store.file_needs_update(&path_str, &content_hash)? {
        stats.files_skipped += 1;
        return Ok(());
    }

    let symbols = parser::extract_symbols(&content, &path_str, language)?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    tracing::debug!(
        path = %path_str,
        symbols = symbols.len(),
        "indexed file"
    );

    store.upsert_file(&path_str, mtime, &content_hash, language, &symbols)?;

    // Store content chunks in FTS5 for semantic search — only for valid UTF-8
    if let Ok(text) = std::str::from_utf8(&content) {
        if let Err(e) = store.upsert_chunks(&path_str, text) {
            tracing::debug!(path = %path_str, error = %e, "failed to index chunks");
        }
    }

    stats.files_indexed += 1;

    Ok(())
}

/// Walk a directory tree, yielding paths with their detected language.
fn walkdir(
    root: &Path,
    extensions: &[String],
    max_file_size: u64,
) -> Vec<Result<(PathBuf, Language)>> {
    let mut results = Vec::new();

    let walker = match std::fs::read_dir(root) {
        Ok(w) => w,
        Err(e) => {
            results.push(Err(e.into()));
            return results;
        }
    };

    let mut dirs_to_visit = vec![root.to_path_buf()];

    while let Some(dir) = dirs_to_visit.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Skip hidden directories and common noise
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "__pycache__"
                    || name == ".git"
                    || name == "venv"
                    || name == ".venv"
                {
                    continue;
                }
            }

            if path.is_dir() {
                dirs_to_visit.push(path);
                continue;
            }

            // Check extension
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_string(),
                None => continue,
            };

            if !extensions.iter().any(|allowed| allowed == &ext) {
                continue;
            }

            // Check language support
            let language = match Language::from_extension(&ext) {
                Some(l) => l,
                None => continue,
            };

            // Check file size
            if let Ok(metadata) = std::fs::metadata(&path) {
                if metadata.len() > max_file_size {
                    continue;
                }
            }

            results.push(Ok((path, language)));
        }
    }

    // Drop the unused initial walker
    drop(walker);

    results
}

fn hex_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("{hash:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_index_temp_directory() {
        let tmp = std::env::temp_dir().join("cortex-test-index");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write a Rust file
        let mut f = std::fs::File::create(tmp.join("test.rs")).unwrap();
        writeln!(f, "fn hello() {{}}").unwrap();
        writeln!(f, "struct Config {{}}").unwrap();

        // Write a Python file
        let mut f = std::fs::File::create(tmp.join("test.py")).unwrap();
        writeln!(f, "def greet():").unwrap();
        writeln!(f, "    pass").unwrap();

        let store = SymbolStore::in_memory().unwrap();
        let extensions: Vec<String> = vec!["rs", "py", "ts"]
            .into_iter()
            .map(String::from)
            .collect();

        let stats = index_directories(&store, &[tmp.clone()], &extensions, 512 * 1024).unwrap();

        assert_eq!(stats.files_indexed, 2);
        assert!(stats.symbols_total >= 3); // hello, Config, greet

        // Re-index should skip unchanged files
        let stats2 = index_directories(&store, &[tmp.clone()], &extensions, 512 * 1024).unwrap();
        assert_eq!(stats2.files_skipped, 2);
        assert_eq!(stats2.files_indexed, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
