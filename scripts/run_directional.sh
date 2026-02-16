#!/bin/bash
#
# Directional Trading Runner
#
# Runs the single-leg directional trading bot with spot price confirmation.
#
# Usage:
#   ./scripts/run_directional.sh [options]           # Run locally
#   ./scripts/run_directional.sh redeploy            # Build + upload binary + migrate on EC2
#   ./scripts/run_directional.sh start [options]     # Start on EC2 (background)
#   ./scripts/run_directional.sh stop                # Stop on EC2
#   ./scripts/run_directional.sh logs                # Tail remote logs
#   ./scripts/run_directional.sh ssh                 # SSH into EC2 instance
#
# Local options:
#   --mode paper|live           Trading mode (default: paper)
#   --duration <time>          How long to run (default: 1h)
#   --coins <list>             Coins to trade (default: btc,eth,sol,xrp)
#   --bet-size <amount>        Fixed bet size in USDC (default: Kelly sizing)
#   --min-edge <val>           Minimum edge to trade (default: 0.03)
#   --max-entry-price <val>    Max entry price (default: 0.55)
#   --max-trades <n>           Max trades per 15-min window (default: 1)
#   --kelly <fraction>         Kelly fraction 0.0-1.0 (default: 0.25)
#   --paper-balance <amt>      Paper mode starting balance (default: 1000)
#   --signals                  Enable Binance signal aggregation
#   --raw-persist              Persist raw Binance data (OB/funding/liq) to DB
#   --verbose                  Show verbose log output instead of dashboard
#   --help                     Show this help
#
# Examples:
#   ./scripts/run_directional.sh                                    # Paper, 1h, all coins
#   ./scripts/run_directional.sh --mode live --bet-size 10          # Live, $10 bets
#   ./scripts/run_directional.sh --coins btc,eth --min-edge 0.05   # BTC+ETH only
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

_DIR_BOT_NAME="directional"
_DIR_PID_FILE="/tmp/directional.pid"
_DIR_LOG_FILE="/tmp/directional.log"
_DIR_PROCESS_PATTERN="algo-trade directional-auto"
_DIR_CLI_CMD="directional-auto"

_dir_build_remote_args() {
    local args="--mode paper --duration 1h --coins btc,eth,sol,xrp --persist --verbose"
    [[ $# -gt 0 ]] && args="$*"
    echo "$args"
}

if ec2_dispatch "$_DIR_BOT_NAME" "$_DIR_PID_FILE" "$_DIR_LOG_FILE" \
    "$_DIR_PROCESS_PATTERN" "$_DIR_CLI_CMD" "_dir_build_remote_args" "$@"; then
    exit 0
fi

# =============================================================================
# Default configuration
# =============================================================================

MODE="paper"
DURATION="1h"
COINS="btc,eth,sol,xrp"
BET_SIZE=""
KELLY_FRACTION="0.25"
MIN_EDGE="0.03"
MAX_ENTRY_PRICE="0.55"
MIN_DELTA="0.0005"
ENTRY_START_MINS="10"
ENTRY_END_MINS="2"
MAX_POSITION="200"
MAX_TRADES_PER_WINDOW="1"
PAPER_BALANCE="1000"
STATS_INTERVAL="5"
VERBOSE=""
PERSIST=""
RAW_PERSIST=""
SESSION_ID=""
SIGNALS=""

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --mode)
            MODE="$2"
            shift 2
            ;;
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --coins)
            COINS="$2"
            shift 2
            ;;
        --bet-size)
            BET_SIZE="$2"
            shift 2
            ;;
        --kelly)
            KELLY_FRACTION="$2"
            shift 2
            ;;
        --min-edge)
            MIN_EDGE="$2"
            shift 2
            ;;
        --max-entry-price)
            MAX_ENTRY_PRICE="$2"
            shift 2
            ;;
        --min-delta)
            MIN_DELTA="$2"
            shift 2
            ;;
        --entry-start-mins)
            ENTRY_START_MINS="$2"
            shift 2
            ;;
        --entry-end-mins)
            ENTRY_END_MINS="$2"
            shift 2
            ;;
        --max-position)
            MAX_POSITION="$2"
            shift 2
            ;;
        --max-trades)
            MAX_TRADES_PER_WINDOW="$2"
            shift 2
            ;;
        --paper-balance)
            PAPER_BALANCE="$2"
            shift 2
            ;;
        --stats-interval)
            STATS_INTERVAL="$2"
            shift 2
            ;;
        --persist)
            PERSIST="1"
            shift
            ;;
        --raw-persist)
            RAW_PERSIST="1"
            shift
            ;;
        --signals)
            SIGNALS="1"
            shift
            ;;
        --session-id)
            SESSION_ID="$2"
            shift 2
            ;;
        --verbose|-v)
            VERBOSE="1"
            shift
            ;;
        --help|-h)
            head -34 "$0" | tail -33
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

if [[ "$MODE" == "live" && -z "${POLYMARKET_PRIVATE_KEY:-}" ]]; then
    echo -e "${RED}ERROR: POLYMARKET_PRIVATE_KEY required for live mode${NC}"
    exit 1
fi

if [[ -n "$PERSIST" && -z "${DATABASE_URL:-}" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL required for --persist${NC}"
    exit 1
fi

# =============================================================================
# Migrations
# =============================================================================

if [[ -n "$PERSIST" && -n "${DATABASE_URL:-}" ]]; then
    echo -e "${DIM}Running database migrations...${NC}"
    "$SCRIPT_DIR/migrate.sh" 2>&1 | grep -E '\[APPLY\]|\[SKIP\]|Error' || true
    echo ""
fi

# =============================================================================
# Build
# =============================================================================

CARGO_PROFILE=""
if [[ "$MODE" == "live" ]]; then
    CARGO_PROFILE="--release"
    echo -e "${DIM}Building release binary...${NC}"
    if ! cargo build -p algo-trade-cli --release 2>&1 | tail -3; then
        echo -e "${RED}Build failed.${NC}"
        exit 1
    fi
    echo ""
fi

# =============================================================================
# Build command
# =============================================================================

CMD=(cargo run -p algo-trade-cli)
[[ -n "$CARGO_PROFILE" ]] && CMD+=($CARGO_PROFILE)
CMD+=(-- directional-auto)
CMD+=(--mode "$MODE")
CMD+=(--duration "$DURATION")
CMD+=(--coins "$COINS")
CMD+=(--min-edge "$MIN_EDGE")
CMD+=(--max-entry-price "$MAX_ENTRY_PRICE")
CMD+=(--min-delta "$MIN_DELTA")
CMD+=(--entry-start-mins "$ENTRY_START_MINS")
CMD+=(--entry-end-mins "$ENTRY_END_MINS")
CMD+=(--max-position "$MAX_POSITION")
CMD+=(--max-trades-per-window "$MAX_TRADES_PER_WINDOW")
CMD+=(--kelly-fraction "$KELLY_FRACTION")
CMD+=(--stats-interval-secs "$STATS_INTERVAL")

[[ -n "$BET_SIZE" ]] && CMD+=(--bet-size "$BET_SIZE")
[[ "$MODE" == "paper" ]] && CMD+=(--paper-balance "$PAPER_BALANCE")
[[ -n "$VERBOSE" ]] && CMD+=(--verbose)
[[ -n "$PERSIST" ]] && CMD+=(--persist)
[[ -n "$RAW_PERSIST" ]] && CMD+=(--raw-persist)
[[ -n "$SIGNALS" ]] && CMD+=(--signals)
[[ -n "$SESSION_ID" ]] && CMD+=(--session-id "$SESSION_ID")

# =============================================================================
# Display configuration
# =============================================================================

clear

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Directional Trading Bot${NC}                                ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Mode:${NC}          ${MODE^^}"
if [[ "$MODE" == "live" ]]; then
    echo -e "                 ${RED}*** REAL FUNDS WILL BE USED ***${NC}"
fi
echo -e "  ${DIM}Duration:${NC}      $DURATION"
echo -e "  ${DIM}Coins:${NC}         ${COINS^^}"
if [[ -n "$BET_SIZE" ]]; then
    echo -e "  ${DIM}Bet Size:${NC}      \$$BET_SIZE (fixed)"
else
    echo -e "  ${DIM}Bet Size:${NC}      Kelly ${KELLY_FRACTION} (dynamic)"
fi
echo -e "  ${DIM}Min Edge:${NC}      ${MIN_EDGE}"
echo -e "  ${DIM}Max Entry:${NC}     \$$MAX_ENTRY_PRICE"
echo -e "  ${DIM}Max/Window:${NC}    \$$MAX_POSITION (${MAX_TRADES_PER_WINDOW} trades)"
if [[ "$MODE" == "paper" ]]; then
    echo -e "  ${DIM}Paper Balance:${NC} \$$PAPER_BALANCE"
fi
if [[ -n "$SIGNALS" ]]; then
    echo -e "  ${DIM}Signals:${NC}       ${GREEN}ENABLED${NC} (Binance OB/funding/liq)"
fi
if [[ -n "$RAW_PERSIST" ]]; then
    echo -e "  ${DIM}Raw Persist:${NC}   ${GREEN}ENABLED${NC} (OB/funding/liq to DB)"
fi
echo ""

# Confirm for live mode
if [[ "$MODE" == "live" ]]; then
    echo -e "${YELLOW}WARNING: You are about to start LIVE trading with real funds!${NC}"
    echo -e "${DIM}Command: ${CMD[*]}${NC}"
    echo ""
    read -rp "Type 'yes' to confirm: " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "Aborted."
        exit 1
    fi
    echo ""
fi

# =============================================================================
# Run
# =============================================================================

mkdir -p "$PROJECT_ROOT/logs"
LOG_FILE="$PROJECT_ROOT/logs/directional-${MODE}-$(date +%Y%m%d-%H%M%S).log"

echo -e "${GREEN}Starting bot...${NC}"
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

if [[ -n "$VERBOSE" ]]; then
    echo -e "${DIM}─────────────────────────────────────────────────────────────────────${NC}"
    echo ""
    "${CMD[@]}" 2>&1 | tee "$LOG_FILE" &
    BOT_PID=$!
else
    "${CMD[@]}" 2>"$LOG_FILE" &
    BOT_PID=$!
fi

wait "$BOT_PID" 2>/dev/null || true
BOT_PID=""

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Bot stopped${NC}"

if [[ -f "$LOG_FILE" ]]; then
    LINES=$(wc -l < "$LOG_FILE")
    echo ""
    echo -e "${WHITE}Log file:${NC} $LOG_FILE ($LINES lines)"
    echo -e "${DIM}View with: less -R $LOG_FILE${NC}"
fi

echo ""
