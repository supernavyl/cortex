Build a thread-safe LRU cache in Rust with O(1) get/put, hand-rolled doubly-linked
list (no `LinkedList`), and `parking_lot::Mutex` for the lock.

Implement in `src/lib.rs`:

```rust
pub struct LruCache<K, V> {
    /* private */
}

impl<K, V> LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    /// Create a new cache with the given capacity (must be > 0; panics otherwise).
    pub fn new(capacity: usize) -> Self;

    /// Capacity passed to new().
    pub fn capacity(&self) -> usize;

    /// Current number of entries.
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Get a value and mark it most-recently-used. None if absent.
    pub fn get(&self, key: &K) -> Option<V>;

    /// Insert; evicts the least-recently-used entry if at capacity.
    /// Returns the evicted (key, value) if eviction occurred.
    pub fn put(&self, key: K, value: V) -> Option<(K, V)>;

    /// Remove a key explicitly. Returns the removed value if present.
    pub fn remove(&self, key: &K) -> Option<V>;

    /// Clear all entries.
    pub fn clear(&self);
}
```

Implementation rules:

- `parking_lot::Mutex` wrapping a single inner state struct containing the hash map
  and the linked-list head/tail pointers
- The linked list is hand-rolled — use `Vec<Node>` as an arena with `Option<NodeIdx>`
  prev/next pointers (NodeIdx = usize), or `Box<Node>` with raw pointers if you
  prefer (requires unsafe with `// SAFETY:` comments)
- HashMap maps `K` to `NodeIdx` (or node ptr)
- `Send + Sync` must be derivable for `LruCache<K: Send, V: Send>` — no `Rc`,
  no raw `*mut` without proper bounds

Add `parking_lot = "0.12"` to `[dependencies]` in `Cargo.toml`.

Tests:

- `test_basic_get_put` — put 3 keys, get them back in any order
- `test_eviction_at_capacity` — capacity=2, put A,B,C — A must be evicted; the
  returned `Option<(K,V)>` from put(C) must be `Some(("A", _))`
- `test_get_promotes_to_mru` — capacity=2, put A,B, get(A), put(C) — B is evicted
  (since A was just promoted)
- `test_remove_then_reinsert` — remove(A) returns Some, then put(A) succeeds
- `test_concurrent_inserts` — spawn 8 threads, each inserts 100 keys into a
  cache(capacity=200) — final state has exactly 200 entries (most-recent 200
  of 800 attempted), no panics, no deadlocks. Use `std::sync::Arc` to share.

`cargo check` clean, `cargo test` all pass.
