Build an ISO9660 read-only filesystem parser project in Rust.

Layout:

```
src/
  main.rs           - CLI: iso9660 ls <iso> <path>, iso9660 cat <iso> <path>
  lib.rs            - public API
  pvd.rs            - Primary Volume Descriptor parser
  directory.rs      - Directory Record parser + tree builder
  path_table.rs     - Path table parser
  read.rs           - File read primitive (handles sector alignment)
error.rs            - error types
tests/
  fixtures.rs       - integration tests using a small embedded fixture .iso
fixtures/
  test.iso          - tiny pre-built ISO with known contents (committed to repo
                      via include_bytes! or written at test setup using mkisofs
                      if available)
```

`Cargo.toml` deps:
- `clap = { version = "4", features = ["derive"] }`
- `thiserror = "1"`

(No external ISO parser dep. Pure Rust.)

ISO9660 basics:
- Sector size = 2048 bytes
- Volume descriptors start at sector 16
- Primary Volume Descriptor (PVD): magic "CD001", logical block size, root directory record
- Each Directory Record: length, extent (start sector), data length, flags,
  filename length, filename (suffixed `;1`), padding to even length

Public API:

```rust
pub struct Iso9660 { /* owns a Read+Seek source */ }

impl Iso9660 {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, IsoError>;

    /// List entries in a directory. Path is `/` or `/sub/`.
    pub fn list(&mut self, dir: &str) -> Result<Vec<Entry>, IsoError>;

    /// Read a file's full content.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, IsoError>;

    /// Get the volume label from the PVD.
    pub fn volume_label(&self) -> &str;
}

pub struct Entry {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
}
```

CLI:
- `iso9660 info <iso>`         — print volume label + total entries
- `iso9660 ls <iso> [path]`    — list entries (path defaults to /)
- `iso9660 cat <iso> <path>`   — print file content to stdout

Tests (use the fixture iso — generate at test runtime if mkisofs is on PATH, else
skip with `#[cfg_attr(not(feature = "fixture"), ignore)]`):
- `test_open_invalid_iso_returns_error` — pass a tmpfile of garbage bytes
- `test_pvd_parses_magic` — write a hand-crafted 2048×17-byte buffer with valid
  PVD bytes; assert parser succeeds and returns expected volume label
- `test_list_root_returns_expected_entries` — fixture
- `test_read_file_returns_known_content` — fixture
- `test_path_with_subdirs` — list /subdir, read /subdir/file.txt

`cargo check` clean, `cargo test` all pass.
