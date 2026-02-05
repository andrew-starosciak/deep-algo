#!/usr/bin/env python3
"""
Debug why only ~10% of transactions are being decoded.
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
        data = json.loads(result.stdout)
        return data.get("result"), data.get("error")
    return None, f"curl failed: {result.returncode}"


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print("Debugging first 20 transactions...\n")

    stats = {
        "fetch_success": 0,
        "fetch_fail": 0,
        "no_input": 0,
        "wrong_selector": 0,
        "decode_success": 0,
        "decode_fail": 0,
    }

    for i, row in enumerate(rows[:20]):
        tx_hash = row["Transaction Hash"]
        print(f"\n=== TX {i+1}: {tx_hash[:20]}... ===")

        tx_data, error = fetch_tx(tx_hash)

        if error:
            print(f"  FETCH ERROR: {error}")
            stats["fetch_fail"] += 1
            continue

        if not tx_data:
            print(f"  FETCH RESULT: None (tx not found?)")
            stats["fetch_fail"] += 1
            continue

        stats["fetch_success"] += 1
        print(f"  FETCH: OK")

        input_data = tx_data.get("input", "")
        if not input_data:
            print(f"  INPUT: Missing")
            stats["no_input"] += 1
            continue

        print(f"  INPUT: {len(input_data)} bytes")
        print(f"  SELECTOR: {input_data[:10]}")

        if not input_data.startswith(MATCH_ORDERS_SELECTOR):
            print(f"  SELECTOR MISMATCH! Expected {MATCH_ORDERS_SELECTOR}")
            stats["wrong_selector"] += 1
            continue

        # Try to decode
        try:
            data = bytes.fromhex(input_data[2:])[4:]
            taker_order_offset = int.from_bytes(data[0:32], 'big')
            maker_amount = int.from_bytes(data[taker_order_offset + 160:taker_order_offset + 192], 'big')
            taker_amount = int.from_bytes(data[taker_order_offset + 192:taker_order_offset + 224], 'big')
            side = int.from_bytes(data[taker_order_offset + 320:taker_order_offset + 352], 'big')

            if maker_amount == 0 or taker_amount == 0:
                print(f"  DECODE: Zero amounts (maker={maker_amount}, taker={taker_amount})")
                stats["decode_fail"] += 1
                continue

            # Calculate price
            if side == 0:  # BUY
                price = (maker_amount / 1_000_000) / (taker_amount / 1_000_000)
            else:  # SELL
                price = (taker_amount / 1_000_000) / (maker_amount / 1_000_000)

            print(f"  DECODE: side={side}, maker={maker_amount}, taker={taker_amount}")
            print(f"  PRICE: ${price:.6f} {'✓' if 0 < price <= 1 else '✗ INVALID'}")

            if 0 < price <= 1:
                stats["decode_success"] += 1
            else:
                stats["decode_fail"] += 1

        except Exception as e:
            print(f"  DECODE ERROR: {e}")
            stats["decode_fail"] += 1

        time.sleep(0.05)

    print("\n" + "="*50)
    print("SUMMARY:")
    for k, v in stats.items():
        print(f"  {k}: {v}")


if __name__ == "__main__":
    main()
