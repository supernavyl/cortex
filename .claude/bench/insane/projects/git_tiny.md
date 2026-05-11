Build a tiny git-like content-addressed object store project in Rust.

Layout:

```
src/
  main.rs           - CLI: init, hash-object, cat-file, write-tree, commit-tree, log
  lib.rs            - public API
  hash.rs           - SHA-1 wrapper + Hex format
  object.rs         - Object enum (Blob | Tree | Commit) + parse/serialize
  store.rs          - on-disk object store (.tinygit/objects/aa/bbbb...)
  refs.rs           - ref management (.tinygit/refs/heads/main, HEAD)
tests/
  repo.rs           - end-to-end: init repo, store objects, traverse log
```

`Cargo.toml` deps:
- `sha1 = "0.10"`
- `hex = "0.4"`
- `clap = { version = "4", features = ["derive"] }`
- `thiserror = "1"`
- `anyhow = "1"`

(No `git2`, no `gix`.)

Object format (one type per Object variant):

```
Blob:
  "blob <len>\0<content bytes>"   →  SHA-1 → 40-char hex object id
Tree:
  "tree <len>\0<entries>"
  entry = "<mode> <name>\0<20-byte raw hash>" (mode = "100644" for file, "040000" for dir)
Commit:
  "commit <len>\0tree <hex>\nparent <hex>\n...\nauthor <name> <email> <unix-ts>\n\n<message>"
```

On-disk layout:
- `.tinygit/objects/<ab>/<cdef...>` — content stored uncompressed for simplicity
  (real git uses zlib; we skip)
- `.tinygit/refs/heads/main` — file containing the current commit hex
- `.tinygit/HEAD` — file containing `ref: refs/heads/main`

CLI subcommands:
- `tinygit init`
- `tinygit hash-object <file>`         → prints hash
- `tinygit cat-file <hash>`            → prints content
- `tinygit write-tree <dir>`           → builds Tree object recursively, prints root tree hash
- `tinygit commit-tree <tree-hash> [-p <parent>] -m <msg>`  → prints commit hash
- `tinygit log [hash]`                 → walks parents, prints each commit

Public API:

```rust
pub use store::ObjectStore;
pub use object::{Object, ObjectKind, Tree, TreeEntry, Commit};
pub use hash::ObjectId;
```

Tests:
- `test_hash_object_blob_known_value` — blob "hello\n" → expected SHA-1 hex
- `test_store_and_retrieve_blob`
- `test_tree_parse_roundtrip` — build a Tree with 3 entries, serialize, parse, check equal
- `test_commit_parse_roundtrip`
- `test_write_tree_traverses_dir_recursively`
- `test_log_walks_parent_chain` — 3-deep commit chain, log returns all 3
- `test_init_creates_layout` — .tinygit/objects, .tinygit/refs/heads, HEAD all present

`cargo check` clean, `cargo test` all pass.
