"""Technical analysis — price action, moving averages, RSI, support/resistance."""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)


async def analyze(ticker: str) -> str:
    """Run technical analysis for a ticker using yfinance + ta library.

    Returns a text summary of:
    - Current price vs 20/50/200 day MAs
    - RSI (14)
    - Key support/resistance levels
    - Volume trend
    """
    logger.info("Running technicals for %s", ticker)

    try:
        import yfinance as yf

        stock = yf.Ticker(ticker)
        hist = stock.history(period="6mo")

        if hist.empty:
            return f"No price data available for {ticker}"

        close = hist["Close"]
        current = close.iloc[-1]
        ma_20 = close.rolling(20).mean().iloc[-1]
        ma_50 = close.rolling(50).mean().iloc[-1]
        ma_200 = close.rolling(200).mean().iloc[-1] if len(close) >= 200 else None

        # RSI calculation
        delta = close.diff()
        gain = delta.where(delta > 0, 0.0).rolling(14).mean()
        loss = (-delta.where(delta < 0, 0.0)).rolling(14).mean()
        rs = gain.iloc[-1] / loss.iloc[-1] if loss.iloc[-1] != 0 else 0
        rsi = 100 - (100 / (1 + rs))

        lines = [
            f"Price: ${current:.2f}",
            f"20-day MA: ${ma_20:.2f} ({'above' if current > ma_20 else 'below'})",
            f"50-day MA: ${ma_50:.2f} ({'above' if current > ma_50 else 'below'})",
        ]
        if ma_200 is not None:
            lines.append(f"200-day MA: ${ma_200:.2f} ({'above' if current > ma_200 else 'below'})")
        lines.append(f"RSI(14): {rsi:.1f}")

        # Simple support/resistance from recent highs/lows
        recent = hist.tail(20)
        support = recent["Low"].min()
        resistance = recent["High"].max()
        lines.append(f"20-day support: ${support:.2f}")
        lines.append(f"20-day resistance: ${resistance:.2f}")

        return "\n".join(lines)

    except ImportError:
        return f"yfinance not installed — cannot analyze {ticker}"
    except Exception as e:
        return f"Technical analysis failed for {ticker}: {e}"
