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
            "You estimate expected stock moves and catalyst timelines so the system "
            "can programmatically select the best contract from real market data."
        )

    @property
    def prompt_file(self) -> str:
        return "thesis_scoring.md"
