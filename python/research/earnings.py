"""Earnings calendar and estimates."""

from __future__ import annotations

import logging
from datetime import date

logger = logging.getLogger(__name__)


async def next_event(ticker: str) -> str:
    """Get next earnings date and consensus estimates for a ticker."""
    logger.info("Checking earnings calendar for %s", ticker)

    try:
        import yfinance as yf

        stock = yf.Ticker(ticker)
        cal = stock.calendar

        if cal is None or cal.empty:
            return f"No upcoming earnings data for {ticker}"

        lines = [f"Earnings calendar for {ticker}:"]
        for key, value in cal.items():
            lines.append(f"  {key}: {value}")

        return "\n".join(lines)

    except ImportError:
        return f"yfinance not installed — cannot check earnings for {ticker}"
    except Exception as e:
        return f"Earnings lookup failed for {ticker}: {e}"
