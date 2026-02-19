#!/bin/bash
#
# Standalone Data Collection
#
# Collects raw market data (order book, funding, liquidations, trade ticks,
# Polymarket odds, news) and optionally computes composite signals.
# Designed for EC2 data collection without running a trading bot.
#
# Usage:
#   ./scripts/run_collect.sh [options]           # Run locally
#   ./scripts/run_collect.sh redeploy            # Build + upload binary + migrate on EC2
#   ./scripts/run_collect.sh start [options]     # Start collector on EC2 (background)
#   ./scripts/run_collect.sh stop                # Stop collector on EC2
#   ./scripts/run_collect.sh logs                # Tail remote collector logs
#   ./scripts/run_collect.sh ssh                 # SSH into EC2 instance
#
# Local options:
#   --duration <time>   How long to collect (default: 24h)
#   --coins <list>      Coins for signal aggregation (default: btc,eth,sol,xrp)
#   --sources <list>    Raw data sources (default: all including clobprices,settlements)
#   --no-signals        Disable composite signal computation
#   --help              Show this help
#
# Examples:
#   ./scripts/run_collect.sh                          # All data + signals, 24h (local)
#   ./scripts/run_collect.sh --duration 7d            # Collect for a week (local)
#   ./scripts/run_collect.sh redeploy                 # Build + upload to EC2
#   ./scripts/run_collect.sh start                    # Start collector on EC2
#   ./scripts/run_collect.sh start --duration 7d      # Start with custom duration
#   ./scripts/run_collect.sh stop                     # Stop EC2 collector
#   ./scripts/run_collect.sh logs                     # Tail EC2 logs
#   ./scripts/run_collect.sh ssh                      # SSH into EC2 instance
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

# =============================================================================
# EC2 support via shared library
# =============================================================================

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

_COLLECT_BOT_NAME="collector"
_COLLECT_PID_FILE="/tmp/collector.pid"
_COLLECT_LOG_FILE="/tmp/collector.log"
_COLLECT_PROCESS_PATTERN="algo-trade collect-signals"
_COLLECT_CLI_CMD="collect-signals"

_collect_build_remote_args() {
    local duration="24h"
    local coins="btc,eth,sol,xrp"
    local sources="orderbook,funding,liquidations,tradeticks,polymarket,news,clobprices,settlements"
    local signals="1"

    while [[ $# -gt 0 ]]; do
        case $1 in
            --duration)  duration="$2"; shift 2 ;;
            --coins)     coins="$2"; shift 2 ;;
            --sources)   sources="$2"; shift 2 ;;
            --no-signals) signals=""; shift ;;
            *)           shift ;;
        esac
    done

    local args="--duration $duration --coins $coins --sources $sources"
    [[ -n "$signals" ]] && args+=" --signals"
    echo "$args"
}

if ec2_dispatch "$_COLLECT_BOT_NAME" "$_COLLECT_PID_FILE" "$_COLLECT_LOG_FILE" \
    "$_COLLECT_PROCESS_PATTERN" "$_COLLECT_CLI_CMD" "_collect_build_remote_args" "$@"; then
    exit 0
fi

# =============================================================================
# Help
# =============================================================================

case "${1:-}" in
    --help|-h)
        head -33 "$0" | tail -32
        exit 0
        ;;
esac

# =============================================================================
# Local run (original behavior)
# =============================================================================

# Default configuration
DURATION="24h"
COINS="btc,eth,sol,xrp"
SOURCES="orderbook,funding,liquidations,tradeticks,polymarket,news,clobprices,settlements"
SIGNALS="1"

# Parse arguments
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
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Validation
if [[ -z "${DATABASE_URL:-}" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL required for data collection${NC}"
    exit 1
fi

# Migrations
echo -e "${DIM}Running database migrations...${NC}"
"$SCRIPT_DIR/migrate.sh" 2>&1 | grep -E '\[APPLY\]|\[SKIP\]|Error' || true
echo ""

# Build
echo -e "${DIM}Building release binary...${NC}"
if ! cargo build -p algo-trade-cli --release 2>&1 | tail -3; then
    echo -e "${RED}Build failed.${NC}"
    exit 1
fi
echo ""

# Build command
CMD=(cargo run -p algo-trade-cli --release -- collect-signals)
CMD+=(--duration "$DURATION")
CMD+=(--coins "$COINS")
CMD+=(--sources "$SOURCES")
[[ -n "$SIGNALS" ]] && CMD+=(--signals)

# Display configuration
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

# Run
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
