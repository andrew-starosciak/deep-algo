"""Critic agent — stress-tests trade theses from the opposing perspective."""

from __future__ import annotations

import logging

from pydantic import BaseModel

from agents.base import BaseAgent

logger = logging.getLogger(__name__)


class CriticAgent(BaseAgent):
    """Stress-tests the analyst's thesis from the opposing perspective."""

    @property
    def role(self) -> str:
        return (
            "devil's advocate who stress-tests trade theses. "
            "You build the strongest case AGAINST the trade, identify blind spots, "
            "and adjust scores to reflect true risk. You are not contrarian for its "
            "own sake — if the thesis is strong, you say so."
        )

    @property
    def prompt_file(self) -> str:
        return "thesis_critique.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Inject cross-ticker system feedback so critic knows historical patterns."""
        if not self.db or not hasattr(self.db, "pool"):
            return {"system_feedback": ""}
        try:
            from db.feedback import FeedbackAggregator

            aggregator = FeedbackAggregator(self.db.pool)
            return {"system_feedback": await aggregator.build_analyst_feedback()}
        except Exception:
            logger.debug("Could not fetch system feedback for critic", exc_info=True)
            return {"system_feedback": ""}
