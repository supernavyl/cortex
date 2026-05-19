Build a thread-safe LRU cache with TTL support in Python (stdlib only).

## Requirements

Implement `lru_cache.py` at the project root:

```python
from typing import TypeVar, Generic, Optional

K = TypeVar("K")
V = TypeVar("V")

class LRUCache(Generic[K, V]):
    def __init__(self, capacity: int, ttl: float | None = None) -> None:
        """capacity = max entries; ttl = seconds before entry expires (None = no expiry)."""
    
    def get(self, key: K) -> V | None:
        """Return value or None if missing/expired. Moves hit to MRU position."""
    
    def put(self, key: K, value: V) -> None:
        """Insert or update. Evicts LRU entry if at capacity."""
    
    def delete(self, key: K) -> bool:
        """Remove key. Return True if it existed."""
    
    def clear(self) -> None:
        """Remove all entries."""
    
    @property
    def stats(self) -> dict[str, int]:
        """Return {"hits": N, "misses": N, "evictions": N, "expirations": N}."""
    
    def __len__(self) -> int:
        """Number of currently valid (non-expired) entries."""
```

Must use `collections.OrderedDict` for O(1) operations. Thread-safe via `threading.RLock`.

## Tests

Write `tests/test_lru_cache.py` with pytest tests covering:

1. Basic get/put: inserted value is retrievable
2. Capacity eviction: inserting beyond capacity evicts LRU entry
3. Access order: getting an entry moves it to MRU, so a different entry becomes LRU
4. TTL expiry: entry expires after TTL and returns None
5. TTL no-expiry: entry with ttl=None never expires
6. Delete: removes entry, subsequent get returns None
7. Stats: hits, misses, evictions, expirations are counted correctly
8. `len()`: reflects only non-expired entries
9. Thread safety: 50 concurrent threads put/get on 10 keys, no exceptions, stats consistent
10. Clear: empties cache, len() = 0

Keep total test time under 2 seconds.
Write no other files. All imports must be stdlib only.
