"""Research pipeline — orchestrates data gathering across all sources."""

from __future__ import annotations

import asyncio
import logging
from datetime import datetime, timezone

from research import edgar, earnings, flow, fred, news, premarket, sectors, technicals

logger = logging.getLogger(__name__)


class ResearchPipeline:
    """Gather data from all sources for a given ticker."""

    async def gather(self, ticker: str, mode: str = "premarket") -> str:
        """Run all data sources in parallel, return combined raw text for LLM.

        Mode options:
        - premarket: Quick scan before market open (news, technicals, flow)
        - midday: Intraday check (lighter weight)
        - postmarket: End of day review
        - weekly_deep_dive: Comprehensive weekend analysis (includes Reddit sentiment)
        """
        logger.info("Gathering research for %s (mode=%s)", ticker, mode)

        # Core sources (always fetch)
        tasks = [
            news.scan(ticker, hours=12),
            technicals.analyze(ticker),
            flow.unusual_activity(ticker),
            earnings.next_event(ticker),
            fred.macro_snapshot(),
        ]
        labels = ["NEWS", "TECHNICALS", "OPTIONS FLOW", "EARNINGS", "MACRO"]

        # Add pre-market pricing for morning scans
        if mode == "premarket":
            tasks.append(premarket.snapshot(ticker))
            labels.append("PRE-MARKET")

        # Add Reddit sentiment for deep dives (helps gauge retail positioning)
        if mode == "weekly_deep_dive":
            tasks.append(news.scan_reddit(ticker, hours=72))  # Last 3 days
            labels.append("REDDIT SENTIMENT")

        # Add economic calendar for deep dives
        if mode == "weekly_deep_dive":
            tasks.append(fred.economic_calendar())
            labels.append("ECONOMIC CALENDAR")

        # Add sector rotation for deep dives (macro context)
        if mode == "weekly_deep_dive":
            tasks.append(sectors.rotation_snapshot(ticker))
            labels.append("SECTOR ROTATION")

        # Add earnings transcript for deep dives (if available)
        if mode == "weekly_deep_dive":
            tasks.append(earnings.recent_transcript(ticker))
            labels.append("EARNINGS TRANSCRIPT")

        results = await asyncio.gather(*tasks, return_exceptions=True)

        sections = []
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
