"""Researcher agent — gathers market data and synthesizes via LLM."""

from __future__ import annotations

import logging
from typing import Any, TypeVar

from pydantic import BaseModel

from agents.base import BaseAgent
from openclaw.llm import LLMClient
from research.pipeline import ResearchPipeline

T = TypeVar("T", bound=BaseModel)

logger = logging.getLogger(__name__)


class ResearcherAgent(BaseAgent):
    """Gathers data from multiple sources, then uses LLM to synthesize."""

    def __init__(self, llm: LLMClient, db: Any = None):
        super().__init__(llm, db)
        self.pipeline = ResearchPipeline()

    @property
    def role(self) -> str:
        return (
            "senior equity research analyst specializing in options trading. "
            "You synthesize news, technicals, options flow, and macro data "
            "into actionable research summaries with opportunity scores (1-10)."
        )

    @property
    def prompt_file(self) -> str:
        return "research_synthesis.md"

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Run the research pipeline to gather raw data before LLM synthesis."""
        data = input_data.model_dump()
        ticker = data.get("ticker", "")
        mode = data.get("mode", "premarket")

        logger.info("Gathering research data for %s (mode=%s)", ticker, mode)
        raw = await self.pipeline.gather(ticker, mode)
        return {"raw_data": raw, "ticker": ticker}
