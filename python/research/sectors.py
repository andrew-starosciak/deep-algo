"""Sector rotation analysis — track relative strength across 11 sectors."""

from __future__ import annotations

import logging
from datetime import datetime, timedelta

logger = logging.getLogger(__name__)


# SPDR Sector ETF mapping
SECTOR_ETFS = {
    "XLK": "Technology",
    "XLF": "Financials",
    "XLV": "Healthcare",
    "XLE": "Energy",
    "XLI": "Industrials",
    "XLY": "Consumer Discretionary",
    "XLP": "Consumer Staples",
    "XLU": "Utilities",
    "XLRE": "Real Estate",
    "XLC": "Communication Services",
    "XLB": "Materials",
}

# Ticker to sector mapping (expand as needed)
TICKER_SECTORS = {
    "AAPL": "XLK",
    "MSFT": "XLK",
    "NVDA": "XLK",
    "GOOGL": "XLC",
    "GOOG": "XLC",
    "META": "XLC",
    "AMZN": "XLY",
    "TSLA": "XLY",
    "JPM": "XLF",
    "BAC": "XLF",
    "JNJ": "XLV",
    "UNH": "XLV",
    "XOM": "XLE",
    "CVX": "XLE",
}


async def rotation_snapshot(ticker: str | None = None) -> str:
    """Get sector rotation snapshot showing momentum across 11 sectors.

    Calculates:
    - 5-day, 20-day, 60-day returns for each sector ETF
    - Relative strength vs S&P 500 (SPY)
    - Identifies which sectors are gaining/losing momentum
    - Flags if ticker's sector is rotating in/out

    Args:
        ticker: Optional ticker to check sector alignment

    Returns:
        Formatted text summary of sector rotation
    """
    logger.info("Calculating sector rotation%s", f" for {ticker}" if ticker else "")

    try:
        import yfinance as yf
        import asyncio

        def _fetch():
            results = {}

            # Fetch SPY (S&P 500) as benchmark
            spy = yf.Ticker("SPY")
            spy_hist = spy.history(period="3mo", interval="1d")

            if spy_hist.empty or len(spy_hist) < 60:
                return {"error": "Insufficient SPY data"}

            # Calculate SPY returns
            spy_5d = ((spy_hist["Close"].iloc[-1] / spy_hist["Close"].iloc[-5]) - 1) * 100
            spy_20d = ((spy_hist["Close"].iloc[-1] / spy_hist["Close"].iloc[-20]) - 1) * 100
            spy_60d = ((spy_hist["Close"].iloc[-1] / spy_hist["Close"].iloc[-60]) - 1) * 100

            results["SPY"] = {
                "name": "S&P 500 (Benchmark)",
                "ret_5d": spy_5d,
                "ret_20d": spy_20d,
                "ret_60d": spy_60d,
            }

            # Fetch all sector ETFs
            for etf, name in SECTOR_ETFS.items():
                try:
                    sector = yf.Ticker(etf)
                    hist = sector.history(period="3mo", interval="1d")

                    if hist.empty or len(hist) < 60:
                        logger.warning("Insufficient data for %s", etf)
                        continue

                    # Calculate returns
                    ret_5d = ((hist["Close"].iloc[-1] / hist["Close"].iloc[-5]) - 1) * 100
                    ret_20d = ((hist["Close"].iloc[-1] / hist["Close"].iloc[-20]) - 1) * 100
                    ret_60d = ((hist["Close"].iloc[-1] / hist["Close"].iloc[-60]) - 1) * 100

                    # Calculate relative strength vs SPY
                    rs_5d = ret_5d - spy_5d
                    rs_20d = ret_20d - spy_20d
                    rs_60d = ret_60d - spy_60d

                    # Momentum score: avg of relative strengths
                    momentum = (rs_5d + rs_20d + rs_60d) / 3

                    results[etf] = {
                        "name": name,
                        "ret_5d": ret_5d,
                        "ret_20d": ret_20d,
                        "ret_60d": ret_60d,
                        "rs_5d": rs_5d,
                        "rs_20d": rs_20d,
                        "rs_60d": rs_60d,
                        "momentum": momentum,
                    }

                except Exception as e:
                    logger.warning("Failed to fetch %s: %s", etf, e)

            return results

        # Run in executor
        results = await asyncio.get_event_loop().run_in_executor(None, _fetch)

        if "error" in results:
            return f"Sector rotation: {results['error']}"

        # Sort sectors by momentum (strongest first)
        sectors = [(etf, data) for etf, data in results.items() if etf != "SPY"]
        sectors.sort(key=lambda x: x[1]["momentum"], reverse=True)

        # Format output
        lines = ["**Sector Rotation Analysis:**"]
        lines.append("")

        # Top 3 strongest sectors (rotating IN)
        lines.append("🟢 **Rotating IN** (strongest momentum):")
        for etf, data in sectors[:3]:
            momentum_indicator = "🔥" if data["momentum"] > 2.0 else "📈"
            lines.append(
                f"  {momentum_indicator} {data['name']} ({etf}): "
                f"{data['momentum']:+.2f}% vs SPY "
                f"[5d: {data['rs_5d']:+.2f}%, 20d: {data['rs_20d']:+.2f}%]"
            )

        lines.append("")

        # Bottom 3 weakest sectors (rotating OUT)
        lines.append("🔴 **Rotating OUT** (weakest momentum):")
        for etf, data in sectors[-3:]:
            momentum_indicator = "🥶" if data["momentum"] < -2.0 else "📉"
            lines.append(
                f"  {momentum_indicator} {data['name']} ({etf}): "
                f"{data['momentum']:+.2f}% vs SPY "
                f"[5d: {data['rs_5d']:+.2f}%, 20d: {data['rs_20d']:+.2f}%]"
            )

        # If ticker provided, analyze its sector
        if ticker:
            ticker_sector_etf = TICKER_SECTORS.get(ticker.upper())
            if ticker_sector_etf and ticker_sector_etf in results:
                lines.append("")
                sector_data = results[ticker_sector_etf]
                sector_name = sector_data["name"]
                momentum = sector_data["momentum"]

                # Determine sector health
                if momentum > 1.0:
                    health = "🟢 STRONG (tailwind for longs)"
                elif momentum < -1.0:
                    health = "🔴 WEAK (headwind for longs, consider hedges)"
                else:
                    health = "⚪ NEUTRAL (sector-neutral setup)"

                lines.append(f"**{ticker} Sector Context:**")
                lines.append(
                    f"  {ticker} is in {sector_name} ({ticker_sector_etf}): "
                    f"{health}"
                )
                lines.append(
                    f"  Sector momentum: {momentum:+.2f}% vs SPY "
                    f"(rank: {[etf for etf, _ in sectors].index(ticker_sector_etf) + 1}/11)"
                )

        lines.append("")
        lines.append(
            f"*Benchmark: SPY 5d: {results['SPY']['ret_5d']:+.2f}%, "
            f"20d: {results['SPY']['ret_20d']:+.2f}%, "
            f"60d: {results['SPY']['ret_60d']:+.2f}%*"
        )

        return "\n".join(lines)

    except ImportError:
        return "Sector rotation: yfinance not installed"
    except Exception as e:
        logger.error("Sector rotation failed: %s", e)
        return f"Sector rotation: Error - {e}"


def get_ticker_sector(ticker: str) -> str | None:
    """Get the sector ETF for a given ticker.

    Returns:
        Sector ETF symbol (e.g., "XLK" for tech) or None if not mapped
    """
    return TICKER_SECTORS.get(ticker.upper())


def is_sector_rotating_in(ticker: str, results: dict) -> bool:
    """Check if a ticker's sector is in the top 3 by momentum.

    Useful for thesis validation - bullish theses in weak sectors are riskier.
    """
    sector_etf = get_ticker_sector(ticker)
    if not sector_etf or sector_etf not in results:
        return False

    # Sort by momentum
    sectors = [(etf, data) for etf, data in results.items() if etf != "SPY"]
    sectors.sort(key=lambda x: x[1]["momentum"], reverse=True)

    top_3 = [etf for etf, _ in sectors[:3]]
    return sector_etf in top_3
