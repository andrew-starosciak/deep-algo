#!/usr/bin/env python3
"""
Decode gabagool's matchOrders transactions to extract actual trade details.

The matchOrders function on Polymarket CTF Exchange takes two orders and matches them.
We need to decode the input data to extract:
- Token IDs (which market/outcome)
- Prices
- Quantities
- Side (buy/sell)
"""

import json
import csv
import sys
from pathlib import Path
from dataclasses import dataclass
from typing import Optional, List
import struct

# matchOrders function selector
MATCH_ORDERS_SELECTOR = "0x2287e350"

@dataclass
class Order:
    """Decoded order from matchOrders call."""
    salt: int
    maker: str
    signer: str
    taker: str
    token_id: str
    maker_amount: int  # Amount maker is selling (in wei-like units)
    taker_amount: int  # Amount taker is paying
    expiration: int
    nonce: int
    fee_rate_bps: int
    side: int  # 0 = BUY, 1 = SELL
    signature_type: int

    @property
    def price(self) -> float:
        """Calculate price from amounts. Price = taker_amount / maker_amount"""
        if self.maker_amount == 0:
            return 0
        # Polymarket uses 6 decimals for USDC, amounts are in micro-units
        return self.taker_amount / self.maker_amount

    @property
    def quantity(self) -> float:
        """Return quantity in human-readable units (divide by 10^6)."""
        return self.maker_amount / 1_000_000

    @property
    def side_str(self) -> str:
        return "BUY" if self.side == 0 else "SELL"


@dataclass
class MatchOrdersCall:
    """Decoded matchOrders function call."""
    taker_order: Order
    maker_orders: List[Order]
    taker_fill_amount: int
    maker_fill_amounts: List[int]

    def __str__(self):
        lines = [
            f"=== MATCH ORDERS ===",
            f"Taker Order: {self.taker_order.side_str} {self.taker_order.quantity:.2f} @ ${self.taker_order.price:.4f}",
            f"  Token: {self.taker_order.token_id[:16]}...",
            f"  Maker: {self.taker_order.maker[:10]}...",
            f"Maker Orders: {len(self.maker_orders)}",
        ]
        for i, order in enumerate(self.maker_orders):
            lines.append(f"  [{i}] {order.side_str} {order.quantity:.2f} @ ${order.price:.4f}")
        return "\n".join(lines)


def decode_uint256(data: bytes, offset: int) -> int:
    """Decode a uint256 from bytes at offset."""
    return int.from_bytes(data[offset:offset+32], 'big')


def decode_address(data: bytes, offset: int) -> str:
    """Decode an address from bytes at offset."""
    return "0x" + data[offset+12:offset+32].hex()


def decode_bytes32(data: bytes, offset: int) -> str:
    """Decode bytes32 as hex string."""
    return "0x" + data[offset:offset+32].hex()


def decode_order(data: bytes, offset: int) -> Order:
    """
    Decode an Order struct from the calldata.

    Order struct layout (from Polymarket contracts):
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        address taker;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint256 expiration;
        uint256 nonce;
        uint256 feeRateBps;
        uint8 side;
        uint8 signatureType;
        bytes signature;
    }
    """
    salt = decode_uint256(data, offset)
    maker = decode_address(data, offset + 32)
    signer = decode_address(data, offset + 64)
    taker = decode_address(data, offset + 96)
    token_id = decode_bytes32(data, offset + 128)
    maker_amount = decode_uint256(data, offset + 160)
    taker_amount = decode_uint256(data, offset + 192)
    expiration = decode_uint256(data, offset + 224)
    nonce = decode_uint256(data, offset + 256)
    fee_rate_bps = decode_uint256(data, offset + 288)
    side = decode_uint256(data, offset + 320)
    signature_type = decode_uint256(data, offset + 352)

    return Order(
        salt=salt,
        maker=maker,
        signer=signer,
        taker=taker,
        token_id=token_id,
        maker_amount=maker_amount,
        taker_amount=taker_amount,
        expiration=expiration,
        nonce=nonce,
        fee_rate_bps=fee_rate_bps,
        side=side,
        signature_type=signature_type
    )


def decode_match_orders(input_data: str) -> Optional[MatchOrdersCall]:
    """
    Decode a matchOrders function call.

    Function signature:
    matchOrders(
        Order takerOrder,
        Order[] makerOrders,
        uint256 takerFillAmount,
        uint256[] makerFillAmounts,
        bytes makerFillArgs,
        bytes affiliateParams,
        bytes affiliateSignature
    )
    """
    if not input_data.startswith(MATCH_ORDERS_SELECTOR):
        return None

    data = bytes.fromhex(input_data[2:])  # Remove 0x prefix

    # Skip the 4-byte selector
    data = data[4:]

    # The data layout uses dynamic types, so we have offsets first
    # Offset to takerOrder (dynamic struct)
    taker_order_offset = decode_uint256(data, 0)
    # Offset to makerOrders array
    maker_orders_offset = decode_uint256(data, 32)
    # takerFillAmount (direct value)
    taker_fill_amount = decode_uint256(data, 64)
    # Offset to makerFillAmounts array
    maker_fill_amounts_offset = decode_uint256(data, 96)

    # Decode taker order
    taker_order = decode_order(data, taker_order_offset)

    # Decode maker orders array
    # First uint256 at the offset is the array length
    num_makers = decode_uint256(data, maker_orders_offset)
    maker_orders = []

    # After the length, we have offsets to each maker order
    for i in range(num_makers):
        maker_offset_ptr = maker_orders_offset + 32 + (i * 32)
        maker_offset = decode_uint256(data, maker_offset_ptr)
        # The offset is relative to the start of the maker orders data
        actual_offset = maker_orders_offset + maker_offset
        maker_orders.append(decode_order(data, actual_offset))

    # Decode maker fill amounts
    num_fills = decode_uint256(data, maker_fill_amounts_offset)
    maker_fill_amounts = []
    for i in range(num_fills):
        fill_amount = decode_uint256(data, maker_fill_amounts_offset + 32 + (i * 32))
        maker_fill_amounts.append(fill_amount)

    return MatchOrdersCall(
        taker_order=taker_order,
        maker_orders=maker_orders,
        taker_fill_amount=taker_fill_amount,
        maker_fill_amounts=maker_fill_amounts
    )


def analyze_trade(call: MatchOrdersCall, gabagool_addr: str) -> dict:
    """Extract key metrics from a trade."""
    taker = call.taker_order

    # Gabagool is always the taker (matching against maker orders)
    is_gabagool_taker = taker.maker.lower() == gabagool_addr.lower()

    # Get the actual trade direction for gabagool
    # The taker order's "side" indicates what the TAKER is doing
    # side=0 means BUY (taker buys from maker)
    # side=1 means SELL (taker sells to maker)

    return {
        "token_id": taker.token_id,
        "side": taker.side_str,
        "price": taker.price,
        "quantity": taker.quantity,
        "taker_amount_usdc": taker.taker_amount / 1_000_000,
        "maker_amount_shares": taker.maker_amount / 1_000_000,
        "num_makers": len(call.maker_orders),
        "total_fill": call.taker_fill_amount / 1_000_000,
    }


def main():
    gabagool_addr = "0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998"

    # Read the sample transaction
    tx_file = Path("/tmp/tx.json")
    if tx_file.exists():
        with open(tx_file) as f:
            tx_data = json.load(f)

        input_data = tx_data.get("result", {}).get("input", "")
        if input_data:
            print("=== DECODING SAMPLE TRANSACTION ===")
            print(f"TX Hash: {tx_data['result']['hash']}")
            print()

            call = decode_match_orders(input_data)
            if call:
                print(call)
                print()

                trade = analyze_trade(call, gabagool_addr)
                print("Trade Analysis:")
                for k, v in trade.items():
                    print(f"  {k}: {v}")

    # Read CSV and analyze patterns
    csv_file = Path("/home/a/Work/gambling/engine/specs/export-0x0c802c7429d3e2c5994db49bf6a7a6af2deaa998.csv")
    if csv_file.exists():
        print("\n=== CSV ANALYSIS ===")

        with open(csv_file) as f:
            reader = csv.DictReader(f)
            rows = list(reader)

        print(f"Total transactions: {len(rows)}")

        # Count by contract
        contract_counts = {}
        for row in rows:
            to_addr = row.get("To", "")
            contract_counts[to_addr] = contract_counts.get(to_addr, 0) + 1

        print("\nTransactions by contract:")
        for addr, count in sorted(contract_counts.items(), key=lambda x: -x[1]):
            name = "CTF Exchange" if "e3f18acc" in addr.lower() else "Neg Risk Exchange"
            print(f"  {addr[:16]}... ({name}): {count}")

        # Analyze timing
        timestamps = [int(row.get("UnixTimestamp", 0)) for row in rows]
        if timestamps:
            start_ts = min(timestamps)
            end_ts = max(timestamps)
            duration_secs = end_ts - start_ts
            duration_mins = duration_secs / 60
            trades_per_min = len(rows) / duration_mins if duration_mins > 0 else 0

            print(f"\nTiming:")
            print(f"  Duration: {duration_mins:.1f} minutes")
            print(f"  Trades per minute: {trades_per_min:.1f}")
            print(f"  Avg seconds between trades: {duration_secs / len(rows):.2f}")

        # Analyze gas
        gas_fees = [float(row.get("TxnFee(POL)", 0)) for row in rows if row.get("TxnFee(POL)")]
        if gas_fees:
            print(f"\nGas Analysis:")
            print(f"  Total gas (POL): {sum(gas_fees):.2f}")
            print(f"  Avg gas per tx (POL): {sum(gas_fees)/len(gas_fees):.4f}")
            print(f"  Min gas (POL): {min(gas_fees):.4f}")
            print(f"  Max gas (POL): {max(gas_fees):.4f}")


if __name__ == "__main__":
    main()
