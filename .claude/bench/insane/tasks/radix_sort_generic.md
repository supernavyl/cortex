Build a generic, stable radix sort in Rust with property tests.

Implement in `src/lib.rs`:

```rust
/// Trait for keys that can be radix-sorted.
/// Implementations must be deterministic and consistent with Ord.
pub trait RadixKey {
    /// Number of bytes in the sort key.
    const KEY_BYTES: usize;
    /// Get the byte at position `pos` (0 = most significant).
    fn key_byte(&self, pos: usize) -> u8;
}

impl RadixKey for u8;
impl RadixKey for u16;
impl RadixKey for u32;
impl RadixKey for u64;
impl RadixKey for i8;   // remember to flip sign bit
impl RadixKey for i16;
impl RadixKey for i32;
impl RadixKey for i64;
impl RadixKey for String;  // KEY_BYTES = 32, pad/truncate

/// Stable radix sort in place. Always succeeds.
pub fn radix_sort<K: RadixKey + Clone>(items: &mut Vec<K>);

/// Stable radix sort by a key function — sort a Vec<T> by a derived RadixKey.
pub fn radix_sort_by_key<T, K: RadixKey + Clone, F: Fn(&T) -> K>(items: &mut Vec<T>, key: F);
```

Implementation rules:

- MSD or LSD — either is fine
- Stability is non-negotiable; verify via property test below
- Signed integers must sort correctly (negatives below positives) — flip the
  sign bit during the byte extraction
- String impl pads short strings with `\0` to KEY_BYTES (truncate longer)

Add `[dev-dependencies] proptest = "1"` for property tests.

Tests:

- `test_empty_input` — radix_sort(&mut vec![]) is a no-op
- `test_single_element` — radix_sort(&mut vec![42u32]) leaves it alone
- `test_u32_sorted` — radix_sort produces same order as `slice::sort()` on 1000
  random u32s
- `test_i32_handles_negatives` — sort [-5, 3, -10, 0, 7], must equal slice::sort result
- `test_string_sort` — sort a Vec<String> of mixed lengths and content
- `test_radix_sort_by_key` — sort Vec<(u32, String)> by the u32; check stability
  (equal keys preserve original order)
- **`prop_matches_stdlib_u32`** — proptest: any Vec<u32> sorted with radix_sort
  equals the same Vec sorted with slice::sort
- **`prop_stable_on_equal_keys`** — proptest: Vec<(u8, u32)> with many equal u8
  keys, radix_sort_by_key on the u8 preserves the relative u32 order

`cargo check` clean, `cargo test` all pass. Sort must be O(n·k) where k = KEY_BYTES.
