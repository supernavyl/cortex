"""Job models."""
from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Callable


class JobStatus(str, Enum):
    """Job execution status."""

    PENDING = "pending"
    RUNNING = "running"
    DONE = "done"
    FAILED = "failed"
    CANCELLED = "cancelled"


@dataclass
class Job:
    """Represents a schedulable unit of work."""

    fn: Callable[..., Any] = field(repr=False)
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    args: tuple[Any, ...] = field(default_factory=tuple)
    kwargs: dict[str, Any] = field(default_factory=dict)
    priority: int = 0
    status: JobStatus = field(default=JobStatus.PENDING)
    created_at: datetime = field(default_factory=datetime.utcnow)
    result: Any | None = None
    error: str | None = None

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Job):
            return NotImplemented
        return self.priority < other.priority
