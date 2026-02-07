#!/bin/bash
# Cross-Market Arbitrage Analysis Script
# Provides comprehensive reporting on collected opportunity data

set -e
cd "$(dirname "$0")/.."

# Load environment
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
fi

if [ -z "$DATABASE_URL" ]; then
    echo "ERROR: DATABASE_URL not set"
    exit 1
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ============================================================================
# PRE-ANALYSIS: PROCESS PENDING SETTLEMENTS
# ============================================================================
echo ""
echo -e "${BOLD}Processing pending settlements...${NC}"

# Count pending before
PENDING_BEFORE=$(psql "$DATABASE_URL" -t -c "
    SELECT COUNT(*) FROM cross_market_opportunities
    WHERE status = 'pending'
    AND window_end < NOW() - INTERVAL '2 minutes';
")

if [ "$PENDING_BEFORE" -gt 0 ]; then
    echo "  Found ${PENDING_BEFORE} opportunities ready for settlement"
    echo "  Running settlement processor..."

    # Run settlement for 60 seconds max (enough to process batch), suppress logs
    timeout 60 cargo run --release -p algo-trade-cli -- cross-market-settle --duration-mins 1 >/dev/null 2>&1 || true

    # Count how many were settled
    PENDING_AFTER=$(psql "$DATABASE_URL" -t -c "
        SELECT COUNT(*) FROM cross_market_opportunities
        WHERE status = 'pending'
        AND window_end < NOW() - INTERVAL '2 minutes';
    ")
    SETTLED_COUNT=$((PENDING_BEFORE - PENDING_AFTER))
    echo -e "  ${GREEN}✓ Settled ${SETTLED_COUNT} opportunities${NC}"
else
    echo -e "  ${GREEN}✓ No pending settlements to process${NC}"
fi

echo ""

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║          CROSS-MARKET ARBITRAGE ANALYSIS REPORT                  ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# ============================================================================
# SECTION 1: DATA OVERVIEW
# ============================================================================
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}1. DATA OVERVIEW${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    COUNT(*) as total_opportunities,
    COUNT(*) FILTER (WHERE status = 'settled') as settled,
    COUNT(*) FILTER (WHERE status = 'pending') as pending,
    MIN(timestamp)::date as first_date,
    MAX(timestamp)::date as last_date,
    COUNT(DISTINCT window_end) FILTER (WHERE status = 'settled') as windows_settled
FROM cross_market_opportunities;
"

# ============================================================================
# SECTION 2: OVERALL P&L SUMMARY
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}2. OVERALL P&L SUMMARY (Settled Only)${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    COUNT(*) as total_trades,
    COUNT(*) FILTER (WHERE trade_result = 'DOUBLE_WIN') as double_wins,
    COUNT(*) FILTER (WHERE trade_result = 'WIN') as single_wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as double_losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / NULLIF(COUNT(*), 0) * 100), 1) as win_rate_pct,
    ROUND(SUM(actual_pnl)::numeric, 2) as net_pnl,
    ROUND(SUM(total_cost)::numeric, 2) as total_risked,
    ROUND((SUM(actual_pnl) / NULLIF(SUM(total_cost), 0) * 100)::numeric, 1) as roi_pct
FROM cross_market_opportunities
WHERE status = 'settled';
"

# ============================================================================
# SECTION 3: TRADE RESULT MECHANICS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}3. TRADE RESULT MECHANICS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "How each result type occurs:"
echo "  DOUBLE_WIN  = Both legs win (coins moved opposite directions, both bets correct)"
echo "  WIN         = One leg wins, one loses (coins correlated, one bet correct)"
echo "  LOSE        = Both legs lose (coins moved opposite to BOTH bets)"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    combination,
    leg1_direction || ' bet → ' || coin1_outcome || ' actual' as leg1_outcome,
    leg2_direction || ' bet → ' || coin2_outcome || ' actual' as leg2_outcome,
    CASE WHEN leg1_direction = coin1_outcome THEN '✓ WIN' ELSE '✗ LOSE' END as leg1,
    CASE WHEN leg2_direction = coin2_outcome THEN '✓ WIN' ELSE '✗ LOSE' END as leg2,
    trade_result,
    COUNT(*) as count,
    ROUND(SUM(actual_pnl)::numeric, 2) as pnl
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY combination, leg1_direction, coin1_outcome, leg2_direction, coin2_outcome, trade_result
ORDER BY combination, trade_result DESC, count DESC;
"

# ============================================================================
# SECTION 4: BY COMBINATION TYPE
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}4. PERFORMANCE BY COMBINATION TYPE${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Combination meanings:"
echo "  Coin1DownCoin2Up = Bet Coin1 DOWN + Bet Coin2 UP"
echo "  Coin1UpCoin2Down = Bet Coin1 UP + Bet Coin2 DOWN"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    combination,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result = 'DOUBLE_WIN') as double_wins,
    COUNT(*) FILTER (WHERE trade_result = 'WIN') as single_wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as double_losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 1) || '%' as win_rate,
    ROUND(SUM(actual_pnl)::numeric, 2) as net_pnl,
    ROUND(AVG(actual_pnl)::numeric, 4) as avg_pnl,
    ROUND(AVG(total_cost)::numeric, 4) as avg_cost,
    ROUND(AVG(spread)::numeric, 4) as avg_spread
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY combination
ORDER BY net_pnl DESC;
"

# ============================================================================
# SECTION 5: BY COIN PAIR
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}5. PERFORMANCE BY COIN PAIR${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    coin1 || '/' || coin2 as pair,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 1) || '%' as win_rate,
    ROUND(SUM(actual_pnl)::numeric, 2) as net_pnl,
    ROUND((SUM(actual_pnl) / SUM(total_cost) * 100)::numeric, 1) || '%' as roi
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY coin1, coin2
ORDER BY SUM(actual_pnl) DESC;
"

# ============================================================================
# SECTION 6: BY COIN PAIR AND COMBINATION
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}6. DETAILED: PAIR + COMBINATION BREAKDOWN${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    coin1 || '/' || coin2 as pair,
    combination,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 0) || '%' as win_rate,
    ROUND(SUM(actual_pnl)::numeric, 2) as pnl
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY coin1, coin2, combination
ORDER BY coin1, coin2, combination;
"

# ============================================================================
# SECTION 7: WINDOW-BY-WINDOW ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}7. WINDOW-BY-WINDOW ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    window_end,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 0) || '%' as win_rate,
    ROUND(SUM(actual_pnl)::numeric, 2) as pnl,
    STRING_AGG(DISTINCT coin1 || '=' || coin1_outcome, ', ' ORDER BY coin1 || '=' || coin1_outcome) as coin_outcomes
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY window_end
ORDER BY window_end;
"

# ============================================================================
# SECTION 8: MARKET DIRECTION ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}8. MARKET DIRECTION ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "How often did each coin go UP vs DOWN?"
echo ""

psql "$DATABASE_URL" -c "
WITH coin_outcomes AS (
    SELECT DISTINCT window_end, coin1 as coin, coin1_outcome as outcome
    FROM cross_market_opportunities WHERE status = 'settled'
    UNION
    SELECT DISTINCT window_end, coin2 as coin, coin2_outcome as outcome
    FROM cross_market_opportunities WHERE status = 'settled'
)
SELECT
    coin,
    COUNT(*) FILTER (WHERE outcome = 'UP') as times_up,
    COUNT(*) FILTER (WHERE outcome = 'DOWN') as times_down,
    COUNT(*) as total_windows,
    ROUND((COUNT(*) FILTER (WHERE outcome = 'UP')::numeric / COUNT(*) * 100), 0) || '%' as up_pct
FROM coin_outcomes
GROUP BY coin
ORDER BY coin;
"

# ============================================================================
# SECTION 9: CORRELATION ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}9. CORRELATION ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Did coins move in the same direction (correlated) or opposite (uncorrelated)?"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    coin1 || '/' || coin2 as pair,
    COUNT(*) FILTER (WHERE coin1_outcome = coin2_outcome) as same_direction,
    COUNT(*) FILTER (WHERE coin1_outcome != coin2_outcome) as opposite_direction,
    COUNT(*) as total,
    ROUND((COUNT(*) FILTER (WHERE coin1_outcome = coin2_outcome)::numeric / COUNT(*) * 100), 0) || '%' as correlation_rate
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY coin1, coin2
ORDER BY coin1, coin2;
"

# ============================================================================
# SECTION 10: DEPTH ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}10. ORDER BOOK DEPTH ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    trade_result,
    COUNT(*) as trades,
    ROUND(AVG(LEAST(leg1_ask_depth, leg2_ask_depth))::numeric, 0) as avg_min_depth,
    ROUND(MIN(LEAST(leg1_ask_depth, leg2_ask_depth))::numeric, 0) as min_depth,
    ROUND(MAX(LEAST(leg1_ask_depth, leg2_ask_depth))::numeric, 0) as max_depth
FROM cross_market_opportunities
WHERE status = 'settled' AND leg1_ask_depth IS NOT NULL
GROUP BY trade_result
ORDER BY trade_result;
"

# ============================================================================
# SECTION 11: SPREAD ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}11. SPREAD VS OUTCOME ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Does higher spread (cheaper cost) correlate with better outcomes?"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    CASE
        WHEN spread < 0.15 THEN '< 0.15 (tight)'
        WHEN spread < 0.25 THEN '0.15-0.25 (medium)'
        WHEN spread < 0.35 THEN '0.25-0.35 (wide)'
        ELSE '>= 0.35 (very wide)'
    END as spread_bucket,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 0) || '%' as win_rate,
    ROUND(SUM(actual_pnl)::numeric, 2) as net_pnl,
    ROUND(AVG(actual_pnl)::numeric, 4) as avg_pnl
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY
    CASE
        WHEN spread < 0.15 THEN '< 0.15 (tight)'
        WHEN spread < 0.25 THEN '0.15-0.25 (medium)'
        WHEN spread < 0.35 THEN '0.25-0.35 (wide)'
        ELSE '>= 0.35 (very wide)'
    END
ORDER BY spread_bucket;
"

# ============================================================================
# SECTION 12: KEY INSIGHTS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}12. KEY INSIGHTS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Calculate key metrics
TOTAL_PNL=$(psql "$DATABASE_URL" -t -c "SELECT ROUND(SUM(actual_pnl)::numeric, 2) FROM cross_market_opportunities WHERE status = 'settled';")
BEST_COMBO=$(psql "$DATABASE_URL" -t -c "SELECT combination || ' (' || ROUND(SUM(actual_pnl)::numeric, 2) || ')' FROM cross_market_opportunities WHERE status = 'settled' GROUP BY combination ORDER BY SUM(actual_pnl) DESC LIMIT 1;")
WORST_COMBO=$(psql "$DATABASE_URL" -t -c "SELECT combination || ' (' || ROUND(SUM(actual_pnl)::numeric, 2) || ')' FROM cross_market_opportunities WHERE status = 'settled' GROUP BY combination ORDER BY SUM(actual_pnl) ASC LIMIT 1;")
BEST_PAIR=$(psql "$DATABASE_URL" -t -c "SELECT coin1 || '/' || coin2 || ' (' || ROUND(SUM(actual_pnl)::numeric, 2) || ')' FROM cross_market_opportunities WHERE status = 'settled' GROUP BY coin1, coin2 ORDER BY SUM(actual_pnl) DESC LIMIT 1;")
WORST_PAIR=$(psql "$DATABASE_URL" -t -c "SELECT coin1 || '/' || coin2 || ' (' || ROUND(SUM(actual_pnl)::numeric, 2) || ')' FROM cross_market_opportunities WHERE status = 'settled' GROUP BY coin1, coin2 ORDER BY SUM(actual_pnl) ASC LIMIT 1;")
DOUBLE_LOSS_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM cross_market_opportunities WHERE status = 'settled' AND trade_result = 'LOSE';")
DOUBLE_WIN_COUNT=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM cross_market_opportunities WHERE status = 'settled' AND trade_result = 'DOUBLE_WIN';")

echo -e "  ${BOLD}Net P&L:${NC}            $TOTAL_PNL"
echo -e "  ${BOLD}Best Combination:${NC}  $BEST_COMBO"
echo -e "  ${BOLD}Worst Combination:${NC} $WORST_COMBO"
echo -e "  ${BOLD}Best Pair:${NC}         $BEST_PAIR"
echo -e "  ${BOLD}Worst Pair:${NC}        $WORST_PAIR"
echo -e "  ${BOLD}Double Wins:${NC}       $DOUBLE_WIN_COUNT"
echo -e "  ${BOLD}Double Losses:${NC}     $DOUBLE_LOSS_COUNT"
echo ""

# ============================================================================
# SECTION 13: COIN PAIR PRIORITY RANKING
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}13. COIN PAIR PRIORITY RANKING${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Pairs ranked by: correlation rate, win rate, ROI, and sample size"
echo ""

psql "$DATABASE_URL" -c "
WITH pair_stats AS (
    SELECT
        coin1 || '/' || coin2 as pair,
        COUNT(*) as trades,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
        COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
        COUNT(*) FILTER (WHERE coin1_outcome = coin2_outcome) as correlated,
        SUM(actual_pnl) as net_pnl,
        SUM(total_cost) as total_cost,
        AVG(spread) as avg_spread,
        COUNT(DISTINCT window_end) as windows
    FROM cross_market_opportunities
    WHERE status = 'settled'
    GROUP BY coin1, coin2
)
SELECT
    pair,
    trades,
    ROUND((correlated::numeric / trades * 100), 0) || '%' as correlation,
    ROUND((wins::numeric / trades * 100), 0) || '%' as win_rate,
    ROUND((net_pnl / total_cost * 100)::numeric, 1) || '%' as roi,
    ROUND(net_pnl::numeric, 2) as net_pnl,
    windows as sample_windows,
    CASE
        WHEN wins::numeric / trades >= 0.9 AND correlated::numeric / trades >= 0.8 THEN '🟢 HIGH'
        WHEN wins::numeric / trades >= 0.7 AND correlated::numeric / trades >= 0.6 THEN '🟡 MEDIUM'
        ELSE '🔴 LOW'
    END as priority
FROM pair_stats
ORDER BY
    (wins::numeric / trades) DESC,
    (correlated::numeric / trades) DESC,
    trades DESC;
"

# ============================================================================
# SECTION 14: ENTRY TIMING ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}14. ENTRY TIMING ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Performance by minutes before window close (when opportunity was detected)"
echo ""

psql "$DATABASE_URL" -c "
WITH timing AS (
    SELECT
        *,
        EXTRACT(EPOCH FROM (window_end - timestamp))/60 as mins_before_close
    FROM cross_market_opportunities
    WHERE status = 'settled'
)
SELECT
    CASE
        WHEN mins_before_close >= 10 THEN '10-15 min (early)'
        WHEN mins_before_close >= 5 THEN '5-10 min (mid)'
        WHEN mins_before_close >= 2 THEN '2-5 min (late)'
        ELSE '0-2 min (very late)'
    END as entry_window,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
    ROUND((COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) * 100), 0) || '%' as win_rate,
    ROUND(AVG(spread)::numeric, 4) as avg_spread,
    ROUND(SUM(actual_pnl)::numeric, 2) as net_pnl,
    ROUND(AVG(actual_pnl)::numeric, 4) as avg_pnl
FROM timing
GROUP BY
    CASE
        WHEN mins_before_close >= 10 THEN '10-15 min (early)'
        WHEN mins_before_close >= 5 THEN '5-10 min (mid)'
        WHEN mins_before_close >= 2 THEN '2-5 min (late)'
        ELSE '0-2 min (very late)'
    END
ORDER BY entry_window DESC;
"

echo ""
echo "Spread evolution by timing (do spreads tighten near close?):"
echo ""

psql "$DATABASE_URL" -c "
WITH timing AS (
    SELECT
        *,
        EXTRACT(EPOCH FROM (window_end - timestamp))/60 as mins_before_close
    FROM cross_market_opportunities
    WHERE status = 'settled'
)
SELECT
    CASE
        WHEN mins_before_close >= 10 THEN '10-15 min'
        WHEN mins_before_close >= 5 THEN '5-10 min'
        WHEN mins_before_close >= 2 THEN '2-5 min'
        ELSE '0-2 min'
    END as timing,
    ROUND(AVG(spread)::numeric, 4) as avg_spread,
    ROUND(MIN(spread)::numeric, 4) as min_spread,
    ROUND(MAX(spread)::numeric, 4) as max_spread,
    ROUND(AVG(total_cost)::numeric, 4) as avg_cost
FROM timing
GROUP BY
    CASE
        WHEN mins_before_close >= 10 THEN '10-15 min'
        WHEN mins_before_close >= 5 THEN '5-10 min'
        WHEN mins_before_close >= 2 THEN '2-5 min'
        ELSE '0-2 min'
    END
ORDER BY timing DESC;
"

# ============================================================================
# SECTION 15: STATISTICAL SIGNIFICANCE
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}15. STATISTICAL SIGNIFICANCE${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Wilson Score 95% Confidence Intervals for win rates"
echo "(Win rate must have CI lower bound > 50% to be statistically significant)"
echo ""

psql "$DATABASE_URL" -c "
WITH stats AS (
    SELECT
        coin1 || '/' || coin2 as pair,
        COUNT(*) as n,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins
    FROM cross_market_opportunities
    WHERE status = 'settled'
    GROUP BY coin1, coin2
),
wilson AS (
    SELECT
        pair,
        n,
        wins,
        wins::numeric / n as win_rate,
        -- Wilson score CI: z = 1.96 for 95%
        (wins::numeric + 1.9208) / (n + 3.8416) as center,
        1.96 * SQRT(wins::numeric * (n - wins) / n + 0.9604) / (n + 3.8416) as spread
    FROM stats
)
SELECT
    pair,
    n as trades,
    wins,
    ROUND(win_rate * 100, 1) || '%' as win_rate,
    ROUND((center - spread) * 100, 1) || '%' as ci_lower,
    ROUND((center + spread) * 100, 1) || '%' as ci_upper,
    CASE
        WHEN (center - spread) > 0.5 THEN '✓ SIGNIFICANT'
        WHEN (center + spread) < 0.5 THEN '✗ SIG. NEGATIVE'
        ELSE '? INCONCLUSIVE'
    END as significance
FROM wilson
ORDER BY win_rate DESC;
"

echo ""
echo "Sample size requirements for statistical power:"
echo "  - Current sample: May be too small for conclusive results"
echo "  - Target: 100+ trades per pair for 80% power at α=0.05"
echo "  - For 5% edge detection: ~784 trades needed"
echo ""

# ============================================================================
# SECTION 16: KELLY CRITERION & BANKROLL ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}16. KELLY CRITERION & BANKROLL SIMULATION${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Kelly Formula: f* = (p * b - q) / b"
echo "  where p = win prob, q = 1-p, b = net odds (payout/cost - 1)"
echo ""
echo "Recommended: Use 1/4 Kelly (25%) for safety"
echo ""

psql "$DATABASE_URL" -c "
WITH pair_stats AS (
    SELECT
        coin1 || '/' || coin2 as pair,
        COUNT(*) as n,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
        AVG(total_cost) as avg_cost,
        AVG(spread) as avg_spread,
        AVG(CASE WHEN trade_result IN ('WIN', 'DOUBLE_WIN') THEN actual_pnl ELSE 0 END) as avg_win,
        AVG(CASE WHEN trade_result = 'LOSE' THEN ABS(actual_pnl) ELSE 0 END) as avg_loss
    FROM cross_market_opportunities
    WHERE status = 'settled'
    GROUP BY coin1, coin2
    HAVING COUNT(*) >= 5  -- Minimum sample
),
kelly AS (
    SELECT
        pair,
        n,
        wins,
        wins::numeric / n as p,
        1 - (wins::numeric / n) as q,
        avg_cost,
        -- Net odds: average win / average cost
        CASE WHEN avg_loss > 0 THEN avg_win / avg_loss ELSE 1 END as b,
        avg_win,
        avg_loss
    FROM pair_stats
)
SELECT
    pair,
    n as trades,
    ROUND(p * 100, 1) || '%' as win_rate,
    ROUND(avg_win::numeric, 3) as avg_win,
    ROUND(avg_loss::numeric, 3) as avg_loss,
    -- Full Kelly: (p * b - q) / b, capped at 0-1
    ROUND(GREATEST(0, LEAST(1, (p * b - q) / NULLIF(b, 0))) * 100, 1) || '%' as full_kelly,
    -- Quarter Kelly (safer)
    ROUND(GREATEST(0, LEAST(0.25, (p * b - q) / NULLIF(b, 0) * 0.25)) * 100, 1) || '%' as quarter_kelly
FROM kelly
ORDER BY (p * b - q) / NULLIF(b, 0) DESC;
"

echo ""
echo -e "${BOLD}Bankroll Projections:${NC}"
echo ""

# Get stats for ALL trades
TOTAL_TRADES=$(psql "$DATABASE_URL" -t -c "
    SELECT COUNT(*) FROM cross_market_opportunities WHERE status = 'settled';
")
TOTAL_WINDOWS=$(psql "$DATABASE_URL" -t -c "
    SELECT COUNT(DISTINCT window_end) FROM cross_market_opportunities WHERE status = 'settled';
")

# Calculate trades per window (using awk instead of bc)
TRADES_PER_WINDOW=$(awk "BEGIN {printf \"%.1f\", $TOTAL_TRADES / $TOTAL_WINDOWS}")

echo -e "${RED}A) ALL TRADES (Current Strategy - NOT Recommended):${NC}"
echo ""

psql "$DATABASE_URL" -c "
WITH stats AS (
    SELECT
        COUNT(*) as n,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) as p,
        AVG(total_cost) as avg_cost,
        SUM(actual_pnl) / COUNT(*) as ev_per_trade,
        SUM(actual_pnl) as total_pnl
    FROM cross_market_opportunities
    WHERE status = 'settled'
)
SELECT
    n as trades,
    ROUND(p * 100, 1) || '%' as win_rate,
    '\$' || ROUND(ev_per_trade::numeric, 4) as ev_per_trade,
    '\$' || ROUND(total_pnl::numeric, 2) as net_pnl,
    CASE WHEN ev_per_trade > 0 THEN '✓ PROFITABLE' ELSE '✗ LOSING' END as status
FROM stats;
"

echo ""
echo -e "${GREEN}B) OPTIMIZED STRATEGY (Priority Pairs + Coin1DownCoin2Up Only):${NC}"
echo ""
echo "  Filter: BTC/ETH, BTC/XRP, SOL/XRP, ETH/XRP + Coin1DownCoin2Up combination"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
stats AS (
    SELECT
        COUNT(*) as n,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / NULLIF(COUNT(*), 0) as p,
        AVG(total_cost) as avg_cost,
        SUM(actual_pnl) / NULLIF(COUNT(*), 0) as ev_per_trade,
        SUM(actual_pnl) as total_pnl,
        AVG(CASE WHEN trade_result IN ('WIN', 'DOUBLE_WIN') THEN actual_pnl END) as avg_win,
        AVG(CASE WHEN trade_result = 'LOSE' THEN ABS(actual_pnl) END) as avg_loss
    FROM optimized
)
SELECT
    n as trades,
    ROUND(p * 100, 1) || '%' as win_rate,
    '\$' || ROUND(ev_per_trade::numeric, 4) as ev_per_trade,
    '\$' || ROUND(total_pnl::numeric, 2) as net_pnl,
    CASE WHEN ev_per_trade > 0 THEN '✓ PROFITABLE' ELSE '✗ LOSING' END as status
FROM stats;
"

echo ""
echo -e "${BOLD}Optimized Bankroll Projections (\$100 / \$500 / \$1000):${NC}"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
stats AS (
    SELECT
        COUNT(*) as n,
        COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / NULLIF(COUNT(*), 0) as p,
        AVG(total_cost) as avg_cost,
        SUM(actual_pnl) / NULLIF(COUNT(*), 0) as ev_per_trade,
        AVG(CASE WHEN trade_result IN ('WIN', 'DOUBLE_WIN') THEN actual_pnl END) as avg_win,
        AVG(CASE WHEN trade_result = 'LOSE' THEN ABS(actual_pnl) END) as avg_loss
    FROM optimized
),
kelly AS (
    SELECT
        *,
        -- Kelly = (p * b - q) / b where b = avg_win/avg_loss
        CASE
            WHEN avg_loss IS NULL OR avg_loss = 0 THEN 0.25  -- No losses, use max quarter kelly
            ELSE GREATEST(0, LEAST(0.25, (p * (avg_win/avg_loss) - (1-p)) / (avg_win/avg_loss) * 0.25))
        END as quarter_kelly
    FROM stats
),
projections AS (
    SELECT
        bankroll,
        quarter_kelly,
        ROUND(quarter_kelly * bankroll, 2) as kelly_bet_size,
        avg_cost,
        ev_per_trade,
        n
    FROM kelly, (VALUES (100), (500), (1000)) as b(bankroll)
)
SELECT
    '\$' || bankroll as bankroll,
    '\$' || kelly_bet_size as kelly_bet,
    ROUND(kelly_bet_size / avg_cost, 1) as trades_per_bet,
    -- Per-trade projection scaled by position size
    '\$' || ROUND(bankroll + (ev_per_trade * kelly_bet_size / avg_cost * 50), 2) as after_50_windows,
    '\$' || ROUND(bankroll + (ev_per_trade * kelly_bet_size / avg_cost * 100), 2) as after_100_windows,
    '\$' || ROUND(bankroll + (ev_per_trade * kelly_bet_size / avg_cost * 500), 2) as after_500_windows
FROM projections;
"

echo ""
echo -e "${BOLD}ROI Breakdown - What We Actually Measured:${NC}"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
stats AS (
    SELECT
        COUNT(*) as trades,
        COUNT(DISTINCT window_end) as windows,
        SUM(total_cost) as total_deployed,
        SUM(actual_pnl) as net_pnl,
        SUM(actual_pnl) / COUNT(DISTINCT window_end) as profit_per_window,
        AVG(total_cost) as avg_trade_size,
        COUNT(*)::numeric / COUNT(DISTINCT window_end) as trades_per_window
    FROM optimized
)
SELECT
    trades as total_trades,
    windows as total_windows,
    '\$' || ROUND(total_deployed::numeric, 2) as capital_deployed,
    '\$' || ROUND(net_pnl::numeric, 2) as net_profit,
    ROUND((net_pnl / total_deployed * 100)::numeric, 1) || '%' as roi_on_capital,
    '\$' || ROUND(profit_per_window::numeric, 2) as profit_per_window,
    ROUND(trades_per_window::numeric, 1) as trades_per_window,
    '\$' || ROUND(avg_trade_size::numeric, 2) as avg_trade_size
FROM stats;
"

echo ""
echo -e "${BOLD}Bankroll Simulation - If You Started With:${NC}"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
stats AS (
    SELECT
        COUNT(DISTINCT window_end) as windows,
        SUM(actual_pnl) / SUM(total_cost) as roi_per_dollar,  -- profit per dollar deployed
        COUNT(*)::numeric / COUNT(DISTINCT window_end) as trades_per_window,
        AVG(total_cost) as avg_trade_size
    FROM optimized
),
simulation AS (
    SELECT
        bankroll,
        windows,
        -- With quarter Kelly, bet ~25% of bankroll per window (spread across trades)
        LEAST(bankroll * 0.25, bankroll) as bet_per_window,
        roi_per_dollar,
        trades_per_window,
        avg_trade_size
    FROM stats, (VALUES (100), (500), (1000)) as b(bankroll)
)
SELECT
    '\$' || bankroll as starting_bankroll,
    '\$' || ROUND(bet_per_window::numeric, 2) as risk_per_window,
    '\$' || ROUND((bet_per_window * roi_per_dollar * windows)::numeric, 2) as total_profit,
    '\$' || ROUND((bankroll + bet_per_window * roi_per_dollar * windows)::numeric, 2) as ending_bankroll,
    ROUND((bet_per_window * roi_per_dollar * windows / bankroll * 100)::numeric, 1) || '%' as total_roi,
    ROUND((bet_per_window * roi_per_dollar * windows / bankroll * 100 / windows * 96)::numeric, 1) || '%' as projected_daily_roi
FROM simulation;
"

echo ""
echo "  How to read this:"
echo "    - 'Risk per Window': Amount deployed each 15-min window (quarter Kelly)"
echo "    - 'Total Profit': Profit over the sample period (${windows:-N} windows)"
echo "    - 'Total ROI': Return on starting bankroll"
echo "    - 'Projected Daily ROI': Extrapolated to 96 windows/day"
echo ""
echo "  Assumptions:"
echo "    - Quarter Kelly sizing (25% of optimal)"
echo "    - Each 'window' = one 15-minute trading opportunity"
echo "    - 96 windows per day (24 hours)"
echo ""
echo -e "${YELLOW}⚠️  WARNING: Projections assume historical performance continues.${NC}"
echo "    Need 100+ windows of data for reliable projections."
echo ""

# ============================================================================
# SECTION 17: FEE & PROFIT ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}17. FEE & PROFIT ANALYSIS (Optimized Strategy)${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Polymarket charges 2% fee on profits (winning amount minus cost)"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
by_result AS (
    SELECT
        trade_result,
        COUNT(*) as trades,
        SUM(total_cost) as cost,
        SUM(CASE
            WHEN trade_result = 'WIN' THEN 1.0
            WHEN trade_result = 'DOUBLE_WIN' THEN 2.0
            ELSE 0.0
        END) as gross_payout,
        SUM(actual_pnl) as net_pnl
    FROM optimized
    GROUP BY trade_result
)
SELECT
    trade_result,
    trades,
    '\$' || ROUND(cost::numeric, 2) as invested,
    '\$' || ROUND(gross_payout::numeric, 2) as gross_payout,
    '\$' || ROUND((gross_payout - cost)::numeric, 2) as gross_profit,
    '\$' || ROUND((gross_payout - cost - net_pnl)::numeric, 2) as fees,
    '\$' || ROUND(net_pnl::numeric, 2) as net_profit
FROM by_result
ORDER BY trade_result;
"

echo ""
echo -e "${BOLD}Summary:${NC}"

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
summary AS (
    SELECT
        COUNT(*) as trades,
        SUM(total_cost) as invested,
        SUM(CASE
            WHEN trade_result = 'WIN' THEN 1.0
            WHEN trade_result = 'DOUBLE_WIN' THEN 2.0
            ELSE 0.0
        END) as gross_payout,
        SUM(actual_pnl) as net_pnl
    FROM optimized
)
SELECT
    trades as total_trades,
    '\$' || ROUND(invested::numeric, 2) as total_invested,
    '\$' || ROUND(gross_payout::numeric, 2) as gross_payouts,
    '\$' || ROUND((gross_payout - invested)::numeric, 2) as gross_profit,
    '\$' || ROUND((gross_payout - invested - net_pnl)::numeric, 2) as total_fees,
    '\$' || ROUND(net_pnl::numeric, 2) as net_profit,
    ROUND((net_pnl / invested * 100)::numeric, 1) || '%' as roi,
    ROUND(((gross_payout - invested - net_pnl) / NULLIF(gross_payout - invested, 0) * 100)::numeric, 1) || '%' as fee_pct
FROM summary;
"

# ============================================================================
# SECTION 18: SLIPPAGE IMPACT ANALYSIS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}18. SLIPPAGE IMPACT ANALYSIS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Impact of sub-optimal fills on profitability (optimized strategy)"
echo "Slippage = paying more than displayed price per leg"
echo ""

psql "$DATABASE_URL" -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
base_stats AS (
    SELECT
        COUNT(*) as trades,
        SUM(total_cost) as base_cost,
        SUM(CASE
            WHEN trade_result = 'WIN' THEN 1.0
            WHEN trade_result = 'DOUBLE_WIN' THEN 2.0
            ELSE 0.0
        END) as gross_payout,
        SUM(actual_pnl) as base_net_pnl,
        -- Average per-leg cost (total_cost / 2 legs)
        AVG(total_cost / 2) as avg_leg_cost
    FROM optimized
),
slippage_scenarios AS (
    SELECT
        slippage_pct,
        trades,
        base_cost,
        gross_payout,
        base_net_pnl,
        -- Additional cost from slippage (slippage % * 2 legs * trades * avg_leg_cost)
        (slippage_pct / 100.0) * 2 * trades * avg_leg_cost as slippage_cost,
        -- Adjusted net P&L
        base_net_pnl - ((slippage_pct / 100.0) * 2 * trades * avg_leg_cost) as adj_net_pnl
    FROM base_stats, (VALUES (0), (0.5), (1), (2), (3), (5)) as s(slippage_pct)
)
SELECT
    slippage_pct || '%' as slippage,
    '\$' || ROUND(slippage_cost::numeric, 2) as extra_cost,
    '\$' || ROUND(adj_net_pnl::numeric, 2) as net_profit,
    ROUND((adj_net_pnl / (base_cost + slippage_cost) * 100)::numeric, 1) || '%' as adj_roi,
    CASE
        WHEN adj_net_pnl > 0 THEN '✓ PROFITABLE'
        ELSE '✗ LOSING'
    END as status
FROM slippage_scenarios
ORDER BY slippage_pct;
"

echo ""
echo "  Interpretation:"
echo "    - 0% slippage: Perfect fills at displayed price"
echo "    - 1% slippage: Pay \$0.01 extra per \$1.00 leg (e.g., \$0.41 instead of \$0.40)"
echo "    - 2% slippage: Pay \$0.02 extra per \$1.00 leg"
echo ""
echo "  Finding the break-even slippage tolerance..."

# Calculate break-even slippage
BREAKEVEN=$(psql "$DATABASE_URL" -t -c "
WITH optimized AS (
    SELECT *
    FROM cross_market_opportunities
    WHERE status = 'settled'
    AND combination = 'Coin1DownCoin2Up'
    AND NOT (
        (coin1 = 'BTC' AND coin2 = 'SOL') OR
        (coin1 = 'ETH' AND coin2 = 'SOL')
    )
),
stats AS (
    SELECT
        COUNT(*) as trades,
        SUM(actual_pnl) as net_pnl,
        AVG(total_cost / 2) as avg_leg_cost
    FROM optimized
)
SELECT ROUND((net_pnl / (2 * trades * avg_leg_cost) * 100)::numeric, 2)
FROM stats;
")
echo ""
echo -e "  ${BOLD}Break-even slippage tolerance: ${BREAKEVEN}% per leg${NC}"
echo "  (Strategy remains profitable if slippage stays below this)"
echo ""

# ============================================================================
# SECTION 19: OPTIMAL TRADING PARAMETERS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}19. OPTIMAL TRADING PARAMETERS (RECOMMENDED)${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Get best performing pairs
BEST_PAIRS=$(psql "$DATABASE_URL" -t -c "
    SELECT STRING_AGG(pair, ', ')
    FROM (
        SELECT coin1 || '/' || coin2 as pair
        FROM cross_market_opportunities
        WHERE status = 'settled'
        GROUP BY coin1, coin2
        HAVING COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) >= 0.9
        ORDER BY SUM(actual_pnl) DESC
        LIMIT 3
    ) t;
")

BEST_TIMING=$(psql "$DATABASE_URL" -t -c "
    WITH timing AS (
        SELECT
            CASE
                WHEN EXTRACT(EPOCH FROM (window_end - timestamp))/60 >= 10 THEN '10-15 min'
                WHEN EXTRACT(EPOCH FROM (window_end - timestamp))/60 >= 5 THEN '5-10 min'
                WHEN EXTRACT(EPOCH FROM (window_end - timestamp))/60 >= 2 THEN '2-5 min'
                ELSE '0-2 min'
            END as entry_window,
            trade_result
        FROM cross_market_opportunities
        WHERE status = 'settled'
    )
    SELECT entry_window
    FROM timing
    GROUP BY entry_window
    ORDER BY COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::numeric / COUNT(*) DESC
    LIMIT 1;
")

OPTIMAL_SPREAD=$(psql "$DATABASE_URL" -t -c "
    SELECT ROUND(AVG(spread)::numeric, 2)
    FROM cross_market_opportunities
    WHERE status = 'settled' AND trade_result IN ('WIN', 'DOUBLE_WIN');
")

echo "  Based on collected data, recommended parameters:"
echo ""
echo -e "    ${GREEN}✓ Priority Pairs:${NC}    ${BEST_PAIRS:-Need more data}"
echo -e "    ${GREEN}✓ Entry Timing:${NC}      ${BEST_TIMING:-Need more data} before close"
echo -e "    ${GREEN}✓ Min Spread:${NC}        \$0.15 (current avg winning: \$${OPTIMAL_SPREAD})"
echo -e "    ${GREEN}✓ Max Cost:${NC}          \$0.85"
echo -e "    ${GREEN}✓ Min Depth:${NC}         100 shares"
echo -e "    ${GREEN}✓ Kelly Fraction:${NC}    25% (quarter Kelly)"
echo ""
echo -e "    ${RED}✗ Avoid Pairs:${NC}       ETH/SOL, BTC/SOL (low correlation)"
echo -e "    ${RED}✗ Avoid:${NC}             Coin1UpCoin2Down combination (historically negative)"
echo ""

# ============================================================================
# SECTION 20: DATA COLLECTION STATUS
# ============================================================================
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}20. DATA COLLECTION STATUS${NC}"
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

PENDING=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM cross_market_opportunities WHERE status = 'pending';")
SETTLED=$(psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM cross_market_opportunities WHERE status = 'settled';")
WINDOWS_NEEDED=$((100 - TOTAL_WINDOWS))

echo "  Current Status:"
echo "    Settled Trades:    ${SETTLED}"
echo "    Pending Trades:    ${PENDING}"
echo "    Windows Analyzed:  ${TOTAL_WINDOWS}"
echo ""
echo "  Statistical Power:"

if [ "$TOTAL_WINDOWS" -lt 20 ]; then
    echo -e "    ${RED}⚠️  INSUFFICIENT DATA${NC}"
    echo "    Need at least 20 windows (~5 hours) for preliminary analysis"
    echo "    Need 100+ windows (~25 hours) for reliable statistics"
elif [ "$TOTAL_WINDOWS" -lt 100 ]; then
    echo -e "    ${YELLOW}⚠️  PRELIMINARY DATA${NC}"
    echo "    Current: ${TOTAL_WINDOWS} windows"
    echo "    Recommended: 100+ windows for statistical significance"
    echo "    Continue collecting for ~$((WINDOWS_NEEDED / 4)) more hours"
else
    echo -e "    ${GREEN}✓ SUFFICIENT DATA${NC}"
    echo "    ${TOTAL_WINDOWS} windows analyzed - results are statistically meaningful"
fi

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}STRATEGY NOTES:${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  The arbitrage strategy bets on OPPOSING directions across two coins:"
echo "    - Coin1DownCoin2Up = Buy Coin1 DOWN + Buy Coin2 UP"
echo "    - Coin1UpCoin2Down = Buy Coin1 UP + Buy Coin2 DOWN"
echo ""
echo "  EXPECTED OUTCOMES (if coins are ~85% correlated):"
echo "    - ~85% chance: Coins move same direction → ONE leg wins (small profit)"
echo "    - ~7.5% chance: Coins move opposite, matching our bets → DOUBLE WIN"
echo "    - ~7.5% chance: Coins move opposite, against our bets → DOUBLE LOSS"
echo ""
echo "  RISK: When correlation breaks in the WRONG direction, both legs lose."
echo "        This happened in the data when SOL diverged from BTC/ETH."
echo ""
echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
