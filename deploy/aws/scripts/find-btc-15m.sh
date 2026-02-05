#!/bin/bash
# Find current BTC 15-minute markets on Polymarket
set -e

echo "=============================================="
echo "BTC 15-Minute Market Discovery"
echo "=============================================="
echo ""

# Get current and next window timestamps
NOW=$(date +%s)
WINDOW_START=$(( (NOW / 900) * 900 ))  # Round down to 15-min boundary
NEXT_WINDOW=$((WINDOW_START + 900))

echo "Current time: $(date)"
echo "Current window: $(date -d @$WINDOW_START 2>/dev/null || date -r $WINDOW_START)"
echo "Next window: $(date -d @$NEXT_WINDOW 2>/dev/null || date -r $NEXT_WINDOW)"
echo ""

echo "=== Searching for active BTC 15-min markets ==="
# Search gamma API for btc-updown markets
MARKETS=$(curl -s "https://gamma-api.polymarket.com/events?active=true&closed=false&limit=50" | \
  jq -r '.[] | select(.slug | test("btc-updown-15m")) | {slug, title, id, enableOrderBook, active}')

if [ -n "$MARKETS" ]; then
    echo "$MARKETS" | jq -r '"[\(.id)] \(.slug)\n  Title: \(.title)\n  OrderBook: \(.enableOrderBook)\n"'
else
    echo "No active BTC 15-min markets found via gamma API"
    echo ""
    echo "Trying direct slug search..."

    # Try specific timestamp patterns
    for OFFSET in 0 900 1800; do
        TS=$((WINDOW_START + OFFSET))
        SLUG="btc-updown-15m-$TS"
        echo -n "Checking $SLUG... "
        RESULT=$(curl -s "https://gamma-api.polymarket.com/events?slug=$SLUG" | jq -r '.[0].active // "not found"')
        echo "$RESULT"
    done
fi

echo ""
echo "=== Get Token IDs for Current Market ==="
CURRENT_SLUG="btc-updown-15m-$WINDOW_START"
echo "Looking for: $CURRENT_SLUG"

MARKET_DATA=$(curl -s "https://gamma-api.polymarket.com/events?slug=$CURRENT_SLUG")
if [ "$(echo "$MARKET_DATA" | jq 'length')" -gt 0 ]; then
    echo ""
    echo "Market found!"
    echo "$MARKET_DATA" | jq -r '.[0] | "Title: \(.title)\nCondition ID: \(.markets[0].conditionId)\nTokens: \(.markets[0].clobTokenIds)"'

    # Extract token IDs
    UP_TOKEN=$(echo "$MARKET_DATA" | jq -r '.[0].markets[0].clobTokenIds' | jq -r '.[0]')
    DOWN_TOKEN=$(echo "$MARKET_DATA" | jq -r '.[0].markets[0].clobTokenIds' | jq -r '.[1]')

    echo ""
    echo "UP Token: $UP_TOKEN"
    echo "DOWN Token: $DOWN_TOKEN"

    echo ""
    echo "=== Current Order Book Snapshot ==="
    echo "UP (best bid/ask):"
    curl -s "https://clob.polymarket.com/book?token_id=$UP_TOKEN" | \
      jq -r '"  Bid: $\(.bids[0].price) x \(.bids[0].size)\n  Ask: $\(.asks[-1].price) x \(.asks[-1].size)"'

    echo "DOWN (best bid/ask):"
    curl -s "https://clob.polymarket.com/book?token_id=$DOWN_TOKEN" | \
      jq -r '"  Bid: $\(.bids[-1].price) x \(.bids[-1].size)\n  Ask: $\(.asks[0].price) x \(.asks[0].size)"'
else
    echo "Market not found for current window"
fi
