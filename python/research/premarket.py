"""Pre-market pricing — futures, gap detection, overnight moves."""

from __future__ import annotations

import logging
from datetime import datetime, timezone

logger = logging.getLogger(__name__)


async def snapshot(ticker: str | None = None) -> str:
    """Get pre-market pricing snapshot.

    Includes:
    - S&P 500 futures (ES) overnight move
    - Nasdaq 100 futures (NQ) overnight move
    - Individual ticker pre-market price (if provided)
    - Gap up/down detection

    Useful for pre-market research (8 AM scan) to understand market sentiment
    before the opening bell.
    """
    logger.info("Fetching pre-market snapshot%s", f" for {ticker}" if ticker else "")

    try:
        import yfinance as yf
        import asyncio

        def _fetch():
            results = {}

            # Fetch futures data
            # ES=F = E-mini S&P 500 futures
            # NQ=F = E-mini Nasdaq 100 futures
            futures_symbols = {
                "ES=F": "S&P 500 Futures",
                "NQ=F": "Nasdaq 100 Futures",
            }

            for symbol, name in futures_symbols.items():
                try:
                    fut = yf.Ticker(symbol)
                    # Get recent history to calculate overnight move
                    hist = fut.history(period="5d", interval="1d")

                    if not hist.empty and len(hist) >= 2:
                        prev_close = hist["Close"].iloc[-2]  # Previous day close
                        current = hist["Close"].iloc[-1]      # Most recent (could be yesterday if before open)

                        # Try to get real-time quote for current futures price
                        info = fut.info
                        current_price = info.get("regularMarketPrice", current)

                        change = current_price - prev_close
                        change_pct = (change / prev_close) * 100

                        results[symbol] = {
                            "name": name,
                            "price": current_price,
                            "prev_close": prev_close,
                            "change": change,
                            "change_pct": change_pct,
                        }
                except Exception as e:
                    logger.warning("Failed to fetch %s: %s", symbol, e)
                    results[symbol] = {"error": str(e)}

            # Fetch individual ticker pre-market if provided
            if ticker:
                try:
                    stock = yf.Ticker(ticker)

                    # Get previous close
                    hist = stock.history(period="5d", interval="1d")
                    if not hist.empty:
                        prev_close = hist["Close"].iloc[-1]

                        # Try to get pre-market price
                        info = stock.info
                        premarket_price = info.get("preMarketPrice")
                        regular_price = info.get("regularMarketPrice")

                        # Use whichever is available
                        current_price = premarket_price or regular_price or prev_close

                        change = current_price - prev_close
                        change_pct = (change / prev_close) * 100

                        results[ticker] = {
                            "name": ticker,
                            "price": current_price,
                            "prev_close": prev_close,
                            "change": change,
                            "change_pct": change_pct,
                            "is_premarket": premarket_price is not None,
                        }
                except Exception as e:
                    logger.warning("Failed to fetch pre-market for %s: %s", ticker, e)
                    results[ticker] = {"error": str(e)}

            return results

        # Run in executor to avoid blocking
        results = await asyncio.get_event_loop().run_in_executor(None, _fetch)

        # Format output
        lines = ["**Pre-Market Snapshot:**"]

        # Futures sentiment
        es_data = results.get("ES=F", {})
        nq_data = results.get("NQ=F", {})

        if "error" not in es_data:
            es_change = es_data["change_pct"]
            es_direction = "📈" if es_change > 0 else "📉"
            lines.append(
                f"- S&P 500 Futures: ${es_data['price']:.2f} "
                f"({es_direction} {es_change:+.2f}% overnight)"
            )

        if "error" not in nq_data:
            nq_change = nq_data["change_pct"]
            nq_direction = "📈" if nq_change > 0 else "📉"
            lines.append(
                f"- Nasdaq 100 Futures: ${nq_data['price']:.2f} "
                f"({nq_direction} {nq_change:+.2f}% overnight)"
            )

        # Overall market sentiment
        if "error" not in es_data and "error" not in nq_data:
            avg_change = (es_data["change_pct"] + nq_data["change_pct"]) / 2
            if avg_change > 0.5:
                sentiment = "🟢 BULLISH (risk-on)"
            elif avg_change < -0.5:
                sentiment = "🔴 BEARISH (risk-off)"
            else:
                sentiment = "⚪ NEUTRAL (flat overnight)"
            lines.append(f"- Market Sentiment: {sentiment}")

        # Individual ticker pre-market
        if ticker and ticker in results:
            ticker_data = results[ticker]
            if "error" not in ticker_data:
                change_pct = ticker_data["change_pct"]
                direction = "📈" if change_pct > 0 else "📉"

                # Gap detection
                gap_type = ""
                if abs(change_pct) > 2.0:
                    gap_type = " (GAP UP!)" if change_pct > 0 else " (GAP DOWN!)"
                elif abs(change_pct) > 1.0:
                    gap_type = " (moderate gap)"

                premarket_note = " [Pre-market]" if ticker_data.get("is_premarket") else ""

                lines.append("")
                lines.append(
                    f"**{ticker}**: ${ticker_data['price']:.2f} "
                    f"({direction} {change_pct:+.2f}%{gap_type}){premarket_note}"
                )
                lines.append(f"  Previous close: ${ticker_data['prev_close']:.2f}")

        return "\n".join(lines)

    except ImportError:
        return "Pre-market snapshot: yfinance not installed"
    except Exception as e:
        logger.error("Pre-market snapshot failed: %s", e)
        return f"Pre-market snapshot: Error - {e}"


async def gap_analysis(ticker: str) -> dict[str, float | bool]:
    """Analyze if a ticker is gapping up or down at market open.

    Returns:
    {
        "prev_close": 255.78,
        "current_price": 258.45,
        "gap_pct": 1.04,
        "is_gap_up": True,
        "is_significant": True,  # >2% move
    }
    """
    try:
        import yfinance as yf
        import asyncio

        def _fetch():
            stock = yf.Ticker(ticker)
            hist = stock.history(period="5d", interval="1d")

            if hist.empty:
                return {"error": "No historical data"}

            prev_close = hist["Close"].iloc[-1]

            # Try to get pre-market or current price
            info = stock.info
            current = info.get("preMarketPrice") or info.get("regularMarketPrice") or prev_close

            gap_pct = ((current - prev_close) / prev_close) * 100

            return {
                "prev_close": float(prev_close),
                "current_price": float(current),
                "gap_pct": float(gap_pct),
                "is_gap_up": gap_pct > 0.5,
                "is_gap_down": gap_pct < -0.5,
                "is_significant": abs(gap_pct) > 2.0,
            }

        return await asyncio.get_event_loop().run_in_executor(None, _fetch)

    except Exception as e:
        logger.error("Gap analysis failed for %s: %s", ticker, e)
        return {"error": str(e)}
