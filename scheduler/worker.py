"""Worker pool that consumes jobs from the queue."""
from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

from scheduler.models import JobStatus

if TYPE_CHECKING:
    from scheduler.models import Job
    from scheduler.queue import AsyncPriorityQueue
    from scheduler.storage import SQLiteStorage

logger = logging.getLogger(__name__)


class WorkerPool:
    """Pool of workers that execute jobs from an async priority queue."""

    def __init__(
        self,
        n_workers: int,
        queue: AsyncPriorityQueue,
        storage: SQLiteStorage,
    ) -> None:
        self.n_workers = n_workers
        self.queue = queue
        self.storage = storage
        self._workers: list[asyncio.Task[None]] = []
        self._shutdown = asyncio.Event()

    async def start(self) -> None:
        """Spawn worker tasks."""
        self._shutdown.clear()
        self._workers = [
            asyncio.create_task(self._worker_loop(f"worker-{i}"))
            for i in range(self.n_workers)
        ]
        logger.info("Started %d workers", self.n_workers)

    async def stop(self) -> None:
        """Signal shutdown and wait for workers to finish."""
        self._shutdown.set()
        for worker in self._workers:
            worker.cancel()
        await asyncio.gather(*self._workers, return_exceptions=True)
        self._workers.clear()
        logger.info("Stopped workers")

    async def _worker_loop(self, name: str) -> None:
        """Continuously get and run jobs until shutdown."""
        while not self._shutdown.is_set():
            try:
                job = await asyncio.wait_for(
                    self.queue.get(), timeout=1.0
                )
            except asyncio.TimeoutError:
                continue
            except asyncio.CancelledError:
                return

            if job.status == JobStatus.CANCELLED:
                logger.debug("[%s] Skipping cancelled job %s", name, job.id)
                continue

            job.status = JobStatus.RUNNING
            await self.storage.update_job(job)
            logger.info("[%s] Running job %s", name, job.id)

            try:
                if asyncio.iscoroutinefunction(job.fn):
                    job.result = await job.fn(*job.args, **job.kwargs)
                else:
                    loop = asyncio.get_running_loop()
                    job.result = await loop.run_in_executor(
                        None, lambda: job.fn(*job.args, **job.kwargs)
                    )
                job.status = JobStatus.DONE
                logger.info("[%s] Job %s done", name, job.id)
            except Exception as exc:  # noqa: BLE001
                job.status = JobStatus.FAILED
                job.error = f"{type(exc).__name__}: {exc}"
                logger.exception("[%s] Job %s failed", name, job.id)

            await self.storage.update_job(job)
