"""Cron-driven workflow scheduling via APScheduler."""

from __future__ import annotations

import logging
from typing import Any

from apscheduler import AsyncScheduler
from apscheduler.triggers.cron import CronTrigger

logger = logging.getLogger(__name__)

# All times in US Eastern
TIMEZONE = "America/New_York"

# Schedule definitions
SCHEDULES = {
    "premarket_prep": {
        "cron": "0 8 * * 1-5",
        "description": "Pre-market research + thesis generation",
    },
    "midday_review": {
        "cron": "30 12 * * 1-5",
        "description": "Midday position review",
    },
    "postmarket_review": {
        "cron": "30 16 * * 1-5",
        "description": "Post-market position review",
    },
    "weekly_deep_dive": {
        "cron": "0 10 * * 6",
        "description": "Saturday deep research + battle plan",
    },
    "catalyst_scan": {
        "cron": "0 18 * * 0",
        "description": "Sunday catalyst calendar scan",
    },
}


class WorkflowScheduler:
    """Schedule and run workflows on cron triggers."""

    def __init__(self, engine: Any):
        self.engine = engine
        self.scheduler = AsyncScheduler()

    async def setup(self) -> None:
        """Register all scheduled workflows."""
        for name, config in SCHEDULES.items():
            parts = config["cron"].split()
            trigger = CronTrigger(
                minute=parts[0],
                hour=parts[1],
                day=parts[2],
                month=parts[3],
                day_of_week=parts[4],
                timezone=TIMEZONE,
            )
            await self.scheduler.add_schedule(
                self._make_runner(name),
                trigger,
                id=name,
            )
            logger.info("Scheduled %s: %s (%s)", name, config["cron"], config["description"])

    def _make_runner(self, workflow_name: str):
        """Create an async callable for the scheduler."""

        async def run_workflow():
            logger.info("Cron triggered: %s", workflow_name)
            try:
                await self.engine.run_by_name(workflow_name)
            except Exception:
                logger.exception("Workflow %s failed", workflow_name)

        return run_workflow

    async def start(self) -> None:
        """Start the scheduler."""
        await self.setup()
        await self.scheduler.run_until_stopped()
