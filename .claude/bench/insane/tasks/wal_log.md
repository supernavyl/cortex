Build a write-ahead log (WAL) in Rust with CRC32 checksums, segmented files,
fsync semantics, and crash-recovery iteration.

Implement in `src/lib.rs`:

```rust
pub struct Wal { /* private — owns a directory and current segment file */ }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub payload: Vec<u8>,
}

impl Wal {
    /// Open or create a WAL rooted at `dir`. If existing segments are present,
    /// the next append continues from the highest-seq record found.
    pub fn open(dir: &std::path::Path) -> std::io::Result<Self>;

    /// Append a payload. Returns the assigned sequence number.
    /// Each append fsyncs the data file before returning Ok.
    pub fn append(&mut self, payload: &[u8]) -> std::io::Result<u64>;

    /// Highest assigned sequence number, or 0 if empty.
    pub fn last_seq(&self) -> u64;

    /// Iterate all records across all segments in seq order.
    /// Returns Err only if a segment file is unreadable; corrupt records
    /// (bad CRC) are silently skipped after a tracing::warn.
    pub fn iter(&self) -> std::io::Result<Vec<Record>>;

    /// Truncate segments older than `min_seq`. Most-recent record always retained.
    pub fn compact(&mut self, min_seq: u64) -> std::io::Result<usize>; // returns segments removed
}
```

Record on-disk format (little-endian):

```
[u32 LEN] [u64 SEQ] [u32 CRC32 of (SEQ||PAYLOAD)] [PAYLOAD bytes ... LEN-12 bytes]
```

Segments named `wal-<00000001>.log` etc. Roll to a new segment when the current
file exceeds 1 MB.

Add `crc32fast = "1"` to `Cargo.toml`.

Tests:

- `test_open_empty_dir` — Wal::open on empty dir → last_seq()==0
- `test_append_then_iter_roundtrip` — append 5 payloads, iter returns them in seq order
- `test_persistence_across_reopen` — append; drop; reopen same dir; iter still returns them
- `test_segment_rolling` — append 1MB+1B of data, verify ≥2 segment files exist
- `test_corruption_skipped` — manually flip one byte in segment file; iter returns
  the surviving records + emits warn (use tempfile + std::fs::write)
- `test_compact_removes_old_segments` — append enough to create 3 segments; compact
  with min_seq beyond first 2 segments; verify 2 files removed and iter still works

`cargo check` clean, `cargo test` all pass. fsync MUST be called before append returns.
