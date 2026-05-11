Build a persistent (immutable) B-tree in Rust with structural sharing.

Implement in `src/lib.rs`:

```rust
pub struct PersistentBTree<K, V> { /* private */ }

impl<K: Ord + Clone, V: Clone> PersistentBTree<K, V> {
    pub fn new() -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Returns a NEW tree with the key inserted; original unchanged.
    pub fn insert(&self, key: K, value: V) -> Self;

    /// Returns a NEW tree with the key removed; original unchanged.
    pub fn remove(&self, key: &K) -> Self;

    /// Lookup by reference.
    pub fn get(&self, key: &K) -> Option<&V>;

    /// In-order iteration over (key, value) pairs.
    pub fn iter(&self) -> Box<dyn Iterator<Item = (&K, &V)> + '_>;
}

impl<K, V> Clone for PersistentBTree<K, V>;  // O(1) — shares Arc with original
```

Implementation rules:

- Use `Arc<Node>` for child pointers — structural sharing is the whole point
- Branching factor: pick a constant T such that nodes hold `T..=2*T-1` keys (B-tree
  invariant); T=4 or T=8 is fine
- `insert` and `remove` rebuild only the path from root to the affected node;
  unrelated subtrees keep their original `Arc<Node>` (verifies via Arc::strong_count)
- Original tree is observably unmodified after a `.insert()` call

Tests:

- `test_empty` — new tree has len()==0, get() returns None
- `test_single_insert` — insert(1, "one"), get(&1) == Some(&"one")
- `test_immutability_after_insert` — tree t1 = empty().insert(1,"a"); t2 = t1.insert(2,"b");
  assert t1.get(&2)==None and t1.len()==1
- `test_inorder_iter` — insert 10 random-order keys, iter() returns them sorted
- `test_remove_present` — insert then remove returns shorter tree; old tree intact
- `test_remove_absent` — remove of a non-existent key returns equivalent tree
- `test_structural_sharing` — insert into a tree of 1000 nodes; assert the new tree
  shares > 50% of nodes with the original (use Arc::strong_count or count distinct
  Arc pointers reachable from each)
- `test_large_1000_keys` — insert 1000 sequential keys, verify all present + len

`cargo check` clean, `cargo test` all pass. No `unwrap()` in lib code.
