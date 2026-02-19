"""Analyst agent — scores thesis and selects contracts."""

from __future__ import annotations

import logging
from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent

logger = logging.getLogger(__name__)


class AnalystAgent(BaseAgent):
    """Evaluates trade theses using the scoring framework and selects contracts."""

    @property
    def role(self) -> str:
        return (
            "options trading analyst who scores trade theses on five dimensions: "
            "information edge, volatility pricing, technical alignment, catalyst clarity, "
            "and risk/reward ratio. You only recommend trades scoring 7.0+ overall. "
            "You estimate expected stock moves and catalyst timelines so the system "
            "can programmatically select the best contract from real market data."
        )

    @property
    def prompt_file(self) -> str:
        return "thesis_scoring.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Fetch previous thesis history with outcomes for this ticker."""
        ticker = getattr(input_data, "ticker", None)
        if not ticker or not self.db:
            return {"historical_context": ""}

        try:
            history = await self.db.get_thesis_history_with_outcomes(ticker, limit=5)
        except Exception:
            logger.debug("Could not fetch thesis history for %s (V016 may not be applied)", ticker)
            return {"historical_context": ""}

        if not history:
            return {"historical_context": ""}

        return {"historical_context": self._format_history(history)}

    @staticmethod
    def _format_history(history: list[dict]) -> str:
        """Format thesis history into a readable section for the prompt."""
        lines = [
            "## Previous Theses for This Ticker",
            "",
            "Review these past calls. Were they correct? What changed since then?",
            "",
        ]
        for row in history:
            date = row.get("created_at")
            date_str = date.strftime("%Y-%m-%d") if date else "unknown"
            direction = row.get("direction", "?")
            score = row.get("overall_score", "?")

            pnl = row.get("outcome_realized_pnl")
            reason = row.get("outcome_close_reason")
            if pnl is not None:
                outcome = f"P&L: ${pnl} ({reason})"
            else:
                outcome = "no outcome yet"

            thesis_text = row.get("thesis_text", "")
            if len(thesis_text) > 200:
                thesis_text = thesis_text[:200] + "..."

            lines.append(f"- **{date_str}** | {direction} | score {score} | {outcome}")
            lines.append(f"  {thesis_text}")
            lines.append("")

        return "\n".join(lines)
