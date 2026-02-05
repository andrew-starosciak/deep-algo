#!/usr/bin/env python3
"""
Quick analysis on first 500 gabagool transactions with corrected decoder.
"""

import json
import csv
import subprocess
from pathlib import Path
from collections import defaultdict
import time
from datetime import datetime, UTC

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"
CTF_EXCHANGE = "0xe3f18acc55091e2c48d883fc8c8413319d4ab7b0"


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


def decode_trade(input_data: str, tx_hash: str, timestamp: int, contract: str):
    """Decode with CORRECT price formula."""
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    data = bytes.fromhex(input_data[2:])[4:]
    taker_order_offset = decode_uint256(data, 0)

    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    if maker_amount == 0 or taker_amount == 0:
        return None

    # CORRECT formula
    if side == 0:  # BUY
        usdc = maker_amount / 1_000_000
        shares = taker_amount / 1_000_000
    else:  # SELL
        shares = maker_amount / 1_000_000
        usdc = taker_amount / 1_000_000

    price = usdc / shares if shares > 0 else 0

    # Skip invalid prices
    if price <= 0 or price > 1:
        return None

    is_ctf = CTF_EXCHANGE in contract.lower()

    return {
        "tx_hash": tx_hash,
        "timestamp": timestamp,
        "datetime": datetime.fromtimestamp(timestamp, UTC).strftime("%Y-%m-%d %H:%M:%S"),
        "token_id": token_id,
        "side": "BUY" if side == 0 else "SELL",
        "price": price,
        "shares": shares,
        "usdc": usdc,
        "contract": "CTF" if is_ctf else "NegRisk",
    }


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print(f"Total transactions: {len(rows)}")
    print(f"Analyzing first 500 with corrected decoder...\n")

    trades = []

    for i, row in enumerate(rows[:500]):
        if i % 50 == 0:
            print(f"  Progress: {i}/500 ({len(trades)} decoded)")

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

        time.sleep(0.015)

    print(f"\n{'='*60}")
    print(f"ANALYSIS RESULTS ({len(trades)} valid trades)")
    print(f"{'='*60}")

    # Price distribution
    prices = [t["price"] for t in trades]
    print(f"\n=== PRICE DISTRIBUTION ===")
    print(f"Range: ${min(prices):.4f} - ${max(prices):.4f}")
    print(f"Mean:  ${sum(prices)/len(prices):.4f}")

    cheap = [p for p in prices if p < 0.35]
    mid = [p for p in prices if 0.35 <= p < 0.65]
    expensive = [p for p in prices if p >= 0.65]

    print(f"\n  CHEAP (< $0.35):     {len(cheap):4d} ({100*len(cheap)/len(prices):5.1f}%)")
    print(f"  MID ($0.35-0.65):    {len(mid):4d} ({100*len(mid)/len(prices):5.1f}%)")
    print(f"  EXPENSIVE (>= $0.65): {len(expensive):4d} ({100*len(expensive)/len(prices):5.1f}%)")

    # Side distribution
    buys = [t for t in trades if t["side"] == "BUY"]
    sells = [t for t in trades if t["side"] == "SELL"]

    print(f"\n=== SIDE DISTRIBUTION ===")
    print(f"BUY:  {len(buys):4d} ({100*len(buys)/len(trades):5.1f}%)")
    print(f"SELL: {len(sells):4d} ({100*len(sells)/len(trades):5.1f}%)")

    # Price by side
    buy_prices = [t["price"] for t in buys]
    sell_prices = [t["price"] for t in sells]

    if buy_prices:
        print(f"\n  BUY prices:  ${min(buy_prices):.4f} - ${max(buy_prices):.4f} (avg ${sum(buy_prices)/len(buy_prices):.4f})")
    if sell_prices:
        print(f"  SELL prices: ${min(sell_prices):.4f} - ${max(sell_prices):.4f} (avg ${sum(sell_prices)/len(sell_prices):.4f})")

    # Contract distribution
    print(f"\n=== CONTRACT TYPE ===")
    ctf = [t for t in trades if t["contract"] == "CTF"]
    negrisk = [t for t in trades if t["contract"] == "NegRisk"]
    print(f"CTF:     {len(ctf):4d} ({100*len(ctf)/len(trades):5.1f}%)")
    print(f"NegRisk: {len(negrisk):4d} ({100*len(negrisk)/len(trades):5.1f}%)")

    # Volume
    print(f"\n=== VOLUME ===")
    total_usdc = sum(t["usdc"] for t in trades)
    total_shares = sum(t["shares"] for t in trades)
    print(f"Total USDC:   ${total_usdc:,.2f}")
    print(f"Total shares: {total_shares:,.2f}")
    print(f"Avg trade:    ${total_usdc/len(trades):.2f}")

    # Trade sizes
    print(f"\n=== TRADE SIZE DISTRIBUTION ===")
    usdc_vals = [t["usdc"] for t in trades]
    for threshold in [10, 50, 100, 500, 1000]:
        count = len([u for u in usdc_vals if u >= threshold])
        print(f"  >= ${threshold}: {count:4d} trades")

    # Token analysis
    print(f"\n=== TOP 10 TOKENS ===")
    by_token = defaultdict(list)
    for t in trades:
        by_token[t["token_id"]].append(t)

    sorted_tokens = sorted(by_token.items(), key=lambda x: -len(x[1]))[:10]
    for token_id, token_trades in sorted_tokens:
        t_buys = [t for t in token_trades if t["side"] == "BUY"]
        t_sells = [t for t in token_trades if t["side"] == "SELL"]
        total_vol = sum(t["usdc"] for t in token_trades)
        avg_price = sum(t["price"] for t in token_trades) / len(token_trades)

        print(f"\n  {token_id[:24]}...")
        print(f"    Trades: {len(token_trades):3d} (BUY: {len(t_buys)}, SELL: {len(t_sells)})")
        print(f"    Volume: ${total_vol:,.2f}")
        print(f"    Avg price: ${avg_price:.4f}")

    # Pattern: tokens with both BUY and SELL (potential arbitrage)
    arb_tokens = [(tk, tr) for tk, tr in by_token.items()
                  if any(t["side"] == "BUY" for t in tr) and any(t["side"] == "SELL" for t in tr)]

    print(f"\n=== TOKENS WITH BOTH BUY AND SELL (potential arb) ===")
    print(f"Count: {len(arb_tokens)} tokens")

    for token_id, token_trades in arb_tokens[:5]:
        buys = [t for t in token_trades if t["side"] == "BUY"]
        sells = [t for t in token_trades if t["side"] == "SELL"]
        avg_buy = sum(t["price"] for t in buys) / len(buys)
        avg_sell = sum(t["price"] for t in sells) / len(sells)

        print(f"\n  {token_id[:24]}...")
        print(f"    BUY:  {len(buys):2d} trades @ avg ${avg_buy:.4f}")
        print(f"    SELL: {len(sells):2d} trades @ avg ${avg_sell:.4f}")
        print(f"    Spread: ${avg_sell - avg_buy:+.4f}")

    # Time analysis
    print(f"\n=== TIME ANALYSIS ===")
    timestamps = [t["timestamp"] for t in trades]
    duration = max(timestamps) - min(timestamps)

    start_dt = datetime.fromtimestamp(min(timestamps), UTC)
    end_dt = datetime.fromtimestamp(max(timestamps), UTC)

    print(f"Period: {start_dt} to {end_dt}")
    print(f"Duration: {duration/60:.1f} minutes")
    print(f"Trades/min: {len(trades)/(duration/60):.1f}")

    # Export
    output_file = Path("/home/a/Work/gambling/engine/specs/gabagool_trades_corrected.json")
    with open(output_file, "w") as f:
        json.dump(trades, f, indent=2)
    print(f"\nExported to {output_file}")


if __name__ == "__main__":
    main()
