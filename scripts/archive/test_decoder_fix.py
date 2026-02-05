#!/usr/bin/env python3
"""
Quick test of the corrected decoder on a small sample.
"""

import json
import csv
import subprocess
from pathlib import Path
import time

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"
CTF_EXCHANGE = "0xe3f18acc864ea3905f63c7c2cf81f0ade6a8becd"


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


def decode_bytes32(data: bytes, offset: int) -> str:
    return "0x" + data[offset:offset+32].hex()


def decode_trade_v1_wrong(input_data: str) -> dict:
    """OLD WRONG decoder for comparison."""
    data = bytes.fromhex(input_data[2:])[4:]
    taker_order_offset = decode_uint256(data, 0)

    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    # OLD WRONG interpretation
    if side == 0:  # BUY
        usdc = taker_amount / 1_000_000
        shares = maker_amount / 1_000_000
    else:  # SELL - THIS WAS WRONG
        shares = taker_amount / 1_000_000  # WRONG
        usdc = maker_amount / 1_000_000    # WRONG

    price = usdc / shares if shares > 0 else 0
    return {
        "side": "BUY" if side == 0 else "SELL",
        "price": price,
        "shares": shares,
        "usdc": usdc,
        "maker_amount": maker_amount,
        "taker_amount": taker_amount,
    }


def decode_trade_v2_correct(input_data: str) -> dict:
    """NEW CORRECT decoder."""
    data = bytes.fromhex(input_data[2:])[4:]
    taker_order_offset = decode_uint256(data, 0)

    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    # NEW CORRECT interpretation
    # For both BUY and SELL: maker_amount = shares, taker_amount relates to USDC
    # But the formula differs:
    # BUY:  taker pays USDC (taker_amount), receives shares (maker_amount)
    # SELL: taker sells shares (maker_amount), receives USDC (taker_amount)

    if side == 0:  # BUY
        shares = maker_amount / 1_000_000
        usdc = taker_amount / 1_000_000
    else:  # SELL - FIXED
        shares = maker_amount / 1_000_000  # FIXED: shares from maker_amount
        usdc = taker_amount / 1_000_000    # FIXED: usdc from taker_amount

    price = usdc / shares if shares > 0 else 0
    return {
        "side": "BUY" if side == 0 else "SELL",
        "price": price,
        "shares": shares,
        "usdc": usdc,
        "maker_amount": maker_amount,
        "taker_amount": taker_amount,
    }


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print(f"Testing decoder fix on first 50 transactions...\n")

    valid_v1 = 0
    valid_v2 = 0

    print(f"{'TX':<12} {'SIDE':<5} {'V1 (wrong)':<15} {'V2 (correct)':<15} {'V2 valid?':<10}")
    print("-" * 60)

    for i, row in enumerate(rows[:50]):
        tx_data = fetch_tx(row["Transaction Hash"])
        if not tx_data or not tx_data.get("input"):
            continue

        input_data = tx_data["input"]
        if not input_data.startswith(MATCH_ORDERS_SELECTOR):
            continue

        v1 = decode_trade_v1_wrong(input_data)
        v2 = decode_trade_v2_correct(input_data)

        v1_valid = 0 < v1["price"] <= 1
        v2_valid = 0 < v2["price"] <= 1

        if v1_valid: valid_v1 += 1
        if v2_valid: valid_v2 += 1

        tx_short = row["Transaction Hash"][:10] + ".."
        v1_str = f"${v1['price']:.4f}" if v1_valid else f"${v1['price']:.2f}*"
        v2_str = f"${v2['price']:.4f}" if v2_valid else f"${v2['price']:.2f}*"

        print(f"{tx_short} {v1['side']:<5} {v1_str:<15} {v2_str:<15} {'✓' if v2_valid else '✗'}")

        time.sleep(0.03)

    print("-" * 60)
    print(f"\nV1 (wrong) valid prices:   {valid_v1}/50")
    print(f"V2 (correct) valid prices: {valid_v2}/50")

    # Detailed breakdown for a few samples
    print("\n\n=== DETAILED COMPARISON FOR SELL TRADES ===")
    sell_count = 0
    for i, row in enumerate(rows[:100]):
        if sell_count >= 5:
            break

        tx_data = fetch_tx(row["Transaction Hash"])
        if not tx_data or not tx_data.get("input"):
            continue

        input_data = tx_data["input"]
        if not input_data.startswith(MATCH_ORDERS_SELECTOR):
            continue

        v2 = decode_trade_v2_correct(input_data)
        if v2["side"] != "SELL":
            continue

        sell_count += 1
        print(f"\nTX: {row['Transaction Hash'][:20]}...")
        print(f"  maker_amount (raw): {v2['maker_amount']:,}")
        print(f"  taker_amount (raw): {v2['taker_amount']:,}")
        print(f"  Decoded: SELL {v2['shares']:.4f} shares for ${v2['usdc']:.4f} USDC")
        print(f"  Price: ${v2['price']:.6f} {'✓ valid' if 0 < v2['price'] <= 1 else '✗ invalid'}")

        time.sleep(0.03)


if __name__ == "__main__":
    main()
