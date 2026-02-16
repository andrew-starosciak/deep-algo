"""Risk checker agent — validates exposure, sizing, correlations."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient


class RiskCheckerAgent(BaseAgent):
    """Independently verifies risk parameters. Does NOT trust the analyst's assessment."""

    def __init__(self, llm: LLMClient, db: Any = None):
        super().__init__(llm, db)

    @property
    def role(self) -> str:
        return (
            "risk management specialist for a multi-platform trading operation. "
            "You verify position sizing (max 2% per trade), total allocation (max 10%), "
            "sector correlation (max 3 correlated positions), and cross-platform exposure "
            "with HyperLiquid and Polymarket positions. You are conservative and independent — "
            "you do NOT defer to the analyst's recommendation."
        )

    @property
    def prompt_file(self) -> str:
        return "risk_verification.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Pull current portfolio state from DB for risk checks."""
        if self.db is None:
            return {"portfolio_state": "Database not connected. Use conservative defaults."}

        # TODO: Query current options positions, HL positions, PM positions
        # Return portfolio state for the LLM to evaluate against
        return {
            "portfolio_state": "Portfolio query not yet implemented.",
            "open_positions_count": 0,
            "total_exposure_pct": "0.0",
        }
