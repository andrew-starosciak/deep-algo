#!/bin/bash
#
# Cross-Market Trading Dashboard
#
# Live dashboard showing trading stats from the database.
# Run this in a separate terminal while cross-market-auto is running.
#
# Usage:
#   ./scripts/cross_market_dashboard.sh [session-id]
#
# If no session-id is provided, shows stats for all recent sessions.
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
BOLD='\033[1m'
NC='\033[0m'

# Configuration
REFRESH_INTERVAL=5
SESSION_FILTER="$1"

# Check DATABASE_URL
if [[ -z "$DATABASE_URL" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL environment variable required${NC}"
    exit 1
fi

# Function to draw a box
draw_box() {
    local title="$1"
    local width=70
    echo -e "${CYAN}┌$(printf '─%.0s' $(seq 1 $((width-2))))┐${NC}"
    printf "${CYAN}│${NC}${WHITE}%*s${NC}${CYAN}│${NC}\n" $(((width-2+${#title})/2)) "$title"
    echo -e "${CYAN}├$(printf '─%.0s' $(seq 1 $((width-2))))┤${NC}"
}

draw_box_bottom() {
    local width=70
    echo -e "${CYAN}└$(printf '─%.0s' $(seq 1 $((width-2))))┘${NC}"
}

# Function to format decimal as currency
format_currency() {
    printf "\$%.2f" "$1"
}

# Function to format percentage
format_pct() {
    printf "%.1f%%" "$1"
}

# Function to get session filter SQL
get_session_filter() {
    if [[ -n "$SESSION_FILTER" ]]; then
        echo "AND session_id = '$SESSION_FILTER'"
    else
        echo ""
    fi
}

# Main dashboard loop
while true; do
    clear

    # Header
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}           ${WHITE}${BOLD}Cross-Market Correlation Arbitrage Dashboard${NC}              ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    NOW=$(date '+%Y-%m-%d %H:%M:%S')
    echo -e "  ${DIM}Last updated: $NOW${NC}    ${DIM}Refresh: ${REFRESH_INTERVAL}s${NC}    ${DIM}Press Ctrl+C to exit${NC}"

    if [[ -n "$SESSION_FILTER" ]]; then
        echo -e "  ${DIM}Session filter: ${CYAN}$SESSION_FILTER${NC}"
    fi
    echo ""

    # Get overall stats
    FILTER=$(get_session_filter)

    STATS=$(psql "$DATABASE_URL" -t -A -F'|' <<EOF 2>/dev/null
SELECT
    COUNT(*) as total_opps,
    COUNT(*) FILTER (WHERE executed = true) as executed,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN') OR status = 'win') as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE' OR status = 'loss') as losses,
    COUNT(*) FILTER (WHERE status = 'pending') as pending,
    COALESCE(SUM(actual_pnl) FILTER (WHERE actual_pnl > 0), 0) as total_won,
    COALESCE(ABS(SUM(actual_pnl) FILTER (WHERE actual_pnl < 0)), 0) as total_lost,
    COALESCE(MAX(spread), 0) as best_spread,
    COALESCE(AVG(win_probability) FILTER (WHERE executed = true), 0) as avg_win_prob,
    COALESCE(AVG(total_cost) FILTER (WHERE executed = true), 0) as avg_cost
FROM cross_market_opportunities
WHERE timestamp > NOW() - INTERVAL '24 hours'
$FILTER;
EOF
)

    if [[ -z "$STATS" || "$STATS" == "||||||||||" ]]; then
        echo -e "  ${YELLOW}No data found for the last 24 hours${NC}"
        echo ""
        sleep $REFRESH_INTERVAL
        continue
    fi

    # Parse stats
    IFS='|' read -r TOTAL_OPPS EXECUTED WINS LOSSES PENDING TOTAL_WON TOTAL_LOST BEST_SPREAD AVG_WIN_PROB AVG_COST <<< "$STATS"

    # Calculate derived metrics
    TOTAL_OPPS=${TOTAL_OPPS:-0}
    EXECUTED=${EXECUTED:-0}
    WINS=${WINS:-0}
    LOSSES=${LOSSES:-0}
    PENDING=${PENDING:-0}
    TOTAL_WON=${TOTAL_WON:-0}
    TOTAL_LOST=${TOTAL_LOST:-0}

    if [[ $EXECUTED -gt 0 ]]; then
        WIN_RATE=$(awk "BEGIN {printf \"%.1f\", $WINS * 100 / $EXECUTED}")
        NET_PNL=$(awk "BEGIN {printf \"%.2f\", $TOTAL_WON - $TOTAL_LOST}")
    else
        WIN_RATE="0.0"
        NET_PNL="0.00"
    fi

    # Overall Stats Box
    draw_box "Overall Statistics (24h)"
    printf "${CYAN}│${NC}  %-25s %15s                     ${CYAN}│${NC}\n" "Total Opportunities:" "$TOTAL_OPPS"
    printf "${CYAN}│${NC}  %-25s %15s                     ${CYAN}│${NC}\n" "Executed Trades:" "$EXECUTED"
    printf "${CYAN}│${NC}  %-25s %15s                     ${CYAN}│${NC}\n" "Pending Settlement:" "$PENDING"
    draw_box_bottom
    echo ""

    # P&L Box
    draw_box "Profit & Loss"
    if awk "BEGIN {exit !($NET_PNL >= 0)}"; then
        PNL_COLOR=$GREEN
    else
        PNL_COLOR=$RED
    fi
    printf "${CYAN}│${NC}  %-25s ${GREEN}%15s${NC}                     ${CYAN}│${NC}\n" "Wins:" "$WINS"
    printf "${CYAN}│${NC}  %-25s ${RED}%15s${NC}                     ${CYAN}│${NC}\n" "Losses:" "$LOSSES"
    printf "${CYAN}│${NC}  %-25s %15s                     ${CYAN}│${NC}\n" "Win Rate:" "${WIN_RATE}%"
    echo -e "${CYAN}│${NC}  $(printf '─%.0s' $(seq 1 40))                       ${CYAN}│${NC}"
    printf "${CYAN}│${NC}  %-25s ${GREEN}\$%14s${NC}                     ${CYAN}│${NC}\n" "Total Won:" "$TOTAL_WON"
    printf "${CYAN}│${NC}  %-25s ${RED}\$%14s${NC}                     ${CYAN}│${NC}\n" "Total Lost:" "$TOTAL_LOST"
    printf "${CYAN}│${NC}  %-25s ${PNL_COLOR}\$%14s${NC}                     ${CYAN}│${NC}\n" "Net P&L:" "$NET_PNL"
    draw_box_bottom
    echo ""

    # Signal Quality Box
    draw_box "Signal Quality"
    printf "${CYAN}│${NC}  %-25s \$%14s                     ${CYAN}│${NC}\n" "Best Spread Seen:" "$BEST_SPREAD"
    printf "${CYAN}│${NC}  %-25s %15s                     ${CYAN}│${NC}\n" "Avg Win Probability:" "$(awk "BEGIN {printf \"%.1f%%\", $AVG_WIN_PROB * 100}")"
    printf "${CYAN}│${NC}  %-25s \$%14s                     ${CYAN}│${NC}\n" "Avg Total Cost:" "$AVG_COST"
    draw_box_bottom
    echo ""

    # Recent Trades
    echo -e "${WHITE}${BOLD}Recent Trades (last 10):${NC}"
    echo ""

    psql "$DATABASE_URL" -c "
    SELECT
        TO_CHAR(timestamp, 'HH24:MI:SS') as \"Time\",
        UPPER(coin1) || '/' || UPPER(coin2) as \"Pair\",
        CASE
            WHEN combination LIKE '%Coin1Down%' THEN 'C1↓C2↑'
            ELSE 'C1↑C2↓'
        END as \"Combo\",
        ROUND(total_cost::numeric, 3) as \"Cost\",
        ROUND(spread::numeric, 3) as \"Spread\",
        ROUND((win_probability * 100)::numeric, 1) || '%' as \"WinP\",
        CASE
            WHEN trade_result IN ('WIN', 'DOUBLE_WIN') THEN '✓ WIN'
            WHEN trade_result = 'LOSE' THEN '✗ LOSS'
            WHEN status = 'settled' AND actual_pnl > 0 THEN '✓ WIN'
            WHEN status = 'settled' AND actual_pnl <= 0 THEN '✗ LOSS'
            WHEN executed = true AND status = 'pending' THEN '⏳ PENDING'
            ELSE '○ SCAN'
        END as \"Status\",
        COALESCE(ROUND(actual_pnl::numeric, 2)::text, '-') as \"P&L\"
    FROM cross_market_opportunities
    WHERE timestamp > NOW() - INTERVAL '24 hours'
    $FILTER
    ORDER BY timestamp DESC
    LIMIT 10;
    " 2>/dev/null || echo "  (Could not fetch trades)"

    echo ""

    # Session breakdown (if no filter)
    if [[ -z "$SESSION_FILTER" ]]; then
        echo -e "${WHITE}${BOLD}Sessions (last 24h):${NC}"
        echo ""

        psql "$DATABASE_URL" -c "
        SELECT
            session_id as \"Session\",
            COUNT(*) as \"Opps\",
            COUNT(*) FILTER (WHERE executed = true) as \"Exec\",
            COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN') OR actual_pnl > 0) as \"Wins\",
            COUNT(*) FILTER (WHERE trade_result = 'LOSE' OR (status = 'settled' AND actual_pnl <= 0)) as \"Loss\",
            COALESCE(ROUND(SUM(actual_pnl)::numeric, 2), 0) as \"P&L\",
            TO_CHAR(MIN(timestamp), 'HH24:MI') || '-' || TO_CHAR(MAX(timestamp), 'HH24:MI') as \"Time Range\"
        FROM cross_market_opportunities
        WHERE timestamp > NOW() - INTERVAL '24 hours'
          AND session_id IS NOT NULL
        GROUP BY session_id
        ORDER BY MIN(timestamp) DESC
        LIMIT 5;
        " 2>/dev/null || echo "  (No sessions found)"
    fi

    # Wait before refresh
    sleep $REFRESH_INTERVAL
done
