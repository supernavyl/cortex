"""Async priority queue implementation."""
from __future__ import annotations

import asyncio
import heapq
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from scheduler.models import Job


class AsyncPriorityQueue:
    """Heap-backed async priority queue with cancellation support."""

    def __init__(self, maxsize: int = 0) -> None:
        self._maxsize: int = maxsize
        self._queue: list[tuple[int, "Job"]] = []
        self._lock = asyncio.Lock()
        self._not_empty = asyncio.Condition(self._lock)
        self._not_full = asyncio.Condition(self._lock)
        self._cancelled_ids: set[str] = set()

    async def put(self, job: "Job") -> None:
        """Enqueue a job. Blocks if queue is full."""
        async with self._not_full:
            if self._maxsize > 0:
                while len(self._queue) >= self._maxsize:
                    await self._not_full.wait()
            heapq.heappush(self._queue, (job.priority, job))
            self._not_empty.notify()

    async def get(self) -> "Job":
        """Dequeue the highest-priority job. Blocks until available."""
        async with self._not_empty:
            while not self._queue:
                await self._not_empty.wait()
            _, job = heapq.heappop(self._queue)
            while job.id in self._cancelled_ids:
                self._cancelled_ids.discard(job.id)
                if not self._queue:
                    await self._not_empty.wait()
                    if not self._queue:
                        continue
                _, job = heapq.heappop(self._queue)
            self._not_full.notify()
            return job

    async def cancel(self, job_id: str) -> bool:
        """Mark a job as cancelled by ID. Returns True if it was in the queue."""
        async with self._lock:
            for idx, (_, job) in enumerate(self._queue):
                if job.id == job_id:
                    self._cancelled_ids.add(job_id)
                    job.status = "cancelled"  # type: ignore[assignment]
                    del self._queue[idx]
                    heapq.heapify(self._queue)
                    self._not_full.notify()
                    return True
            self._cancelled_ids.add(job_id)
            return False

    def qsize(self) -> int:
        """Return approximate queue size."""
        return len(self._queue)
