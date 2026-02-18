#!/bin/bash
#
# CLOB First-Move Timing Strategy Runner
#
# Observes CLOB prices 2.5-5 min into each 15-min window.
# When displacement from midpoint (0.50) exceeds threshold, trades in that direction.
# Data shows 82% win rate at 15c, 93-96% at 20c for BTC+ETH.
#
# Usage:
#   ./scripts/run_clob_timing.sh [options]           # Run locally
#   ./scripts/run_clob_timing.sh settle [options]    # Settle unsettled trades from previous sessions
#   ./scripts/run_clob_timing.sh redeploy            # Build + upload binary + migrate on EC2
#   ./scripts/run_clob_timing.sh start [options]     # Start on EC2 (background)
#   ./scripts/run_clob_timing.sh stop                # Stop on EC2
#   ./scripts/run_clob_timing.sh logs                # Tail remote logs
#   ./scripts/run_clob_timing.sh ssh                 # SSH into EC2 instance
#
# Settle options:
#   --coin <coin>              Filter by coin (e.g., BTC)
#   --session <id>             Filter by session ID
#   --max-age-hours <hours>    Only trades newer than this
#   --dry-run                  Show pending trades without settling
#
# Local options:
#   --mode paper|live           Trading mode (default: paper)
#   --duration <time>          How long to run (default: 4h)
#   --coins <list>             Coins to trade (default: btc,eth)
#   --bet-size <amount>        Fixed bet size in USDC (default: Kelly sizing)
#   --kelly <fraction>         Kelly fraction 0.0-1.0 (default: 0.25)
#   --min-displacement <val>   Min CLOB deviation from 0.50 (default: 0.15)
#   --max-entry-price <val>    Max entry price (default: 0.85)
#   --observation-delay <sec>  Seconds into window to start checking (default: 150)
#   --observation-end <sec>    Seconds into window to stop checking (default: 300)
#   --min-edge <val>           Minimum edge to trade (default: 0.05)
#   --max-position <amt>       Max position per window (default: 200)
#   --max-trades <n>           Max trades per 15-min window (default: 1)
#   --paper-balance <amt>      Paper mode starting balance (default: 1000)
#   --exclude-hours <hours>     UTC hours to skip (default: 4,9,21,22,23)
#   --persist                  Save trades to database
#   --verbose                  Show verbose log output instead of dashboard
#   --help                     Show this help
#
# Examples:
#   ./scripts/run_clob_timing.sh                                          # Paper, 4h, BTC+ETH
#   ./scripts/run_clob_timing.sh --mode live --bet-size 10                # Live, $10 bets
#   ./scripts/run_clob_timing.sh --min-displacement 0.20 --coins btc     # BTC only, 20c threshold
#   ./scripts/run_clob_timing.sh settle                                   # Settle all pending trades
#   ./scripts/run_clob_timing.sh settle --dry-run                         # Show pending without settling
#   ./scripts/run_clob_timing.sh settle --coin BTC --max-age-hours 24     # Settle recent BTC trades
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

_CT_BOT_NAME="clob-timing"
_CT_PID_FILE="/tmp/clob-timing.pid"
_CT_LOG_FILE="/tmp/clob-timing.log"
_CT_PROCESS_PATTERN="algo-trade clob-timing"
_CT_CLI_CMD="clob-timing"

_ct_build_remote_args() {
    local args="--mode live --duration 24h --coins btc,eth,sol,xrp --min-displacement 0.15 --bet-size 5 --exclude-hours 4,9,14 --max-position 20 --max-trades-per-window 4 --verbose --persist"
    [[ $# -gt 0 ]] && args="$*"
    echo "$args"
}

if ec2_dispatch "$_CT_BOT_NAME" "$_CT_PID_FILE" "$_CT_LOG_FILE" \
    "$_CT_PROCESS_PATTERN" "$_CT_CLI_CMD" "_ct_build_remote_args" "$@"; then
    exit 0
fi

# =============================================================================
# Settle subcommand
# =============================================================================

if [[ "${1:-}" == "settle" ]]; then
    shift

    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Directional Trade Settlement (EC2: ${PUBLIC_IP})${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    # Build remote args string
    SETTLE_ARGS=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)
                SETTLE_ARGS+=" --dry-run"
                shift
                ;;
            --coin)
                SETTLE_ARGS+=" --coin $2"
                shift 2
                ;;
            --session)
                SETTLE_ARGS+=" --session $2"
                shift 2
                ;;
            --max-age-hours)
                SETTLE_ARGS+=" --max-age-hours $2"
                shift 2
                ;;
            --no-redeem)
                SETTLE_ARGS+=" --no-redeem"
                shift
                ;;
            --verbose|-v)
                SETTLE_ARGS+=" --verbose"
                shift
                ;;
            *)
                echo "Unknown settle option: $1"
                exit 1
                ;;
        esac
    done

    info "Running settlement on EC2..."
    remote_ssh "bash -c 'set -a && source ~/.env && set +a && RUST_LOG=info ~/algo-trade directional-settle${SETTLE_ARGS}'"
    exit $?
fi

# =============================================================================
# Default configuration
# =============================================================================

MODE="paper"
DURATION="4h"
COINS="btc,eth,sol,xrp"
BET_SIZE="5"
KELLY_FRACTION="0.25"
MIN_DISPLACEMENT="0.15"
MAX_ENTRY_PRICE="0.85"
OBSERVATION_DELAY="150"
OBSERVATION_END="300"
MIN_EDGE="0.05"
MAX_POSITION="20"
MAX_TRADES_PER_WINDOW="1"
PAPER_BALANCE="1000"
EXCLUDE_HOURS="4,9,14"
VERBOSE=""
PERSIST=""
SESSION_ID=""

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
        --min-displacement)
            MIN_DISPLACEMENT="$2"
            shift 2
            ;;
        --max-entry-price)
            MAX_ENTRY_PRICE="$2"
            shift 2
            ;;
        --observation-delay)
            OBSERVATION_DELAY="$2"
            shift 2
            ;;
        --observation-end)
            OBSERVATION_END="$2"
            shift 2
            ;;
        --min-edge)
            MIN_EDGE="$2"
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
        --exclude-hours)
            EXCLUDE_HOURS="$2"
            shift 2
            ;;
        --persist)
            PERSIST="1"
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
            head -39 "$0" | tail -38
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
CMD+=(-- clob-timing)
CMD+=(--mode "$MODE")
CMD+=(--duration "$DURATION")
CMD+=(--coins "$COINS")
CMD+=(--kelly-fraction "$KELLY_FRACTION")
CMD+=(--min-displacement "$MIN_DISPLACEMENT")
CMD+=(--max-entry-price "$MAX_ENTRY_PRICE")
CMD+=(--observation-delay "$OBSERVATION_DELAY")
CMD+=(--observation-end "$OBSERVATION_END")
CMD+=(--min-edge "$MIN_EDGE")
CMD+=(--max-position "$MAX_POSITION")
CMD+=(--max-trades-per-window "$MAX_TRADES_PER_WINDOW")
CMD+=(--exclude-hours "$EXCLUDE_HOURS")

[[ -n "$BET_SIZE" ]] && CMD+=(--bet-size "$BET_SIZE")
[[ "$MODE" == "paper" ]] && CMD+=(--paper-balance "$PAPER_BALANCE")
[[ -n "$VERBOSE" ]] && CMD+=(--verbose)
[[ -n "$PERSIST" ]] && CMD+=(--persist)
[[ -n "$SESSION_ID" ]] && CMD+=(--session-id "$SESSION_ID")

# =============================================================================
# Display configuration
# =============================================================================

clear

echo -e "${CYAN}+==================================================================+${NC}"
echo -e "${CYAN}|${NC}        ${WHITE}CLOB First-Move Timing Strategy${NC}                       ${CYAN}|${NC}"
echo -e "${CYAN}+==================================================================+${NC}"
echo ""

echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Mode:${NC}             ${MODE^^}"
if [[ "$MODE" == "live" ]]; then
    echo -e "                    ${RED}*** REAL FUNDS WILL BE USED ***${NC}"
fi
echo -e "  ${DIM}Duration:${NC}         $DURATION"
echo -e "  ${DIM}Coins:${NC}            ${COINS^^}"
if [[ -n "$BET_SIZE" ]]; then
    echo -e "  ${DIM}Bet Size:${NC}         \$$BET_SIZE (fixed)"
else
    echo -e "  ${DIM}Bet Size:${NC}         Kelly ${KELLY_FRACTION} (dynamic)"
fi
echo -e "  ${DIM}Min Displacement:${NC} ${MIN_DISPLACEMENT} (from 0.50)"
echo -e "  ${DIM}Max Entry:${NC}        \$$MAX_ENTRY_PRICE"
echo -e "  ${DIM}Observation:${NC}      ${OBSERVATION_DELAY}s - ${OBSERVATION_END}s into window"
echo -e "  ${DIM}Min Edge:${NC}         ${MIN_EDGE}"
echo -e "  ${DIM}Excluded Hours:${NC}   ${EXCLUDE_HOURS} UTC"
echo -e "  ${DIM}Max/Window:${NC}       \$$MAX_POSITION (${MAX_TRADES_PER_WINDOW} trades)"
if [[ "$MODE" == "paper" ]]; then
    echo -e "  ${DIM}Paper Balance:${NC}    \$$PAPER_BALANCE"
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
# Settle outstanding trades before starting
# =============================================================================

if [[ -n "$PERSIST" && -n "${DATABASE_URL:-}" ]]; then
    echo -e "${WHITE}Settling outstanding trades from previous sessions...${NC}"
    if cargo run -p algo-trade-cli -- directional-settle 2>&1 | tail -20; then
        echo ""
    else
        echo -e "${YELLOW}Settlement check completed (some may have failed)${NC}"
        echo ""
    fi
fi

# =============================================================================
# Run
# =============================================================================

mkdir -p "$PROJECT_ROOT/logs"
LOG_FILE="$PROJECT_ROOT/logs/clob-timing-${MODE}-$(date +%Y%m%d-%H%M%S).log"

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
    echo -e "${DIM}---------------------------------------------------------------------${NC}"
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
echo -e "${CYAN}===================================================================${NC}"
echo -e "${WHITE}Bot stopped${NC}"

if [[ -f "$LOG_FILE" ]]; then
    LINES=$(wc -l < "$LOG_FILE")
    echo ""
    echo -e "${WHITE}Log file:${NC} $LOG_FILE ($LINES lines)"
    echo -e "${DIM}View with: less -R $LOG_FILE${NC}"
fi

echo ""
