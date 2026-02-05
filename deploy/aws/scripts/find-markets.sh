#!/bin/bash
# Find suitable markets for the trading bot
set -e

echo "=============================================="
echo "Polymarket Market Discovery"
echo "=============================================="
echo ""

echo "=== Active Crypto Price Markets ==="
curl -s "https://gamma-api.polymarket.com/events?active=true&limit=500" | \
  jq -r '.[] | select(.title | test("BTC|Bitcoin|ETH|Ethereum|crypto"; "i")) | select(.title | test("price|above|below|hit|reach"; "i")) | "[\(.id)] \(.title)"' 2>/dev/null

echo ""
echo "=== Short-term Markets (hours/minutes) ==="
curl -s "https://gamma-api.polymarket.com/events?active=true&limit=500" | \
  jq -r '.[] | select(.title | test("hour|minute|noon|midnight|today"; "i")) | "[\(.id)] \(.title)"' 2>/dev/null

echo ""
echo "=== Binary Outcome Markets (will X happen) ==="
curl -s "https://gamma-api.polymarket.com/events?active=true&limit=500" | \
  jq -r '.[] | select(.markets | length == 2) | select(.title | test("Will"; "i")) | "[\(.id)] \(.title) (2 outcomes)"' 2>/dev/null | head -30

echo ""
echo "=== Market Stats ==="
TOTAL=$(curl -s "https://gamma-api.polymarket.com/events?active=true&limit=500" | jq 'length')
echo "Total active events: $TOTAL"
echo ""
echo "Ideal markets for arbitrage:"
echo "  - Binary (YES/NO) outcomes"
echo "  - Short settlement time (hours, not days)"
echo "  - High volume/liquidity"
echo "  - Price-based (objective settlement)"
