"""Analyst agent — scores thesis and selects contracts."""

from __future__ import annotations

from agents.base import BaseAgent


class AnalystAgent(BaseAgent):
    """Evaluates trade theses using the scoring framework and selects contracts."""

    @property
    def role(self) -> str:
        return (
            "options trading analyst who scores trade theses on five dimensions: "
            "information edge, volatility pricing, technical alignment, catalyst clarity, "
            "and risk/reward ratio. You only recommend trades scoring 7.0+ overall. "
            "For contract selection, you prefer 2x catalyst timeline for expiry, "
            "slightly OTM strikes, and check liquidity (bid-ask < 10%, OI > 500)."
        )

    @property
    def prompt_file(self) -> str:
        return "thesis_scoring.md"
