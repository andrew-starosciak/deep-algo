#!/usr/bin/env python3
"""
Fetch and analyze multiple gabagool transactions to identify trading patterns.
"""

import json
import csv
import subprocess
from pathlib import Path
from dataclasses import dataclass
from typing import Optional, List, Dict
from collections import defaultdict
import time

# Polygon RPC endpoint
RPC_URL = "https://polygon-rpc.com"

# matchOrders function selector
MATCH_ORDERS_SELECTOR = "0x2287e350"

@dataclass
class Trade:
    """Simplified trade representation."""
    tx_hash: str
    timestamp: int
    token_id: str
    side: str  # BUY or SELL
    price: float
    quantity: float
    usdc_amount: float
    contract: str  # CTF or NegRisk


def fetch_tx(tx_hash: str) -> Optional[dict]:
    """Fetch transaction data from Polygon RPC."""
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
        data = json.loads(result.stdout)
        return data.get("result")
    return None


def decode_uint256(data: bytes, offset: int) -> int:
    """Decode a uint256 from bytes at offset."""
    return int.from_bytes(data[offset:offset+32], 'big')


def decode_address(data: bytes, offset: int) -> str:
    """Decode an address from bytes at offset."""
    return "0x" + data[offset+12:offset+32].hex()


def decode_bytes32(data: bytes, offset: int) -> str:
    """Decode bytes32 as hex string."""
    return "0x" + data[offset:offset+32].hex()


def decode_trade(input_data: str, tx_hash: str, timestamp: int, contract: str) -> Optional[Trade]:
    """Decode a matchOrders call into a Trade."""
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    data = bytes.fromhex(input_data[2:])
    data = data[4:]  # Skip selector

    # Get taker order offset
    taker_order_offset = decode_uint256(data, 0)
    taker_fill_amount = decode_uint256(data, 64)

    # Decode taker order fields at offset
    # Order struct: salt, maker, signer, taker, tokenId, makerAmount, takerAmount, ...
    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)  # shares
    taker_amount = decode_uint256(data, taker_order_offset + 192)  # USDC
    side = decode_uint256(data, taker_order_offset + 320)

    # Calculate price
    if maker_amount == 0:
        return None
    price = taker_amount / maker_amount

    # Convert from micro-units
    quantity = maker_amount / 1_000_000
    usdc_amount = taker_amount / 1_000_000

    return Trade(
        tx_hash=tx_hash,
        timestamp=timestamp,
        token_id=token_id,
        side="BUY" if side == 0 else "SELL",
        price=price,
        quantity=quantity,
        usdc_amount=usdc_amount,
        contract="CTF" if "e3f18acc" in contract.lower() else "NegRisk"
    )


def main():
    # Read CSV
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        reader = csv.DictReader(f)
        rows = list(reader)

    print(f"Total transactions in CSV: {len(rows)}")
    print()

    # Sample transactions to fetch (every 100th to get a good spread)
    sample_indices = list(range(0, min(500, len(rows)), 10))  # First 50 samples
    print(f"Fetching {len(sample_indices)} sample transactions...")

    trades = []
    token_ids = set()

    for i, idx in enumerate(sample_indices):
        row = rows[idx]
        tx_hash = row["Transaction Hash"]
        timestamp = int(row["UnixTimestamp"])
        contract = row["To"]

        if i % 10 == 0:
            print(f"  Progress: {i}/{len(sample_indices)} ({len(trades)} decoded)")

        tx_data = fetch_tx(tx_hash)
        if tx_data and tx_data.get("input"):
            trade = decode_trade(tx_data["input"], tx_hash, timestamp, contract)
            if trade:
                trades.append(trade)
                token_ids.add(trade.token_id)

        time.sleep(0.1)  # Rate limit

    print(f"\nDecoded {len(trades)} trades")
    print(f"Unique tokens: {len(token_ids)}")

    # Analyze by token
    print("\n=== TRADES BY TOKEN ===")
    by_token = defaultdict(list)
    for t in trades:
        by_token[t.token_id].append(t)

    for token_id, token_trades in sorted(by_token.items(), key=lambda x: -len(x[1])):
        buys = [t for t in token_trades if t.side == "BUY"]
        sells = [t for t in token_trades if t.side == "SELL"]

        avg_buy = sum(t.price for t in buys) / len(buys) if buys else 0
        avg_sell = sum(t.price for t in sells) / len(sells) if sells else 0
        total_qty = sum(t.quantity for t in token_trades)

        print(f"\nToken: {token_id[:20]}...")
        print(f"  Trades: {len(token_trades)} (BUY: {len(buys)}, SELL: {len(sells)})")
        print(f"  Avg BUY price:  ${avg_buy:.4f}" if buys else "  No buys")
        print(f"  Avg SELL price: ${avg_sell:.4f}" if sells else "  No sells")
        print(f"  Total quantity: {total_qty:.2f}")

    # Analyze patterns
    print("\n=== TRADE PATTERNS ===")

    # Price distribution
    all_prices = [t.price for t in trades]
    if all_prices:
        print(f"Price range: ${min(all_prices):.4f} - ${max(all_prices):.4f}")
        print(f"Avg price: ${sum(all_prices)/len(all_prices):.4f}")

    # Side distribution
    buys = [t for t in trades if t.side == "BUY"]
    sells = [t for t in trades if t.side == "SELL"]
    print(f"\nBUY trades: {len(buys)} ({100*len(buys)/len(trades):.1f}%)")
    print(f"SELL trades: {len(sells)} ({100*len(sells)/len(trades):.1f}%)")

    # Quantity distribution
    quantities = [t.quantity for t in trades]
    if quantities:
        print(f"\nQuantity range: {min(quantities):.2f} - {max(quantities):.2f}")
        print(f"Avg quantity: {sum(quantities)/len(quantities):.2f}")
        print(f"Total volume: {sum(quantities):.2f}")

    # USDC distribution
    usdc_amounts = [t.usdc_amount for t in trades]
    if usdc_amounts:
        print(f"\nUSDC per trade: ${min(usdc_amounts):.2f} - ${max(usdc_amounts):.2f}")
        print(f"Avg USDC: ${sum(usdc_amounts)/len(usdc_amounts):.2f}")
        print(f"Total USDC: ${sum(usdc_amounts):.2f}")

    # Contract distribution
    ctf = [t for t in trades if t.contract == "CTF"]
    negrisk = [t for t in trades if t.contract == "NegRisk"]
    print(f"\nCTF Exchange: {len(ctf)} trades")
    print(f"NegRisk Exchange: {len(negrisk)} trades")

    # Price buckets
    print("\n=== PRICE BUCKETS ===")
    buckets = {
        "< $0.20": [],
        "$0.20-0.30": [],
        "$0.30-0.40": [],
        "$0.40-0.50": [],
        "$0.50-0.60": [],
        "$0.60-0.70": [],
        "$0.70-0.80": [],
        "> $0.80": [],
    }
    for t in trades:
        p = t.price
        if p < 0.20:
            buckets["< $0.20"].append(t)
        elif p < 0.30:
            buckets["$0.20-0.30"].append(t)
        elif p < 0.40:
            buckets["$0.30-0.40"].append(t)
        elif p < 0.50:
            buckets["$0.40-0.50"].append(t)
        elif p < 0.60:
            buckets["$0.50-0.60"].append(t)
        elif p < 0.70:
            buckets["$0.60-0.70"].append(t)
        elif p < 0.80:
            buckets["$0.70-0.80"].append(t)
        else:
            buckets["> $0.80"].append(t)

    for bucket, bucket_trades in buckets.items():
        if bucket_trades:
            bucket_buys = len([t for t in bucket_trades if t.side == "BUY"])
            bucket_sells = len([t for t in bucket_trades if t.side == "SELL"])
            print(f"  {bucket}: {len(bucket_trades)} trades (BUY: {bucket_buys}, SELL: {bucket_sells})")

    # Export detailed trades
    output_file = Path("/home/a/Work/gambling/engine/specs/gabagool_decoded_trades.json")
    trades_data = [
        {
            "tx_hash": t.tx_hash,
            "timestamp": t.timestamp,
            "token_id": t.token_id,
            "side": t.side,
            "price": t.price,
            "quantity": t.quantity,
            "usdc_amount": t.usdc_amount,
            "contract": t.contract
        }
        for t in trades
    ]

    with open(output_file, "w") as f:
        json.dump(trades_data, f, indent=2)

    print(f"\nExported {len(trades_data)} trades to {output_file}")


if __name__ == "__main__":
    main()
