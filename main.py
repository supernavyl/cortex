"""FastAPI application entrypoint."""
from contextlib import asynccontextmanager
from typing import AsyncIterator

import uvicorn
from fastapi import FastAPI

from scheduler.api import router
from scheduler.config import Config
from scheduler.scheduler import Scheduler

config = Config()


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    """Start the scheduler on app startup and stop on shutdown."""
    scheduler = Scheduler(
        n_workers=config.n_workers,
        db_path=config.db_path,
        max_queue_size=config.max_queue_size,
    )
    app.state.scheduler = scheduler
    await scheduler.start()
    yield
    await scheduler.stop()


app = FastAPI(title="Async Job Scheduler", lifespan=lifespan)
app.include_router(router)


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    """Health check endpoint."""
    return {"status": "ok"}


if __name__ == "__main__":
    uvicorn.run("main:app", host="0.0.0.0", port=8000, log_level=config.log_level.lower())
