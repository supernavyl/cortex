Build an in-process pub/sub event bus in Python (stdlib only).

## Requirements

Implement `event_bus.py` at the project root:

```python
from typing import Callable, Any

Handler = Callable[[str, Any], None]

class EventBus:
    def subscribe(self, topic: str, handler: Handler) -> str:
        """Subscribe handler to topic. Return subscription ID for unsubscribe.
        
        Topic can contain wildcards:
          - "*" matches any single segment (e.g. "user.*" matches "user.created")
          - "**" matches any number of segments (e.g. "user.**" matches "user.profile.updated")
        """
    
    def unsubscribe(self, subscription_id: str) -> bool:
        """Remove subscription by ID. Return True if it existed."""
    
    def publish(self, topic: str, payload: Any = None) -> int:
        """Publish event to topic. Return count of handlers notified.
        
        Handlers are called synchronously in subscription order.
        Exceptions in handlers are caught and do NOT stop other handlers.
        """
    
    def publish_async(self, topic: str, payload: Any = None) -> None:
        """Publish to topic in a background thread (fire-and-forget).
        All handlers for this topic are dispatched in one background thread.
        """
    
    def subscriber_count(self, topic: str) -> int:
        """Count handlers that would receive events for the given exact topic."""
    
    def clear(self) -> None:
        """Remove all subscriptions."""
```

Thread-safe. Wildcard matching must work for both `*` and `**`.

## Tests

Write `tests/test_event_bus.py` with pytest tests covering:

1. Basic subscribe + publish: handler is called with correct topic and payload
2. Multiple handlers: all receive the same event
3. Exact topic: "user.created" does not trigger subscriber for "user.deleted"
4. Wildcard `*`: "user.*" matches "user.created" and "user.deleted", not "user.profile.updated"
5. Wildcard `**`: "user.**" matches "user.created" and "user.profile.updated"
6. Unsubscribe: removed handler not called on subsequent publish
7. Unsubscribe returns False for unknown ID
8. Exception in handler does not prevent other handlers from running
9. publish returns correct handler count
10. subscriber_count reflects active subscriptions including wildcard matches
11. publish_async: handler is eventually called (use threading.Event with timeout)
12. clear: removes all subscriptions
13. Thread safety: 20 concurrent publishes and 20 concurrent subscribes, no exceptions

Write no other files. All imports must be stdlib only.
