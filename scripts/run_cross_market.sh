#!/bin/bash
# Cross-market arbitrage scanner and settlement runner
# Usage: ./run_cross_market.sh [scan|settle|both|backtest]

set -e
cd "$(dirname "$0")/.."

# Load environment
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
fi

if [ -z "$DATABASE_URL" ]; then
    echo "ERROR: DATABASE_URL not set. Create .env file or export it."
    exit 1
fi

export RUST_MIN_STACK=33554432

# Scanner settings - arbitrage only by default
DURATION_MINS=${DURATION_MINS:-720}  # 12 hours default
MAX_COST=${MAX_COST:-0.85}           # Only signal when cost < $0.85
MIN_SPREAD=${MIN_SPREAD:-0.15}       # $0.15 minimum spread (15% ROI)
CORRELATION=${CORRELATION:-0.85}     # Assumed correlation
ARBITRAGE_ONLY=${ARBITRAGE_ONLY:-1}  # 1 = arbitrage only (default), 0 = all combinations
MIN_DEPTH=${MIN_DEPTH:-100}          # Minimum order book depth (shares)

# Build arbitrage flag
if [ "$ARBITRAGE_ONLY" = "1" ]; then
    ARB_FLAG="--arbitrage-only"
    ARB_MODE="ARBITRAGE ONLY (opposing directions)"
else
    ARB_FLAG=""
    ARB_MODE="ALL COMBINATIONS"
fi

case "${1:-both}" in
    scan)
        echo "Starting cross-market scanner with depth tracking..."
        echo "  Mode: $ARB_MODE"
        echo "  Duration: $DURATION_MINS mins | Max cost: \$$MAX_COST | Min spread: \$$MIN_SPREAD | Min depth: $MIN_DEPTH"
        cargo run --release -p algo-trade-cli -- cross-market-scan \
            --persist \
            --track-depth \
            --verbose \
            $ARB_FLAG \
            --duration-mins "$DURATION_MINS" \
            --max-cost "$MAX_COST" \
            --min-spread "$MIN_SPREAD" \
            --correlation "$CORRELATION" \
            --min-depth "$MIN_DEPTH"
        ;;
    settle)
        echo "Starting settlement handler ($DURATION_MINS mins)..."
        cargo run --release -p algo-trade-cli -- cross-market-settle --duration-mins "$DURATION_MINS"
        ;;
    backtest)
        echo "Running backtest on collected data..."
        cargo run --release -p algo-trade-cli -- cross-market-backtest
        ;;
    both)
        echo "Starting scanner (with depth) and settlement handler..."
        echo "  Mode: $ARB_MODE"
        echo "  Duration: $DURATION_MINS mins | Max cost: \$$MAX_COST | Min spread: \$$MIN_SPREAD | Min depth: $MIN_DEPTH"
        echo "  Scanner log: scanner.log"
        echo "  Settlement log: settle.log"
        echo ""

        # Start scanner in background with depth tracking
        cargo run --release -p algo-trade-cli -- cross-market-scan \
            --persist \
            --track-depth \
            --verbose \
            $ARB_FLAG \
            --duration-mins "$DURATION_MINS" \
            --max-cost "$MAX_COST" \
            --min-spread "$MIN_SPREAD" \
            --correlation "$CORRELATION" \
            --min-depth "$MIN_DEPTH" > scanner.log 2>&1 &
        SCANNER_PID=$!
        echo "Scanner started (PID: $SCANNER_PID)"

        # Start settlement in background
        cargo run --release -p algo-trade-cli -- cross-market-settle \
            --duration-mins "$DURATION_MINS" > settle.log 2>&1 &
        SETTLE_PID=$!
        echo "Settlement started (PID: $SETTLE_PID)"

        echo ""
        echo "Both running. To stop: kill $SCANNER_PID $SETTLE_PID"
        echo "Or run: pkill -f 'cross-market'"
        echo ""
        echo "Tailing logs (Ctrl+C to stop viewing, processes continue)..."
        tail -f scanner.log settle.log
        ;;
    *)
        echo "Usage: $0 [scan|settle|both|backtest]"
        echo "  scan     - Run scanner with depth tracking (persists to DB)"
        echo "  settle   - Run settlement only (processes outcomes)"
        echo "  both     - Run both in background (default)"
        echo "  backtest - Run backtest on collected data"
        echo ""
        echo "Environment overrides:"
        echo "  DURATION_MINS=720   - Duration in minutes (default: 720 = 12h)"
        echo "  MAX_COST=0.85       - Max combined cost threshold"
        echo "  MIN_SPREAD=0.15     - Min spread threshold"
        echo "  CORRELATION=0.85    - Assumed correlation"
        echo "  ARBITRAGE_ONLY=1    - 1=arbitrage only (default), 0=all combinations"
        echo "  MIN_DEPTH=100       - Min order book depth (shares, default: 100)"
        echo ""
        echo "Examples:"
        echo "  ./run_cross_market.sh scan                      # 12h arbitrage scan"
        echo "  DURATION_MINS=60 ./run_cross_market.sh scan     # 1h scan"
        echo "  ARBITRAGE_ONLY=0 ./run_cross_market.sh scan     # Include directional bets"
        echo "  ./run_cross_market.sh backtest                  # Analyze collected data"
        exit 1
        ;;
esac
