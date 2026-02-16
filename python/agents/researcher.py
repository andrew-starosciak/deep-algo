"""Researcher agent — gathers market data and synthesizes via LLM."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient


class ResearcherAgent(BaseAgent):
    """Gathers data from multiple sources, then uses LLM to synthesize."""

    def __init__(self, llm: LLMClient, db: Any = None, research_pipeline: Any = None):
        super().__init__(llm, db)
        self.research_pipeline = research_pipeline

    @property
    def role(self) -> str:
        return (
            "senior equity research analyst specializing in options trading. "
            "You synthesize news, technicals, options flow, and macro data "
            "into actionable research summaries with opportunity scores."
        )

    @property
    def prompt_file(self) -> str:
        return "research_synthesis.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Run the research pipeline to gather raw data before LLM synthesis."""
        data = input_data.model_dump()
        ticker = data.get("ticker", "")
        mode = data.get("mode", "premarket")

        if self.research_pipeline is None:
            return {"raw_data": "Research pipeline not configured. Use input data only."}

        raw = await self.research_pipeline.gather(ticker, mode)
        return {"raw_data": raw}
