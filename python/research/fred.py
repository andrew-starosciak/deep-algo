"""Federal Reserve / macro data via FRED API."""

from __future__ import annotations

import logging
import os
from datetime import datetime

logger = logging.getLogger(__name__)


async def macro_snapshot() -> str:
    """Get current macro environment summary.

    Fetches key indicators from FRED (Federal Reserve Economic Data):
    - Fed funds rate (FEDFUNDS)
    - CPI year-over-year (CPIAUCSL)
    - Unemployment rate (UNRATE)
    - GDP growth (GDP)

    Requires FRED_API_KEY environment variable.
    Get free key: https://fred.stlouisfed.org/docs/api/api_key.html
    """
    logger.info("Fetching macro snapshot")

    api_key = os.environ.get("FRED_API_KEY")
    if not api_key:
        return (
            "Macro snapshot: FRED_API_KEY not set.\n"
            "Get free key: https://fred.stlouisfed.org/docs/api/api_key.html\n"
            "Then: export FRED_API_KEY=your_key_here"
        )

    try:
        import httpx

        # FRED API endpoints
        base_url = "https://api.stlouisfed.org/fred/series/observations"

        # Series IDs for key indicators
        series = {
            "FEDFUNDS": "Fed Funds Rate",
            "CPIAUCSL": "CPI (All Urban Consumers)",
            "UNRATE": "Unemployment Rate",
            "GDP": "GDP",
        }

        results = []

        async with httpx.AsyncClient(timeout=10.0) as client:
            for series_id, name in series.items():
                try:
                    # Get most recent observation
                    params = {
                        "series_id": series_id,
                        "api_key": api_key,
                        "file_type": "json",
                        "sort_order": "desc",
                        "limit": 1,
                    }

                    resp = await client.get(base_url, params=params)
                    resp.raise_for_status()
                    data = resp.json()

                    if "observations" in data and data["observations"]:
                        obs = data["observations"][0]
                        value = obs.get("value", "N/A")
                        date = obs.get("date", "N/A")

                        # Format based on series type
                        if series_id == "FEDFUNDS":
                            formatted = f"{name}: {value}% (as of {date})"
                        elif series_id == "CPIAUCSL":
                            # CPI is an index, show it raw
                            formatted = f"{name}: {value} (as of {date})"
                        elif series_id == "UNRATE":
                            formatted = f"{name}: {value}% (as of {date})"
                        elif series_id == "GDP":
                            # GDP is in billions
                            formatted = f"{name}: ${value}B (as of {date})"
                        else:
                            formatted = f"{name}: {value} (as of {date})"

                        results.append(formatted)

                except Exception as e:
                    logger.warning("FRED API failed for %s: %s", series_id, e)
                    results.append(f"{name}: Error fetching data")

        if not results:
            return "Macro snapshot: All FRED API calls failed"

        # Add interpretation context
        header = "**Current Macro Environment:**"
        footer = (
            "\nContext: Fed funds rate indicates monetary policy stance. "
            "Rising rates = tightening = headwind for growth stocks. "
            "CPI indicates inflation pressure. "
            "Unemployment indicates labor market strength."
        )

        return f"{header}\n" + "\n".join(f"- {r}" for r in results) + footer

    except ImportError:
        return "Error: httpx not installed"
    except Exception as e:
        logger.error("FRED API macro snapshot failed: %s", e)
        return f"Macro snapshot: Error - {e}"


async def economic_calendar() -> str:
    """Get upcoming economic events that could move markets.

    High-impact events:
    - FOMC meetings (Fed rate decisions)
    - CPI releases (inflation data)
    - Jobs reports (unemployment, NFP)
    - GDP releases

    Note: This is a stub. Full implementation would scrape from:
    - TradingEconomics.com
    - Investing.com economic calendar
    - Fed website for FOMC schedule
    """
    logger.info("Fetching economic calendar")

    # Hardcoded FOMC schedule for 2026 (publicly known dates)
    fomc_dates = [
        "2026-03-18 to 2026-03-19",
        "2026-04-29 to 2026-04-30",
        "2026-06-10 to 2026-06-11",
        "2026-07-29 to 2026-07-30",
        "2026-09-16 to 2026-09-17",
        "2026-11-04 to 2026-11-05",
        "2026-12-16 to 2026-12-17",
    ]

    upcoming = []
    today = datetime.now().date()

    for date_range in fomc_dates:
        # Parse end date (decision day)
        end_date_str = date_range.split(" to ")[1]
        end_date = datetime.strptime(end_date_str, "%Y-%m-%d").date()

        if end_date >= today:
            days_until = (end_date - today).days
            upcoming.append(f"- FOMC Meeting: {date_range} ({days_until} days)")

    if not upcoming:
        return "Economic calendar: No upcoming FOMC meetings in near term"

    header = "**Upcoming High-Impact Events:**"
    footer = (
        "\nNote: CPI and Jobs data release dates vary. "
        "Check TradingEconomics.com for full calendar."
    )

    return header + "\n" + "\n".join(upcoming[:3]) + footer  # Next 3 meetings
