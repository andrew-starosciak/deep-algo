#!/usr/bin/env python3
"""
Raw decode of a matchOrders call to understand the struct layout.
"""

import json

def decode_uint256(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset:offset+32], 'big')

def decode_address(data: bytes, offset: int) -> str:
    return "0x" + data[offset+12:offset+32].hex()

def decode_bytes32(data: bytes, offset: int) -> str:
    return "0x" + data[offset:offset+32].hex()

def main():
    # Load the sample transaction
    with open("/tmp/tx.json") as f:
        tx = json.load(f)["result"]

    input_data = tx["input"]
    print(f"TX Hash: {tx['hash']}")
    print(f"To: {tx['to']}")
    print()

    # Parse the input
    data = bytes.fromhex(input_data[2:])  # Remove 0x

    selector = data[:4].hex()
    print(f"Function selector: 0x{selector}")
    print()

    # Skip selector
    data = data[4:]

    # Print the first 10 uint256 values (offsets and direct values)
    print("=== HEADER (first 10 uint256 slots) ===")
    for i in range(10):
        val = decode_uint256(data, i * 32)
        print(f"Slot {i:2d} (offset {i*32:4d}): {val:>20d}  (0x{val:x})")

    print()

    # Taker order offset is slot 0
    taker_offset = decode_uint256(data, 0)
    print(f"=== TAKER ORDER (at offset {taker_offset}) ===")

    # Order struct fields (each 32 bytes):
    # 0: salt
    # 1: maker address
    # 2: signer address
    # 3: taker address
    # 4: tokenId
    # 5: makerAmount
    # 6: takerAmount
    # 7: expiration
    # 8: nonce
    # 9: feeRateBps
    # 10: side
    # 11: signatureType

    field_names = [
        "salt", "maker", "signer", "taker", "tokenId",
        "makerAmount", "takerAmount", "expiration", "nonce",
        "feeRateBps", "side", "signatureType"
    ]

    for i, name in enumerate(field_names):
        offset = taker_offset + i * 32
        raw_val = decode_uint256(data, offset)

        if name in ["maker", "signer", "taker"]:
            val_str = decode_address(data, offset)
        elif name == "tokenId":
            val_str = decode_bytes32(data, offset)[:20] + "..."
        else:
            val_str = f"{raw_val:,}"

        print(f"  {name:15s}: {val_str}")

    # Calculate price interpretations
    maker_amount = decode_uint256(data, taker_offset + 5 * 32)
    taker_amount = decode_uint256(data, taker_offset + 6 * 32)
    side = decode_uint256(data, taker_offset + 10 * 32)

    print()
    print("=== PRICE CALCULATIONS ===")
    print(f"maker_amount: {maker_amount:,} ({maker_amount / 1_000_000:.6f} with 6 decimals)")
    print(f"taker_amount: {taker_amount:,} ({taker_amount / 1_000_000:.6f} with 6 decimals)")
    print(f"side: {side} ({'BUY' if side == 0 else 'SELL'})")
    print()

    # Try different price interpretations
    if maker_amount > 0 and taker_amount > 0:
        p1 = taker_amount / maker_amount
        p2 = maker_amount / taker_amount

        print(f"Price interpretation 1 (taker/maker): {p1:.6f}")
        print(f"Price interpretation 2 (maker/taker): {p2:.6f}")
        print()

        # In Polymarket binary options:
        # - If you BUY at $0.40, you pay $0.40 USDC to get 1 share
        # - If you SELL at $0.40, you give 1 share and get $0.40 USDC
        #
        # For a taker BUY order:
        #   - taker provides USDC
        #   - maker provides shares
        #   - price = USDC / shares
        #
        # If side=0 means BUY:
        #   - taker_amount = USDC (what taker provides)
        #   - maker_amount = shares (what maker provides)
        #   - price = taker_amount / maker_amount

        if p1 < 1.0 and p1 > 0:
            print("*** Price 1 is in valid range (0-1) ***")
        if p2 < 1.0 and p2 > 0:
            print("*** Price 2 is in valid range (0-1) ***")

    # Check maker orders
    maker_orders_offset = decode_uint256(data, 32)
    num_makers = decode_uint256(data, maker_orders_offset)
    print(f"\n=== MAKER ORDERS ({num_makers} orders) ===")

    for m in range(num_makers):
        # Offset to this maker order (relative to maker_orders_offset + 32)
        ptr_offset = maker_orders_offset + 32 + m * 32
        maker_ptr = decode_uint256(data, ptr_offset)
        actual_offset = maker_orders_offset + maker_ptr

        m_maker_amount = decode_uint256(data, actual_offset + 5 * 32)
        m_taker_amount = decode_uint256(data, actual_offset + 6 * 32)
        m_side = decode_uint256(data, actual_offset + 10 * 32)

        print(f"Maker {m}:")
        print(f"  maker_amount: {m_maker_amount:,}")
        print(f"  taker_amount: {m_taker_amount:,}")
        print(f"  side: {m_side} ({'BUY' if m_side == 0 else 'SELL'})")

        if m_maker_amount > 0 and m_taker_amount > 0:
            print(f"  price (t/m): {m_taker_amount / m_maker_amount:.6f}")
            print(f"  price (m/t): {m_maker_amount / m_taker_amount:.6f}")


if __name__ == "__main__":
    main()
