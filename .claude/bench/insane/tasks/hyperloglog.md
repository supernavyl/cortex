Build a HyperLogLog++ cardinality estimator in Rust.

Implement in `src/lib.rs`:

```rust
pub struct HyperLogLog {
    registers: Vec<u8>,
    precision: u8,  // typically 14 → m = 16384 registers
    m: usize,
}

impl HyperLogLog {
    /// Create with given precision (4..=16). Panics outside that range.
    pub fn new(precision: u8) -> Self;

    /// Insert any hashable item.
    pub fn insert<T: std::hash::Hash>(&mut self, item: &T);

    /// Cardinality estimate using HyperLogLog++ algorithm:
    ///   - Raw estimate via harmonic mean of 2^M[i] values
    ///   - Linear counting for small cardinalities
    ///   - Bias correction (RFC linear regression OR Heule++ table — Heule++ preferred)
    pub fn estimate(&self) -> u64;

    /// Merge another HLL (must have same precision) into self. Panics on mismatch.
    pub fn merge(&mut self, other: &Self);

    /// Serialize.
    pub fn to_bytes(&self) -> Vec<u8>;
    /// Deserialize. Err on truncated or mismatched precision.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, std::io::Error>;
}
```

Implementation rules:

- Each register stores the maximum leading-zero count for its bucket
- Use SipHash (from std) or FNV-1a — must be a uniform hash; reject the trivial
  default-hasher-with-no-salt approach if the model picks it for simplicity
- Bucket index = top `precision` bits of the hash; leading-zero count = ctz(rest)+1
- `estimate()` must apply small-range linear-counting correction when ≤2.5×m

Tests:

- `test_empty_estimate_is_zero` — fresh HLL → estimate()==0
- `test_small_set_exact_ish` — insert 100 unique strings, estimate within ±5
- `test_large_set_within_2_percent` — insert 100_000 unique strings (e.g.,
  format!("item-{}", i) for i in 0..100_000), estimate within ±2%
- `test_duplicates_dont_inflate` — insert "x" 10_000 times → estimate ≈ 1
- `test_merge_yields_union_cardinality` — two HLLs each with 50k distinct items;
  merged HLL estimates ≈ 100k (within ±2%)
- `test_serialize_roundtrip` — to_bytes → from_bytes → same estimate
- `test_from_bytes_rejects_mismatched_precision`

`cargo check` clean, `cargo test` all pass. Standard error should be ~1.04/sqrt(m) ≈ 0.81% for precision=14.
