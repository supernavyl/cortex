Build a Bloom filter in Rust with configurable error rate, two hash families,
and disk save/load.

Implement in `src/lib.rs`:

```rust
pub struct BloomFilter {
    bits: Vec<u64>,
    num_bits: usize,
    num_hashes: usize,
    item_count: usize,
}

impl BloomFilter {
    /// Construct sized for `expected_items` with desired `false_positive_rate`.
    /// false_positive_rate must be in (0.0, 1.0); panics outside.
    pub fn new(expected_items: usize, false_positive_rate: f64) -> Self;

    /// Insert a hashable item.
    pub fn insert<T: std::hash::Hash>(&mut self, item: &T);

    /// Query — may have false positives, never false negatives.
    pub fn contains<T: std::hash::Hash>(&self, item: &T) -> bool;

    /// Number of items inserted (not unique — total insert() calls).
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Estimated false-positive rate at current load.
    pub fn estimated_fpr(&self) -> f64;

    /// Serialize to bytes for on-disk persistence.
    pub fn to_bytes(&self) -> Vec<u8>;

    /// Reconstruct from bytes. Returns Err on truncated/corrupt input.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, std::io::Error>;
}
```

Use **two independent hash families** to derive the `num_hashes` indices via the
Kirsch-Mitzenmacher double-hashing trick: `h_i(x) = h1(x) + i * h2(x)`. Use Rust's
built-in `DefaultHasher` for `h1` and a different-seed `DefaultHasher` (or FNV-1a)
for `h2`. The number of hashes should be chosen to minimize FPR for the given
size — `k = (m / n) * ln(2)`.

Bit storage uses `Vec<u64>` (not `Vec<bool>` — wastes 8x memory).

Add tests:

- `test_insert_then_contains_positive` — insert 100 strings, all return true on contains
- `test_negative_rate_under_target` — insert 1000 items into a filter sized for 1000
  at fpr=0.01; check 10_000 unseen items have FPR ≤ 0.025 (allow 2.5x slop)
- `test_empty_filter_no_false_positives` — fresh filter returns false for any query
- `test_serialize_roundtrip` — to_bytes then from_bytes yields identical contains() behaviour
- `test_from_bytes_rejects_short_input` — Err on truncated input

`cargo check` clean, `cargo test` all pass.
