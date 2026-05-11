//! Kairos — proactive observe→decide→act→reflect cycle.
//!
//! Runs as a background task in the daemon. Watches workspace files for changes,
//! debounces them, and triggers incremental re-indexing. Tracks git status and
//! maintains telemetry on file change frequency.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use cortex_context::store::SymbolStore;
use cortex_core::config::Config;

/// Debounce window — wait this long after last change before re-indexing.
const DEBOUNCE_MS: u64 = 800;

/// How often to check git status (seconds).
const GIT_POLL_INTERVAL_SECS: u64 = 30;

/// How often to clean up stale indexed files (seconds).
const STALE_CLEANUP_INTERVAL_SECS: u64 = 300;

/// Shared state that the daemon can query for Kairos telemetry.
#[derive(Debug, Default)]
pub struct KairosState {
    /// Number of incremental re-indexes performed.
    pub reindex_count: u64,
    /// Total files re-indexed across all cycles.
    pub files_reindexed: u64,
    /// Files deleted from index (no longer on disk).
    pub files_cleaned: u64,
    /// Current git branch (if in a git repo).
    pub git_branch: Option<String>,
    /// Git dirty file count.
    pub git_dirty_count: u32,
    /// File change frequency: path → change count since daemon start.
    pub hot_files: HashMap<String, u32>,
    /// Last re-index timestamp.
    pub last_reindex: Option<Instant>,
}

/// The Kairos engine — spawns filesystem watchers and background tasks.
pub struct KairosEngine {
    /// Kept alive to maintain the filesystem watch.
    _watcher: Option<RecommendedWatcher>,
}

impl KairosEngine {
    /// Start the Kairos engine as background tasks.
    ///
    /// Returns a handle to the shared state for querying from request handlers.
    pub fn start(config: Config, symbols: Arc<Mutex<SymbolStore>>) -> Arc<Mutex<KairosState>> {
        let state = Arc::new(Mutex::new(KairosState::default()));

        if config.context.watch_dirs.is_empty() {
            tracing::info!("kairos: no watch directories, file watcher disabled");
            return state;
        }

        let (fs_tx, fs_rx) = mpsc::unbounded_channel::<PathBuf>();

        // Set up the filesystem watcher
        let tx_clone = fs_tx.clone();
        let watcher = Self::create_watcher(tx_clone, &config.context.watch_dirs);

        if let Some(ref _w) = watcher {
            tracing::info!(
                dirs = ?config.context.watch_dirs,
                "kairos: file watcher started"
            );
        }

        // Spawn the debounced re-index task
        let state_clone = Arc::clone(&state);
        let symbols_clone = Arc::clone(&symbols);
        let max_file_size = config.context.max_file_size;
        tokio::spawn(async move {
            debounce_and_reindex(fs_rx, symbols_clone, state_clone, max_file_size).await;
        });

        // Spawn the periodic git status check
        let watch_dirs = config.context.watch_dirs.clone();
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            git_status_loop(state_clone, &watch_dirs).await;
        });

        // Spawn the stale file cleanup task
        let state_clone = Arc::clone(&state);
        let symbols_clone = Arc::clone(&symbols);
        tokio::spawn(async move {
            stale_cleanup_loop(symbols_clone, state_clone).await;
        });

        // Keep the watcher alive (it drops when engine drops, killing the watch)
        let _engine = KairosEngine { _watcher: watcher };
        // Leak intentionally — engine lives for daemon lifetime
        std::mem::forget(_engine);

        state
    }

    fn create_watcher(
        tx: mpsc::UnboundedSender<PathBuf>,
        watch_dirs: &[PathBuf],
    ) -> Option<RecommendedWatcher> {
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            for path in event.paths {
                                // Skip hidden files and common noise
                                let path_str = path.to_string_lossy();
                                if path_str.contains("/.")
                                    || path_str.contains("/target/")
                                    || path_str.contains("/node_modules/")
                                    || path_str.contains("/__pycache__/")
                                {
                                    continue;
                                }
                                let _ = tx.send(path);
                            }
                        }
                        _ => {}
                    }
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(error = %e, "kairos: failed to create file watcher");
                    return None;
                }
            };

        for dir in watch_dirs {
            if dir.exists()
                && let Err(e) = watcher.watch(dir, RecursiveMode::Recursive)
            {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "kairos: failed to watch directory"
                );
            }
        }

        Some(watcher)
    }
}

/// Collect file change events, debounce, then re-index in batch.
async fn debounce_and_reindex(
    mut rx: mpsc::UnboundedReceiver<PathBuf>,
    symbols: Arc<Mutex<SymbolStore>>,
    state: Arc<Mutex<KairosState>>,
    max_file_size: u64,
) {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();

    loop {
        // Wait for at least one event or check for debounce expiry
        tokio::select! {
            Some(path) = rx.recv() => {
                pending.insert(path, Instant::now());
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }

        // Drain any additional queued events
        while let Ok(path) = rx.try_recv() {
            pending.insert(path, Instant::now());
        }

        // Find paths that have been quiet for the debounce window
        let now = Instant::now();
        let debounce = Duration::from_millis(DEBOUNCE_MS);

        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, last_change)| now.duration_since(**last_change) >= debounce)
            .map(|(path, _)| path.clone())
            .collect();

        if ready.is_empty() {
            continue;
        }

        // Remove ready paths from pending
        for path in &ready {
            pending.remove(path);
        }

        // Update hot file tracking
        {
            let mut st = state.lock().unwrap();
            for path in &ready {
                let key = path.to_string_lossy().to_string();
                *st.hot_files.entry(key).or_insert(0) += 1;
            }
        }

        // Perform incremental re-index
        let store = symbols.lock().unwrap();
        match cortex_context::indexer::index_files(&store, &ready, max_file_size) {
            Ok(stats) => {
                if stats.files_indexed > 0 || stats.files_errored > 0 {
                    tracing::info!(
                        indexed = stats.files_indexed,
                        skipped = stats.files_skipped,
                        errors = stats.files_errored,
                        elapsed_ms = stats.elapsed_ms,
                        "kairos: incremental re-index"
                    );
                }
                let mut st = state.lock().unwrap();
                st.reindex_count += 1;
                st.files_reindexed += stats.files_indexed;
                st.last_reindex = Some(Instant::now());
            }
            Err(e) => {
                tracing::warn!(error = %e, "kairos: re-index failed");
            }
        }
    }
}

/// Periodically check git status in watch directories.
async fn git_status_loop(state: Arc<Mutex<KairosState>>, watch_dirs: &[PathBuf]) {
    // Find the first watch dir that's inside a git repo
    let git_dir = watch_dirs.iter().find_map(|d| {
        let mut dir = d.clone();
        loop {
            if dir.join(".git").exists() {
                return Some(dir);
            }
            if !dir.pop() {
                return None;
            }
        }
    });

    let git_dir = match git_dir {
        Some(d) => d,
        None => {
            tracing::debug!("kairos: no git repo found in watch dirs, git tracking disabled");
            return;
        }
    };

    loop {
        tokio::time::sleep(Duration::from_secs(GIT_POLL_INTERVAL_SECS)).await;

        // Get current branch
        let branch = tokio::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&git_dir)
            .output()
            .await
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        // Count dirty files
        let dirty_count = tokio::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&git_dir)
            .output()
            .await
            .ok()
            .map(|o| {
                if o.status.success() {
                    String::from_utf8_lossy(&o.stdout).lines().count() as u32
                } else {
                    0
                }
            })
            .unwrap_or(0);

        let mut st = state.lock().unwrap();
        let branch_changed = st.git_branch.as_deref() != branch.as_deref();
        st.git_branch = branch;
        st.git_dirty_count = dirty_count;

        if branch_changed && let Some(ref b) = st.git_branch {
            tracing::info!(branch = %b, dirty = dirty_count, "kairos: git branch changed");
        }
    }
}

/// Periodically clean up stale indexed files (files that no longer exist on disk).
async fn stale_cleanup_loop(symbols: Arc<Mutex<SymbolStore>>, state: Arc<Mutex<KairosState>>) {
    loop {
        tokio::time::sleep(Duration::from_secs(STALE_CLEANUP_INTERVAL_SECS)).await;

        let indexed_files = {
            let store = symbols.lock().unwrap();
            store.indexed_files().unwrap_or_default()
        };

        let mut removed = 0u64;
        for file_path in &indexed_files {
            let path = std::path::Path::new(file_path);
            if !path.exists() {
                let store = symbols.lock().unwrap();
                if store.remove_file(file_path).is_ok() {
                    removed += 1;
                    tracing::debug!(path = %file_path, "kairos: removed stale file from index");
                }
            }
        }

        if removed > 0 {
            tracing::info!(count = removed, "kairos: cleaned stale files from index");
            let mut st = state.lock().unwrap();
            st.files_cleaned += removed;
        }
    }
}
