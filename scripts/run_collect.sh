#!/bin/bash
#
# Standalone Data Collection
#
# Collects raw market data (order book, funding, liquidations, trade ticks,
# Polymarket odds, news) and optionally computes composite signals.
# Designed for EC2 data collection without running a trading bot.
#
# Usage:
#   ./scripts/run_collect.sh [options]
#
# Options:
#   --duration <time>   How long to collect (default: 24h)
#   --coins <list>      Coins for signal aggregation (default: btc,eth,sol,xrp)
#   --sources <list>    Raw data sources (default: orderbook,funding,liquidations,tradeticks,polymarket,news)
#   --no-signals        Disable composite signal computation
#   --help              Show this help
#
# Examples:
#   ./scripts/run_collect.sh                          # All data + signals, 24h
#   ./scripts/run_collect.sh --duration 7d            # Collect for a week
#   ./scripts/run_collect.sh --coins btc,eth          # Specific coins only
#   ./scripts/run_collect.sh --no-signals             # Raw data only, no composite signals
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Auto-source .env file if it exists
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

# =============================================================================
# Default configuration
# =============================================================================

DURATION="24h"
COINS="btc,eth,sol,xrp"
SOURCES="orderbook,funding,liquidations,tradeticks,polymarket,news"
SIGNALS="1"

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --coins)
            COINS="$2"
            shift 2
            ;;
        --sources)
            SOURCES="$2"
            shift 2
            ;;
        --no-signals)
            SIGNALS=""
            shift
            ;;
        --help|-h)
            head -28 "$0" | tail -27
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# =============================================================================
# Validation
# =============================================================================

if [[ -z "${DATABASE_URL:-}" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL required for data collection${NC}"
    exit 1
fi

# =============================================================================
# Migrations
# =============================================================================

echo -e "${DIM}Running database migrations...${NC}"
"$SCRIPT_DIR/migrate.sh" 2>&1 | grep -E '\[APPLY\]|\[SKIP\]|Error' || true
echo ""

# =============================================================================
# Build
# =============================================================================

echo -e "${DIM}Building release binary...${NC}"
if ! cargo build -p algo-trade-cli --release 2>&1 | tail -3; then
    echo -e "${RED}Build failed.${NC}"
    exit 1
fi
echo ""

# =============================================================================
# Build command
# =============================================================================

CMD=(cargo run -p algo-trade-cli --release -- collect-signals)
CMD+=(--duration "$DURATION")
CMD+=(--coins "$COINS")
CMD+=(--sources "$SOURCES")
[[ -n "$SIGNALS" ]] && CMD+=(--signals)

# =============================================================================
# Display configuration
# =============================================================================

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Data Collection${NC}                                        ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Duration:${NC}   $DURATION"
echo -e "  ${DIM}Coins:${NC}      ${COINS^^}"
echo -e "  ${DIM}Sources:${NC}    $SOURCES"
if [[ -n "$SIGNALS" ]]; then
    echo -e "  ${DIM}Signals:${NC}    ${GREEN}ENABLED${NC} (7-signal composite)"
else
    echo -e "  ${DIM}Signals:${NC}    ${DIM}disabled${NC}"
fi
echo ""

# =============================================================================
# Run
# =============================================================================

mkdir -p "$PROJECT_ROOT/logs"
LOG_FILE="$PROJECT_ROOT/logs/collect-$(date +%Y%m%d-%H%M%S).log"

echo -e "${GREEN}Starting data collection...${NC}"
echo -e "${DIM}Press Ctrl+C to stop${NC}"
echo -e "${DIM}Logs: ${LOG_FILE}${NC}"
echo ""

BOT_PID=""
cleanup() {
    if [[ -n "$BOT_PID" ]]; then
        kill "$BOT_PID" 2>/dev/null || true
        wait "$BOT_PID" 2>/dev/null || true
    fi
    echo ""
    echo -e "${DIM}Log file: ${LOG_FILE}${NC}"
}
trap cleanup EXIT INT TERM

export RUST_LOG="${RUST_LOG:-info}"

echo -e "${DIM}─────────────────────────────────────────────────────────────────────${NC}"
echo ""
"${CMD[@]}" 2>&1 | tee "$LOG_FILE" &
BOT_PID=$!

wait "$BOT_PID" 2>/dev/null || true
BOT_PID=""

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Data collection stopped${NC}"

if [[ -f "$LOG_FILE" ]]; then
    LINES=$(wc -l < "$LOG_FILE")
    echo ""
    echo -e "${WHITE}Log file:${NC} $LOG_FILE ($LINES lines)"
    echo -e "${DIM}View with: less -R $LOG_FILE${NC}"
fi

echo ""
