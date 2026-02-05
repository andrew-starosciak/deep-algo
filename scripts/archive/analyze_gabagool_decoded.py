#!/usr/bin/env python3
"""
Analyze decoded gabagool trades to identify trading patterns and edges.

Usage:
    python3 analyze_gabagool_decoded.py /tmp/gabagool_decoded.json
"""

import json
import argparse
from pathlib import Path
from collections import defaultdict
from datetime import datetime, timezone
from typing import List, Dict, Any


def load_trades(filepath: Path) -> List[Dict[str, Any]]:
    """Load decoded trades from JSON."""
    with open(filepath) as f:
        return json.load(f)


def analyze_price_distribution(trades: List[Dict]) -> Dict:
    """Analyze price distribution."""
    valid = [t for t in trades if 0 < t["price"] <= 1]
    prices = [t["price"] for t in valid]

    buckets = {
        "< $0.10": (0, 0.10),
        "$0.10-0.20": (0.10, 0.20),
        "$0.20-0.30": (0.20, 0.30),
        "$0.30-0.35": (0.30, 0.35),
        "$0.35-0.42": (0.35, 0.42),
        "$0.42-0.48": (0.42, 0.48),
        "$0.48-0.55": (0.48, 0.55),
        "$0.55-0.65": (0.55, 0.65),
        "$0.65-0.80": (0.65, 0.80),
        "> $0.80": (0.80, 1.01),
    }

    distribution = {}
    for name, (lo, hi) in buckets.items():
        count = len([p for p in prices if lo <= p < hi])
        distribution[name] = {
            "count": count,
            "pct": 100 * count / len(valid) if valid else 0
        }

    return {
        "total_valid": len(valid),
        "min": min(prices) if prices else 0,
        "max": max(prices) if prices else 0,
        "mean": sum(prices) / len(prices) if prices else 0,
        "distribution": distribution
    }


def analyze_by_side(trades: List[Dict]) -> Dict:
    """Analyze BUY vs SELL patterns."""
    valid = [t for t in trades if 0 < t["price"] <= 1]
    buys = [t for t in valid if t["side"] == "BUY"]
    sells = [t for t in valid if t["side"] == "SELL"]

    def stats(lst):
        prices = [t["price"] for t in lst]
        usdc = [t["usdc"] for t in lst]
        return {
            "count": len(lst),
            "pct": 100 * len(lst) / len(valid) if valid else 0,
            "avg_price": sum(prices) / len(prices) if prices else 0,
            "min_price": min(prices) if prices else 0,
            "max_price": max(prices) if prices else 0,
            "total_usdc": sum(usdc),
            "avg_usdc": sum(usdc) / len(usdc) if usdc else 0
        }

    return {
        "BUY": stats(buys),
        "SELL": stats(sells)
    }


def analyze_by_token(trades: List[Dict]) -> Dict:
    """Analyze trading patterns by token."""
    valid = [t for t in trades if 0 < t["price"] <= 1]
    by_token = defaultdict(list)
    for t in valid:
        by_token[t["token_id"]].append(t)

    token_stats = []
    for token_id, token_trades in by_token.items():
        buys = [t for t in token_trades if t["side"] == "BUY"]
        sells = [t for t in token_trades if t["side"] == "SELL"]

        buy_prices = [t["price"] for t in buys]
        sell_prices = [t["price"] for t in sells]

        stat = {
            "token_id": token_id[:24] + "...",
            "token_full": token_id,
            "total_trades": len(token_trades),
            "buy_count": len(buys),
            "sell_count": len(sells),
            "buy_avg_price": sum(buy_prices) / len(buy_prices) if buy_prices else None,
            "sell_avg_price": sum(sell_prices) / len(sell_prices) if sell_prices else None,
            "total_usdc": sum(t["usdc"] for t in token_trades),
            "is_market_making": len(buys) > 0 and len(sells) > 0
        }

        # Calculate spread if both sides exist
        if stat["buy_avg_price"] and stat["sell_avg_price"]:
            stat["spread"] = stat["sell_avg_price"] - stat["buy_avg_price"]

        token_stats.append(stat)

    # Sort by trade count
    token_stats.sort(key=lambda x: -x["total_trades"])

    return {
        "unique_tokens": len(by_token),
        "market_making_tokens": len([s for s in token_stats if s["is_market_making"]]),
        "top_tokens": token_stats[:20]
    }


def analyze_trade_sizes(trades: List[Dict]) -> Dict:
    """Analyze trade size distribution."""
    valid = [t for t in trades if t["usdc"] > 0]
    usdc_amounts = [t["usdc"] for t in valid]

    buckets = {
        "< $5": (0, 5),
        "$5-10": (5, 10),
        "$10-20": (10, 20),
        "$20-50": (20, 50),
        "$50-100": (50, 100),
        "$100-250": (100, 250),
        "$250-500": (250, 500),
        "$500-1K": (500, 1000),
        "> $1K": (1000, float("inf")),
    }

    distribution = {}
    for name, (lo, hi) in buckets.items():
        trades_in_bucket = [t for t in valid if lo <= t["usdc"] < hi]
        count = len(trades_in_bucket)
        distribution[name] = {
            "count": count,
            "pct": 100 * count / len(valid) if valid else 0,
            "total_usdc": sum(t["usdc"] for t in trades_in_bucket)
        }

    return {
        "total_usdc": sum(usdc_amounts),
        "total_trades": len(valid),
        "avg_trade_size": sum(usdc_amounts) / len(usdc_amounts) if usdc_amounts else 0,
        "min_trade": min(usdc_amounts) if usdc_amounts else 0,
        "max_trade": max(usdc_amounts) if usdc_amounts else 0,
        "distribution": distribution
    }


def analyze_timing(trades: List[Dict]) -> Dict:
    """Analyze trade timing patterns."""
    valid = sorted(trades, key=lambda t: t["timestamp"])

    if len(valid) < 2:
        return {}

    timestamps = [t["timestamp"] for t in valid]
    intervals = [timestamps[i+1] - timestamps[i] for i in range(len(timestamps)-1)]

    # Group by 15-minute windows
    by_window = defaultdict(list)
    for t in valid:
        window_start = (t["timestamp"] // 900) * 900
        by_window[window_start].append(t)

    window_stats = []
    for ws, window_trades in sorted(by_window.items()):
        dt = datetime.fromtimestamp(ws, tz=timezone.utc)
        window_stats.append({
            "window_start": ws,
            "datetime": dt.strftime("%Y-%m-%d %H:%M UTC"),
            "trade_count": len(window_trades),
            "total_usdc": sum(t["usdc"] for t in window_trades),
            "buy_count": len([t for t in window_trades if t["side"] == "BUY"]),
            "sell_count": len([t for t in window_trades if t["side"] == "SELL"])
        })

    return {
        "total_duration_secs": timestamps[-1] - timestamps[0],
        "trades_per_minute": len(valid) / ((timestamps[-1] - timestamps[0]) / 60) if timestamps[-1] > timestamps[0] else 0,
        "interval_stats": {
            "min": min(intervals),
            "max": max(intervals),
            "mean": sum(intervals) / len(intervals),
            "median": sorted(intervals)[len(intervals)//2]
        },
        "windows": window_stats
    }


def analyze_edge_opportunities(trades: List[Dict]) -> Dict:
    """Analyze potential edge in trades."""
    valid = [t for t in trades if 0 < t["price"] <= 1]
    buys = [t for t in valid if t["side"] == "BUY"]

    # Categorize buys by our detection thresholds
    cheap_buys = [t for t in buys if t["price"] < 0.35]  # Conservative
    mid_cheap = [t for t in buys if 0.35 <= t["price"] < 0.42]  # Default threshold
    mid_buys = [t for t in buys if 0.42 <= t["price"] < 0.48]  # Aggressive
    near_fair = [t for t in buys if 0.48 <= t["price"] < 0.55]
    expensive = [t for t in buys if t["price"] >= 0.55]

    def bucket_stats(lst, name):
        if not lst:
            return {"name": name, "count": 0, "pct": 0, "usdc": 0}
        return {
            "name": name,
            "count": len(lst),
            "pct": 100 * len(lst) / len(buys) if buys else 0,
            "usdc": sum(t["usdc"] for t in lst),
            "avg_price": sum(t["price"] for t in lst) / len(lst)
        }

    return {
        "total_buys": len(buys),
        "categories": [
            bucket_stats(cheap_buys, "< $0.35 (conservative catches)"),
            bucket_stats(mid_cheap, "$0.35-0.42 (default catches)"),
            bucket_stats(mid_buys, "$0.42-0.48 (aggressive catches)"),
            bucket_stats(near_fair, "$0.48-0.55 (near fair value)"),
            bucket_stats(expensive, "> $0.55 (expensive side)"),
        ],
        "our_bot_would_catch": {
            "conservative": len(cheap_buys),
            "default": len(cheap_buys) + len(mid_cheap),
            "aggressive": len(cheap_buys) + len(mid_cheap) + len(mid_buys),
        }
    }


def print_report(analysis: Dict):
    """Print formatted analysis report."""
    print("=" * 70)
    print("GABAGOOL TRADING ANALYSIS REPORT")
    print("=" * 70)

    # Price distribution
    print("\n1. PRICE DISTRIBUTION")
    print("-" * 40)
    pd = analysis["price_distribution"]
    print(f"Valid trades: {pd['total_valid']}")
    print(f"Price range:  ${pd['min']:.4f} - ${pd['max']:.4f}")
    print(f"Mean price:   ${pd['mean']:.4f}")
    print("\nDistribution:")
    for name, data in pd["distribution"].items():
        if data["count"] > 0:
            bar = "#" * int(data["pct"] / 2)
            print(f"  {name:15s}: {data['count']:4d} ({data['pct']:5.1f}%) {bar}")

    # Side analysis
    print("\n2. BUY vs SELL")
    print("-" * 40)
    for side, data in analysis["by_side"].items():
        print(f"{side}:")
        print(f"  Count:     {data['count']} ({data['pct']:.1f}%)")
        print(f"  Avg price: ${data['avg_price']:.4f}")
        print(f"  Total USD: ${data['total_usdc']:,.2f}")

    # Trade sizes
    print("\n3. TRADE SIZES")
    print("-" * 40)
    ts = analysis["trade_sizes"]
    print(f"Total volume: ${ts['total_usdc']:,.2f}")
    print(f"Avg trade:    ${ts['avg_trade_size']:.2f}")
    print(f"Range:        ${ts['min_trade']:.2f} - ${ts['max_trade']:.2f}")
    print("\nSize distribution:")
    for name, data in ts["distribution"].items():
        if data["count"] > 0:
            print(f"  {name:10s}: {data['count']:4d} ({data['pct']:5.1f}%) = ${data['total_usdc']:,.2f}")

    # Token analysis
    print("\n4. TOKEN ANALYSIS")
    print("-" * 40)
    ta = analysis["by_token"]
    print(f"Unique tokens:        {ta['unique_tokens']}")
    print(f"Market-making tokens: {ta['market_making_tokens']}")
    print("\nTop 10 tokens by volume:")
    for t in ta["top_tokens"][:10]:
        mm = " [MM]" if t["is_market_making"] else ""
        print(f"  {t['token_id']}{mm}")
        print(f"    Trades: {t['total_trades']} (B:{t['buy_count']}, S:{t['sell_count']}) | ${t['total_usdc']:.2f}")
        if t.get("spread"):
            print(f"    Spread: ${t['spread']:.4f}")

    # Timing
    print("\n5. TIMING ANALYSIS")
    print("-" * 40)
    tm = analysis["timing"]
    if tm:
        print(f"Duration:          {tm['total_duration_secs']/60:.1f} minutes")
        print(f"Trades per minute: {tm['trades_per_minute']:.1f}")
        print(f"\nInter-trade intervals:")
        print(f"  Min:    {tm['interval_stats']['min']:.1f}s")
        print(f"  Median: {tm['interval_stats']['median']:.1f}s")
        print(f"  Mean:   {tm['interval_stats']['mean']:.1f}s")
        print(f"  Max:    {tm['interval_stats']['max']:.1f}s")

        if tm.get("windows"):
            print("\nBy 15-min window:")
            for w in tm["windows"]:
                print(f"  {w['datetime']}: {w['trade_count']} trades, ${w['total_usdc']:.2f}")

    # Edge analysis
    print("\n6. EDGE OPPORTUNITY ANALYSIS")
    print("-" * 40)
    ea = analysis["edge"]
    print(f"Total BUY trades: {ea['total_buys']}")
    print("\nBuy price categories:")
    for cat in ea["categories"]:
        if cat["count"] > 0:
            print(f"  {cat['name']}")
            print(f"    {cat['count']} trades ({cat['pct']:.1f}%) = ${cat['usdc']:.2f}")

    print("\nOur bot would catch:")
    for mode, count in ea["our_bot_would_catch"].items():
        pct = 100 * count / ea["total_buys"] if ea["total_buys"] else 0
        print(f"  {mode:12s}: {count} trades ({pct:.1f}%)")

    print("\n" + "=" * 70)
    print("RECOMMENDATIONS")
    print("=" * 70)

    # Generate recommendations based on analysis
    if ea["total_buys"] > 0:
        aggressive_catch = ea["our_bot_would_catch"]["aggressive"]
        aggressive_pct = 100 * aggressive_catch / ea["total_buys"]
        cheap_pct = ea["categories"][0]["pct"] + ea["categories"][1]["pct"]

        if aggressive_pct < 50:
            print(f"- Aggressive mode only catches {aggressive_pct:.0f}% of gabagool's buys")
            print("- Consider: gabagool may be doing directional trading, not just cheap-side arb")

        if cheap_pct > 30:
            print(f"- {cheap_pct:.0f}% of buys are at <$0.42 - clear cheap-side opportunities exist")

        if ta["market_making_tokens"] > ta["unique_tokens"] * 0.2:
            print(f"- {ta['market_making_tokens']} tokens have both BUY+SELL - possible market making")


def main():
    parser = argparse.ArgumentParser(description="Analyze decoded gabagool trades")
    parser.add_argument("input", help="Input JSON file with decoded trades")
    parser.add_argument("--json", help="Output analysis as JSON to file")
    args = parser.parse_args()

    trades = load_trades(Path(args.input))
    print(f"Loaded {len(trades)} trades from {args.input}")

    analysis = {
        "price_distribution": analyze_price_distribution(trades),
        "by_side": analyze_by_side(trades),
        "by_token": analyze_by_token(trades),
        "trade_sizes": analyze_trade_sizes(trades),
        "timing": analyze_timing(trades),
        "edge": analyze_edge_opportunities(trades)
    }

    print_report(analysis)

    if args.json:
        # Remove non-serializable parts for JSON output
        output = analysis.copy()
        with open(args.json, "w") as f:
            json.dump(output, f, indent=2, default=str)
        print(f"\nAnalysis saved to {args.json}")


if __name__ == "__main__":
    main()
