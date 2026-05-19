Build a thread-safe token bucket rate limiter in Python (stdlib only, no third-party packages).

## Requirements

Implement `rate_limiter.py` at the project root with:

```python
class TokenBucket:
    def __init__(self, capacity: float, refill_rate: float) -> None:
        """capacity = max tokens; refill_rate = tokens per second."""
    
    def acquire(self, tokens: float = 1.0, timeout: float | None = None) -> bool:
        """Block until tokens available (or timeout). Return True if acquired."""
    
    def try_acquire(self, tokens: float = 1.0) -> bool:
        """Non-blocking. Return True if tokens were available and consumed."""
    
    @property
    def available(self) -> float:
        """Current available token count (lazily refilled)."""

class SlidingWindowRateLimiter:
    def __init__(self, limit: int, window_seconds: float) -> None:
        """Allow at most `limit` requests per rolling `window_seconds`."""
    
    def is_allowed(self, key: str) -> bool:
        """Return True if request for `key` is within rate limit."""
    
    def reset(self, key: str) -> None:
        """Clear rate limit state for a key."""
```

Both classes must be thread-safe (tested with concurrent.futures.ThreadPoolExecutor).

## Tests

Write `tests/test_rate_limiter.py` with pytest tests covering:

1. `TokenBucket.try_acquire` returns True when tokens available, False when empty
2. `TokenBucket.acquire` blocks and returns True after refill (use small capacity + fast refill, e.g. 1 token/0.05s)
3. `TokenBucket.acquire` returns False on timeout when bucket stays empty
4. Concurrent `try_acquire` from 10 threads: total acquired must not exceed capacity
5. `SlidingWindowRateLimiter.is_allowed` allows up to limit requests in window
6. `SlidingWindowRateLimiter.is_allowed` rejects requests over limit within window
7. `SlidingWindowRateLimiter.is_allowed` allows requests again after window expires
8. `SlidingWindowRateLimiter.reset` clears state and allows requests again
9. Concurrent `is_allowed` from 20 threads: total allowed must not exceed limit

Use `time.sleep` sparingly — keep total test time under 3 seconds.
Write no other files. All imports must be stdlib only.
