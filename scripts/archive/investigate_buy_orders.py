#!/usr/bin/env python3
"""
Deep investigation of BUY order structure to find correct price calculation.
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


def decode_bytes32(data: bytes, offset: int) -> str:
    return "0x" + data[offset:offset+32].hex()


def analyze_buy_order(input_data: str, tx_hash: str):
    """Deep analysis of a BUY order."""
    data = bytes.fromhex(input_data[2:])[4:]
    taker_order_offset = decode_uint256(data, 0)
    maker_orders_offset = decode_uint256(data, 32)
    taker_fill_amount = decode_uint256(data, 64)

    # Taker order fields
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    if side != 0:
        return None  # Not a BUY

    print(f"\n{'='*60}")
    print(f"TX: {tx_hash}")
    print(f"{'='*60}")
    print(f"\nTAKER ORDER (gabagool as taker):")
    print(f"  side: {side} (BUY)")
    print(f"  maker_amount: {maker_amount:,} ({maker_amount/1e6:.6f})")
    print(f"  taker_amount: {taker_amount:,} ({taker_amount/1e6:.6f})")
    print(f"  taker_fill_amount: {taker_fill_amount:,} ({taker_fill_amount/1e6:.6f})")

    # Try different price interpretations
    p1 = (taker_amount / maker_amount) if maker_amount > 0 else 0
    p2 = (maker_amount / taker_amount) if taker_amount > 0 else 0

    print(f"\nPRICE INTERPRETATIONS:")
    print(f"  taker_amount/maker_amount = {p1:.6f} {'✓' if 0 < p1 <= 1 else '✗'}")
    print(f"  maker_amount/taker_amount = {p2:.6f} {'✓' if 0 < p2 <= 1 else '✗'}")

    # Look at maker orders for more context
    num_makers = decode_uint256(data, maker_orders_offset)
    print(f"\nMAKER ORDERS ({num_makers} orders):")

    for m in range(min(num_makers, 3)):  # Show first 3 makers
        ptr_offset = maker_orders_offset + 32 + m * 32
        maker_ptr = decode_uint256(data, ptr_offset)
        actual_offset = maker_orders_offset + maker_ptr

        m_maker_amount = decode_uint256(data, actual_offset + 160)
        m_taker_amount = decode_uint256(data, actual_offset + 192)
        m_side = decode_uint256(data, actual_offset + 320)

        print(f"\n  Maker {m}:")
        print(f"    side: {m_side} ({'BUY' if m_side == 0 else 'SELL'})")
        print(f"    maker_amount: {m_maker_amount:,} ({m_maker_amount/1e6:.6f})")
        print(f"    taker_amount: {m_taker_amount:,} ({m_taker_amount/1e6:.6f})")

        mp1 = (m_taker_amount / m_maker_amount) if m_maker_amount > 0 else 0
        mp2 = (m_maker_amount / m_taker_amount) if m_taker_amount > 0 else 0
        print(f"    price (t/m): {mp1:.6f} {'✓' if 0 < mp1 <= 1 else '✗'}")
        print(f"    price (m/t): {mp2:.6f} {'✓' if 0 < mp2 <= 1 else '✗'}")

    # Analysis
    print(f"\nANALYSIS:")
    print(f"  If gabagool BUYs shares:")
    print(f"    - gabagool gives USDC, receives shares")
    print(f"    - If maker_amount = shares gabagool gets: {maker_amount/1e6:.4f} shares")
    print(f"    - If taker_amount = USDC gabagool pays: ${taker_amount/1e6:.4f}")
    print(f"    - Price would be: ${taker_amount/maker_amount:.6f}/share")

    print(f"\n  If amounts are inverted for BUY:")
    print(f"    - If taker_amount = shares gabagool gets: {taker_amount/1e6:.4f} shares")
    print(f"    - If maker_amount = USDC gabagool pays: ${maker_amount/1e6:.4f}")
    print(f"    - Price would be: ${maker_amount/taker_amount:.6f}/share")

    return {
        "maker_amount": maker_amount,
        "taker_amount": taker_amount,
        "p1": p1,
        "p2": p2
    }


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print("Investigating BUY order structure...\n")

    buy_count = 0
    for row in rows[:200]:
        if buy_count >= 5:
            break

        tx_data = fetch_tx(row["Transaction Hash"])
        if not tx_data or not tx_data.get("input"):
            continue

        input_data = tx_data["input"]
        if not input_data.startswith(MATCH_ORDERS_SELECTOR):
            continue

        # Check if BUY
        data = bytes.fromhex(input_data[2:])[4:]
        taker_order_offset = decode_uint256(data, 0)
        side = decode_uint256(data, taker_order_offset + 320)

        if side == 0:  # BUY
            buy_count += 1
            analyze_buy_order(input_data, row["Transaction Hash"])

        time.sleep(0.03)


if __name__ == "__main__":
    main()
