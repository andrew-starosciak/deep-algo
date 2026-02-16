"""Sentiment analysis — Reddit, Twitter/X, StockTwits.

Lower priority. Add after core research loop is working.
"""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)


async def scan(ticker: str) -> str:
    """Scan social media sentiment for a ticker.

    Phase 1: Stub
    Phase 2: Reddit API (r/wallstreetbets, r/options)
    Phase 3: StockTwits API
    """
    logger.info("Scanning sentiment for %s", ticker)
    return f"Sentiment for {ticker}: Not yet implemented."
