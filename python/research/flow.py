"""Options flow — unusual activity detection.

Initially a stub. Integrate Unusual Whales API ($50/mo) when profitable.
"""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)


async def unusual_activity(ticker: str) -> str:
    """Check for unusual options activity.

    Phase 1: Basic put/call ratio from yfinance
    Phase 2: Unusual Whales API for flow, dark pool, congress trades
    """
    logger.info("Checking options flow for %s", ticker)

    try:
        import yfinance as yf

        stock = yf.Ticker(ticker)
        # Get options expirations
        expirations = stock.options
        if not expirations:
            return f"No options data available for {ticker}"

        # Check nearest expiration for basic metrics
        chain = stock.option_chain(expirations[0])
        calls_volume = chain.calls["volume"].sum()
        puts_volume = chain.puts["volume"].sum()
        pc_ratio = puts_volume / calls_volume if calls_volume > 0 else 0

        return (
            f"Options flow for {ticker} (nearest expiry: {expirations[0]}):\n"
            f"  Calls volume: {calls_volume:,.0f}\n"
            f"  Puts volume: {puts_volume:,.0f}\n"
            f"  P/C ratio: {pc_ratio:.2f}\n"
            f"  Expirations available: {len(expirations)}"
        )

    except ImportError:
        return f"yfinance not installed — cannot check flow for {ticker}"
    except Exception as e:
        return f"Options flow check failed for {ticker}: {e}"
