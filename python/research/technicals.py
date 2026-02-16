"""Technical analysis — price action, moving averages, RSI, support/resistance."""

from __future__ import annotations

import logging
from datetime import datetime, timedelta

logger = logging.getLogger(__name__)


async def analyze(ticker: str) -> str:
    """Run technical analysis for a ticker using yfinance + ta library.

    Returns a text summary of:
    - Current price vs 20/50/200 day MAs
    - RSI (14)
    - Key support/resistance levels
    - Volume trend
    - IV Rank (implied volatility percentile)
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

        # IV Rank calculation
        iv_rank_result = await iv_rank(ticker)
        if iv_rank_result:
            lines.append(f"\n{iv_rank_result}")

        return "\n".join(lines)

    except ImportError:
        return f"yfinance not installed — cannot analyze {ticker}"
    except Exception as e:
        return f"Technical analysis failed for {ticker}: {e}"


async def iv_rank(ticker: str) -> str:
    """Calculate IV Rank for a ticker.

    IV Rank = (current_iv - 52w_low_iv) / (52w_high_iv - 52w_low_iv) * 100

    Shows where current implied volatility sits relative to its 52-week range:
    - 0-30%: Options are cheap (low IV historically)
    - 30-70%: Normal range
    - 70-100%: Options are expensive (high IV historically)

    Note: Uses ATM option IV for current, and historical volatility (HV) of
    underlying as proxy for IV range. This is an approximation - storing
    actual historical IV in TimescaleDB would be more accurate.
    """
    logger.info("Calculating IV rank for %s", ticker)

    try:
        import yfinance as yf
        import numpy as np

        stock = yf.Ticker(ticker)

        # Get 52 weeks of price history for HV calculation
        hist = stock.history(period="1y")
        if hist.empty or len(hist) < 200:  # ~9 months of trading days
            return f"IV Rank: Insufficient historical data (need ~1 year, got {len(hist)} days)"

        # Calculate current IV from ATM options
        try:
            # Get nearest expiration options chain
            expirations = stock.options
            if not expirations:
                return "IV Rank: No options data available"

            # Use nearest expiration (typically most liquid)
            nearest_exp = expirations[0]
            chain = stock.option_chain(nearest_exp)

            # Find ATM option (closest strike to current price)
            current_price = hist["Close"].iloc[-1]

            calls = chain.calls
            if calls.empty:
                return "IV Rank: No call options found"

            # Get strike closest to current price
            calls["strike_diff"] = abs(calls["strike"] - current_price)
            atm_call = calls.loc[calls["strike_diff"].idxmin()]

            current_iv = atm_call.get("impliedVolatility")
            if current_iv is None or np.isnan(current_iv):
                return "IV Rank: No IV data for ATM option"

            current_iv = float(current_iv)

        except Exception as e:
            logger.warning("Failed to get current IV for %s: %s", ticker, e)
            return f"IV Rank: Failed to fetch current IV ({e})"

        # Calculate historical volatility range (52 weeks)
        # Use rolling 30-day HV as proxy for IV
        returns = np.log(hist["Close"] / hist["Close"].shift(1))
        returns = returns.dropna()

        # Calculate 30-day rolling volatility (annualized)
        rolling_vol = returns.rolling(window=30).std() * np.sqrt(252)
        rolling_vol = rolling_vol.dropna()

        if len(rolling_vol) < 30:
            return "IV Rank: Insufficient data for volatility range"

        hv_52w_low = rolling_vol.min()
        hv_52w_high = rolling_vol.max()

        # Calculate IV Rank using current IV vs HV range
        # This is an approximation - ideally we'd compare IV to historical IV
        if hv_52w_high == hv_52w_low:
            iv_rank_pct = 50.0  # Default to middle if no range
        else:
            iv_rank_pct = ((current_iv - hv_52w_low) / (hv_52w_high - hv_52w_low)) * 100
            iv_rank_pct = max(0, min(100, iv_rank_pct))  # Clamp to 0-100

        # Interpretation
        if iv_rank_pct < 30:
            interpretation = "cheap (low IV)"
        elif iv_rank_pct > 70:
            interpretation = "expensive (high IV)"
        else:
            interpretation = "normal range"

        return (
            f"IV Rank: {iv_rank_pct:.1f}% ({interpretation})\n"
            f"Current IV: {current_iv:.2%}, 52w HV range: {hv_52w_low:.2%} - {hv_52w_high:.2%}"
        )

    except ImportError:
        return "IV Rank: Required libraries not installed (yfinance, numpy)"
    except Exception as e:
        logger.error("IV rank calculation failed for %s: %s", ticker, e)
        return f"IV Rank: Calculation failed ({e})"
