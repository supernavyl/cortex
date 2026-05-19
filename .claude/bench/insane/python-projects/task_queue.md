Build a priority task queue with worker pool in Python (stdlib only).

## Requirements

Implement `task_queue.py` at the project root:

```python
from typing import Callable, Any
from enum import IntEnum

class Priority(IntEnum):
    HIGH = 1
    NORMAL = 5
    LOW = 10

class TaskResult:
    task_id: str
    success: bool
    result: Any        # return value of the task function
    error: str | None  # error message if failed
    attempts: int      # how many times it was tried

class TaskQueue:
    def __init__(
        self,
        workers: int = 4,
        max_retries: int = 3,
        retry_delay: float = 0.1,  # seconds (doubles each retry)
    ) -> None: ...
    
    def submit(
        self,
        fn: Callable[..., Any],
        *args: Any,
        priority: Priority = Priority.NORMAL,
        task_id: str | None = None,
        **kwargs: Any,
    ) -> str:
        """Submit task. Return task_id."""
    
    def wait(self, task_id: str, timeout: float | None = None) -> TaskResult:
        """Block until task completes. Raise TimeoutError if timeout exceeded."""
    
    def wait_all(self, timeout: float | None = None) -> list[TaskResult]:
        """Block until all submitted tasks complete."""
    
    def cancel(self, task_id: str) -> bool:
        """Cancel a pending (not yet running) task. Return True if cancelled."""
    
    def shutdown(self, wait: bool = True) -> None:
        """Stop accepting new tasks. If wait=True, drain queue first."""
    
    @property
    def pending(self) -> int:
        """Count of tasks not yet started."""
    
    @property
    def running(self) -> int:
        """Count of tasks currently executing."""
```

Workers pick highest-priority tasks first. Retry with exponential backoff on exception.

## Tests

Write `tests/test_task_queue.py` covering:

1. submit + wait: task runs and returns result
2. Multiple tasks: all complete, wait_all returns all results
3. Priority ordering: HIGH tasks run before LOW tasks (submit several, check order)
4. Retry on failure: function raising exception retried up to max_retries
5. Permanent failure: after max_retries exhausted, TaskResult.success=False, attempts=max_retries
6. cancel: pending task cancelled, not executed
7. shutdown(wait=True): drains all pending tasks before returning
8. Concurrent workers: 10 tasks submitted, completes faster with 4 workers than 1
9. wait timeout: TimeoutError raised when task doesn't finish in time
10. task_id: custom task_id returned by submit and usable in wait

Keep total test time under 5 seconds.
Write no other files. All imports must be stdlib only.
