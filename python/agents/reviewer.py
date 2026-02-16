"""Reviewer agent — evaluates open positions and recommends actions."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient


class ReviewerAgent(BaseAgent):
    """Reviews open positions: thesis validity, P&L, recommended actions."""

    def __init__(self, llm: LLMClient, db: Any = None):
        super().__init__(llm, db)

    @property
    def role(self) -> str:
        return (
            "portfolio manager reviewing open options positions. "
            "For each position, you evaluate whether the original thesis is still valid, "
            "assess P&L trajectory, and recommend one of: hold, add, reduce, close, or roll. "
            "You consider new information, technical changes, and theta decay impact."
        )

    @property
    def prompt_file(self) -> str:
        return "position_review.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Pull open positions and recent news from DB."""
        if self.db is None:
            return {"positions": "Database not connected."}

        # TODO: Query open positions with current P&L
        # TODO: Query recent news for position tickers
        return {"positions": "Position query not yet implemented."}
