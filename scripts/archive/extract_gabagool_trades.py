#!/usr/bin/env python3
"""
Corrected decoder for gabagool's matchOrders transactions.

Key fix: For SELL orders, the maker_amount and taker_amount interpretation was reversed.

Polymarket order struct semantics:
- BUY (side=0): taker pays USDC (taker_amount), receives shares (maker_amount)
  -> price = taker_amount / maker_amount
- SELL (side=1): taker sells shares (maker_amount), receives USDC (taker_amount)
  -> price = taker_amount / maker_amount (same formula!)
"""

import json
import csv
import subprocess
from pathlib import Path
from dataclasses import dataclass, asdict
from typing import Optional, List
from collections import defaultdict
import time
from datetime import datetime

RPC_URL = "https://polygon-rpc.com"
MATCH_ORDERS_SELECTOR = "0x2287e350"

# Contract addresses
CTF_EXCHANGE = "0xe3f18acc864ea3905f63c7c2cf81f0ade6a8becd"
NEGRISK_EXCHANGE = "0xb768891ea4e1e0bdfc6a7d7e3c987e4a0f3b3a1a"


@dataclass
class Trade:
    """Decoded trade with correct price calculation."""
    tx_hash: str
    block_number: int
    timestamp: int
    datetime_str: str
    token_id: str
    side: str  # BUY or SELL (from gabagool's perspective as taker)
    price: float  # Price per share (0-1 for binary options)
    shares: float  # Number of shares
    usdc: float  # USDC amount
    contract: str  # CTF or NegRisk
    maker_amount_raw: int
    taker_amount_raw: int
    taker_fill_amount: int


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


def decode_trade(input_data: str, tx_hash: str, block_number: int,
                 timestamp: int, contract_addr: str) -> Optional[Trade]:
    """Decode matchOrders into a Trade with CORRECTED price calculation."""
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    data = bytes.fromhex(input_data[2:])[4:]  # Remove 0x and selector

    # Parse header offsets
    taker_order_offset = decode_uint256(data, 0)
    taker_fill_amount = decode_uint256(data, 64)

    # Parse taker order fields
    # Order struct: salt(0), maker(32), signer(64), taker(96), tokenId(128),
    #               makerAmount(160), takerAmount(192), expiration(224),
    #               nonce(256), feeRateBps(288), side(320), signatureType(352)
    token_id = decode_bytes32(data, taker_order_offset + 128)
    maker_amount = decode_uint256(data, taker_order_offset + 160)
    taker_amount = decode_uint256(data, taker_order_offset + 192)
    side = decode_uint256(data, taker_order_offset + 320)

    if maker_amount == 0 or taker_amount == 0:
        return None

    # CORRECTED price calculation based on actual transaction analysis:
    # The formula is DIFFERENT for BUY vs SELL:
    #
    # BUY (side=0): gabagool buys shares
    #   - maker_amount = USDC paid
    #   - taker_amount = shares received
    #   - price = maker_amount / taker_amount = USDC / shares
    #
    # SELL (side=1): gabagool sells shares
    #   - maker_amount = shares sold
    #   - taker_amount = USDC received
    #   - price = taker_amount / maker_amount = USDC / shares

    if side == 0:  # BUY
        usdc = maker_amount / 1_000_000
        shares = taker_amount / 1_000_000
        price = usdc / shares if shares > 0 else 0
    else:  # SELL
        shares = maker_amount / 1_000_000
        usdc = taker_amount / 1_000_000
        price = usdc / shares if shares > 0 else 0

    # Determine contract type
    contract = "CTF" if CTF_EXCHANGE in contract_addr.lower() else "NegRisk"

    dt = datetime.utcfromtimestamp(timestamp)

    return Trade(
        tx_hash=tx_hash,
        block_number=block_number,
        timestamp=timestamp,
        datetime_str=dt.strftime("%Y-%m-%d %H:%M:%S"),
        token_id=token_id,
        side="BUY" if side == 0 else "SELL",
        price=price,
        shares=shares,
        usdc=usdc,
        contract=contract,
        maker_amount_raw=maker_amount,
        taker_amount_raw=taker_amount,
        taker_fill_amount=taker_fill_amount
    )


def analyze_trades(trades: List[Trade]):
    """Analyze trade patterns."""
    print(f"\n{'='*60}")
    print(f"GABAGOOL TRADE ANALYSIS ({len(trades)} trades)")
    print(f"{'='*60}")

    # Price validation
    valid_prices = [t for t in trades if 0 < t.price <= 1]
    invalid_prices = [t for t in trades if t.price > 1 or t.price <= 0]

    print(f"\n=== PRICE VALIDATION ===")
    print(f"Valid prices (0-1):   {len(valid_prices)} ({100*len(valid_prices)/len(trades):.1f}%)")
    print(f"Invalid prices (>1):  {len(invalid_prices)} ({100*len(invalid_prices)/len(trades):.1f}%)")

    if invalid_prices:
        print(f"\nSample invalid prices:")
        for t in invalid_prices[:5]:
            print(f"  {t.side} @ ${t.price:.4f} - {t.shares:.2f} shares, ${t.usdc:.2f} USDC")
            print(f"    raw: maker={t.maker_amount_raw}, taker={t.taker_amount_raw}")

    # Price distribution (valid only)
    if valid_prices:
        prices = [t.price for t in valid_prices]
        print(f"\n=== PRICE DISTRIBUTION (valid trades) ===")
        print(f"Range: ${min(prices):.4f} - ${max(prices):.4f}")
        print(f"Mean:  ${sum(prices)/len(prices):.4f}")

        buckets = {
            "< $0.10": 0, "$0.10-0.20": 0, "$0.20-0.30": 0, "$0.30-0.40": 0,
            "$0.40-0.50": 0, "$0.50-0.60": 0, "$0.60-0.70": 0, "$0.70-0.80": 0,
            "$0.80-0.90": 0, "$0.90-1.00": 0
        }
        for p in prices:
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

        print("\nDistribution:")
        for bucket, count in buckets.items():
            if count > 0:
                pct = 100 * count / len(valid_prices)
                bar = "#" * int(pct / 2)
                print(f"  {bucket}: {count:4d} ({pct:5.1f}%) {bar}")

    # Side analysis
    print(f"\n=== SIDE ANALYSIS ===")
    buys = [t for t in trades if t.side == "BUY"]
    sells = [t for t in trades if t.side == "SELL"]
    print(f"BUY:  {len(buys):4d} ({100*len(buys)/len(trades):.1f}%)")
    print(f"SELL: {len(sells):4d} ({100*len(sells)/len(trades):.1f}%)")

    # Price by side
    buy_prices = [t.price for t in buys if 0 < t.price <= 1]
    sell_prices = [t.price for t in sells if 0 < t.price <= 1]
    if buy_prices:
        print(f"\nBUY price range:  ${min(buy_prices):.4f} - ${max(buy_prices):.4f} (avg ${sum(buy_prices)/len(buy_prices):.4f})")
    if sell_prices:
        print(f"SELL price range: ${min(sell_prices):.4f} - ${max(sell_prices):.4f} (avg ${sum(sell_prices)/len(sell_prices):.4f})")

    # Contract analysis
    print(f"\n=== CONTRACT TYPE ===")
    ctf = [t for t in trades if t.contract == "CTF"]
    negrisk = [t for t in trades if t.contract == "NegRisk"]
    print(f"CTF Exchange:     {len(ctf):4d} ({100*len(ctf)/len(trades):.1f}%)")
    print(f"NegRisk Exchange: {len(negrisk):4d} ({100*len(negrisk)/len(trades):.1f}%)")

    # Volume analysis
    print(f"\n=== VOLUME ANALYSIS ===")
    total_usdc = sum(t.usdc for t in trades)
    total_shares = sum(t.shares for t in trades)
    print(f"Total USDC:   ${total_usdc:,.2f}")
    print(f"Total shares: {total_shares:,.2f}")
    print(f"Avg trade:    ${total_usdc/len(trades):.2f} USDC, {total_shares/len(trades):.2f} shares")

    # Trade size distribution
    usdc_amounts = [t.usdc for t in trades]
    print(f"\n=== TRADE SIZE DISTRIBUTION ===")
    size_buckets = {
        "< $10": 0, "$10-50": 0, "$50-100": 0, "$100-500": 0,
        "$500-1K": 0, "$1K-5K": 0, "> $5K": 0
    }
    for u in usdc_amounts:
        if u < 10: size_buckets["< $10"] += 1
        elif u < 50: size_buckets["$10-50"] += 1
        elif u < 100: size_buckets["$50-100"] += 1
        elif u < 500: size_buckets["$100-500"] += 1
        elif u < 1000: size_buckets["$500-1K"] += 1
        elif u < 5000: size_buckets["$1K-5K"] += 1
        else: size_buckets["> $5K"] += 1

    for bucket, count in size_buckets.items():
        if count > 0:
            pct = 100 * count / len(trades)
            print(f"  {bucket:10s}: {count:4d} ({pct:5.1f}%)")

    # Token analysis
    print(f"\n=== TOP 10 TOKENS BY TRADE COUNT ===")
    by_token = defaultdict(list)
    for t in trades:
        by_token[t.token_id].append(t)

    sorted_tokens = sorted(by_token.items(), key=lambda x: -len(x[1]))[:10]
    for token_id, token_trades in sorted_tokens:
        t_buys = [t for t in token_trades if t.side == "BUY"]
        t_sells = [t for t in token_trades if t.side == "SELL"]
        total_vol = sum(t.usdc for t in token_trades)

        valid_buys = [t.price for t in t_buys if 0 < t.price <= 1]
        valid_sells = [t.price for t in t_sells if 0 < t.price <= 1]

        print(f"\n{token_id[:24]}...")
        print(f"  Trades: {len(token_trades)} (BUY: {len(t_buys)}, SELL: {len(t_sells)})")
        print(f"  Volume: ${total_vol:,.2f}")
        if valid_buys:
            print(f"  BUY avg:  ${sum(valid_buys)/len(valid_buys):.4f}")
        if valid_sells:
            print(f"  SELL avg: ${sum(valid_sells)/len(valid_sells):.4f}")

    # Look for arbitrage patterns (same token, both BUY and SELL)
    print(f"\n=== POTENTIAL ARBITRAGE (tokens with both BUY and SELL) ===")
    arb_count = 0
    for token_id, token_trades in by_token.items():
        buys = [t for t in token_trades if t.side == "BUY" and 0 < t.price <= 1]
        sells = [t for t in token_trades if t.side == "SELL" and 0 < t.price <= 1]

        if buys and sells:
            arb_count += 1
            avg_buy = sum(t.price for t in buys) / len(buys)
            avg_sell = sum(t.price for t in sells) / len(sells)
            spread = avg_sell - avg_buy

            if arb_count <= 10:  # Show first 10
                print(f"\n{token_id[:24]}...")
                print(f"  BUY:  {len(buys)} trades @ avg ${avg_buy:.4f}")
                print(f"  SELL: {len(sells)} trades @ avg ${avg_sell:.4f}")
                print(f"  Spread: ${spread:+.4f} ({'profit' if spread > 0 else 'loss'})")

    print(f"\nTotal tokens with both BUY and SELL: {arb_count}")

    # Time analysis
    print(f"\n=== TIME ANALYSIS ===")
    timestamps = [t.timestamp for t in trades]
    duration = max(timestamps) - min(timestamps)
    trades_per_min = len(trades) / (duration / 60) if duration > 0 else 0

    start_dt = datetime.utcfromtimestamp(min(timestamps))
    end_dt = datetime.utcfromtimestamp(max(timestamps))

    print(f"Period: {start_dt} to {end_dt}")
    print(f"Duration: {duration/60:.1f} minutes ({duration/3600:.2f} hours)")
    print(f"Trade frequency: {trades_per_min:.1f} trades/minute")

    # Inter-trade timing
    sorted_trades = sorted(trades, key=lambda t: t.timestamp)
    intervals = []
    for i in range(1, len(sorted_trades)):
        interval = sorted_trades[i].timestamp - sorted_trades[i-1].timestamp
        intervals.append(interval)

    if intervals:
        print(f"\nInter-trade intervals:")
        print(f"  Min:    {min(intervals):.1f}s")
        print(f"  Max:    {max(intervals):.1f}s")
        print(f"  Median: {sorted(intervals)[len(intervals)//2]:.1f}s")
        print(f"  Mean:   {sum(intervals)/len(intervals):.1f}s")


def main():
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")

    with open(csv_file) as f:
        rows = list(csv.DictReader(f))

    print(f"Total transactions in CSV: {len(rows)}")
    print(f"Fetching and decoding all transactions...")

    trades = []
    errors = []

    for i, row in enumerate(rows):
        if i % 100 == 0:
            print(f"  Progress: {i}/{len(rows)} ({len(trades)} decoded, {len(errors)} errors)")

        tx_hash = row["Transaction Hash"]
        block_number = int(row.get("Blockno", 0))
        timestamp = int(row["UnixTimestamp"])
        contract = row["To"]

        tx_data = fetch_tx(tx_hash)
        if tx_data and tx_data.get("input"):
            try:
                trade = decode_trade(
                    tx_data["input"],
                    tx_hash,
                    block_number,
                    timestamp,
                    contract
                )
                if trade:
                    trades.append(trade)
            except Exception as e:
                errors.append((tx_hash, str(e)))

        time.sleep(0.02)  # Rate limit

    print(f"\nDecoded {len(trades)} trades ({len(errors)} errors)")

    if trades:
        analyze_trades(trades)

        # Export to JSON
        output_file = Path("/home/a/Work/gambling/engine/specs/gabagool_trades_corrected.json")
        with open(output_file, "w") as f:
            json.dump([asdict(t) for t in trades], f, indent=2)
        print(f"\nExported to {output_file}")

        # Export summary CSV for easy viewing
        summary_file = Path("/home/a/Work/gambling/engine/specs/gabagool_trades_summary.csv")
        with open(summary_file, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(["timestamp", "datetime", "side", "price", "shares", "usdc", "contract", "token_id", "tx_hash"])
            for t in sorted(trades, key=lambda x: x.timestamp):
                writer.writerow([
                    t.timestamp, t.datetime_str, t.side,
                    f"{t.price:.6f}", f"{t.shares:.4f}", f"{t.usdc:.2f}",
                    t.contract, t.token_id[:24] + "...", t.tx_hash
                ])
        print(f"Exported summary to {summary_file}")


if __name__ == "__main__":
    main()
