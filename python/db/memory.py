"""In-memory database for testing workflows without Postgres.

Implements the same interface as db.repositories.Database so the workflow
engine doesn't know the difference.
"""

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from typing import Any

logger = logging.getLogger(__name__)


class MemoryDatabase:
    """In-memory store that satisfies the Database interface."""

    def __init__(self):
        self._run_counter = 0
        self._step_counter = 0
        self.workflow_runs: dict[int, dict] = {}
        self.step_logs: list[dict] = []

    async def create_workflow_run(
        self, workflow_id: str, trigger: str, input_data: dict
    ) -> int:
        self._run_counter += 1
        run_id = self._run_counter
        self.workflow_runs[run_id] = {
            "id": run_id,
            "workflow_id": workflow_id,
            "trigger": trigger,
            "input": input_data,
            "status": "running",
            "result": None,
            "started_at": datetime.now(timezone.utc).isoformat(),
            "completed_at": None,
        }
        logger.debug("Created workflow run %d for %s", run_id, workflow_id)
        return run_id

    async def complete_workflow_run(
        self, run_id: int, status: str = "completed", result: dict | None = None
    ):
        if run_id in self.workflow_runs:
            self.workflow_runs[run_id]["status"] = status
            self.workflow_runs[run_id]["result"] = result
            self.workflow_runs[run_id]["completed_at"] = datetime.now(timezone.utc).isoformat()
        logger.debug("Completed workflow run %d: %s", run_id, status)

    async def log_step(
        self,
        run_id: int,
        step_id: str,
        agent: str,
        attempt: int,
        input_data: dict,
        output_data: dict | None,
        passed_gate: bool,
        duration_ms: int,
    ):
        self._step_counter += 1
        entry = {
            "id": self._step_counter,
            "run_id": run_id,
            "step_id": step_id,
            "agent": agent,
            "attempt": attempt,
            "passed_gate": passed_gate,
            "duration_ms": duration_ms,
        }
        self.step_logs.append(entry)

        status = "PASS" if passed_gate else "FAIL"
        logger.info(
            "  Step [%s] agent=%s attempt=%d %s (%dms)",
            step_id, agent, attempt, status, duration_ms,
        )

    async def save_research_summary(
        self, run_id: int, ticker: str, mode: str, summary: dict, opportunity_score: int
    ) -> int:
        logger.debug("save_research_summary (in-memory noop) for %s", ticker)
        return 0

    async def get_thesis_history_with_outcomes(
        self, ticker: str, limit: int = 5
    ) -> list[dict]:
        return []

    async def get_thesis_id_for_position(self, position_id: int) -> int | None:
        return None

    async def update_thesis_outcome(
        self, thesis_id: int, realized_pnl: Any, close_reason: str, position_id: int
    ):
        pass
