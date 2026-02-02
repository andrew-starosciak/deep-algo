#!/bin/bash
# Binary Backtest Runner
#
# Runs the full pipeline: collect signals, backfill, and backtest
# Usage: ./scripts/run_binary_backtest.sh [--start DATE] [--end DATE] [--signal NAME]

set -e

# Default values
START_DATE="${START_DATE:-$(date -d '30 days ago' '+%Y-%m-%dT00:00:00Z' 2>/dev/null || date -v-30d '+%Y-%m-%dT00:00:00Z')}"
END_DATE="${END_DATE:-$(date '+%Y-%m-%dT00:00:00Z')}"
SIGNAL="${SIGNAL:-liquidation_cascade}"
SYMBOL="${SYMBOL:-BTCUSDT}"
EXCHANGE="${EXCHANGE:-binance}"
MIN_STRENGTH="${MIN_STRENGTH:-0.5}"
STAKE="${STAKE:-100}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --start)
            START_DATE="$2"
            shift 2
            ;;
        --end)
            END_DATE="$2"
            shift 2
            ;;
        --signal)
            SIGNAL="$2"
            shift 2
            ;;
        --symbol)
            SYMBOL="$2"
            shift 2
            ;;
        --min-strength)
            MIN_STRENGTH="$2"
            shift 2
            ;;
        --stake)
            STAKE="$2"
            shift 2
            ;;
        --backfill-only)
            BACKFILL_ONLY=1
            shift
            ;;
        --backtest-only)
            BACKTEST_ONLY=1
            shift
            ;;
        --full-analysis)
            FULL_ANALYSIS=1
            shift
            ;;
        --help)
            echo "Binary Backtest Runner"
            echo ""
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --start DATE        Start date (ISO 8601, default: 30 days ago)"
            echo "  --end DATE          End date (ISO 8601, default: now)"
            echo "  --signal NAME       Signal to backtest (default: liquidation_cascade)"
            echo "  --symbol SYMBOL     Trading symbol (default: BTCUSDT)"
            echo "  --min-strength N    Minimum signal strength 0-1 (default: 0.5)"
            echo "  --stake N           Stake per bet in USD (default: 100)"
            echo "  --backfill-only     Only run backfill, skip backtest"
            echo "  --backtest-only     Only run backtest, skip backfill"
            echo "  --full-analysis     Enable Phase 3 statistical analysis"
            echo ""
            echo "Available signals:"
            echo "  obi, order_book_imbalance  - Order book bid/ask imbalance"
            echo "  funding, funding_rate      - Perpetual funding rate"
            echo "  liq, liquidation_cascade   - Liquidation pressure"
            echo "  news                       - News sentiment"
            echo "  fp, funding_percentile     - 30-day funding percentile"
            echo "  me, momentum_exhaustion    - Stalling after big moves"
            echo "  wb, wall_bias              - Order book walls"
            echo "  rn, composite_require_n    - 2+ signals agree"
            echo "  lr, liquidation_ratio      - Long/short liquidation ratio"
            echo ""
            echo "Environment variables:"
            echo "  DATABASE_URL        PostgreSQL connection string (required)"
            echo ""
            echo "Examples:"
            echo "  # Basic backtest with liquidation signal"
            echo "  $0 --signal liquidation_cascade"
            echo ""
            echo "  # Full analysis on funding percentile signal"
            echo "  $0 --signal funding_percentile --full-analysis"
            echo ""
            echo "  # Backtest all signals for comparison"
            echo "  for sig in obi funding liq fp me wb rn lr; do"
            echo "    $0 --signal \$sig --backtest-only"
            echo "  done"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check DATABASE_URL
if [ -z "$DATABASE_URL" ]; then
    echo "Error: DATABASE_URL environment variable is not set"
    echo "Export it with: export DATABASE_URL='postgres://user:pass@host/db'"
    exit 1
fi

echo "=========================================="
echo "Binary Backtest Runner"
echo "=========================================="
echo "Start:        $START_DATE"
echo "End:          $END_DATE"
echo "Signal:       $SIGNAL"
echo "Symbol:       $SYMBOL"
echo "Min Strength: $MIN_STRENGTH"
echo "Stake:        \$$STAKE"
echo "=========================================="
echo ""

# Step 1: Backfill signals (if not backtest-only)
if [ -z "$BACKTEST_ONLY" ]; then
    echo "[1/2] Backfilling signal snapshots..."
    echo "      This computes signal values from raw data at 15-minute intervals"
    echo ""

    cargo run -p algo-trade-cli --release -- backfill-signals \
        --start "$START_DATE" \
        --end "$END_DATE" \
        --signals "$SIGNAL" \
        --symbol "$SYMBOL" \
        --exchange "$EXCHANGE" \
        --interval 15m

    echo ""
    echo "      Backfill complete!"
    echo ""
fi

# Exit if backfill-only
if [ -n "$BACKFILL_ONLY" ]; then
    echo "Backfill complete. Skipping backtest (--backfill-only mode)"
    exit 0
fi

# Step 2: Run backtest
echo "[2/2] Running binary backtest..."
echo ""

BACKTEST_ARGS=(
    --start "$START_DATE"
    --end "$END_DATE"
    --signal "$SIGNAL"
    --symbol "$SYMBOL"
    --exchange "$EXCHANGE"
    --min-strength "$MIN_STRENGTH"
    --stake "$STAKE"
)

if [ -n "$FULL_ANALYSIS" ]; then
    BACKTEST_ARGS+=(--full-analysis)
fi

cargo run -p algo-trade-cli --release -- binary-backtest "${BACKTEST_ARGS[@]}"

echo ""
echo "=========================================="
echo "Backtest complete!"
echo "=========================================="
