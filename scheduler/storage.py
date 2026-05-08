"""SQLite storage backend using aiosqlite."""
from __future__ import annotations

import json
import sqlite3
from datetime import datetime
from typing import Any

import aiosqlite

from scheduler.models import Job, JobStatus


class SQLiteStorage:
    """Persistent storage for jobs."""

    def __init__(self, db_path: str = "scheduler.db") -> None:
        self.db_path = db_path
        self._db: aiosqlite.Connection | None = None

    async def init_db(self) -> None:
        """Create tables if they don't exist."""
        self._db = await aiosqlite.connect(self.db_path)
        await self._db.execute(
            """
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                fn_name TEXT NOT NULL,
                args TEXT,
                kwargs TEXT,
                priority INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                result TEXT,
                error TEXT
            )
            """
        )
        await self._db.commit()

    async def close(self) -> None:
        """Close the underlying connection."""
        if self._db is not None:
            await self._db.close()
            self._db = None

    async def save_job(self, job: Job) -> None:
        """Insert a new job record."""
        if self._db is None:
            raise RuntimeError("Database not initialized. Call init_db() first.")
        await self._db.execute(
            """
            INSERT INTO jobs (id, fn_name, args, kwargs, priority, status, created_at, result, error)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                job.id,
                _fn_name(job.fn),
                json.dumps(job.args, default=_json_default),
                json.dumps(job.kwargs, default=_json_default),
                job.priority,
                job.status.value,
                job.created_at.isoformat(),
                json.dumps(job.result, default=_json_default) if job.result is not None else None,
                job.error,
            ),
        )
        await self._db.commit()

    async def update_job(self, job: Job) -> None:
        """Update an existing job record."""
        if self._db is None:
            raise RuntimeError("Database not initialized. Call init_db() first.")
        await self._db.execute(
            """
            UPDATE jobs
            SET status = ?, result = ?, error = ?
            WHERE id = ?
            """,
            (
                job.status.value,
                json.dumps(job.result, default=_json_default) if job.result is not None else None,
                job.error,
                job.id,
            ),
        )
        await self._db.commit()

    async def get_job(self, job_id: str) -> Job | None:
        """Retrieve a single job by ID."""
        if self._db is None:
            raise RuntimeError("Database not initialized. Call init_db() first.")
        self._db.row_factory = aiosqlite.Row
        async with self._db.execute(
            "SELECT * FROM jobs WHERE id = ?", (job_id,)
        ) as cursor:
            row = await cursor.fetchone()
            if row is None:
                return None
            return _row_to_job(row)

    async def list_jobs(self, status: JobStatus | None = None) -> list[Job]:
        """List all jobs, optionally filtered by status."""
        if self._db is None:
            raise RuntimeError("Database not initialized. Call init_db() first.")
        self._db.row_factory = aiosqlite.Row
        if status is not None:
            async with self._db.execute(
                "SELECT * FROM jobs WHERE status = ? ORDER BY created_at DESC",
                (status.value,),
            ) as cursor:
                rows = await cursor.fetchall()
        else:
            async with self._db.execute(
                "SELECT * FROM jobs ORDER BY created_at DESC"
            ) as cursor:
                rows = await cursor.fetchall()
        return [_row_to_job(row) for row in rows]


def _fn_name(fn: Any) -> str:
    return getattr(fn, "__name__", repr(fn))


def _json_default(obj: Any) -> Any:
    if isinstance(obj, datetime):
        return obj.isoformat()
    raise TypeError(f"Object of type {type(obj).__name__} is not JSON serializable")


def _row_to_job(row: aiosqlite.Row) -> Job:
    return Job(
        fn=_dummy_fn,
        id=row["id"],
        args=tuple(json.loads(row["args"])) if row["args"] else (),
        kwargs=json.loads(row["kwargs"]) if row["kwargs"] else {},
        priority=row["priority"],
        status=JobStatus(row["status"]),
        created_at=datetime.fromisoformat(row["created_at"]),
        result=json.loads(row["result"]) if row["result"] else None,
        error=row["error"],
    )


def _dummy_fn(*args: Any, **kwargs: Any) -> Any:  # noqa: ARG001
    """Placeholder function for deserialized jobs."""
    return None
