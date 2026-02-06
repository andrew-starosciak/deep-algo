#!/bin/bash
#
# Cross-Market Auto Trading Runner
#
# Runs the automated cross-market correlation arbitrage bot with a live dashboard.
#
# Usage:
#   ./scripts/run_cross_market_auto.sh [options]
#
# Options:
#   --overnight           Run overnight (12h duration, persist, log to file)
#   --mode paper|live     Trading mode (default: paper)
#   --duration <time>     How long to run (default: 1h)
#   --bet-size <amount>   Fixed bet size per leg in USDC (default: Kelly sizing)
#   --pair <coins>        Coin pair to trade (default: btc,eth)
#   --combination <type>  Combination filter (default: coin1down_coin2up)
#   --min-spread <val>    Minimum spread required (default: 0.03)
#   --min-win-prob <val>  Minimum win probability (default: 0.85)
#   --max-position <val>  Max position per window in USDC (default: 500)
#   --kelly <fraction>    Kelly fraction 0.0-1.0 (default: 0.25)
#   --paper-balance <amt> Paper mode starting balance (default: 1000)
#   --no-persist          Disable database persistence
#   --session <id>        Custom session ID
#   --verbose             Show verbose log output instead of dashboard
#   --help                Show this help
#
# Examples:
#   ./scripts/run_cross_market_auto.sh                                   # Paper, 1h
#   ./scripts/run_cross_market_auto.sh --mode live --bet-size 5          # Live, $5 bets
#   ./scripts/run_cross_market_auto.sh --overnight                       # Paper, 12h
#   ./scripts/run_cross_market_auto.sh --mode live --overnight           # Live, 12h
#

set -euo pipefail

# =============================================================================
# Setup
# =============================================================================

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
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

# =============================================================================
# Default configuration
# =============================================================================

MODE="paper"
DURATION="1h"
PAIR="btc,eth"
COMBINATION="coin1down_coin2up"
BET_SIZE=""
KELLY_FRACTION="0.25"
MIN_SPREAD="0.03"
MIN_WIN_PROB="0.85"
MAX_POSITION="15"
PAPER_BALANCE="1000"
PERSIST="--persist"
SESSION_ID=""
VERBOSE=""
OVERNIGHT=""
LOG_FILE=""
STATS_INTERVAL="1"

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --overnight)
            OVERNIGHT="1"
            DURATION="12h"
            shift
            ;;
        --mode)
            MODE="$2"
            shift 2
            ;;
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --pair)
            PAIR="$2"
            shift 2
            ;;
        --combination)
            COMBINATION="$2"
            shift 2
            ;;
        --bet-size)
            BET_SIZE="$2"
            shift 2
            ;;
        --min-spread)
            MIN_SPREAD="$2"
            shift 2
            ;;
        --min-win-prob)
            MIN_WIN_PROB="$2"
            shift 2
            ;;
        --max-position)
            MAX_POSITION="$2"
            shift 2
            ;;
        --paper-balance)
            PAPER_BALANCE="$2"
            shift 2
            ;;
        --kelly)
            KELLY_FRACTION="$2"
            shift 2
            ;;
        --stats-interval)
            STATS_INTERVAL="$2"
            shift 2
            ;;
        --no-persist)
            PERSIST=""
            shift
            ;;
        --session)
            SESSION_ID="$2"
            shift 2
            ;;
        --verbose|-v)
            VERBOSE="1"
            shift
            ;;
        --help|-h)
            head -35 "$0" | tail -34
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

# Check for DATABASE_URL if persisting
if [[ -n "$PERSIST" && -z "${DATABASE_URL:-}" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL environment variable required for --persist${NC}"
    echo "Set it in .env or: export DATABASE_URL=postgres://user:pass@localhost/dbname"
    echo "Or disable with: --no-persist"
    exit 1
fi

# Check for wallet key in live mode
if [[ "$MODE" == "live" && -z "${POLYMARKET_PRIVATE_KEY:-}" ]]; then
    echo -e "${RED}ERROR: POLYMARKET_PRIVATE_KEY environment variable required for live mode${NC}"
    echo "Set it in .env or: export POLYMARKET_PRIVATE_KEY=your_64char_hex_key"
    exit 1
fi

# Generate session ID if not provided and persisting
if [[ -n "$PERSIST" && -z "$SESSION_ID" ]]; then
    SESSION_ID="auto-$(date +%Y%m%d-%H%M%S)"
fi

# Set log file for overnight mode
if [[ -n "$OVERNIGHT" ]]; then
    mkdir -p "$PROJECT_ROOT/logs"
    LOG_FILE="$PROJECT_ROOT/logs/${MODE}-$(date +%Y%m%d-%H%M%S).log"
fi

# =============================================================================
# Build release binary (live mode always uses release for performance)
# =============================================================================

CARGO_PROFILE=""
if [[ "$MODE" == "live" ]]; then
    CARGO_PROFILE="--release"
    echo -e "${DIM}Building release binary...${NC}"
    if ! cargo build -p algo-trade-cli --release 2>&1 | tail -3; then
        echo -e "${RED}Build failed. Fix errors before live trading.${NC}"
        exit 1
    fi
    echo ""
fi

# =============================================================================
# Preflight check (live mode)
# =============================================================================

if [[ "$MODE" == "live" ]]; then
    echo -e "${WHITE}Running preflight checks...${NC}"
    if ! cargo run -p algo-trade-cli $CARGO_PROFILE -- preflight --coins "$PAIR" 2>&1; then
        echo ""
        echo -e "${RED}Preflight failed. Fix issues before live trading.${NC}"
        exit 1
    fi
    echo ""
fi

# =============================================================================
# Build command
# =============================================================================

CMD=(cargo run -p algo-trade-cli)
[[ -n "$CARGO_PROFILE" ]] && CMD+=($CARGO_PROFILE)
CMD+=(-- cross-market-auto)
CMD+=(--pair "$PAIR")
CMD+=(--combination "$COMBINATION")
CMD+=(--mode "$MODE")
CMD+=(--duration "$DURATION")
CMD+=(--min-spread "$MIN_SPREAD")
CMD+=(--min-win-prob "$MIN_WIN_PROB")
CMD+=(--max-position "$MAX_POSITION")
CMD+=(--kelly-fraction "$KELLY_FRACTION")
CMD+=(--stats-interval-secs "$STATS_INTERVAL")

# Only pass --bet-size if explicitly set (otherwise Kelly sizing is used)
[[ -n "$BET_SIZE" ]] && CMD+=(--bet-size "$BET_SIZE")

# Paper mode settings
[[ "$MODE" == "paper" ]] && CMD+=(--paper-balance "$PAPER_BALANCE")

# Persistence
[[ -n "$PERSIST" ]] && CMD+=($PERSIST)
[[ -n "$SESSION_ID" ]] && CMD+=(--session-id "$SESSION_ID")

# Verbose
[[ -n "$VERBOSE" ]] && CMD+=(--verbose)

# =============================================================================
# Display configuration
# =============================================================================

clear

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Cross-Market Correlation Arbitrage Bot${NC}                   ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Mode:${NC}          ${MODE^^}"
if [[ "$MODE" == "live" ]]; then
    echo -e "                 ${RED}*** REAL FUNDS WILL BE USED ***${NC}"
fi
echo -e "  ${DIM}Duration:${NC}      $DURATION"
echo -e "  ${DIM}Pair:${NC}          ${PAIR^^}"
echo -e "  ${DIM}Combination:${NC}   $COMBINATION"
if [[ -n "$BET_SIZE" ]]; then
    echo -e "  ${DIM}Bet Size:${NC}      \$$BET_SIZE (fixed)"
else
    echo -e "  ${DIM}Bet Size:${NC}      Kelly ${KELLY_FRACTION} (dynamic)"
fi
echo -e "  ${DIM}Min Spread:${NC}    \$$MIN_SPREAD"
echo -e "  ${DIM}Min Win Prob:${NC}  ${MIN_WIN_PROB}"
echo -e "  ${DIM}Max/Window:${NC}    \$$MAX_POSITION"
if [[ "$MODE" == "paper" ]]; then
    echo -e "  ${DIM}Paper Balance:${NC} \$$PAPER_BALANCE"
fi
echo -e "  ${DIM}Persistence:${NC}   $([ -n "$PERSIST" ] && echo "ENABLED" || echo "disabled")"
[[ -n "$SESSION_ID" ]] && echo -e "  ${DIM}Session:${NC}       $SESSION_ID"
[[ -n "$LOG_FILE" ]] && echo -e "  ${DIM}Log File:${NC}      $LOG_FILE"
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

# Always create a log file for tracing output
mkdir -p "$PROJECT_ROOT/logs"
if [[ -z "$LOG_FILE" ]]; then
    LOG_FILE="$PROJECT_ROOT/logs/${MODE}-$(date +%Y%m%d-%H%M%S).log"
fi

echo -e "${GREEN}Starting bot...${NC}"
echo -e "${DIM}Press Ctrl+C to stop${NC}"
echo -e "${DIM}Logs: ${LOG_FILE}${NC}"
echo ""

# Trap to clean up child processes
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

# Format and colorize output (for --verbose mode)
colorize_output() {
    while IFS= read -r line; do
        local timestamp
        timestamp=$(date '+%H:%M:%S')

        if echo "$line" | grep -q "Both legs filled\|FILLED\|Execution successful"; then
            echo -e "${GREEN}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "TRIM\|Trimming\|trim"; then
            echo -e "${CYAN}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "Early exit\|EARLY EXIT\|FULLY EXITED"; then
            echo -e "${CYAN}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "SETTLED\|Paper trade settled"; then
            echo -e "${WHITE}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "RECOVERED\|recovery"; then
            echo -e "${YELLOW}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "Partial fill\|PARTIAL\|directional exposure"; then
            echo -e "${YELLOW}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "Opps:\|Vol:\|Bal:"; then
            echo -e "${WHITE}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "ERROR\|error\|Error"; then
            echo -e "${RED}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "WARN\|warn\|Warning"; then
            echo -e "${YELLOW}[$timestamp] $line${NC}"
        elif echo "$line" | grep -q "SESSION COMPLETE\|==="; then
            echo ""
            echo -e "${WHITE}$line${NC}"
        else
            # Skip noisy debug lines in verbose mode
            if echo "$line" | grep -qv "periodic settlement\|Running periodic\|Window transition"; then
                echo -e "${DIM}[$timestamp]${NC} $line"
            fi
        fi
    done
}

export RUST_LOG="${RUST_LOG:-info}"

if [[ -n "$VERBOSE" ]]; then
    # Verbose mode: merge stdout+stderr, colorize, tee to log
    echo -e "${DIM}─────────────────────────────────────────────────────────────────────${NC}"
    echo ""
    "${CMD[@]}" 2>&1 | tee "$LOG_FILE" | colorize_output &
    BOT_PID=$!
else
    # Dashboard mode: stderr (tracing) goes to log file, stdout (dashboard) to terminal
    # The Rust dashboard handles its own ANSI formatting and screen clearing
    "${CMD[@]}" 2>"$LOG_FILE" &
    BOT_PID=$!
fi

wait "$BOT_PID" 2>/dev/null || true
BOT_PID=""

# =============================================================================
# Post-run summary
# =============================================================================

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Bot stopped${NC}"

# Always show log file location
if [[ -f "$LOG_FILE" ]]; then
    LINES=$(wc -l < "$LOG_FILE")
    echo ""
    echo -e "${WHITE}Log file:${NC} $LOG_FILE ($LINES lines)"
    echo -e "${DIM}View with: less -R $LOG_FILE${NC}"
    echo -e "${DIM}Tail key events: grep -E 'FILLED|TRIM|SETTLED|EARLY|PARTIAL|ERROR' $LOG_FILE${NC}"
fi

# Show final database stats if persisting
if [[ -n "$PERSIST" && -n "${DATABASE_URL:-}" && -n "$SESSION_ID" ]]; then
    echo ""
    echo -e "${WHITE}Database Summary for session: ${CYAN}$SESSION_ID${NC}"
    echo ""

    psql "$DATABASE_URL" -c "
    SELECT
        COUNT(*) as \"Total Opps\",
        COUNT(*) FILTER (WHERE executed = true) as \"Executed\",
        COUNT(*) FILTER (WHERE status = 'win') as \"Wins\",
        COUNT(*) FILTER (WHERE status = 'loss') as \"Losses\",
        ROUND(COALESCE(SUM(spread) FILTER (WHERE executed = true), 0)::numeric, 4) as \"Total Spread\",
        ROUND(COALESCE(MAX(spread), 0)::numeric, 4) as \"Best Spread\"
    FROM cross_market_opportunities
    WHERE session_id = \$\$${SESSION_ID}\$\$;
    " 2>/dev/null || echo -e "${DIM}(Could not fetch database stats)${NC}"
fi

echo ""
