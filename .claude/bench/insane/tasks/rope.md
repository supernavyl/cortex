Build a Rope data structure in Rust for efficient large-string editing.

Implement in `src/lib.rs`:

```rust
pub struct Rope { /* private */ }

impl Rope {
    pub fn new() -> Self;
    pub fn from_str(s: &str) -> Self;
    pub fn len(&self) -> usize;   // character count
    pub fn is_empty(&self) -> bool;
    pub fn insert(&mut self, idx: usize, s: &str);
    pub fn delete(&mut self, start: usize, end: usize);
    pub fn char_at(&self, idx: usize) -> Option<char>;
    pub fn to_string(&self) -> String;
}
```

The internal representation must be a tree of leaf nodes containing string slices,
with internal nodes tracking left-subtree weights for O(log n) lookup. Do NOT use
a flat `String` or `Vec<char>` — that defeats the structural purpose. A small
constant chunk size (e.g. 64–128 chars per leaf) is fine.

Add unit tests in the `#[cfg(test)] mod tests` block:

- `test_empty_rope` — new() then len()==0, char_at(0)==None
- `test_from_str_roundtrip` — Rope::from_str(s).to_string() == s for short and long strings
- `test_insert_at_boundary` — insert at index 0 and at len()
- `test_insert_in_middle` — insert "XYZ" at index 5 of "0123456789", result is "01234XYZ56789"
- `test_delete_range` — delete chars [3..7) from "abcdefghij", result is "abchij"
- `test_char_at_out_of_bounds_returns_none`
- `test_large_rope_10000_chars` — build a 10k-char rope from "abc" × 3333, verify len + char_at(5000)

The crate must compile clean (`cargo check`) and all tests must pass (`cargo test`).
No `unwrap()` or `expect()` in the public API. Use `?` propagation where applicable.
