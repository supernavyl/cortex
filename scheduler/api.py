"""FastAPI router for the scheduler REST API."""
from __future__ import annotations

from typing import Any

from fastapi import APIRouter, HTTPException, Query
from pydantic import BaseModel

from scheduler.models import JobStatus
from scheduler.scheduler import Scheduler

router = APIRouter(prefix="/jobs", tags=["jobs"])

# Shared scheduler instance; injected via app state


class SubmitRequest(BaseModel):
    """Request body to submit a job."""

    fn_path: str
    args: list[Any] = []
    kwargs: dict[str, Any] = {}
    priority: int = 0


class JobResponse(BaseModel):
    """Response model for a job."""

    id: str
    priority: int
    status: str
    created_at: str
    result: Any | None
    error: str | None


def _get_scheduler(request: Any) -> Scheduler:
    return request.app.state.scheduler  # type: ignore[no-any-return]


def _resolve_fn(fn_path: str) -> Any:
    """Import a callable from a dotted path."""
    if "." not in fn_path:
        raise HTTPException(status_code=400, detail="fn_path must be 'module.callable'")
    module_name, attr_name = fn_path.rsplit(".", 1)
    try:
        import importlib

        module = importlib.import_module(module_name)
        return getattr(module, attr_name)
    except (ImportError, AttributeError) as exc:
        raise HTTPException(status_code=400, detail=f"Could not resolve {fn_path}: {exc}") from exc


@router.post("", response_model=JobResponse)
async def submit_job(body: SubmitRequest, request: Any) -> JobResponse:
    """Submit a new job."""
    scheduler = _get_scheduler(request)
    fn = _resolve_fn(body.fn_path)
    job_id = await scheduler.submit(fn, *body.args, priority=body.priority, **body.kwargs)
    # Immediately fetch stored representation
    job = await scheduler.storage.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=500, detail="Job not found after creation")
    return JobResponse(
        id=job.id,
        priority=job.priority,
        status=job.status.value,
        created_at=job.created_at.isoformat(),
        result=job.result,
        error=job.error,
    )


@router.get("/{job_id}", response_model=JobResponse)
async def get_job(job_id: str, request: Any) -> JobResponse:
    """Get job details by ID."""
    scheduler = _get_scheduler(request)
    job = await scheduler.storage.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Job not found")
    return JobResponse(
        id=job.id,
        priority=job.priority,
        status=job.status.value,
        created_at=job.created_at.isoformat(),
        result=job.result,
        error=job.error,
    )


@router.delete("/{job_id}", response_model=JobResponse)
async def cancel_job(job_id: str, request: Any) -> JobResponse:
    """Cancel a job by ID."""
    scheduler = _get_scheduler(request)
    await scheduler.cancel(job_id)
    job = await scheduler.storage.get_job(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="Job not found")
    return JobResponse(
        id=job.id,
        priority=job.priority,
        status=job.status.value,
        created_at=job.created_at.isoformat(),
        result=job.result,
        error=job.error,
    )


@router.get("", response_model=list[JobResponse])
async def list_jobs(
    status: str | None = Query(None),
    request: Any = None,
) -> list[JobResponse]:
    """List jobs, optionally filtered by status."""
    scheduler = _get_scheduler(request)
    job_status = JobStatus(status) if status else None
    jobs = await scheduler.list_jobs(job_status)
    return [
        JobResponse(
            id=j.id,
            priority=j.priority,
            status=j.status.value,
            created_at=j.created_at.isoformat(),
            result=j.result,
            error=j.error,
        )
        for j in jobs
    ]
