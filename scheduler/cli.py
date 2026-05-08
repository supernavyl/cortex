"""Click CLI for the scheduler."""
from __future__ import annotations

import asyncio
from typing import Any

import click

from scheduler.config import Config
from scheduler.models import JobStatus
from scheduler.scheduler import Scheduler


def _get_scheduler() -> Scheduler:
    config = Config()
    return Scheduler(
        n_workers=config.n_workers,
        db_path=config.db_path,
        max_queue_size=config.max_queue_size,
    )


def _resolve_fn(fn_path: str) -> Any:
    """Import a callable from a dotted path."""
    if "." not in fn_path:
        raise click.BadParameter("fn_path must be 'module.callable'")
    module_name, attr_name = fn_path.rsplit(".", 1)
    try:
        import importlib

        module = importlib.import_module(module_name)
        return getattr(module, attr_name)
    except (ImportError, AttributeError) as exc:
        raise click.BadParameter(f"Could not resolve {fn_path}: {exc}") from exc


@click.group(name="sched")
def cli() -> None:
    """Async job scheduler CLI."""


@cli.command()
@click.argument("fn_path")
@click.option("--priority", default=0, type=int, help="Job priority (lower = higher priority)")
@click.option("--args", default="", help="Comma-separated positional args (as strings)")
@click.option("--kwargs", default="", help="Comma-separated key=value kwargs")
def submit(fn_path: str, priority: int, args: str, kwargs: str) -> None:
    """Submit a job by dotted function path."""
    fn = _resolve_fn(fn_path)
    parsed_args = [a.strip() for a in args.split(",") if a.strip()]
    parsed_kwargs = {}
    if kwargs:
        for item in kwargs.split(","):
            if "=" in item:
                k, v = item.split("=", 1)
                parsed_kwargs[k.strip()] = v.strip()

    async def _run() -> None:
        scheduler = _get_scheduler()
        await scheduler.start()
        job_id = await scheduler.submit(fn, *parsed_args, priority=priority, **parsed_kwargs)
        click.echo(job_id)
        await scheduler.stop()

    asyncio.run(_run())


@cli.command()
@click.argument("job_id")
def status(job_id: str) -> None:
    """Get the status of a job."""
    async def _run() -> None:
        scheduler = _get_scheduler()
        await scheduler.start()
        st = await scheduler.status(job_id)
        if st is None:
            click.echo("Not found")
        else:
            click.echo(st.value)
        await scheduler.stop()

    asyncio.run(_run())


@cli.command()
@click.argument("job_id")
def cancel(job_id: str) -> None:
    """Cancel a pending job."""
    async def _run() -> None:
        scheduler = _get_scheduler()
        await scheduler.start()
        ok = await scheduler.cancel(job_id)
        click.echo("Cancelled" if ok else "Not found or not pending")
        await scheduler.stop()

    asyncio.run(_run())


@cli.command()
@click.option("--status", "status_filter", type=str, default=None, help="Filter by status")
def list(status_filter: str | None) -> None:
    """List jobs."""
    async def _run() -> None:
        scheduler = _get_scheduler()
        await scheduler.start()
        job_status = JobStatus(status_filter) if status_filter else None
        jobs = await scheduler.list_jobs(job_status)
        for job in jobs:
            click.echo(f"{job.id}  {job.status.value:10}  priority={job.priority}")
        if not jobs:
            click.echo("No jobs")
        await scheduler.stop()

    asyncio.run(_run())


if __name__ == "__main__":
    cli()
