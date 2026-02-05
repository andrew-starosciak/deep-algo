#!/usr/bin/env python3
"""
Rate-limit-aware gabagool transaction decoder.

Slowly decodes all transactions respecting RPC rate limits.
Progress is checkpointed so it can be resumed if interrupted.

Usage:
    python3 decode_gabagool_ratelimit.py --csv /tmp/gabagool.csv --output /tmp/gabagool_decoded.json

    # Resume from checkpoint:
    python3 decode_gabagool_ratelimit.py --csv /tmp/gabagool.csv --output /tmp/gabagool_decoded.json --resume
"""

import json
import csv
import subprocess
import argparse
import time
import sys
from pathlib import Path
from datetime import datetime, timezone
from typing import Optional, Dict, List, Any

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"

# Rate limiting config
REQUESTS_PER_SECOND = 2  # Conservative rate
RETRY_DELAY = 5  # Seconds to wait after rate limit hit
MAX_RETRIES = 3


def fetch_tx(tx_hash: str, retries: int = 0) -> Optional[dict]:
    """Fetch transaction with retry logic."""
    payload = {
        "jsonrpc": "2.0",
        "method": "eth_getTransactionByHash",
        "params": [tx_hash],
        "id": 1
    }

    try:
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", RPC_URL,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True,
            text=True,
            timeout=10
        )

        if result.returncode == 0:
            data = json.loads(result.stdout)
            if data.get("result"):
                return data["result"]
            elif data.get("error"):
                # Rate limited or other error
                if retries < MAX_RETRIES:
                    time.sleep(RETRY_DELAY)
                    return fetch_tx(tx_hash, retries + 1)
    except Exception as e:
        if retries < MAX_RETRIES:
            time.sleep(RETRY_DELAY)
            return fetch_tx(tx_hash, retries + 1)

    return None


def decode_uint256(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset:offset+32], 'big')


def decode_bytes32(data: bytes, offset: int) -> str:
    return "0x" + data[offset:offset+32].hex()


def decode_trade(input_data: str, tx_hash: str, timestamp: int, to_addr: str) -> Optional[Dict[str, Any]]:
    """Decode matchOrders transaction."""
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    try:
        data = bytes.fromhex(input_data[2:])[4:]  # Remove 0x and selector

        taker_order_offset = decode_uint256(data, 0)
        taker_fill_amount = decode_uint256(data, 64)

        token_id = decode_bytes32(data, taker_order_offset + 128)
        maker_amount = decode_uint256(data, taker_order_offset + 160)
        taker_amount = decode_uint256(data, taker_order_offset + 192)
        side = decode_uint256(data, taker_order_offset + 320)

        if maker_amount == 0 or taker_amount == 0:
            return None

        # Price calculation based on side
        if side == 0:  # BUY
            usdc = maker_amount / 1_000_000
            shares = taker_amount / 1_000_000
        else:  # SELL
            shares = maker_amount / 1_000_000
            usdc = taker_amount / 1_000_000

        price = usdc / shares if shares > 0 else 0

        # Sanity check
        if price <= 0 or price > 2:
            return None

        dt = datetime.fromtimestamp(timestamp, tz=timezone.utc)
        contract = "CTF" if "e3f18acc" in to_addr.lower() else "NegRisk"

        return {
            "tx_hash": tx_hash,
            "timestamp": timestamp,
            "datetime": dt.strftime("%Y-%m-%d %H:%M:%S UTC"),
            "token_id": token_id,
            "side": "BUY" if side == 0 else "SELL",
            "price": round(price, 6),
            "shares": round(shares, 4),
            "usdc": round(usdc, 4),
            "contract": contract,
            "taker_fill_amount": taker_fill_amount
        }
    except Exception as e:
        return None


def load_checkpoint(checkpoint_file: Path) -> Dict[str, Any]:
    """Load checkpoint if exists."""
    if checkpoint_file.exists():
        with open(checkpoint_file) as f:
            return json.load(f)
    return {"processed_hashes": set(), "trades": [], "errors": 0, "last_index": 0}


def save_checkpoint(checkpoint_file: Path, state: Dict[str, Any]):
    """Save checkpoint."""
    # Convert set to list for JSON serialization
    state_copy = state.copy()
    state_copy["processed_hashes"] = list(state["processed_hashes"])
    with open(checkpoint_file, "w") as f:
        json.dump(state_copy, f)


def save_trades(output_file: Path, trades: List[Dict]):
    """Save decoded trades to JSON."""
    with open(output_file, "w") as f:
        json.dump(trades, f, indent=2)


def main():
    parser = argparse.ArgumentParser(description="Decode gabagool transactions with rate limiting")
    parser.add_argument("--csv", required=True, help="Input CSV file")
    parser.add_argument("--output", required=True, help="Output JSON file for decoded trades")
    parser.add_argument("--resume", action="store_true", help="Resume from checkpoint")
    parser.add_argument("--start-ts", type=int, help="Filter: start timestamp")
    parser.add_argument("--end-ts", type=int, help="Filter: end timestamp")
    parser.add_argument("--limit", type=int, help="Max transactions to process")
    parser.add_argument("--rate", type=float, default=2.0, help="Requests per second (default: 2)")
    args = parser.parse_args()

    csv_file = Path(args.csv)
    output_file = Path(args.output)
    checkpoint_file = output_file.with_suffix(".checkpoint.json")

    # Read CSV
    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    # Filter by timestamp if specified
    if args.start_ts or args.end_ts:
        filtered = []
        for r in rows:
            ts = int(r["UnixTimestamp"])
            if args.start_ts and ts < args.start_ts:
                continue
            if args.end_ts and ts > args.end_ts:
                continue
            filtered.append(r)
        rows = filtered

    print(f"CSV loaded: {len(rows)} transactions")

    # Load or initialize state
    if args.resume and checkpoint_file.exists():
        state = load_checkpoint(checkpoint_file)
        state["processed_hashes"] = set(state["processed_hashes"])
        print(f"Resuming from checkpoint: {len(state['trades'])} trades decoded, {state['errors']} errors")
    else:
        state = {"processed_hashes": set(), "trades": [], "errors": 0, "last_index": 0}

    # Calculate rate limiting delay
    delay = 1.0 / args.rate

    print(f"Rate: {args.rate} req/s (delay: {delay:.2f}s)")
    print(f"Starting decode loop... Press Ctrl+C to stop (progress is saved)")
    print()

    try:
        total = len(rows) if not args.limit else min(args.limit, len(rows))
        start_time = time.time()

        for i, row in enumerate(rows[:total]):
            tx_hash = row["Transaction Hash"]

            # Skip if already processed
            if tx_hash in state["processed_hashes"]:
                continue

            # Progress update
            if i % 50 == 0:
                elapsed = time.time() - start_time
                rate = (i - state["last_index"]) / elapsed if elapsed > 0 else 0
                print(f"[{i}/{total}] {len(state['trades'])} decoded, {state['errors']} errors, {rate:.1f} tx/s")

            # Fetch and decode
            tx_data = fetch_tx(tx_hash)

            if tx_data and tx_data.get("input"):
                trade = decode_trade(
                    tx_data["input"],
                    tx_hash,
                    int(row["UnixTimestamp"]),
                    row["To"]
                )
                if trade:
                    state["trades"].append(trade)
                else:
                    state["errors"] += 1
            else:
                state["errors"] += 1

            state["processed_hashes"].add(tx_hash)

            # Checkpoint every 100 transactions
            if i % 100 == 0 and i > 0:
                save_checkpoint(checkpoint_file, state)
                save_trades(output_file, state["trades"])

            # Rate limiting
            time.sleep(delay)

    except KeyboardInterrupt:
        print("\n\nInterrupted! Saving progress...")

    finally:
        # Final save
        save_checkpoint(checkpoint_file, state)
        save_trades(output_file, state["trades"])

        print(f"\n{'='*60}")
        print(f"FINAL STATUS")
        print(f"{'='*60}")
        print(f"Transactions processed: {len(state['processed_hashes'])}")
        print(f"Trades decoded:         {len(state['trades'])}")
        print(f"Errors/non-trades:      {state['errors']}")
        print(f"Output saved to:        {output_file}")
        print(f"Checkpoint saved to:    {checkpoint_file}")

        if state["trades"]:
            prices = [t["price"] for t in state["trades"] if 0 < t["price"] <= 1]
            if prices:
                print(f"\nQuick stats:")
                print(f"  Price range: ${min(prices):.4f} - ${max(prices):.4f}")
                print(f"  Avg price:   ${sum(prices)/len(prices):.4f}")
                buys = len([t for t in state["trades"] if t["side"] == "BUY"])
                print(f"  BUY/SELL:    {buys}/{len(state['trades'])-buys}")


if __name__ == "__main__":
    main()
