"""SEC EDGAR integration — recent filings, insider transactions."""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)

# SEC EDGAR API base URL (free, no key required)
EDGAR_BASE = "https://efts.sec.gov/LATEST"


async def recent_filings(ticker: str, filing_types: list[str] | None = None) -> str:
    """Fetch recent SEC filings for a ticker.

    filing_types: e.g., ["10-K", "10-Q", "8-K", "4"] (4 = insider transactions)
    """
    logger.info("Checking EDGAR filings for %s", ticker)

    if filing_types is None:
        filing_types = ["10-K", "10-Q", "8-K", "4"]

    # TODO: Implement EDGAR API calls
    # 1. Look up CIK from ticker: https://efts.sec.gov/LATEST/search-index?q=NVDA&dateRange=custom
    # 2. Fetch recent filings: https://efts.sec.gov/LATEST/search-index?q=...&forms=8-K
    # 3. Parse and summarize key filings

    return f"EDGAR filings for {ticker}: Not yet implemented."
