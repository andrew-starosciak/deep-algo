"""Earnings calendar, estimates, and transcript analysis."""

from __future__ import annotations

import asyncio
import logging
import re
from datetime import datetime, timedelta, timezone

logger = logging.getLogger(__name__)


async def next_event(ticker: str) -> str:
    """Get next earnings date and consensus estimates for a ticker."""
    logger.info("Checking earnings calendar for %s", ticker)

    try:
        import yfinance as yf

        stock = yf.Ticker(ticker)
        cal = stock.calendar

        if not cal:
            return f"No upcoming earnings data for {ticker}"

        lines = [f"Earnings calendar for {ticker}:"]
        for key, value in cal.items():
            lines.append(f"  {key}: {value}")

        return "\n".join(lines)

    except ImportError:
        return f"yfinance not installed — cannot check earnings for {ticker}"
    except Exception as e:
        return f"Earnings lookup failed for {ticker}: {e}"


async def recent_transcript(ticker: str, max_age_days: int = 90) -> str:
    """Provide guidance on accessing earnings transcripts.

    Note: Automated transcript scraping is not reliable due to anti-bot protection.
    This function provides quick links to where transcripts can be manually accessed.

    Args:
        ticker: Stock ticker
        max_age_days: Not used, kept for API compatibility

    Returns:
        Formatted text with transcript access links
    """
    # Check if earnings date is upcoming or recent
    try:
        import yfinance as yf

        stock = yf.Ticker(ticker)
        cal = stock.calendar

        earnings_date = None
        if cal and "Earnings Date" in cal:
            earnings_dates = cal["Earnings Date"]
            if earnings_dates and len(earnings_dates) > 0:
                earnings_date = earnings_dates[0]

        lines = ["**Earnings Transcript Resources:**"]
        lines.append("")

        if earnings_date:
            lines.append(f"Next earnings: {earnings_date}")
            lines.append("")

        lines.append("📄 **Access full transcripts:**")
        lines.append(f"• Seeking Alpha: https://seekingalpha.com/symbol/{ticker}/earnings/transcripts")
        lines.append(f"• Company IR: Search '{ticker} investor relations' for official transcripts")
        lines.append(f"• SEC 8-K filings: Often include prepared remarks within 4 days of earnings")
        lines.append("")
        lines.append(
            "💡 **Key sections to review:**\n"
            "   - Management prepared remarks (guidance, outlook, strategic initiatives)\n"
            "   - Q&A session (analyst concerns, management responses)\n"
            "   - Forward guidance and assumptions"
        )

        return "\n".join(lines)

    except Exception as e:
        logger.debug("Transcript resource generation failed: %s", e)
        return ""  # Fail silently


async def transcript_themes(ticker: str) -> dict[str, str]:
    """Extract key themes from recent earnings transcript using LLM.

    This is a more advanced version that uses LLM to summarize themes.
    Requires openclaw.llm integration.

    Returns:
        Dict with keys: guidance, product_updates, competitive_position, risks
    """
    # Placeholder for future LLM integration
    # For now, the raw transcript extraction above is sufficient
    pass
