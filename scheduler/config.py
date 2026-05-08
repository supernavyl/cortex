"""Scheduler configuration."""
from dataclasses import dataclass


@dataclass
class Config:
    """Scheduler runtime configuration."""

    n_workers: int = 4
    db_path: str = "scheduler.db"
    max_queue_size: int = 1000
    log_level: str = "INFO"
