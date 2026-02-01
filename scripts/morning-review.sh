#!/bin/bash
# =============================================================================
# Morning Review Script - Polymarket BTC 15-min Trading
# =============================================================================
# Run this each morning to review:
#   1. Data collection status
#   2. Paper trading results
#   3. Signal alignment analysis (composite opportunities)
#   4. Phase 1 signal validation (Go/No-Go)
#
# Usage:
#   ./scripts/morning-review.sh [--hours 24] [--verbose]
# =============================================================================

# Don't exit on errors - we want to show partial results
set +e

# Load .env file if it exists
if [ -f .env ]; then
    set -a
    source .env
    set +a
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Default parameters
HOURS="${HOURS:-24}"
VERBOSE="${VERBOSE:-false}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --hours)
            HOURS="$2"
            shift 2
            ;;
        --verbose|-v)
            VERBOSE="true"
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [--hours N] [--verbose]"
            echo "  --hours N    Review last N hours (default: 24)"
            echo "  --verbose    Show detailed output"
            exit 0
            ;;
        *)
            shift
            ;;
    esac
done

# Check DATABASE_URL
if [ -z "$DATABASE_URL" ]; then
    echo -e "${RED}ERROR: DATABASE_URL not set${NC}"
    exit 1
fi

# Extract DB connection info for psql
DB_HOST=$(echo $DATABASE_URL | sed -n 's/.*@\([^:]*\).*/\1/p')
DB_PORT=$(echo $DATABASE_URL | sed -n 's/.*:\([0-9]*\)\/.*/\1/p')
DB_NAME=$(echo $DATABASE_URL | sed -n 's/.*\/\([^?]*\).*/\1/p')
DB_USER=$(echo $DATABASE_URL | sed -n 's/.*:\/\/\([^:]*\):.*/\1/p')
DB_PASS=$(echo $DATABASE_URL | sed -n 's/.*:\/\/[^:]*:\([^@]*\)@.*/\1/p')

# Function to run SQL
run_sql() {
    PGPASSWORD="$DB_PASS" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -t -A -c "$1" 2>/dev/null
}

# Function to run SQL with headers
run_sql_pretty() {
    PGPASSWORD="$DB_PASS" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -c "$1" 2>/dev/null
}

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║                    MORNING REVIEW - $(date '+%Y-%m-%d %H:%M %Z')                      ║${NC}"
echo -e "${BOLD}║                         Last ${HOURS} Hours Analysis                              ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# =============================================================================
# SECTION 1: DATA COLLECTION STATUS
# =============================================================================
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}📊 DATA COLLECTION STATUS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

run_sql_pretty "
SELECT
    'Liquidations' as source,
    COUNT(*) as records,
    TO_CHAR(MIN(timestamp), 'MM-DD HH24:MI') as earliest,
    TO_CHAR(MAX(timestamp), 'MM-DD HH24:MI') as latest,
    ROUND(EXTRACT(EPOCH FROM (MAX(timestamp) - MIN(timestamp)))/3600, 1) as hours
FROM liquidations
UNION ALL
SELECT 'Funding Rates', COUNT(*), TO_CHAR(MIN(timestamp), 'MM-DD HH24:MI'),
       TO_CHAR(MAX(timestamp), 'MM-DD HH24:MI'),
       ROUND(EXTRACT(EPOCH FROM (MAX(timestamp) - MIN(timestamp)))/3600, 1)
FROM funding_rates
UNION ALL
SELECT 'Orderbook', COUNT(*), TO_CHAR(MIN(timestamp), 'MM-DD HH24:MI'),
       TO_CHAR(MAX(timestamp), 'MM-DD HH24:MI'),
       ROUND(EXTRACT(EPOCH FROM (MAX(timestamp) - MIN(timestamp)))/3600, 1)
FROM orderbook_snapshots
UNION ALL
SELECT 'Polymarket Odds', COUNT(*), TO_CHAR(MIN(timestamp), 'MM-DD HH24:MI'),
       TO_CHAR(MAX(timestamp), 'MM-DD HH24:MI'),
       ROUND(EXTRACT(EPOCH FROM (MAX(timestamp) - MIN(timestamp)))/3600, 1)
FROM polymarket_odds;
"

# Check for data gaps
echo ""
echo -e "${YELLOW}Recent Activity (last 1 hour):${NC}"
run_sql_pretty "
SELECT
    'Liquidations' as source,
    COUNT(*) as records_1h
FROM liquidations WHERE timestamp > NOW() - INTERVAL '1 hour'
UNION ALL
SELECT 'Funding Rates', COUNT(*) FROM funding_rates WHERE timestamp > NOW() - INTERVAL '1 hour'
UNION ALL
SELECT 'Orderbook', COUNT(*) FROM orderbook_snapshots WHERE timestamp > NOW() - INTERVAL '1 hour'
UNION ALL
SELECT 'Polymarket', COUNT(*) FROM polymarket_odds WHERE timestamp > NOW() - INTERVAL '1 hour';
"

# =============================================================================
# SECTION 2: PAPER TRADING RESULTS
# =============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}💰 PAPER TRADING RESULTS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

echo ""
echo -e "${BOLD}All-Time Summary:${NC}"
run_sql_pretty "
SELECT
    COUNT(*) as total_trades,
    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
    SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
    CASE WHEN COUNT(*) FILTER (WHERE status = 'settled') > 0
         THEN ROUND(100.0 * SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) /
              COUNT(*) FILTER (WHERE status = 'settled'), 1)
         ELSE 0 END as win_rate_pct,
    COALESCE(SUM(stake)::numeric(12,2), 0) as total_stake,
    COALESCE(SUM(pnl)::numeric(12,2), 0) as total_pnl,
    CASE WHEN SUM(stake) > 0
         THEN ROUND(100.0 * SUM(pnl) / SUM(stake), 1)
         ELSE 0 END as roi_pct
FROM paper_trades;
"

echo ""
echo -e "${BOLD}Last ${HOURS}h Trades:${NC}"
run_sql_pretty "
SELECT
    id,
    TO_CHAR(timestamp, 'MM-DD HH24:MI') as time,
    direction,
    stake::numeric(10,2) as stake,
    outcome,
    pnl::numeric(10,2) as pnl,
    signal_strength::numeric(4,2) as signal
FROM paper_trades
WHERE timestamp > NOW() - INTERVAL '${HOURS} hours'
ORDER BY timestamp DESC
LIMIT 10;
"

# BTC Price Analysis (if data available)
BTC_PRICE_DATA=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE btc_price_at_entry IS NOT NULL AND status = 'settled';" | tr -d ' ')
if [ "$BTC_PRICE_DATA" -gt 0 ]; then
    echo ""
    echo -e "${BOLD}BTC Price Analysis (Settled Trades):${NC}"
    run_sql_pretty "
    SELECT
        id,
        direction as dir,
        outcome,
        btc_price_window_start::numeric(10,2) as btc_start,
        btc_price_at_entry::numeric(10,2) as btc_entry,
        btc_price_window_end::numeric(10,2) as btc_end,
        (btc_price_at_entry - btc_price_window_start)::numeric(8,2) as pre_entry,
        (btc_price_window_end - btc_price_at_entry)::numeric(8,2) as post_entry,
        (btc_price_window_end - btc_price_window_start)::numeric(8,2) as total_move
    FROM paper_trades
    WHERE btc_price_at_entry IS NOT NULL
      AND status = 'settled'
      AND timestamp > NOW() - INTERVAL '${HOURS} hours'
    ORDER BY timestamp DESC
    LIMIT 10;
    "

    echo ""
    echo -e "${BOLD}Entry Timing Summary:${NC}"
    run_sql_pretty "
    SELECT
        COUNT(*) as trades,
        AVG(btc_price_at_entry - btc_price_window_start)::numeric(8,2) as avg_pre_entry_move,
        AVG(btc_price_window_end - btc_price_at_entry)::numeric(8,2) as avg_post_entry_move,
        AVG(btc_price_window_end - btc_price_window_start)::numeric(8,2) as avg_total_move,
        COUNT(*) FILTER (
            WHERE (direction = 'yes' AND btc_price_window_end > btc_price_at_entry)
               OR (direction = 'no' AND btc_price_window_end < btc_price_at_entry)
        ) as correct_direction_after_entry
    FROM paper_trades
    WHERE btc_price_at_entry IS NOT NULL
      AND btc_price_window_end IS NOT NULL
      AND status = 'settled';
    "
fi

# =============================================================================
# SECTION 3: SIGNAL ALIGNMENT ANALYSIS
# =============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}🎯 SIGNAL ALIGNMENT ANALYSIS (Last ${HOURS}h)${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

echo ""
echo -e "${BOLD}Composite Signal Opportunities:${NC}"
run_sql_pretty "
WITH windows AS (
    SELECT generate_series(
        date_trunc('hour', NOW() - INTERVAL '${HOURS} hours') +
        (EXTRACT(MINUTE FROM NOW())::int / 15) * INTERVAL '15 minutes',
        NOW(),
        '15 minutes'::interval
    ) as window_start
),
liq AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        la.net_delta,
        CASE
            WHEN la.net_delta < -30000 THEN 'UP'
            WHEN la.net_delta > 30000 THEN 'DOWN'
            ELSE 'NEUTRAL'
        END as liq_dir
    FROM windows w
    LEFT JOIN liquidation_aggregates la ON
        la.timestamp >= w.window_start AND la.timestamp < w.window_start + interval '15 minutes'
        AND la.symbol = 'BTCUSDT' AND la.window_minutes = 5
    ORDER BY w.window_start, la.timestamp DESC
),
funding AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        fr.rate_zscore,
        CASE
            WHEN fr.rate_zscore > 2 THEN 'DOWN'
            WHEN fr.rate_zscore < -2 THEN 'UP'
            ELSE 'NEUTRAL'
        END as fund_dir
    FROM windows w
    LEFT JOIN funding_rates fr ON
        fr.timestamp >= w.window_start AND fr.timestamp < w.window_start + interval '15 minutes'
        AND fr.symbol = 'BTCUSDT'
    ORDER BY w.window_start, fr.timestamp DESC
),
orderbook AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        os.imbalance,
        CASE
            WHEN os.imbalance > 0.3 THEN 'UP'
            WHEN os.imbalance < -0.3 THEN 'DOWN'
            ELSE 'NEUTRAL'
        END as ob_dir
    FROM windows w
    LEFT JOIN orderbook_snapshots os ON
        os.timestamp >= w.window_start AND os.timestamp < w.window_start + interval '15 minutes'
        AND os.symbol = 'BTCUSDT'
    ORDER BY w.window_start, os.timestamp DESC
),
outcomes AS (
    SELECT
        end_date as window_end,
        CASE
            WHEN MAX(outcome_yes_price) > 0.7 THEN 'UP'
            WHEN MAX(outcome_yes_price) < 0.3 THEN 'DOWN'
            ELSE '?'
        END as actual
    FROM polymarket_odds
    WHERE end_date IS NOT NULL AND end_date > NOW() - INTERVAL '${HOURS} hours'
    GROUP BY end_date
)
SELECT
    TO_CHAR(l.window_start, 'MM-DD HH24:MI') as window,
    l.liq_dir as liq,
    f.fund_dir as fund,
    o.ob_dir as ob,
    CASE
        WHEN l.liq_dir = f.fund_dir AND f.fund_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL'
        THEN '*** 3-MATCH ***'
        WHEN (l.liq_dir = f.fund_dir AND l.liq_dir != 'NEUTRAL') OR
             (l.liq_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL') OR
             (f.fund_dir = o.ob_dir AND f.fund_dir != 'NEUTRAL')
        THEN '2-match'
        ELSE '-'
    END as composite,
    COALESCE(oc.actual, '-') as outcome
FROM liq l
LEFT JOIN funding f ON l.window_start = f.window_start
LEFT JOIN orderbook o ON l.window_start = o.window_start
LEFT JOIN outcomes oc ON l.window_start + interval '15 minutes' = oc.window_end
WHERE l.liq_dir != 'NEUTRAL' OR f.fund_dir != 'NEUTRAL' OR o.ob_dir != 'NEUTRAL'
ORDER BY l.window_start DESC
LIMIT 20;
"

# Summary stats
echo ""
echo -e "${BOLD}Composite Signal Summary:${NC}"
run_sql_pretty "
WITH windows AS (
    SELECT generate_series(
        NOW() - INTERVAL '${HOURS} hours',
        NOW(),
        '15 minutes'::interval
    ) as window_start
),
liq AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        CASE
            WHEN la.net_delta < -30000 THEN 'UP'
            WHEN la.net_delta > 30000 THEN 'DOWN'
            ELSE 'NEUTRAL'
        END as liq_dir
    FROM windows w
    LEFT JOIN liquidation_aggregates la ON
        la.timestamp >= w.window_start AND la.timestamp < w.window_start + interval '15 minutes'
        AND la.symbol = 'BTCUSDT' AND la.window_minutes = 5
    ORDER BY w.window_start, la.timestamp DESC
),
funding AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        CASE
            WHEN fr.rate_zscore > 2 THEN 'DOWN'
            WHEN fr.rate_zscore < -2 THEN 'UP'
            ELSE 'NEUTRAL'
        END as fund_dir
    FROM windows w
    LEFT JOIN funding_rates fr ON
        fr.timestamp >= w.window_start AND fr.timestamp < w.window_start + interval '15 minutes'
        AND fr.symbol = 'BTCUSDT'
    ORDER BY w.window_start, fr.timestamp DESC
),
orderbook AS (
    SELECT DISTINCT ON (w.window_start)
        w.window_start,
        CASE
            WHEN os.imbalance > 0.3 THEN 'UP'
            WHEN os.imbalance < -0.3 THEN 'DOWN'
            ELSE 'NEUTRAL'
        END as ob_dir
    FROM windows w
    LEFT JOIN orderbook_snapshots os ON
        os.timestamp >= w.window_start AND os.timestamp < w.window_start + interval '15 minutes'
        AND os.symbol = 'BTCUSDT'
    ORDER BY w.window_start, os.timestamp DESC
),
outcomes AS (
    SELECT
        end_date as window_end,
        CASE
            WHEN MAX(outcome_yes_price) > 0.7 THEN 'UP'
            WHEN MAX(outcome_yes_price) < 0.3 THEN 'DOWN'
            ELSE 'UNCERTAIN'
        END as actual
    FROM polymarket_odds
    WHERE end_date IS NOT NULL
    GROUP BY end_date
),
combined AS (
    SELECT
        l.window_start,
        l.liq_dir,
        f.fund_dir,
        o.ob_dir,
        oc.actual,
        CASE
            WHEN l.liq_dir = f.fund_dir AND f.fund_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL' THEN 3
            WHEN (l.liq_dir = f.fund_dir AND l.liq_dir != 'NEUTRAL') OR
                 (l.liq_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL') OR
                 (f.fund_dir = o.ob_dir AND f.fund_dir != 'NEUTRAL') THEN 2
            ELSE 0
        END as match_count,
        CASE
            WHEN l.liq_dir = f.fund_dir AND f.fund_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL' THEN l.liq_dir
            WHEN l.liq_dir = o.ob_dir AND l.liq_dir != 'NEUTRAL' THEN l.liq_dir
            WHEN l.liq_dir = f.fund_dir AND l.liq_dir != 'NEUTRAL' THEN l.liq_dir
            WHEN f.fund_dir = o.ob_dir AND f.fund_dir != 'NEUTRAL' THEN f.fund_dir
            ELSE NULL
        END as signal_dir
    FROM liq l
    LEFT JOIN funding f ON l.window_start = f.window_start
    LEFT JOIN orderbook o ON l.window_start = o.window_start
    LEFT JOIN outcomes oc ON l.window_start + interval '15 minutes' = oc.window_end
)
SELECT
    '3-Signal Match' as type,
    COUNT(*) FILTER (WHERE match_count = 3) as opportunities,
    COUNT(*) FILTER (WHERE match_count = 3 AND signal_dir = actual) as wins,
    COUNT(*) FILTER (WHERE match_count = 3 AND signal_dir != actual AND actual != 'UNCERTAIN') as losses
UNION ALL
SELECT
    '2-Signal Match',
    COUNT(*) FILTER (WHERE match_count = 2),
    COUNT(*) FILTER (WHERE match_count = 2 AND signal_dir = actual),
    COUNT(*) FILTER (WHERE match_count = 2 AND signal_dir != actual AND actual != 'UNCERTAIN')
FROM combined;
"

# =============================================================================
# SECTION 4: PHASE 1 SIGNAL VALIDATION (GO/NO-GO)
# =============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}🔬 PHASE 1 SIGNAL VALIDATION (GO/NO-GO)${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Calculate date range for validation
START_DATE=$(date -u -d "7 days ago" '+%Y-%m-%dT00:00:00Z')
END_DATE=$(date -u '+%Y-%m-%dT23:59:59Z')

echo -e "${YELLOW}Running signal validation from $START_DATE to $END_DATE...${NC}"
echo ""

# Run the validation command
cd /home/a/Work/gambling/engine
if cargo run -p algo-trade-cli --quiet -- validate-signals \
    --start "$START_DATE" \
    --end "$END_DATE" 2>&1 | tee /tmp/signal_validation.txt | \
    grep -E "(===|---|╔|╠|╚|║|Signals by|APPROVED|CONDITIONAL|REJECTED|NEEDS|Sample Size|P-value|Win Rate|Correlation|IC|Recommendation|GO|NO-GO)"; then
    :
else
    echo -e "${YELLOW}Note: Some validation output may be filtered. Full output in /tmp/signal_validation.txt${NC}"
fi

# =============================================================================
# SECTION 5: RECOMMENDATIONS
# =============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}📋 RECOMMENDATIONS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Check data freshness
LIQ_AGE=$(run_sql "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(timestamp)))/60 FROM liquidations;" | tr -d ' ')
FUNDING_AGE=$(run_sql "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(timestamp)))/60 FROM funding_rates;" | tr -d ' ')
OB_AGE=$(run_sql "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(timestamp)))/60 FROM orderbook_snapshots;" | tr -d ' ')
PM_AGE=$(run_sql "SELECT EXTRACT(EPOCH FROM (NOW() - MAX(timestamp)))/60 FROM polymarket_odds;" | tr -d ' ')

echo -e "${BOLD}Data Freshness:${NC}"
# Use awk for floating point comparison (more portable than bc)
if [ -n "$LIQ_AGE" ] && awk "BEGIN {exit !($LIQ_AGE > 10)}"; then
    echo -e "  ${RED}⚠ Liquidations: ${LIQ_AGE%.*} min old - STALE${NC}"
else
    echo -e "  ${GREEN}✓ Liquidations: ${LIQ_AGE%.*} min old${NC}"
fi

if [ -n "$FUNDING_AGE" ] && awk "BEGIN {exit !($FUNDING_AGE > 10)}"; then
    echo -e "  ${RED}⚠ Funding Rates: ${FUNDING_AGE%.*} min old - STALE${NC}"
else
    echo -e "  ${GREEN}✓ Funding Rates: ${FUNDING_AGE%.*} min old${NC}"
fi

if [ -n "$OB_AGE" ] && awk "BEGIN {exit !($OB_AGE > 10)}"; then
    echo -e "  ${RED}⚠ Orderbook: ${OB_AGE%.*} min old - STALE${NC}"
else
    echo -e "  ${GREEN}✓ Orderbook: ${OB_AGE%.*} min old${NC}"
fi

if [ -n "$PM_AGE" ] && awk "BEGIN {exit !($PM_AGE > 60)}"; then
    echo -e "  ${RED}⚠ Polymarket: ${PM_AGE%.*} min old - STALE (check collector)${NC}"
else
    echo -e "  ${GREEN}✓ Polymarket: ${PM_AGE%.*} min old${NC}"
fi

# Trading recommendation
echo ""
echo -e "${BOLD}Trading Status:${NC}"
PENDING=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE status = 'pending';" | tr -d ' ')
if [ "$PENDING" -gt 0 ]; then
    echo -e "  ${YELLOW}⏳ $PENDING pending trade(s) awaiting settlement${NC}"
fi

TOTAL_TRADES=$(run_sql "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled';" | tr -d ' ')
if [ "$TOTAL_TRADES" -lt 30 ]; then
    echo -e "  ${YELLOW}📊 $TOTAL_TRADES settled trades - need 30+ for Phase 3 gate${NC}"
else
    echo -e "  ${GREEN}✓ $TOTAL_TRADES settled trades - sufficient for analysis${NC}"
fi

echo ""
echo -e "${BOLD}═══════════════════════════════════════════════════════════════════════════════${NC}"
echo -e "Review complete at $(date '+%Y-%m-%d %H:%M:%S %Z')"
echo ""
