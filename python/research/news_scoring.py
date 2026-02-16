"""News quality scoring and filtering."""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass
from datetime import datetime

logger = logging.getLogger(__name__)


@dataclass
class ScoredHeadline:
    """A headline with quality and relevance scores."""

    headline: str
    url: str
    source: str
    published: datetime | None
    quality_score: float  # 0.0 to 10.0
    relevance_score: float  # 0.0 to 1.0
    sentiment_score: float | None  # -1.0 to 1.0 (if available)
    tags: list[str]  # Material, Fluff, Catalyst, etc.


def score_headlines(headlines: list[dict], ticker: str) -> list[ScoredHeadline]:
    """Score and filter headlines for quality and relevance.

    Scoring rubric (0-10):
    - 9-10: Game-changer (M&A, CEO change, regulatory action, guidance)
    - 7-8: Material (earnings beat/miss, product launch, analyst upgrade)
    - 4-6: Background (useful context but not actionable)
    - 1-3: Noise (fluff, clickbait, rehashed content)

    Args:
        headlines: List of dicts with keys: headline, url, source, published,
                   relevance_score (optional), sentiment_score (optional)
        ticker: Stock ticker for context

    Returns:
        List of ScoredHeadline objects, sorted by quality (best first)
    """
    scored = []

    for h in headlines:
        headline_text = h.get("headline", "")
        if not headline_text:
            continue

        # Base quality score from heuristics
        quality = _heuristic_quality_score(headline_text, ticker)

        # Adjust based on Alpha Vantage relevance (if available)
        av_relevance = h.get("relevance_score", 0.0)
        if av_relevance > 0:
            # Boost quality if AV says it's highly relevant
            quality = quality * 0.7 + (av_relevance * 10) * 0.3

        # Extract tags
        tags = _extract_tags(headline_text)

        scored.append(
            ScoredHeadline(
                headline=headline_text,
                url=h.get("url", ""),
                source=h.get("source", "Unknown"),
                published=h.get("published"),
                quality_score=quality,
                relevance_score=av_relevance or _heuristic_relevance(headline_text, ticker),
                sentiment_score=h.get("sentiment_score"),
                tags=tags,
            )
        )

    # Sort by quality (highest first)
    scored.sort(key=lambda x: x.quality_score, reverse=True)

    return scored


def filter_material_news(scored: list[ScoredHeadline], min_quality: float = 6.0) -> list[ScoredHeadline]:
    """Filter to only material news (quality >= 6.0 by default).

    Args:
        scored: List of scored headlines
        min_quality: Minimum quality score to keep (default 6.0 = material threshold)

    Returns:
        Filtered list of high-quality headlines
    """
    return [h for h in scored if h.quality_score >= min_quality]


def deduplicate_headlines(scored: list[ScoredHeadline]) -> list[ScoredHeadline]:
    """Remove near-duplicate headlines (same story from multiple sources).

    Keeps the highest quality version of each story.
    """
    seen_fingerprints = set()
    unique = []

    for headline in scored:
        # Create fingerprint by removing common words and normalizing
        fingerprint = _headline_fingerprint(headline.headline)

        if fingerprint not in seen_fingerprints:
            seen_fingerprints.add(fingerprint)
            unique.append(headline)

    return unique


def format_scored_headlines(scored: list[ScoredHeadline], max_count: int = 10) -> str:
    """Format scored headlines for LLM consumption.

    Args:
        scored: List of scored headlines
        max_count: Maximum number to include

    Returns:
        Formatted string with scores and tags
    """
    if not scored:
        return "No material news found."

    lines = []
    for i, h in enumerate(scored[:max_count], 1):
        # Format sentiment if available
        sentiment = ""
        if h.sentiment_score is not None:
            if h.sentiment_score > 0.15:
                sentiment = " [📈 bullish]"
            elif h.sentiment_score < -0.15:
                sentiment = " [📉 bearish]"

        # Format tags
        tags = f" ({', '.join(h.tags)})" if h.tags else ""

        lines.append(
            f"{i}. [{h.quality_score:.1f}/10] {h.headline}{sentiment}{tags}\n"
            f"   Source: {h.source} | {h.url}"
        )

    return "\n".join(lines)


def _heuristic_quality_score(headline: str, ticker: str) -> float:
    """Score headline quality using keyword patterns.

    Returns score 0.0-10.0
    """
    headline_lower = headline.lower()
    ticker_lower = ticker.lower()

    # Check if ticker is actually mentioned (relevance filter)
    if ticker_lower not in headline_lower:
        # Check for company name variants (would need mapping)
        # For now, assume it's relevant if it came from ticker-filtered feed
        pass

    # Game-changers (9-10)
    if any(
        keyword in headline_lower
        for keyword in [
            "acquires",
            "acquisition",
            "merger",
            "ceo depart",
            "ceo resign",
            "regulatory action",
            "fda approval",
            "fda reject",
            "guidance raise",
            "guidance cut",
            "bankruptcy",
        ]
    ):
        return 9.5

    # Material news (7-8)
    if any(
        keyword in headline_lower
        for keyword in [
            "earnings beat",
            "earnings miss",
            "upgrade",
            "downgrade",
            "price target",
            "analyst",
            "product launch",
            "partnership",
            "contract win",
            "revenue",
            "profit",
        ]
    ):
        return 7.5

    # Background context (4-6)
    if any(
        keyword in headline_lower
        for keyword in [
            "forecast",
            "outlook",
            "expects",
            "estimates",
            "sector",
            "industry",
            "market",
        ]
    ):
        return 5.0

    # Noise indicators (1-3)
    if any(
        keyword in headline_lower
        for keyword in [
            "top stock",
            "best stock",
            "should you buy",
            "is it time to",
            "here's what",
            "here's why",
            "3 stocks",
            "5 stocks",
            "warren buffett",  # Unless actually about his holdings
            "what to know",
        ]
    ):
        return 2.0

    # Default: moderate quality
    return 5.0


def _heuristic_relevance(headline: str, ticker: str) -> float:
    """Estimate relevance score 0.0-1.0 if not provided by API."""
    headline_lower = headline.lower()
    ticker_lower = ticker.lower()

    # Check if ticker appears in headline
    if ticker_lower in headline_lower:
        # Check if it's the main subject (appears early)
        if headline_lower.index(ticker_lower) < len(headline_lower) / 3:
            return 0.9
        return 0.7

    # Generic relevance (came from ticker feed but not explicit mention)
    return 0.5


def _extract_tags(headline: str) -> list[str]:
    """Extract tags based on headline content."""
    headline_lower = headline.lower()
    tags = []

    # Catalyst indicators
    if any(kw in headline_lower for kw in ["earnings", "report", "results"]):
        tags.append("Earnings")
    if any(kw in headline_lower for kw in ["fda", "approval", "clinical"]):
        tags.append("FDA/Regulatory")
    if any(kw in headline_lower for kw in ["merger", "acquisition", "acquires"]):
        tags.append("M&A")
    if any(kw in headline_lower for kw in ["ceo", "executive", "management"]):
        tags.append("Leadership")
    if any(kw in headline_lower for kw in ["upgrade", "downgrade", "price target", "analyst"]):
        tags.append("Analyst")
    if any(kw in headline_lower for kw in ["product", "launch", "release"]):
        tags.append("Product")

    # Sentiment indicators
    if any(kw in headline_lower for kw in ["surge", "soar", "rally", "jump", "gain"]):
        tags.append("Bullish")
    if any(kw in headline_lower for kw in ["plunge", "drop", "fall", "tumble", "decline"]):
        tags.append("Bearish")

    return tags


def _headline_fingerprint(headline: str) -> str:
    """Create a fingerprint for deduplication.

    Removes common words, punctuation, and normalizes to catch similar stories.
    """
    # Remove common words
    stopwords = {
        "the",
        "a",
        "an",
        "and",
        "or",
        "but",
        "is",
        "are",
        "was",
        "were",
        "in",
        "on",
        "at",
        "to",
        "for",
        "of",
        "with",
        "by",
        "from",
        "as",
        "this",
        "that",
    }

    # Lowercase and split into words
    words = re.findall(r"\w+", headline.lower())

    # Keep only significant words
    significant = [w for w in words if w not in stopwords and len(w) > 3]

    # Sort to catch reordered stories
    significant.sort()

    return " ".join(significant[:8])  # First 8 significant words
