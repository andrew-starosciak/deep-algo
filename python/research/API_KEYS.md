# Research API Keys Setup

The research pipeline uses multiple news sources. Tier 1 sources work without API keys, while Tier 2 sources require free API registration.

## Tier 1: Always Enabled (No Keys Required)
- Yahoo Finance RSS
- Google News RSS
- SEC EDGAR filings
- Reddit (public API)

## Tier 2: Optional (Free API Keys)

### 1. Finnhub (Recommended - Best Quality/Rate Limits)

**Get API Key:** https://finnhub.io/register

**Rate Limit:** 60 calls/min (free tier)

**Setup:**
```bash
export FINNHUB_API_KEY=your_key_here
```

**Quality:** High - filters spam well, aggregates from major outlets

---

### 2. Alpha Vantage (AI Sentiment Scoring)

**Get API Key:** https://www.alphavantage.co/support/#api-key

**Rate Limit:** 500 calls/day (free tier)

**Setup:**
```bash
export ALPHAVANTAGE_API_KEY=your_key_here
```

**Quality:** High - provides AI-generated sentiment scores (-1.0 to +1.0) and relevance scores

---

### 3. NewsAPI.org (Premium Sources)

**Get API Key:** https://newsapi.org/register

**Rate Limit:** 100 requests/day (free tier, development only)

**Setup:**
```bash
export NEWSAPI_KEY=your_key_here
```

**Quality:** Very high - aggregates from WSJ, Reuters, Bloomberg, 80,000+ sources

---

## Adding Keys to Environment

### Option 1: `.env` file (Recommended)

Add to `/home/a/Work/gambling/engine/.env`:
```bash
# Tier 2 News APIs (optional - free registration)
FINNHUB_API_KEY=your_finnhub_key
ALPHAVANTAGE_API_KEY=your_alphavantage_key
NEWSAPI_KEY=your_newsapi_key

# FRED API (optional - for macro data)
FRED_API_KEY=your_fred_key
```

### Option 2: Shell profile

Add to `~/.bashrc` or `~/.zshrc`:
```bash
export FINNHUB_API_KEY=your_finnhub_key
export ALPHAVANTAGE_API_KEY=your_alphavantage_key
export NEWSAPI_KEY=your_newsapi_key
```

Then: `source ~/.bashrc`

---

## Testing API Keys

```bash
# Test Finnhub
python -c "import os; print('Finnhub:', 'ENABLED' if os.environ.get('FINNHUB_API_KEY') else 'DISABLED')"

# Test Alpha Vantage
python -c "import os; print('Alpha Vantage:', 'ENABLED' if os.environ.get('ALPHAVANTAGE_API_KEY') else 'DISABLED')"

# Test NewsAPI
python -c "import os; print('NewsAPI:', 'ENABLED' if os.environ.get('NEWSAPI_KEY') else 'DISABLED')"
```

---

## Rate Limit Management

With 8 tickers on watchlist:

**Without Tier 2 APIs:**
- RSS + SEC: ~400 requests/day (all free, unlimited)

**With Tier 2 APIs:**
- Finnhub: 60/min = 3,600/hour (plenty for 8 tickers × 24 checks/day = 192 req/day)
- Alpha Vantage: 500/day (62 checks per ticker per day)
- NewsAPI: 100/day (12 checks per ticker per day)

**Recommended polling frequency:**
- Pre-market (8 AM): All sources
- Midday (12:30 PM): RSS + Finnhub only
- Post-market (4:30 PM): All sources
- Weekend deep dive: All sources + extended history

This keeps you well under rate limits even with 8 tickers.

---

## Priority Order

If you can only register for one, choose **Finnhub** (best rate limits + quality).

If you can register for two, add **Alpha Vantage** (sentiment scoring is valuable).

NewsAPI is lowest priority (100/day limit is tight, but quality is highest).
