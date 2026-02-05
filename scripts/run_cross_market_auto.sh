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
#   --mode paper|live     Trading mode (default: paper)
#   --duration <time>     How long to run (default: 1h)
#   --bet-size <amount>   Fixed bet size per leg in USDC
#   --pair <coins>        Coin pair to trade (default: btc,eth)
#   --min-spread <val>    Minimum spread required (default: 0.03)
#   --no-persist          Disable database persistence
#   --session <id>        Custom session ID
#   --help                Show this help
#

set -e

# Auto-source .env file if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m' # No Color

# Default configuration
MODE="paper"
DURATION="1h"
PAIR="btc,eth"
COMBINATION="coin1down_coin2up"
MIN_SPREAD="0.03"
MIN_WIN_PROB="0.85"
MAX_POSITION="200"
PAPER_BALANCE="1000"
KELLY_FRACTION="0.25"
BET_SIZE=""
PERSIST="--persist"
SESSION_ID=""
STATS_INTERVAL="10"
VERBOSE=""

# Parse arguments
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
        --pair)
            PAIR="$2"
            shift 2
            ;;
        --combination)
            COMBINATION="$2"
            shift 2
            ;;
        --bet-size)
            BET_SIZE="--bet-size $2"
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
        --no-persist)
            PERSIST=""
            shift
            ;;
        --session)
            SESSION_ID="--session-id $2"
            shift 2
            ;;
        --verbose|-v)
            VERBOSE="--verbose"
            shift
            ;;
        --help|-h)
            head -20 "$0" | tail -19
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Generate session ID if not provided and persisting
if [[ -n "$PERSIST" && -z "$SESSION_ID" ]]; then
    SESSION_ID="--session-id auto-$(date +%Y%m%d-%H%M%S)"
fi

# Extract session ID value for display
SESSION_DISPLAY=$(echo "$SESSION_ID" | sed 's/--session-id //')
if [[ -z "$SESSION_DISPLAY" ]]; then
    SESSION_DISPLAY="(none)"
fi

# Check for DATABASE_URL if persisting
if [[ -n "$PERSIST" && -z "$DATABASE_URL" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL environment variable required for --persist${NC}"
    echo "Set it with: export DATABASE_URL=postgres://user:pass@localhost/dbname"
    exit 1
fi

# Build the command
CMD="cargo run -p algo-trade-cli --release -- cross-market-auto"
CMD="$CMD --mode $MODE"
CMD="$CMD --duration $DURATION"
CMD="$CMD --pair $PAIR"
CMD="$CMD --combination $COMBINATION"
CMD="$CMD --min-spread $MIN_SPREAD"
CMD="$CMD --min-win-prob $MIN_WIN_PROB"
CMD="$CMD --max-position $MAX_POSITION"
CMD="$CMD --kelly-fraction $KELLY_FRACTION"
CMD="$CMD --stats-interval-secs $STATS_INTERVAL"
[[ "$MODE" == "paper" ]] && CMD="$CMD --paper-balance $PAPER_BALANCE"
[[ -n "$BET_SIZE" ]] && CMD="$CMD $BET_SIZE"
[[ -n "$PERSIST" ]] && CMD="$CMD $PERSIST"
[[ -n "$SESSION_ID" ]] && CMD="$CMD $SESSION_ID"
[[ -n "$VERBOSE" ]] && CMD="$CMD $VERBOSE"

# Clear screen and show header
clear

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Cross-Market Correlation Arbitrage Bot${NC}                   ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# Show configuration
echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Mode:${NC}          ${MODE^^}"
if [[ "$MODE" == "live" ]]; then
    echo -e "                 ${RED}*** REAL FUNDS WILL BE USED ***${NC}"
fi
echo -e "  ${DIM}Duration:${NC}      $DURATION"
echo -e "  ${DIM}Pair:${NC}          ${PAIR^^}"
echo -e "  ${DIM}Combination:${NC}   $COMBINATION"
echo -e "  ${DIM}Min Spread:${NC}    \$$MIN_SPREAD"
echo -e "  ${DIM}Min Win Prob:${NC}  ${MIN_WIN_PROB}%"
echo -e "  ${DIM}Max Position:${NC}  \$$MAX_POSITION/window"
echo -e "  ${DIM}Kelly:${NC}         ${KELLY_FRACTION}x"
if [[ -n "$BET_SIZE" ]]; then
    echo -e "  ${DIM}Fixed Bet:${NC}     $(echo $BET_SIZE | cut -d' ' -f2)"
fi
if [[ "$MODE" == "paper" ]]; then
    echo -e "  ${DIM}Paper Balance:${NC} \$$PAPER_BALANCE"
fi
echo -e "  ${DIM}Persistence:${NC}   $([ -n "$PERSIST" ] && echo "ENABLED" || echo "disabled")"
echo -e "  ${DIM}Session:${NC}       $SESSION_DISPLAY"
echo ""

# Confirm for live mode
if [[ "$MODE" == "live" ]]; then
    echo -e "${YELLOW}WARNING: You are about to start LIVE trading with real funds!${NC}"
    read -p "Type 'yes' to confirm: " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "Aborted."
        exit 1
    fi
    echo ""
fi

echo -e "${GREEN}Starting bot...${NC}"
echo -e "${DIM}Press Ctrl+C to stop${NC}"
echo ""
echo -e "${DIM}─────────────────────────────────────────────────────────────────────${NC}"
echo ""

# Create a temp file for output
OUTPUT_FILE=$(mktemp)
trap "rm -f $OUTPUT_FILE" EXIT

# Function to display live stats from database
show_db_stats() {
    if [[ -z "$PERSIST" || -z "$DATABASE_URL" ]]; then
        return
    fi

    local session_val=$(echo "$SESSION_ID" | sed 's/--session-id //')

    # Query database for stats
    psql "$DATABASE_URL" -t -A -F'|' <<EOF 2>/dev/null || return
SELECT
    COUNT(*) as total,
    COUNT(*) FILTER (WHERE executed = true) as executed,
    COUNT(*) FILTER (WHERE status = 'win') as wins,
    COUNT(*) FILTER (WHERE status = 'loss') as losses,
    COALESCE(SUM(spread) FILTER (WHERE executed = true), 0) as total_spread,
    COALESCE(MAX(spread), 0) as best_spread,
    COALESCE(AVG(win_probability) FILTER (WHERE executed = true), 0) as avg_win_prob
FROM cross_market_opportunities
WHERE session_id = '$session_val'
  AND timestamp > NOW() - INTERVAL '1 day';
EOF
}

# Run the command with output processing
RUST_LOG=info $CMD 2>&1 | while IFS= read -r line; do
    # Filter and format output
    timestamp=$(date '+%H:%M:%S')

    # Check for key events and highlight them
    if echo "$line" | grep -q "Both legs filled\|Execution successful"; then
        echo -e "${GREEN}[$timestamp] ✓ $line${NC}"
    elif echo "$line" | grep -q "Opportunity detected\|Signal:"; then
        echo -e "${CYAN}[$timestamp] ⚡ $line${NC}"
    elif echo "$line" | grep -q "Opps:\|Vol:"; then
        echo -e "${WHITE}[$timestamp] 📊 $line${NC}"
    elif echo "$line" | grep -q "ERROR\|error\|Error"; then
        echo -e "${RED}[$timestamp] ✗ $line${NC}"
    elif echo "$line" | grep -q "WARN\|warn\|Warning"; then
        echo -e "${YELLOW}[$timestamp] ⚠ $line${NC}"
    elif echo "$line" | grep -q "Position limit\|filtered\|skipped"; then
        echo -e "${DIM}[$timestamp] $line${NC}"
    elif echo "$line" | grep -q "Final Summary\|==="; then
        echo ""
        echo -e "${WHITE}$line${NC}"
    else
        echo -e "${DIM}[$timestamp]${NC} $line"
    fi
done

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Bot stopped${NC}"

# Show final database stats if persisting
if [[ -n "$PERSIST" && -n "$DATABASE_URL" ]]; then
    session_val=$(echo "$SESSION_ID" | sed 's/--session-id //')
    echo ""
    echo -e "${WHITE}Database Summary for session: ${CYAN}$session_val${NC}"
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
    WHERE session_id = '$session_val';
    " 2>/dev/null || echo "(Could not fetch database stats)"
fi

echo ""
