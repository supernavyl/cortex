Build a concurrent skip list in Rust with atomic operations and lock-free reads.

Implement in `src/lib.rs`:

```rust
pub struct SkipList<K: Ord + Clone, V: Clone> { /* private */ }

impl<K: Ord + Clone, V: Clone> SkipList<K, V> {
    pub fn new() -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Insert; replaces any existing value for key. Returns prior value if any.
    pub fn insert(&self, key: K, value: V) -> Option<V>;

    /// Lookup; lock-free read path.
    pub fn contains(&self, key: &K) -> bool;
    pub fn get(&self, key: &K) -> Option<V>;

    /// Remove; returns the removed value if present.
    pub fn remove(&self, key: &K) -> Option<V>;
}
```

Implementation rules:

- Levels 0..MAX (16 is fine). Each node has `Vec<AtomicPtr<Node>>` next pointers,
  one per level it participates in
- Reads (contains/get) are lock-free — pure atomic loads
- Writes (insert/remove) may use a per-bucket `parking_lot::Mutex` OR a CAS loop;
  full lock-free is bonus, not required
- Random level for a new node uses geometric distribution (each level p=0.5)
- `unsafe` blocks must have `// SAFETY:` comments justifying the invariant

Add `parking_lot = "0.12"` to `Cargo.toml`.

Tests:

- `test_insert_get` — insert (1, "a"), get(&1)==Some("a")
- `test_insert_replaces` — insert (1, "a"), insert (1, "b") → returns Some("a") and get→"b"
- `test_remove_returns_value` — insert (1, "a"), remove(&1)==Some("a"), then get→None
- `test_ordered_iteration_via_level0` — insert 1, 5, 3, 2, 4; assert get(&i) for i in 1..=5 works
- `test_concurrent_inserts_no_loss` — spawn 8 threads, each insert keys i..i+1000;
  after all threads finish, every inserted key must be present. Use Arc<SkipList>
- `test_concurrent_read_during_write` — one writer thread inserting 10_000 keys,
  4 reader threads continuously calling contains() on random keys; no panic,
  no data race (run with cargo test --release for any speed-sensitive checks)

`cargo check` clean, `cargo test` all pass.
