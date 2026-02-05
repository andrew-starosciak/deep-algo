#!/usr/bin/env python3
"""
Improved decoder for gabagool's matchOrders transactions.

Key insight: The order structure interpretation depends on the side:
- BUY (side=0): taker gives USDC, receives shares. Price = takerAmount/makerAmount
- SELL (side=1): taker gives shares, receives USDC. Price = makerAmount/takerAmount
"""

import json
import csv
import subprocess
from pathlib import Path
from dataclasses import dataclass
from typing import Optional, List, Dict
from collections import defaultdict
import time
from datetime import datetime

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"

# Known token mappings (will be populated as we find them)
TOKEN_NAMES = {
    # We'll try to identify these from Polymarket API
}

@dataclass
class Trade:
    """Decoded trade."""
    tx_hash: str
    timestamp: int
    datetime_str: str
    token_id: str
    side: str  # BUY or SELL (what gabagool did)
    price: float  # Price per share in USDC
    shares: float  # Number of shares
    usdc: float  # USDC amount
    contract: str  # CTF or NegRisk
    fill_amount: int  # Raw fill amount


def fetch_tx(tx_hash: str) -> Optional[dict]:
    """Fetch transaction from RPC."""
    payload = {
        "jsonrpc": "2.0",
        "method": "eth_getTransactionByHash",
        "params": [tx_hash],
        "id": 1
    }
    result = subprocess.run(
        ["curl", "-s", "-X", "POST", RPC_URL,
         "-H", "Content-Type: application/json",
         "-d", json.dumps(payload)],
        capture_output=True,
        text=True
    )
    if result.returncode == 0:
        return json.loads(result.stdout).get("result")
    return None


def decode_uint256(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset:offset+32], 'big')


def decode_bytes32(data: bytes, offset: int) -> str:
    return "0x" + data[offset:offset+32].hex()


def decode_trade(input_data: str, tx_hash: str, timestamp: int, contract: str) -> Optional[Trade]:
    """Decode matchOrders into a Trade."""
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    data = bytes.fromhex(input_data[2:])[4:]  # Remove 0x and selector

    # Parse header offsets
    taker_order_offset = decode_uint256(data, 0)
    maker_orders_offset = decode_uint256(data, 32)
    taker_fill_amount = decode_uint256(data, 64)

    # Parse taker order
    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    if maker_amount == 0 or taker_amount == 0:
        return None

    # Calculate price based on side
    # The taker is the one initiating (gabagool)
    # side=0 (BUY): taker gives USDC, maker gives shares
    #   price = USDC / shares = taker_amount / maker_amount
    # side=1 (SELL): taker gives shares, maker gives USDC
    #   price = USDC / shares = maker_amount / taker_amount

    if side == 0:  # BUY
        # taker pays USDC (taker_amount), receives shares (maker_amount)
        usdc = taker_amount / 1_000_000
        shares = maker_amount / 1_000_000
        price = usdc / shares if shares > 0 else 0
    else:  # SELL
        # taker gives shares (taker_amount), receives USDC (maker_amount)
        shares = taker_amount / 1_000_000
        usdc = maker_amount / 1_000_000
        price = usdc / shares if shares > 0 else 0

    # Sanity check - binary option prices should be 0-1
    # If price > 1, something is wrong with our interpretation
    # This might happen with different decimal handling

    dt = datetime.utcfromtimestamp(timestamp)

    return Trade(
        tx_hash=tx_hash,
        timestamp=timestamp,
        datetime_str=dt.strftime("%Y-%m-%d %H:%M:%S"),
        token_id=token_id,
        side="BUY" if side == 0 else "SELL",
        price=price,
        shares=shares,
        usdc=usdc,
        contract="CTF" if "e3f18acc" in contract.lower() else "NegRisk",
        fill_amount=taker_fill_amount
    )


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print(f"Total transactions: {len(rows)}")

    # Get first 100 transactions to analyze
    print("Fetching transactions...")

    trades = []
    for i, row in enumerate(rows[:100]):
        if i % 20 == 0:
            print(f"  {i}/100")

        tx_data = fetch_tx(row["Transaction Hash"])
        if tx_data and tx_data.get("input"):
            trade = decode_trade(
                tx_data["input"],
                row["Transaction Hash"],
                int(row["UnixTimestamp"]),
                row["To"]
            )
            if trade:
                trades.append(trade)
        time.sleep(0.05)

    print(f"\nDecoded {len(trades)} trades")

    # Display raw data for first few trades
    print("\n=== SAMPLE TRADES (RAW) ===")
    for t in trades[:10]:
        print(f"{t.datetime_str} | {t.side:4} | ${t.price:.6f} | {t.shares:.2f} shares | ${t.usdc:.2f} USDC | {t.contract}")
        print(f"  Token: {t.token_id[:24]}...")

    # Analyze price distribution
    print("\n=== PRICE ANALYSIS ===")
    valid_prices = [t.price for t in trades if 0 < t.price <= 1]
    high_prices = [t.price for t in trades if t.price > 1]
    zero_prices = [t.price for t in trades if t.price <= 0]

    print(f"Valid prices (0-1): {len(valid_prices)}")
    print(f"High prices (>1): {len(high_prices)}")
    print(f"Zero/negative: {len(zero_prices)}")

    if valid_prices:
        print(f"\nValid price range: ${min(valid_prices):.4f} - ${max(valid_prices):.4f}")
        print(f"Valid price avg: ${sum(valid_prices)/len(valid_prices):.4f}")

        # Bucket valid prices
        buckets = {
            "< $0.10": 0, "$0.10-0.20": 0, "$0.20-0.30": 0, "$0.30-0.40": 0,
            "$0.40-0.50": 0, "$0.50-0.60": 0, "$0.60-0.70": 0, "$0.70-0.80": 0,
            "$0.80-0.90": 0, "$0.90-1.00": 0
        }
        for p in valid_prices:
            if p < 0.10: buckets["< $0.10"] += 1
            elif p < 0.20: buckets["$0.10-0.20"] += 1
            elif p < 0.30: buckets["$0.20-0.30"] += 1
            elif p < 0.40: buckets["$0.30-0.40"] += 1
            elif p < 0.50: buckets["$0.40-0.50"] += 1
            elif p < 0.60: buckets["$0.50-0.60"] += 1
            elif p < 0.70: buckets["$0.60-0.70"] += 1
            elif p < 0.80: buckets["$0.70-0.80"] += 1
            elif p < 0.90: buckets["$0.80-0.90"] += 1
            else: buckets["$0.90-1.00"] += 1

        print("\nPrice distribution:")
        for bucket, count in buckets.items():
            if count > 0:
                pct = 100 * count / len(valid_prices)
                bar = "#" * int(pct / 2)
                print(f"  {bucket}: {count:3d} ({pct:5.1f}%) {bar}")

    # Side analysis
    print("\n=== SIDE ANALYSIS ===")
    buys = [t for t in trades if t.side == "BUY"]
    sells = [t for t in trades if t.side == "SELL"]
    print(f"BUY:  {len(buys):3d} trades")
    print(f"SELL: {len(sells):3d} trades")

    # Analyze by token
    print("\n=== BY TOKEN ===")
    by_token = defaultdict(list)
    for t in trades:
        by_token[t.token_id].append(t)

    for token_id, token_trades in sorted(by_token.items(), key=lambda x: -len(x[1]))[:10]:
        buys = [t for t in token_trades if t.side == "BUY"]
        sells = [t for t in token_trades if t.side == "SELL"]
        print(f"\n{token_id[:24]}...")
        print(f"  Trades: {len(token_trades)} (BUY: {len(buys)}, SELL: {len(sells)})")
        if buys:
            avg_buy = sum(t.price for t in buys) / len(buys)
            print(f"  Avg BUY:  ${avg_buy:.4f}" if avg_buy <= 1 else f"  Avg BUY:  ${avg_buy:.2f} (HIGH!)")
        if sells:
            avg_sell = sum(t.price for t in sells) / len(sells)
            print(f"  Avg SELL: ${avg_sell:.4f}" if avg_sell <= 1 else f"  Avg SELL: ${avg_sell:.2f} (HIGH!)")

    # Look for paired trades (same token, both BUY and SELL)
    print("\n=== PAIRED TOKENS (potential arb) ===")
    for token_id, token_trades in by_token.items():
        buys = [t for t in token_trades if t.side == "BUY"]
        sells = [t for t in token_trades if t.side == "SELL"]
        if buys and sells:
            avg_buy = sum(t.price for t in buys) / len(buys)
            avg_sell = sum(t.price for t in sells) / len(sells)
            if avg_buy <= 1 and avg_sell <= 1:
                print(f"  {token_id[:24]}...")
                print(f"    BUY {len(buys)} @ ${avg_buy:.4f}, SELL {len(sells)} @ ${avg_sell:.4f}")
                print(f"    Spread: ${avg_sell - avg_buy:.4f}")

    # Export
    output = Path("/home/a/Work/gambling/engine/specs/gabagool_trades_v2.json")
    with open(output, "w") as f:
        json.dump([{
            "tx": t.tx_hash,
            "time": t.datetime_str,
            "token": t.token_id,
            "side": t.side,
            "price": t.price,
            "shares": t.shares,
            "usdc": t.usdc,
            "contract": t.contract
        } for t in trades], f, indent=2)
    print(f"\nExported to {output}")


if __name__ == "__main__":
    main()
