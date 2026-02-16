"""News aggregation — RSS feeds, SEC filings, Reddit sentiment."""

from __future__ import annotations

import asyncio
import logging
import os
from datetime import datetime, timedelta, timezone
from typing import Optional

from research.news_scoring import (
    deduplicate_headlines,
    filter_material_news,
    format_scored_headlines,
    score_headlines,
)

logger = logging.getLogger(__name__)


async def scan(ticker: str, hours: int = 12) -> str:
    """Scan recent news for a ticker from multiple sources.

    Tier 1 (always enabled):
    1. Yahoo Finance RSS
    2. Google News RSS
    3. SEC EDGAR filings (8-K, Form 4)

    Tier 2 (requires API keys):
    4. Finnhub News API
    5. Alpha Vantage News Sentiment
    6. NewsAPI.org
    """
    logger.info("Scanning news for %s (last %dh)", ticker, hours)

    cutoff = datetime.now(timezone.utc) - timedelta(hours=hours)

    # Tier 1: Always gather (no API keys required)
    tier1_tasks = [
        _yahoo_rss(ticker, cutoff),
        _google_news_rss(ticker, cutoff),
        _sec_edgar_filings(ticker, cutoff),
    ]
    tier1_sources = ["Yahoo Finance RSS", "Google News RSS", "SEC EDGAR Filings"]

    # Tier 2: Only if API keys available
    tier2_tasks = []
    tier2_sources = []

    if os.environ.get("FINNHUB_API_KEY"):
        tier2_tasks.append(_finnhub_news(ticker, cutoff))
        tier2_sources.append("Finnhub News")

    if os.environ.get("ALPHAVANTAGE_API_KEY"):
        tier2_tasks.append(_alphavantage_news(ticker, cutoff))
        tier2_sources.append("Alpha Vantage News")

    if os.environ.get("NEWSAPI_KEY"):
        tier2_tasks.append(_newsapi_news(ticker, cutoff))
        tier2_sources.append("NewsAPI.org")

    # Gather all in parallel
    all_tasks = tier1_tasks + tier2_tasks
    all_sources = tier1_sources + tier2_sources

    results = await asyncio.gather(*all_tasks, return_exceptions=True)

    # Collect all headlines from all sources for unified scoring
    all_headlines = []
    errors = []

    for source, result in zip(all_sources, results):
        if isinstance(result, Exception):
            logger.warning("News source %s failed: %s", source, result)
            errors.append(f"{source}: {result}")
        elif result:
            # Parse headlines from result text
            # Each source returns formatted text with headlines
            # We need to extract the structured data
            # For now, this is a simple approach - we could improve by
            # having each source return structured data directly
            lines = result.split("\n")
            for line in lines:
                if line.strip() and line.startswith("-"):
                    # Extract headline and URL
                    parts = line.split("\n  ")
                    headline_text = parts[0].strip("- ")
                    url = parts[1].strip() if len(parts) > 1 else ""

                    # Extract source from headline if present
                    extracted_source = source
                    if "(" in headline_text and ")" in headline_text:
                        # Format: "Headline (Source)"
                        headline_text, extracted_source = headline_text.rsplit("(", 1)
                        extracted_source = extracted_source.rstrip(")")
                        headline_text = headline_text.strip()

                    all_headlines.append({
                        "headline": headline_text,
                        "url": url,
                        "source": extracted_source,
                        "published": None,  # Would need to parse from each source
                    })

    if not all_headlines and not errors:
        return f"No recent news found for {ticker} in last {hours}h"

    # Score, filter, and deduplicate
    scored = score_headlines(all_headlines, ticker)
    material = filter_material_news(scored, min_quality=4.0)  # Keep quality >= 4.0
    unique = deduplicate_headlines(material)

    # Format output
    output_lines = ["**Curated News Feed** (scored and filtered for quality):"]
    output_lines.append("")
    output_lines.append(format_scored_headlines(unique, max_count=15))

    # Add error summary if any sources failed
    if errors:
        output_lines.append("")
        output_lines.append("**Source Errors:**")
        for error in errors:
            output_lines.append(f"- {error}")

    # Add stats
    output_lines.append("")
    output_lines.append(
        f"*Scanned {len(all_headlines)} headlines, filtered to "
        f"{len(unique)} material stories (quality ≥ 4.0)*"
    )

    return "\n".join(output_lines)


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


async def _finnhub_news(ticker: str, cutoff: datetime) -> str:
    """Fetch news from Finnhub API.

    Finnhub provides high-quality news from major outlets with good spam filtering.
    Free tier: 60 calls/min

    API key: Get free at https://finnhub.io/register
    Set: export FINNHUB_API_KEY=your_key_here
    """
    try:
        import httpx

        api_key = os.environ.get("FINNHUB_API_KEY")
        if not api_key:
            return ""

        # Calculate date range (Finnhub uses YYYY-MM-DD format)
        from_date = cutoff.strftime("%Y-%m-%d")
        to_date = datetime.now(timezone.utc).strftime("%Y-%m-%d")

        url = "https://finnhub.io/api/v1/company-news"
        params = {
            "symbol": ticker,
            "from": from_date,
            "to": to_date,
            "token": api_key,
        }

        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(url, params=params)
            resp.raise_for_status()
            articles = resp.json()

        if not articles:
            return ""

        headlines = []
        for article in articles[:10]:  # Top 10
            headline = article.get("headline", "")
            url = article.get("url", "")
            source = article.get("source", "Unknown")
            published = article.get("datetime", 0)

            # Convert Unix timestamp to datetime
            if published:
                published_dt = datetime.fromtimestamp(published, tz=timezone.utc)
                if published_dt < cutoff:
                    continue

            headlines.append(f"- {headline} ({source})\n  {url}")

        if not headlines:
            return ""

        return "\n".join(headlines)

    except Exception as e:
        logger.error("Finnhub API failed for %s: %s", ticker, e)
        return f"Error: {e}"


async def _alphavantage_news(ticker: str, cutoff: datetime) -> str:
    """Fetch news with AI sentiment scoring from Alpha Vantage.

    Alpha Vantage provides news articles with AI-generated sentiment scores.
    Free tier: 500 calls/day

    API key: Get free at https://www.alphavantage.co/support/#api-key
    Set: export ALPHAVANTAGE_API_KEY=your_key_here
    """
    try:
        import httpx

        api_key = os.environ.get("ALPHAVANTAGE_API_KEY")
        if not api_key:
            return ""

        url = "https://www.alphavantage.co/query"
        params = {
            "function": "NEWS_SENTIMENT",
            "tickers": ticker,
            "apikey": api_key,
            "limit": 50,  # Get more to filter by time
        }

        async with httpx.AsyncClient(timeout=15.0) as client:
            resp = await client.get(url, params=params)
            resp.raise_for_status()
            data = resp.json()

        feed = data.get("feed", [])
        if not feed:
            return ""

        headlines = []
        for article in feed:
            # Parse timestamp (format: "20260216T093000")
            time_published = article.get("time_published", "")
            if time_published:
                try:
                    published_dt = datetime.strptime(time_published, "%Y%m%dT%H%M%S")
                    published_dt = published_dt.replace(tzinfo=timezone.utc)
                    if published_dt < cutoff:
                        continue
                except ValueError:
                    pass  # Skip if can't parse

            title = article.get("title", "")
            url = article.get("url", "")

            # Get sentiment score for this ticker
            sentiment_score = 0.0
            relevance_score = 0.0
            for ticker_sentiment in article.get("ticker_sentiment", []):
                if ticker_sentiment.get("ticker") == ticker:
                    sentiment_score = float(ticker_sentiment.get("ticker_sentiment_score", 0))
                    relevance_score = float(ticker_sentiment.get("relevance_score", 0))
                    break

            # Format sentiment: -1 to 1 scale
            sentiment_label = "neutral"
            if sentiment_score > 0.15:
                sentiment_label = "bullish"
            elif sentiment_score < -0.15:
                sentiment_label = "bearish"

            headlines.append(
                f"- {title} (sentiment: {sentiment_label} {sentiment_score:+.2f}, "
                f"relevance: {relevance_score:.2f})\n  {url}"
            )

            if len(headlines) >= 10:
                break

        if not headlines:
            return ""

        return "\n".join(headlines)

    except Exception as e:
        logger.error("Alpha Vantage API failed for %s: %s", ticker, e)
        return f"Error: {e}"


async def _newsapi_news(ticker: str, cutoff: datetime) -> str:
    """Fetch news from NewsAPI.org.

    NewsAPI aggregates from 80,000+ sources including WSJ, Reuters, Bloomberg.
    Free tier: 100 requests/day (development only)

    API key: Get free at https://newsapi.org/register
    Set: export NEWSAPI_KEY=your_key_here
    """
    try:
        import httpx

        api_key = os.environ.get("NEWSAPI_KEY")
        if not api_key:
            return ""

        url = "https://newsapi.org/v2/everything"

        # Calculate time range (NewsAPI uses ISO 8601)
        from_time = cutoff.isoformat()
        to_time = datetime.now(timezone.utc).isoformat()

        # Search for ticker + company name for better results
        # Note: Ideally we'd have a ticker → company name mapping
        query = f"{ticker} stock"

        params = {
            "q": query,
            "from": from_time,
            "to": to_time,
            "sortBy": "publishedAt",
            "language": "en",
            "apiKey": api_key,
        }

        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(url, params=params)
            resp.raise_for_status()
            data = resp.json()

        articles = data.get("articles", [])
        if not articles:
            return ""

        headlines = []
        for article in articles[:10]:  # Top 10
            title = article.get("title", "")
            url = article.get("url", "")
            source = article.get("source", {}).get("name", "Unknown")
            published = article.get("publishedAt", "")

            # NewsAPI returns ISO format timestamps
            if published:
                try:
                    published_dt = datetime.fromisoformat(published.replace("Z", "+00:00"))
                    if published_dt < cutoff:
                        continue
                except ValueError:
                    pass

            headlines.append(f"- {title} ({source})\n  {url}")

        if not headlines:
            return ""

        return "\n".join(headlines)

    except Exception as e:
        logger.error("NewsAPI failed for %s: %s", ticker, e)
        return f"Error: {e}"
