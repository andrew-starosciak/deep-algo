#!/usr/bin/env python3
"""
Verify the final corrected decoder works for both BUY and SELL.
"""

import json
import csv
import subprocess
from pathlib import Path
import time

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"


def fetch_tx(tx_hash: str):
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
        capture_output=True, text=True
    )
    if result.returncode == 0:
        return json.loads(result.stdout).get("result")
    return None


def decode_uint256(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset:offset+32], 'big')


def decode_trade_final(input_data: str) -> dict:
    """Final corrected decoder."""
    data = bytes.fromhex(input_data[2:])[4:]
    taker_order_offset = decode_uint256(data, 0)

    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    # FINAL CORRECTED formula:
    # BUY:  price = maker_amount / taker_amount
    # SELL: price = taker_amount / maker_amount
    if side == 0:  # BUY
        usdc = maker_amount / 1_000_000
        shares = taker_amount / 1_000_000
    else:  # SELL
        shares = maker_amount / 1_000_000
        usdc = taker_amount / 1_000_000

    price = usdc / shares if shares > 0 else 0

    return {
        "side": "BUY" if side == 0 else "SELL",
        "price": price,
        "shares": shares,
        "usdc": usdc,
    }


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print(f"Testing FINAL decoder on first 100 transactions...\n")

    buys_valid = 0
    buys_total = 0
    sells_valid = 0
    sells_total = 0

    results = []

    print(f"{'TX':<12} {'SIDE':<5} {'PRICE':<12} {'SHARES':<12} {'USDC':<12} {'VALID'}")
    print("-" * 70)

    for i, row in enumerate(rows[:100]):
        tx_data = fetch_tx(row["Transaction Hash"])
        if not tx_data or not tx_data.get("input"):
            continue

        input_data = tx_data["input"]
        if not input_data.startswith(MATCH_ORDERS_SELECTOR):
            continue

        t = decode_trade_final(input_data)
        is_valid = 0 < t["price"] <= 1

        if t["side"] == "BUY":
            buys_total += 1
            if is_valid: buys_valid += 1
        else:
            sells_total += 1
            if is_valid: sells_valid += 1

        results.append(t)

        tx_short = row["Transaction Hash"][:10] + ".."
        price_str = f"${t['price']:.6f}" if is_valid else f"${t['price']:.2f}*"
        print(f"{tx_short} {t['side']:<5} {price_str:<12} {t['shares']:<12.4f} ${t['usdc']:<11.2f} {'✓' if is_valid else '✗'}")

        time.sleep(0.02)

    print("-" * 70)
    print(f"\nSUMMARY:")
    print(f"  BUY trades:  {buys_valid}/{buys_total} valid ({100*buys_valid/buys_total:.1f}%)" if buys_total else "  No BUY trades")
    print(f"  SELL trades: {sells_valid}/{sells_total} valid ({100*sells_valid/sells_total:.1f}%)" if sells_total else "  No SELL trades")
    print(f"  TOTAL:       {buys_valid+sells_valid}/{buys_total+sells_total} valid ({100*(buys_valid+sells_valid)/(buys_total+sells_total):.1f}%)")

    # Show price distribution
    valid_prices = [r["price"] for r in results if 0 < r["price"] <= 1]
    if valid_prices:
        print(f"\nVALID PRICE DISTRIBUTION:")
        print(f"  Min:  ${min(valid_prices):.4f}")
        print(f"  Max:  ${max(valid_prices):.4f}")
        print(f"  Mean: ${sum(valid_prices)/len(valid_prices):.4f}")

        # Bucket
        cheap = len([p for p in valid_prices if p < 0.35])
        mid = len([p for p in valid_prices if 0.35 <= p < 0.65])
        expensive = len([p for p in valid_prices if p >= 0.65])
        print(f"\n  < $0.35 (cheap):     {cheap} trades")
        print(f"  $0.35-0.65 (mid):    {mid} trades")
        print(f"  >= $0.65 (expensive): {expensive} trades")


if __name__ == "__main__":
    main()
