#!/bin/bash
#
# Analysis Pipeline: Sync EC2 data → local Docker DB → compute returns → validate signals
#
# Usage:
#   ./scripts/run_analysis.sh                          # Sync last 24h, analyze all
#   ./scripts/run_analysis.sh --hours 48               # Sync last 48h
#   ./scripts/run_analysis.sh --start 2026-02-10 --end 2026-02-12
#   ./scripts/run_analysis.sh --skip-sync              # Skip sync, just run analysis
#   ./scripts/run_analysis.sh --skip-returns           # Skip calculate-returns
#   ./scripts/run_analysis.sh --symbols btc,eth        # Only analyze specific coins
#   --help                                              # Show this help
#
# Requires:
#   - Docker running with algo-trade-db container (or docker-compose up timescaledb)
#   - EC2 state file (for sync, skip with --skip-sync)
#   - psql available locally
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Source .env
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
# Defaults
# =============================================================================

HOURS="24"
START=""
END=""
SKIP_SYNC=""
SKIP_RETURNS=""
SYMBOLS="btc,eth,sol,xrp"
TABLES="all"
MIN_SAMPLES="30"
PRICE_SOURCE="orderbook"

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --hours)
            HOURS="$2"
            shift 2
            ;;
        --start)
            START="$2"
            shift 2
            ;;
        --end)
            END="$2"
            shift 2
            ;;
        --skip-sync)
            SKIP_SYNC="1"
            shift
            ;;
        --skip-returns)
            SKIP_RETURNS="1"
            shift
            ;;
        --symbols)
            SYMBOLS="$2"
            shift 2
            ;;
        --tables)
            TABLES="$2"
            shift 2
            ;;
        --min-samples)
            MIN_SAMPLES="$2"
            shift 2
            ;;
        --price-source)
            PRICE_SOURCE="$2"
            shift 2
            ;;
        --help|-h)
            head -17 "$0" | tail -16
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# =============================================================================
# Docker DB check
# =============================================================================

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Analysis Pipeline${NC}                                      ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# Determine local DATABASE_URL (Docker container)
if [[ -z "${DATABASE_URL:-}" ]]; then
    if [[ -f "$PROJECT_ROOT/secrets/db_password.txt" ]]; then
        DB_PASSWORD=$(cat "$PROJECT_ROOT/secrets/db_password.txt")
        DATABASE_URL="postgresql://postgres:${DB_PASSWORD}@localhost:${DB_PORT:-5432}/algo_trade"
        export DATABASE_URL
    else
        echo -e "${RED}ERROR: DATABASE_URL not set and no secrets/db_password.txt found${NC}"
        exit 1
    fi
fi

# Check if Docker DB is running
echo -e "${DIM}Checking Docker database...${NC}"
if docker ps --format '{{.Names}}' 2>/dev/null | grep -q 'algo-trade-db'; then
    echo -e "  ${GREEN}algo-trade-db is running${NC}"
elif command -v docker &>/dev/null; then
    echo -e "  ${YELLOW}algo-trade-db not running, starting...${NC}"
    docker compose -f "$PROJECT_ROOT/docker-compose.yml" up -d timescaledb
    echo -e "  ${DIM}Waiting for DB to be healthy...${NC}"
    for i in $(seq 1 30); do
        if docker exec algo-trade-db pg_isready -U postgres -d algo_trade &>/dev/null; then
            echo -e "  ${GREEN}Database ready${NC}"
            break
        fi
        if [[ $i -eq 30 ]]; then
            echo -e "  ${RED}Database failed to start${NC}"
            exit 1
        fi
        sleep 1
    done
else
    echo -e "  ${YELLOW}Docker not available, assuming DATABASE_URL points to a running DB${NC}"
fi

# Run local migrations
echo -e "${DIM}Running migrations...${NC}"
"$SCRIPT_DIR/migrate.sh" 2>&1 | grep -E '\[APPLY\]' || echo -e "  ${DIM}All migrations current${NC}"
echo ""

# =============================================================================
# Compute time range
# =============================================================================

if [[ -n "$START" && -n "$END" ]]; then
    ANALYSIS_START="$START"
    ANALYSIS_END="$END"
    RANGE_DESC="$START → $END"
else
    # Compute ISO timestamps for N hours ago
    ANALYSIS_END=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    ANALYSIS_START=$(date -u -d "${HOURS} hours ago" +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null \
        || date -u -v-"${HOURS}"H +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null)
    RANGE_DESC="last ${HOURS}h ($ANALYSIS_START → $ANALYSIS_END)"
fi

echo -e "${WHITE}Configuration:${NC}"
echo -e "  ${DIM}Range:${NC}        $RANGE_DESC"
echo -e "  ${DIM}Symbols:${NC}      ${SYMBOLS^^}"
echo -e "  ${DIM}Price source:${NC} $PRICE_SOURCE"
echo -e "  ${DIM}Min samples:${NC}  $MIN_SAMPLES"
echo -e "  ${DIM}Sync:${NC}         $(if [[ -n "$SKIP_SYNC" ]]; then echo 'SKIP'; else echo 'EC2 → local'; fi)"
echo ""

# =============================================================================
# Step 1: Sync from EC2
# =============================================================================

if [[ -z "$SKIP_SYNC" ]]; then
    echo -e "${CYAN}━━━ Step 1: Sync EC2 → Local ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    SYNC_ARGS=(--tables "$TABLES")
    if [[ -n "$START" && -n "$END" ]]; then
        SYNC_ARGS+=(--start "$START" --end "$END")
    else
        SYNC_ARGS+=(--hours "$HOURS")
    fi

    if "$SCRIPT_DIR/db-sync.sh" "${SYNC_ARGS[@]}"; then
        echo -e "  ${GREEN}Sync complete${NC}"
    else
        echo -e "  ${YELLOW}Sync failed (continuing with local data)${NC}"
    fi
    echo ""
else
    echo -e "${DIM}Skipping EC2 sync${NC}"
    echo ""
fi

# =============================================================================
# Step 2: Data inventory
# =============================================================================

echo -e "${CYAN}━━━ Step 2: Data Inventory ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

psql "$DATABASE_URL" -c "
SELECT
    'signal_snapshots' as table_name,
    COUNT(*) as rows,
    MIN(timestamp)::text as earliest,
    MAX(timestamp)::text as latest
FROM signal_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
UNION ALL
SELECT 'orderbook_snapshots', COUNT(*), MIN(timestamp)::text, MAX(timestamp)::text
FROM orderbook_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
UNION ALL
SELECT 'funding_rates', COUNT(*), MIN(timestamp)::text, MAX(timestamp)::text
FROM funding_rates
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
ORDER BY table_name;
" 2>/dev/null || echo -e "  ${RED}Failed to query data inventory${NC}"

# Show signal snapshot breakdown by symbol
echo ""
echo -e "${DIM}Signal snapshots by symbol:${NC}"
psql "$DATABASE_URL" -c "
SELECT symbol, direction, COUNT(*) as count
FROM signal_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
  AND signal_name = 'directional_composite'
GROUP BY symbol, direction
ORDER BY symbol, direction;
" 2>/dev/null || true

echo ""

# =============================================================================
# Step 3: Calculate forward returns
# =============================================================================

if [[ -z "$SKIP_RETURNS" ]]; then
    echo -e "${CYAN}━━━ Step 3: Calculate Forward Returns ━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    # Run calculate-returns per symbol
    IFS=',' read -ra COIN_LIST <<< "$SYMBOLS"
    for coin in "${COIN_LIST[@]}"; do
        SYMBOL="${coin^^}USDT"
        echo -e "  ${WHITE}$SYMBOL${NC}..."

        cargo run -p algo-trade-cli --release -- calculate-returns \
            --start "$ANALYSIS_START" \
            --end "$ANALYSIS_END" \
            --symbol "$SYMBOL" \
            --price-source "$PRICE_SOURCE" \
            2>&1 | grep -E 'Updated|processed|No snapshots|Error|returns' || echo -e "    ${DIM}(no output)${NC}"
    done
    echo ""
else
    echo -e "${DIM}Skipping forward return calculation${NC}"
    echo ""
fi

# =============================================================================
# Step 4: Validate signals
# =============================================================================

echo -e "${CYAN}━━━ Step 4: Validate Signals ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

cargo run -p algo-trade-cli --release -- validate-signals \
    --start "$ANALYSIS_START" \
    --end "$ANALYSIS_END" \
    --min-samples "$MIN_SAMPLES" \
    2>&1

echo ""

# =============================================================================
# Step 5: Per-signal factor analysis
# =============================================================================

echo -e "${CYAN}━━━ Step 5: Per-Signal Factor Analysis ━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Check if metadata is populated
META_COUNT=$(psql "$DATABASE_URL" -tAc "
SELECT COUNT(*)
FROM signal_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
  AND signal_name = 'directional_composite'
  AND metadata IS NOT NULL
  AND metadata != '{}'::jsonb;
" 2>/dev/null || echo "0")

if [[ "$META_COUNT" -lt "$MIN_SAMPLES" ]]; then
    echo -e "  ${YELLOW}Insufficient metadata rows ($META_COUNT < $MIN_SAMPLES).${NC}"
    echo -e "  ${DIM}Run collector with updated code to populate per-signal metadata.${NC}"
    echo ""
else
    echo -e "  ${GREEN}$META_COUNT snapshots with per-signal metadata${NC}"
    echo ""

    # --- 5a: Per-signal correlation with forward returns ---
    echo -e "${WHITE}  5a) Per-signal correlation with 15m forward return:${NC}"
    echo ""

    SIGNAL_NAMES=("orderbook_imbalance" "liquidation_cascade" "funding_rate" "cvd_divergence" "momentum_exhaustion" "news_sentiment" "liquidation_ratio")

    for sig in "${SIGNAL_NAMES[@]}"; do
        psql "$DATABASE_URL" -c "
SELECT
    '${sig}' as signal,
    COUNT(*) as n,
    ROUND(CORR(
        (metadata->>'${sig}_direction')::float * (metadata->>'${sig}_strength')::float,
        forward_return_15m
    )::numeric, 4) as corr_with_return,
    ROUND(AVG((metadata->>'${sig}_strength')::float)::numeric, 4) as avg_strength,
    ROUND(AVG(CASE WHEN (metadata->>'${sig}_direction')::float = 1 THEN forward_return_15m END)::numeric, 6) as avg_ret_when_up,
    ROUND(AVG(CASE WHEN (metadata->>'${sig}_direction')::float = -1 THEN forward_return_15m END)::numeric, 6) as avg_ret_when_down
FROM signal_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
  AND signal_name = 'directional_composite'
  AND forward_return_15m IS NOT NULL
  AND metadata->>'${sig}_direction' IS NOT NULL;
" 2>/dev/null || echo -e "    ${DIM}(${sig}: no data)${NC}"
    done

    echo ""

    # --- 5b: Signal agreement analysis ---
    echo -e "${WHITE}  5b) Signal agreement vs hit rate:${NC}"
    echo ""

    psql "$DATABASE_URL" -c "
WITH signal_dirs AS (
    SELECT
        timestamp,
        symbol,
        direction,
        forward_return_15m,
        (CASE WHEN (metadata->>'orderbook_imbalance_direction')::float = 1 THEN 1
              WHEN (metadata->>'orderbook_imbalance_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'liquidation_cascade_direction')::float = 1 THEN 1
              WHEN (metadata->>'liquidation_cascade_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'funding_rate_direction')::float = 1 THEN 1
              WHEN (metadata->>'funding_rate_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'cvd_divergence_direction')::float = 1 THEN 1
              WHEN (metadata->>'cvd_divergence_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'momentum_exhaustion_direction')::float = 1 THEN 1
              WHEN (metadata->>'momentum_exhaustion_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'news_sentiment_direction')::float = 1 THEN 1
              WHEN (metadata->>'news_sentiment_direction')::float = -1 THEN -1 ELSE 0 END
        + CASE WHEN (metadata->>'liquidation_ratio_direction')::float = 1 THEN 1
              WHEN (metadata->>'liquidation_ratio_direction')::float = -1 THEN -1 ELSE 0 END
        ) as net_agreement
    FROM signal_snapshots
    WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
      AND signal_name = 'directional_composite'
      AND forward_return_15m IS NOT NULL
      AND metadata != '{}'::jsonb
)
SELECT
    net_agreement,
    COUNT(*) as n,
    ROUND(AVG(forward_return_15m)::numeric, 6) as avg_return,
    ROUND(STDDEV(forward_return_15m)::numeric, 6) as std_return,
    ROUND((COUNT(CASE WHEN (net_agreement > 0 AND forward_return_15m > 0)
                   OR (net_agreement < 0 AND forward_return_15m < 0)
              THEN 1 END)::float / NULLIF(COUNT(*), 0))::numeric, 3) as hit_rate
FROM signal_dirs
GROUP BY net_agreement
ORDER BY net_agreement;
" 2>/dev/null || echo -e "  ${DIM}(agreement analysis failed)${NC}"

    echo ""

    # --- 5c: Factor attribution summary ---
    echo -e "${WHITE}  5c) Factor attribution (all signals, ranked by |correlation|):${NC}"
    echo ""

    psql "$DATABASE_URL" -c "
WITH factors AS (
    SELECT
        unnest(ARRAY['orderbook_imbalance','liquidation_cascade','funding_rate','cvd_divergence','momentum_exhaustion','news_sentiment','liquidation_ratio']) as signal_name,
        unnest(ARRAY[
            CORR((metadata->>'orderbook_imbalance_direction')::float * (metadata->>'orderbook_imbalance_strength')::float, forward_return_15m),
            CORR((metadata->>'liquidation_cascade_direction')::float * (metadata->>'liquidation_cascade_strength')::float, forward_return_15m),
            CORR((metadata->>'funding_rate_direction')::float * (metadata->>'funding_rate_strength')::float, forward_return_15m),
            CORR((metadata->>'cvd_divergence_direction')::float * (metadata->>'cvd_divergence_strength')::float, forward_return_15m),
            CORR((metadata->>'momentum_exhaustion_direction')::float * (metadata->>'momentum_exhaustion_strength')::float, forward_return_15m),
            CORR((metadata->>'news_sentiment_direction')::float * (metadata->>'news_sentiment_strength')::float, forward_return_15m),
            CORR((metadata->>'liquidation_ratio_direction')::float * (metadata->>'liquidation_ratio_strength')::float, forward_return_15m)
        ]) as correlation
    FROM signal_snapshots
    WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
      AND signal_name = 'directional_composite'
      AND forward_return_15m IS NOT NULL
      AND metadata != '{}'::jsonb
)
SELECT
    signal_name,
    ROUND(correlation::numeric, 4) as correlation,
    CASE WHEN ABS(correlation) > 0.1 THEN '***'
         WHEN ABS(correlation) > 0.05 THEN '**'
         WHEN ABS(correlation) > 0.02 THEN '*'
         ELSE '' END as significance
FROM factors
ORDER BY ABS(correlation) DESC NULLS LAST;
" 2>/dev/null || echo -e "  ${DIM}(factor attribution failed)${NC}"

    echo ""
fi

# =============================================================================
# Summary
# =============================================================================

echo -e "${CYAN}━━━ Quick Stats ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Show signals with forward returns computed
psql "$DATABASE_URL" -c "
SELECT
    symbol,
    COUNT(*) as total_snapshots,
    COUNT(forward_return_15m) as with_returns,
    ROUND(AVG(forward_return_15m)::numeric, 6) as avg_return,
    ROUND(AVG(CASE WHEN direction = 'up' THEN forward_return_15m END)::numeric, 6) as avg_up_return,
    ROUND(AVG(CASE WHEN direction = 'down' THEN forward_return_15m END)::numeric, 6) as avg_down_return,
    COUNT(CASE WHEN direction != 'neutral' THEN 1 END) as directional_signals
FROM signal_snapshots
WHERE timestamp >= '$ANALYSIS_START'::timestamptz AND timestamp < '$ANALYSIS_END'::timestamptz
  AND signal_name = 'directional_composite'
GROUP BY symbol
ORDER BY symbol;
" 2>/dev/null || echo -e "  ${DIM}(no return data yet)${NC}"

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Analysis complete${NC}"
echo ""
