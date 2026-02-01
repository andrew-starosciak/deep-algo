#!/bin/bash
# trading-insights.sh - Comprehensive trading analysis and recommendations
#
# Usage: ./scripts/trading-insights.sh [--days N]
#
# Analyzes all collected data to generate insights and recommendations
# for improving trading performance.

set -o pipefail

# =============================================================================
# CONFIGURATION
# =============================================================================
DAYS=${1:-7}
if [[ "$1" == "--days" ]]; then
    DAYS=${2:-7}
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Load environment
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
fi

if [ -z "$DATABASE_URL" ]; then
    echo -e "${RED}ERROR: DATABASE_URL not set${NC}"
    exit 1
fi

# =============================================================================
# HELPER FUNCTIONS
# =============================================================================
run_sql() {
    psql "$DATABASE_URL" -t -A -c "$1" 2>/dev/null
}

run_sql_pretty() {
    psql "$DATABASE_URL" -c "$1" 2>/dev/null
}

print_header() {
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BOLD}$1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_insight() {
    echo -e "  ${GREEN}→${NC} $1"
}

print_warning() {
    echo -e "  ${YELLOW}⚠${NC} $1"
}

print_recommendation() {
    echo -e "  ${CYAN}★${NC} $1"
}

# =============================================================================
# HEADER
# =============================================================================
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║                    TRADING INSIGHTS REPORT                                   ║${NC}"
echo -e "${BOLD}║                    $(date '+%Y-%m-%d %H:%M %Z')                                   ║${NC}"
echo -e "${BOLD}║                    Analysis Period: Last ${DAYS} Days                              ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════════════════╝${NC}"

# =============================================================================
# SECTION 1: DATA INVENTORY
# =============================================================================
print_header "1. DATA INVENTORY"

echo -e "${BOLD}Data Collection Summary:${NC}"
run_sql_pretty "
SELECT
    'Paper Trades' as source,
    COUNT(*) as total_records,
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '${DAYS} days') as last_${DAYS}d,
    MIN(timestamp)::date as first_record,
    MAX(timestamp)::date as latest_record
FROM paper_trades
UNION ALL
SELECT
    'Liquidations',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '${DAYS} days'),
    MIN(timestamp)::date,
    MAX(timestamp)::date
FROM liquidations
UNION ALL
SELECT
    'Funding Rates',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '${DAYS} days'),
    MIN(timestamp)::date,
    MAX(timestamp)::date
FROM funding_rates
UNION ALL
SELECT
    'Orderbook',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '${DAYS} days'),
    MIN(timestamp)::date,
    MAX(timestamp)::date
FROM orderbook_snapshots
UNION ALL
SELECT
    'Polymarket Odds',
    COUNT(*),
    COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '${DAYS} days'),
    MIN(timestamp)::date,
    MAX(timestamp)::date
FROM polymarket_odds;
"

# =============================================================================
# SECTION 2: OVERALL TRADE PERFORMANCE
# =============================================================================
print_header "2. OVERALL TRADE PERFORMANCE"

TOTAL_TRADES=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled';")
if [ "$TOTAL_TRADES" -lt 1 ]; then
    echo -e "${YELLOW}No settled trades yet. Need more data for analysis.${NC}"
else
    echo -e "${BOLD}Performance Metrics:${NC}"
    run_sql_pretty "
    SELECT
        COUNT(*) as total_trades,
        SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
        SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
        ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) as win_rate_pct,
        SUM(stake)::numeric(12,2) as total_staked,
        SUM(pnl)::numeric(12,2) as total_pnl,
        ROUND(100.0 * SUM(pnl) / NULLIF(SUM(stake), 0), 1) as roi_pct,
        AVG(pnl)::numeric(10,2) as avg_pnl_per_trade,
        AVG(CASE WHEN outcome = 'win' THEN pnl END)::numeric(10,2) as avg_win,
        AVG(CASE WHEN outcome = 'loss' THEN pnl END)::numeric(10,2) as avg_loss
    FROM paper_trades
    WHERE status = 'settled';
    "

    # Win rate analysis
    WIN_RATE=$(run_sql "SELECT ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) FROM paper_trades WHERE status = 'settled';")

    echo ""
    echo -e "${BOLD}Performance Assessment:${NC}"
    # Use awk for reliable floating point comparison
    if [ -n "$WIN_RATE" ] && awk "BEGIN {exit !($WIN_RATE > 55)}"; then
        print_insight "Win rate ${WIN_RATE}% is above target (>55%) - strategy showing edge"
    elif [ -n "$WIN_RATE" ] && awk "BEGIN {exit !($WIN_RATE > 50)}"; then
        print_warning "Win rate ${WIN_RATE}% is marginally profitable - monitor closely"
    else
        print_warning "Win rate ${WIN_RATE}% is below breakeven - review strategy parameters"
    fi
fi

# =============================================================================
# SECTION 3: WIN RATE BY SIGNAL STRENGTH
# =============================================================================
print_header "3. SIGNAL STRENGTH ANALYSIS"

TRADES_WITH_SIGNAL=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled' AND signal_strength IS NOT NULL;")
if [ "$TRADES_WITH_SIGNAL" -gt 4 ]; then
    echo -e "${BOLD}Win Rate by Signal Strength Band:${NC}"
    run_sql_pretty "
    SELECT
        CASE
            WHEN signal_strength >= 0.9 THEN '0.90-1.00 (Very Strong)'
            WHEN signal_strength >= 0.8 THEN '0.80-0.89 (Strong)'
            WHEN signal_strength >= 0.7 THEN '0.70-0.79 (Moderate)'
            WHEN signal_strength >= 0.6 THEN '0.60-0.69 (Weak)'
            ELSE '< 0.60 (Very Weak)'
        END as signal_band,
        COUNT(*) as trades,
        SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
        ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) as win_rate_pct,
        SUM(pnl)::numeric(10,2) as total_pnl
    FROM paper_trades
    WHERE status = 'settled' AND signal_strength IS NOT NULL
    GROUP BY 1
    ORDER BY 1 DESC;
    "

    # Check if stronger signals perform better
    STRONG_WINRATE=$(run_sql "SELECT ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / NULLIF(COUNT(*), 0), 1) FROM paper_trades WHERE status = 'settled' AND signal_strength >= 0.8;" | tr -d ' ')
    WEAK_WINRATE=$(run_sql "SELECT ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / NULLIF(COUNT(*), 0), 1) FROM paper_trades WHERE status = 'settled' AND signal_strength < 0.7;" | tr -d ' ')

    echo ""
    echo -e "${BOLD}Signal Strength Insights:${NC}"
    if [ -n "$STRONG_WINRATE" ] && [ -n "$WEAK_WINRATE" ] && [ "$STRONG_WINRATE" != "" ] && [ "$WEAK_WINRATE" != "" ]; then
        if awk "BEGIN {exit !($STRONG_WINRATE > $WEAK_WINRATE)}"; then
            print_insight "Stronger signals (>=0.8) have higher win rate (${STRONG_WINRATE}% vs ${WEAK_WINRATE}%)"
            print_recommendation "Consider increasing min_signal_strength threshold to 0.70-0.75"
        else
            print_warning "Weaker signals performing similarly to strong - signal calibration may need review"
        fi
    fi
else
    echo -e "${YELLOW}Need more trades with signal data for analysis.${NC}"
fi

# =============================================================================
# SECTION 4: WIN RATE BY DIRECTION
# =============================================================================
print_header "4. DIRECTION ANALYSIS"

echo -e "${BOLD}Performance by Direction:${NC}"
run_sql_pretty "
SELECT
    direction,
    COUNT(*) as trades,
    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
    ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) as win_rate_pct,
    SUM(pnl)::numeric(10,2) as total_pnl,
    AVG(signal_strength)::numeric(4,2) as avg_signal
FROM paper_trades
WHERE status = 'settled'
GROUP BY direction
ORDER BY direction;
"

YES_WINRATE=$(run_sql "SELECT ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / NULLIF(COUNT(*), 0), 1) FROM paper_trades WHERE status = 'settled' AND direction = 'yes';" | tr -d ' ')
NO_WINRATE=$(run_sql "SELECT ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / NULLIF(COUNT(*), 0), 1) FROM paper_trades WHERE status = 'settled' AND direction = 'no';" | tr -d ' ')

echo ""
echo -e "${BOLD}Direction Insights:${NC}"
if [ -n "$YES_WINRATE" ] && [ -n "$NO_WINRATE" ] && [ "$YES_WINRATE" != "" ] && [ "$NO_WINRATE" != "" ]; then
    DIFF=$(awk "BEGIN {print $YES_WINRATE - $NO_WINRATE}")
    ABS_DIFF=$(awk "BEGIN {print ($DIFF < 0) ? -$DIFF : $DIFF}")
    if awk "BEGIN {exit !($ABS_DIFF > 15)}"; then
        if awk "BEGIN {exit !($YES_WINRATE > $NO_WINRATE)}"; then
            print_insight "'Yes' (Up) trades significantly outperform 'No' (Down) trades"
            print_recommendation "Consider higher confidence threshold for 'No' direction trades"
        else
            print_insight "'No' (Down) trades significantly outperform 'Yes' (Up) trades"
            print_recommendation "Consider higher confidence threshold for 'Yes' direction trades"
        fi
    else
        print_insight "Both directions performing similarly - no bias detected"
    fi
fi

# =============================================================================
# SECTION 5: TIME OF DAY ANALYSIS
# =============================================================================
print_header "5. TIME OF DAY ANALYSIS"

echo -e "${BOLD}Performance by Hour (UTC):${NC}"
run_sql_pretty "
SELECT
    EXTRACT(HOUR FROM timestamp)::int as hour_utc,
    COUNT(*) as trades,
    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
    ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) as win_rate_pct,
    SUM(pnl)::numeric(10,2) as pnl
FROM paper_trades
WHERE status = 'settled'
GROUP BY 1
HAVING COUNT(*) >= 2
ORDER BY win_rate_pct DESC
LIMIT 10;
"

# Best and worst hours
BEST_HOUR=$(run_sql "
SELECT EXTRACT(HOUR FROM timestamp)::int
FROM paper_trades
WHERE status = 'settled'
GROUP BY 1
HAVING COUNT(*) >= 2
ORDER BY (100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*)) DESC
LIMIT 1;
" | tr -d ' ')

WORST_HOUR=$(run_sql "
SELECT EXTRACT(HOUR FROM timestamp)::int
FROM paper_trades
WHERE status = 'settled'
GROUP BY 1
HAVING COUNT(*) >= 2
ORDER BY (100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*)) ASC
LIMIT 1;
" | tr -d ' ')

if [ -n "$BEST_HOUR" ] && [ -n "$WORST_HOUR" ]; then
    echo ""
    echo -e "${BOLD}Time Insights:${NC}"
    print_insight "Best performing hour: ${BEST_HOUR}:00 UTC"
    print_insight "Worst performing hour: ${WORST_HOUR}:00 UTC"
fi

# =============================================================================
# SECTION 6: BTC PRICE MOVEMENT ANALYSIS
# =============================================================================
print_header "6. BTC PRICE MOVEMENT ANALYSIS"

BTC_DATA=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE btc_price_at_entry IS NOT NULL AND btc_price_window_end IS NOT NULL AND status = 'settled';")
if [ "$BTC_DATA" -gt 0 ]; then
    echo -e "${BOLD}Entry Timing Statistics:${NC}"
    run_sql_pretty "
    SELECT
        COUNT(*) as trades_with_btc_data,
        AVG(btc_price_at_entry - btc_price_window_start)::numeric(8,2) as avg_pre_entry_move,
        AVG(btc_price_window_end - btc_price_at_entry)::numeric(8,2) as avg_post_entry_move,
        AVG(btc_price_window_end - btc_price_window_start)::numeric(8,2) as avg_total_move,
        AVG(ABS(btc_price_window_end - btc_price_window_start))::numeric(8,2) as avg_abs_move
    FROM paper_trades
    WHERE btc_price_at_entry IS NOT NULL
      AND btc_price_window_end IS NOT NULL
      AND status = 'settled';
    "

    echo ""
    echo -e "${BOLD}Win Rate by BTC Movement Size:${NC}"
    run_sql_pretty "
    SELECT
        CASE
            WHEN ABS(btc_price_window_end - btc_price_window_start) < 50 THEN '< $50 (Small)'
            WHEN ABS(btc_price_window_end - btc_price_window_start) < 150 THEN '$50-150 (Medium)'
            ELSE '> $150 (Large)'
        END as move_size,
        COUNT(*) as trades,
        SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
        ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) / COUNT(*), 1) as win_rate_pct
    FROM paper_trades
    WHERE btc_price_window_end IS NOT NULL
      AND btc_price_window_start IS NOT NULL
      AND status = 'settled'
    GROUP BY 1
    ORDER BY 1;
    "

    echo ""
    echo -e "${BOLD}Entry Timing Quality:${NC}"
    run_sql_pretty "
    SELECT
        CASE
            WHEN direction = 'yes' AND (btc_price_window_end - btc_price_at_entry) > 0 THEN 'Correct (Up after entry)'
            WHEN direction = 'no' AND (btc_price_window_end - btc_price_at_entry) < 0 THEN 'Correct (Down after entry)'
            ELSE 'Wrong direction after entry'
        END as entry_quality,
        COUNT(*) as trades,
        ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER(), 1) as pct
    FROM paper_trades
    WHERE btc_price_at_entry IS NOT NULL
      AND btc_price_window_end IS NOT NULL
      AND status = 'settled'
    GROUP BY 1;
    "

    # Entry timing insight
    CORRECT_DIRECTION=$(run_sql "
    SELECT COUNT(*) FROM paper_trades
    WHERE btc_price_at_entry IS NOT NULL
      AND btc_price_window_end IS NOT NULL
      AND status = 'settled'
      AND (
          (direction = 'yes' AND btc_price_window_end > btc_price_at_entry)
          OR (direction = 'no' AND btc_price_window_end < btc_price_at_entry)
      );
    " | tr -d ' ')

    TOTAL_BTC=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE btc_price_at_entry IS NOT NULL AND btc_price_window_end IS NOT NULL AND status = 'settled';" | tr -d ' ')

    if [ "$TOTAL_BTC" -gt 0 ]; then
        CORRECT_PCT=$(awk "BEGIN {printf \"%.1f\", 100 * $CORRECT_DIRECTION / $TOTAL_BTC}")
        echo ""
        echo -e "${BOLD}Entry Timing Insights:${NC}"
        if awk "BEGIN {exit !($CORRECT_PCT > 55)}"; then
            print_insight "BTC moved in predicted direction ${CORRECT_PCT}% of time after entry"
        else
            print_warning "BTC only moved in predicted direction ${CORRECT_PCT}% after entry"
            print_recommendation "Review entry timing strategy - consider entering earlier in window"
        fi
    fi
else
    echo -e "${YELLOW}No BTC price data available yet. New trades will capture this.${NC}"
fi

# =============================================================================
# SECTION 7: SIGNAL VALIDATION STATUS
# =============================================================================
print_header "7. SIGNAL VALIDATION STATUS"

echo -e "${BOLD}Running signal validation...${NC}"
echo ""

# Run validation and capture output
cd /home/a/Work/gambling/engine
VALIDATION_OUTPUT=$(cargo run -p algo-trade-cli --quiet -- validate-signals \
    --start "$(date -d '7 days ago' '+%Y-%m-%dT00:00:00Z')" \
    --end "$(date '+%Y-%m-%dT23:59:59Z')" 2>&1)

# Show summary
echo "$VALIDATION_OUTPUT" | grep -E "(APPROVED|CONDITIONAL|REJECTED|NEEDS|>>> GO|>>> NO-GO|Signals by)" || echo "Validation completed"

# =============================================================================
# SECTION 8: RECOMMENDATIONS
# =============================================================================
print_header "8. RECOMMENDATIONS"

echo ""
echo -e "${BOLD}Based on the analysis:${NC}"
echo ""

# Sample size check
if [ "$TOTAL_TRADES" -lt 30 ]; then
    print_warning "Only ${TOTAL_TRADES} settled trades - need 30+ for reliable statistics"
    print_recommendation "Continue paper trading to collect more data before adjusting parameters"
    echo ""
fi

# Win rate based recommendations
if [ -n "$WIN_RATE" ] && [ "$WIN_RATE" != "" ]; then
    if awk "BEGIN {exit !($WIN_RATE > 60)}"; then
        print_recommendation "Strong performance - consider increasing kelly_fraction from 0.25 to 0.30"
        print_recommendation "Consider moving to live trading after 50+ paper trades"
    elif awk "BEGIN {exit !($WIN_RATE > 52)}"; then
        print_recommendation "Positive edge detected - maintain current parameters"
        print_recommendation "Focus on increasing trade volume with current settings"
    else
        print_recommendation "Review min_signal_strength - consider raising to 0.70+"
        print_recommendation "Analyze losing trades for common patterns"
    fi
fi

# Data collection recommendations
echo ""
echo -e "${BOLD}Data Collection:${NC}"
POLY_FRESH=$(run_sql "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(timestamp)))/60 FROM polymarket_odds;" | tr -d ' ')
if [ -n "$POLY_FRESH" ] && awk "BEGIN {exit !($POLY_FRESH > 60)}"; then
    print_warning "Polymarket odds data is ${POLY_FRESH%.*} minutes old"
    print_recommendation "Check if paper trading bot is running: pgrep -f polymarket"
fi

LIQ_COUNT=$(run_sql "SELECT COUNT(*) FROM liquidations WHERE timestamp > NOW() - INTERVAL '1 hour';" | tr -d ' ')
if [ "$LIQ_COUNT" -lt 10 ]; then
    print_warning "Low liquidation data in last hour (${LIQ_COUNT} records)"
fi

# =============================================================================
# SECTION 9: SUGGESTED NEXT ACTIONS
# =============================================================================
print_header "9. NEXT ACTIONS CHECKLIST"

echo ""
echo "  [ ] Review any losing trades for patterns"
echo "  [ ] Check signal validation status"
echo "  [ ] Ensure data collectors are running"
echo "  [ ] Monitor win rate trend over time"
if [ "$TOTAL_TRADES" -ge 30 ]; then
    echo "  [ ] Consider parameter optimization based on signal strength analysis"
fi
if [ "$TOTAL_TRADES" -ge 50 ] && [ -n "$WIN_RATE" ] && awk "BEGIN {exit !(${WIN_RATE:-0} > 53)}"; then
    echo "  [ ] Evaluate readiness for live trading (Phase 6 gate)"
fi

echo ""
echo -e "${BOLD}═══════════════════════════════════════════════════════════════════════════════${NC}"
echo -e "Report generated at $(date '+%Y-%m-%d %H:%M:%S %Z')"
echo -e "Run './scripts/trading-insights.sh --days 14' for longer analysis period"
echo ""
