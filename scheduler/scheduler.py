"""Main Scheduler orchestrating queue, workers, and storage."""
from __future__ import annotations

import logging
from typing import Any, Callable

from scheduler.models import Job, JobStatus
from scheduler.queue import AsyncPriorityQueue
from scheduler.storage import SQLiteStorage
from scheduler.worker import WorkerPool

logger = logging.getLogger(__name__)


class Scheduler:
    """High-level scheduler for submitting and managing async jobs."""

    def __init__(
        self,
        n_workers: int = 4,
        db_path: str = "scheduler.db",
        max_queue_size: int = 1000,
    ) -> None:
        self.queue = AsyncPriorityQueue(maxsize=max_queue_size)
        self.storage = SQLiteStorage(db_path)
        self.workers = WorkerPool(n_workers, self.queue, self.storage)

    async def start(self) -> None:
        """Initialize storage and start workers."""
        await self.storage.init_db()
        await self.workers.start()
        logger.info("Scheduler started")

    async def stop(self) -> None:
        """Stop workers and close storage."""
        await self.workers.stop()
        await self.storage.close()
        logger.info("Scheduler stopped")

    async def submit(
        self,
        fn: Callable[..., Any],
        *args: Any,
        priority: int = 0,
        **kwargs: Any,
    ) -> str:
        """Submit a function for execution."""
        job = Job(fn=fn, args=args, kwargs=kwargs, priority=priority)
        await self.storage.save_job(job)
        await self.queue.put(job)
        logger.info("Submitted job %s with priority %s", job.id, priority)
        return job.id

    async def cancel(self, job_id: str) -> bool:
        """Cancel a pending job."""
        in_queue = await self.queue.cancel(job_id)
        job = await self.storage.get_job(job_id)
        if job is not None and job.status == JobStatus.PENDING:
            job.status = JobStatus.CANCELLED
            await self.storage.update_job(job)
            return True
        return in_queue

    async def status(self, job_id: str) -> JobStatus | None:
        """Get the status of a job."""
        job = await self.storage.get_job(job_id)
        if job is None:
            return None
        return job.status

    async def list_jobs(self, status: JobStatus | None = None) -> list[Job]:
        """List jobs, optionally filtered by status."""
        return await self.storage.list_jobs(status)
