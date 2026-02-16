"""News aggregation — RSS feeds, SEC filings, Reddit sentiment."""

from __future__ import annotations

import asyncio
import logging
from datetime import datetime, timedelta, timezone
from typing import Optional

logger = logging.getLogger(__name__)


async def scan(ticker: str, hours: int = 12) -> str:
    """Scan recent news for a ticker from multiple sources.

    Sources:
    1. Yahoo Finance RSS
    2. Google News RSS
    3. SEC EDGAR filings (8-K, Form 4)
    """
    logger.info("Scanning news for %s (last %dh)", ticker, hours)

    cutoff = datetime.now(timezone.utc) - timedelta(hours=hours)

    # Gather from all sources in parallel
    results = await asyncio.gather(
        _yahoo_rss(ticker, cutoff),
        _google_news_rss(ticker, cutoff),
        _sec_edgar_filings(ticker, cutoff),
        return_exceptions=True,
    )

    sections = []
    sources = ["Yahoo Finance RSS", "Google News RSS", "SEC EDGAR Filings"]

    for source, result in zip(sources, results):
        if isinstance(result, Exception):
            logger.warning("News source %s failed: %s", source, result)
            sections.append(f"**{source}**: Error - {result}")
        elif result:
            sections.append(f"**{source}**:\n{result}")

    if not sections:
        return f"No recent news found for {ticker} in last {hours}h"

    return "\n\n".join(sections)


async def _yahoo_rss(ticker: str, cutoff: datetime) -> str:
    """Fetch headlines from Yahoo Finance RSS feed."""
    try:
        import feedparser

        url = f"https://finance.yahoo.com/rss/headline?s={ticker}"

        def _parse():
            return feedparser.parse(url)

        # Run in executor to avoid blocking
        feed = await asyncio.get_event_loop().run_in_executor(None, _parse)

        if not feed.entries:
            return ""

        headlines = []
        for entry in feed.entries[:15]:  # Top 15
            # Parse published time
            published = None
            if hasattr(entry, 'published_parsed') and entry.published_parsed:
                published = datetime(*entry.published_parsed[:6], tzinfo=timezone.utc)

            # Filter by time
            if published and published < cutoff:
                continue

            headlines.append(f"- {entry.title}\n  {entry.link}")

        if not headlines:
            return ""

        return "\n".join(headlines[:10])  # Top 10 after filtering

    except ImportError:
        return "Error: feedparser not installed"
    except Exception as e:
        logger.error("Yahoo RSS failed for %s: %s", ticker, e)
        return f"Error: {e}"


async def _google_news_rss(ticker: str, cutoff: datetime) -> str:
    """Fetch headlines from Google News RSS feed."""
    try:
        import feedparser

        # Google News search for "{TICKER} stock"
        query = f"{ticker} stock"
        url = f"https://news.google.com/rss/search?q={query.replace(' ', '+')}"

        def _parse():
            return feedparser.parse(url)

        feed = await asyncio.get_event_loop().run_in_executor(None, _parse)

        if not feed.entries:
            return ""

        headlines = []
        for entry in feed.entries[:20]:  # Top 20
            # Parse published time
            published = None
            if hasattr(entry, 'published_parsed') and entry.published_parsed:
                published = datetime(*entry.published_parsed[:6], tzinfo=timezone.utc)

            # Filter by time
            if published and published < cutoff:
                continue

            # Extract source from title (Google News format: "Title - Source")
            title = entry.title
            source = ""
            if " - " in title:
                title, source = title.rsplit(" - ", 1)

            headlines.append(f"- {title} ({source})\n  {entry.link}")

        if not headlines:
            return ""

        return "\n".join(headlines[:10])  # Top 10 after filtering

    except ImportError:
        return "Error: feedparser not installed"
    except Exception as e:
        logger.error("Google News RSS failed for %s: %s", ticker, e)
        return f"Error: {e}"


async def _sec_edgar_filings(ticker: str, cutoff: datetime) -> str:
    """Check for recent SEC filings (8-K, Form 4) via EDGAR RSS.

    8-K = Material events (M&A, CEO change, earnings guidance, lawsuits)
    Form 4 = Insider trades (execs buying/selling)
    """
    try:
        import httpx
        import xml.etree.ElementTree as ET

        # First need to lookup CIK (SEC company identifier) from ticker
        cik = await _lookup_cik(ticker)
        if not cik:
            return f"CIK lookup failed for {ticker}"

        # RSS feed for this company's filings
        url = f"https://data.sec.gov/rss?cik={cik}&type=8-K,4&count=20"

        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(url, headers={
                "User-Agent": "Research Bot research@example.com"
            })
            resp.raise_for_status()
            xml_content = resp.text

        # Parse XML manually
        root = ET.fromstring(xml_content)

        # Atom namespace
        ns = {'atom': 'http://www.w3.org/2005/Atom'}

        filings = []
        for entry in root.findall('atom:entry', ns):
            # Get filing date
            updated_elem = entry.find('atom:updated', ns)
            if updated_elem is not None:
                updated_str = updated_elem.text
                published = datetime.fromisoformat(updated_str.replace('Z', '+00:00'))

                if published < cutoff:
                    continue

            # Get filing type from category
            category = entry.find('atom:category', ns)
            filing_type = category.get('term') if category is not None else 'Unknown'
            filing_type = f"Form {filing_type}" if filing_type.isdigit() else filing_type

            # Get title and link
            title_elem = entry.find('atom:title', ns)
            link_elem = entry.find('atom:link', ns)

            title = title_elem.text if title_elem is not None else 'Unknown'
            link = link_elem.get('href') if link_elem is not None else ''

            filings.append(f"- {filing_type}: {title}\n  {link}")

        if not filings:
            return ""

        return "\n".join(filings[:5])  # Top 5 recent filings

    except ImportError as e:
        return f"Error: {e}"
    except Exception as e:
        logger.error("SEC EDGAR RSS failed for %s: %s", ticker, e)
        return f"Error: {e}"


async def _lookup_cik(ticker: str) -> Optional[str]:
    """Look up SEC CIK (Central Index Key) for a ticker.

    Uses SEC's company tickers JSON mapping.
    """
    try:
        import httpx

        # SEC maintains a JSON mapping of ticker → CIK
        url = "https://www.sec.gov/files/company_tickers.json"

        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(url, headers={
                "User-Agent": "Research Bot research@example.com"  # SEC requires User-Agent
            })
            resp.raise_for_status()
            data = resp.json()

        # Find ticker in the mapping
        ticker_upper = ticker.upper()
        for entry in data.values():
            if entry.get("ticker") == ticker_upper:
                # CIK needs to be zero-padded to 10 digits
                cik = str(entry["cik_str"]).zfill(10)
                return cik

        logger.warning("CIK not found for ticker %s", ticker)
        return None

    except Exception as e:
        logger.error("CIK lookup failed for %s: %s", ticker, e)
        return None


async def scan_reddit(ticker: str, hours: int = 24) -> str:
    """Scan Reddit (WSB + /r/stocks) for sentiment on a ticker.

    This is separate from the main news scan because Reddit is more
    about sentiment/positioning than material news.
    """
    logger.info("Scanning Reddit for %s (last %dh)", ticker, hours)

    try:
        import httpx

        cutoff = datetime.now(timezone.utc) - timedelta(hours=hours)

        # Search both subreddits
        subreddits = ["wallstreetbets", "stocks"]
        posts = []

        async with httpx.AsyncClient(timeout=10.0) as client:
            for sub in subreddits:
                url = f"https://www.reddit.com/r/{sub}/search.json"
                params = {
                    "q": ticker,
                    "sort": "hot",
                    "limit": 25,
                    "restrict_sr": "on",
                }

                try:
                    resp = await client.get(url, params=params, headers={
                        "User-Agent": "Research Bot v1.0"
                    })
                    resp.raise_for_status()
                    data = resp.json()

                    for post in data.get("data", {}).get("children", []):
                        post_data = post.get("data", {})
                        created = datetime.fromtimestamp(post_data.get("created_utc", 0), tz=timezone.utc)

                        if created < cutoff:
                            continue

                        posts.append({
                            "title": post_data.get("title"),
                            "subreddit": sub,
                            "score": post_data.get("score", 0),
                            "num_comments": post_data.get("num_comments", 0),
                            "url": f"https://reddit.com{post_data.get('permalink')}",
                        })

                except Exception as e:
                    logger.warning("Reddit API failed for r/%s: %s", sub, e)

        if not posts:
            return f"No Reddit posts found for {ticker} in last {hours}h"

        # Sort by score (upvotes)
        posts.sort(key=lambda p: p["score"], reverse=True)

        lines = []
        for post in posts[:10]:  # Top 10
            lines.append(
                f"- [{post['score']} upvotes, {post['num_comments']} comments] "
                f"{post['title']} (r/{post['subreddit']})\n  {post['url']}"
            )

        return "\n".join(lines)

    except ImportError:
        return "Error: httpx not installed"
    except Exception as e:
        logger.error("Reddit scan failed for %s: %s", ticker, e)
        return f"Error: {e}"
