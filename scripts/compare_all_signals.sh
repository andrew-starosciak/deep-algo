#!/bin/bash
# Compare All Signals
#
# Runs backtest on all available signals and outputs comparison
# Usage: ./scripts/compare_all_signals.sh [--start DATE] [--end DATE]

set -e

# Default values
START_DATE="${START_DATE:-$(date -d '30 days ago' '+%Y-%m-%dT00:00:00Z' 2>/dev/null || date -v-30d '+%Y-%m-%dT00:00:00Z')}"
END_DATE="${END_DATE:-$(date '+%Y-%m-%dT00:00:00Z')}"
OUTPUT_DIR="${OUTPUT_DIR:-./backtest_results}"

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
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --help)
            echo "Compare All Signals"
            echo ""
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --start DATE       Start date (ISO 8601)"
            echo "  --end DATE         End date (ISO 8601)"
            echo "  --output-dir DIR   Output directory for JSON results"
            exit 0
            ;;
        *)
            shift
            ;;
    esac
done

# Check DATABASE_URL
if [ -z "$DATABASE_URL" ]; then
    echo "Error: DATABASE_URL environment variable is not set"
    exit 1
fi

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "=========================================="
echo "Signal Comparison Runner"
echo "=========================================="
echo "Start: $START_DATE"
echo "End:   $END_DATE"
echo "=========================================="
echo ""

# All signals to test
SIGNALS=(
    "order_book_imbalance"
    "funding_rate"
    "liquidation_cascade"
    "funding_percentile"
    "momentum_exhaustion"
    "wall_bias"
    "composite_require_n"
    "liquidation_ratio"
)

# First, backfill all signals
echo "[1/2] Backfilling all signals..."
cargo run -p algo-trade-cli --release -- backfill-signals \
    --start "$START_DATE" \
    --end "$END_DATE" \
    --signals "all" \
    --interval 15m

echo ""
echo "[2/2] Running backtests for each signal..."
echo ""

# Results summary
SUMMARY_FILE="$OUTPUT_DIR/summary_$(date '+%Y%m%d_%H%M%S').txt"

echo "Signal Comparison Results" > "$SUMMARY_FILE"
echo "=========================" >> "$SUMMARY_FILE"
echo "Period: $START_DATE to $END_DATE" >> "$SUMMARY_FILE"
echo "" >> "$SUMMARY_FILE"
printf "%-25s %8s %8s %10s %10s %12s\n" "Signal" "Bets" "Win%" "p-value" "EV/bet" "Go/No-Go" >> "$SUMMARY_FILE"
printf "%-25s %8s %8s %10s %10s %12s\n" "-------------------------" "--------" "--------" "----------" "----------" "------------" >> "$SUMMARY_FILE"

for signal in "${SIGNALS[@]}"; do
    echo "Testing: $signal"

    OUTPUT_FILE="$OUTPUT_DIR/${signal}.json"

    # Run backtest with JSON output
    cargo run -p algo-trade-cli --release -- binary-backtest \
        --start "$START_DATE" \
        --end "$END_DATE" \
        --signal "$signal" \
        --min-strength 0.5 \
        --stake 100 \
        --format json \
        --output "$OUTPUT_FILE" 2>/dev/null || {
            echo "  (no data or error)"
            continue
        }

    # Extract metrics if file exists
    if [ -f "$OUTPUT_FILE" ]; then
        BETS=$(jq -r '.backtest.metrics.total_bets // 0' "$OUTPUT_FILE")
        WIN_RATE=$(jq -r '.backtest.metrics.win_rate // 0' "$OUTPUT_FILE" | awk '{printf "%.1f", $1 * 100}')
        P_VALUE=$(jq -r '.backtest.metrics.binomial_p_value // 1' "$OUTPUT_FILE" | awk '{printf "%.4f", $1}')
        EV=$(jq -r '.backtest.metrics.ev_per_bet // 0' "$OUTPUT_FILE" | awk '{printf "%.2f", $1}')

        # Determine Go/No-Go
        GO_NOGO="NO-GO"
        if [ "$BETS" -ge 100 ] && [ "$(echo "$WIN_RATE > 53" | bc)" -eq 1 ] && [ "$(echo "$P_VALUE < 0.05" | bc)" -eq 1 ]; then
            GO_NOGO="GO"
        elif [ "$BETS" -lt 100 ]; then
            GO_NOGO="PENDING"
        elif [ "$(echo "$P_VALUE < 0.10" | bc)" -eq 1 ] && [ "$(echo "$WIN_RATE > 52" | bc)" -eq 1 ]; then
            GO_NOGO="CONDITIONAL"
        fi

        printf "%-25s %8d %7s%% %10s $%9s %12s\n" "$signal" "$BETS" "$WIN_RATE" "$P_VALUE" "$EV" "$GO_NOGO" >> "$SUMMARY_FILE"
        echo "  Bets: $BETS, Win: ${WIN_RATE}%, p=$P_VALUE -> $GO_NOGO"
    fi
done

echo ""
echo "=========================================="
echo "Results saved to: $SUMMARY_FILE"
echo "=========================================="
echo ""
cat "$SUMMARY_FILE"
