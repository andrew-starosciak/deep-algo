"""Research pipeline — orchestrates data gathering across all sources."""

from __future__ import annotations

import asyncio
import logging
from datetime import datetime, timezone

from research import edgar, earnings, flow, fred, news, technicals

logger = logging.getLogger(__name__)


class ResearchPipeline:
    """Gather data from all sources for a given ticker."""

    async def gather(self, ticker: str, mode: str = "premarket") -> str:
        """Run all data sources in parallel, return combined raw text for LLM."""
        logger.info("Gathering research for %s (mode=%s)", ticker, mode)

        results = await asyncio.gather(
            news.scan(ticker, hours=12),
            technicals.analyze(ticker),
            flow.unusual_activity(ticker),
            earnings.next_event(ticker),
            fred.macro_snapshot(),
            return_exceptions=True,
        )

        sections = []
        labels = ["NEWS", "TECHNICALS", "OPTIONS FLOW", "EARNINGS", "MACRO"]

        for label, result in zip(labels, results):
            if isinstance(result, Exception):
                sections.append(f"## {label}\nError: {result}")
                logger.warning("Research source %s failed: %s", label, result)
            else:
                sections.append(f"## {label}\n{result}")

        return "\n\n".join(sections)

    async def gather_batch(self, tickers: list[str], mode: str = "premarket") -> dict[str, str]:
        """Gather research for multiple tickers in parallel."""
        tasks = {ticker: self.gather(ticker, mode) for ticker in tickers}
        results = {}
        for ticker, coro in tasks.items():
            results[ticker] = await coro
        return results
