#!/bin/bash
#
# Database Sync: Pull signal & raw data from EC2 to local for analysis
#
# Usage:
#   ./scripts/db-sync.sh                    # Sync last 24h of data
#   ./scripts/db-sync.sh --hours 48         # Sync last 48h
#   ./scripts/db-sync.sh --start 2026-02-10 --end 2026-02-12  # Specific range
#   ./scripts/db-sync.sh --tables signals   # Only signal_snapshots
#   ./scripts/db-sync.sh --tables raw       # Only OB/funding/liq
#   ./scripts/db-sync.sh --tables all       # Everything (default)
#   ./scripts/db-sync.sh --dump             # Dump to CSV instead of DB insert
#
# Requires:
#   - EC2 state file from polymarket.sh deploy
#   - DATABASE_URL set locally (target database)
#   - psql available on both local and remote
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Source .env
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Use Polymarket state file (falls back to old name via ec2-common defaults)
if [[ -f "$SCRIPT_DIR/.polymarket.state" ]]; then
    EC2_STATE_FILE="$SCRIPT_DIR/.polymarket.state"
    EC2_KEY_FILE="$SCRIPT_DIR/.polymarket-key.pem"
    export EC2_STATE_FILE EC2_KEY_FILE
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

# =============================================================================
# Defaults
# =============================================================================

HOURS="24"
START=""
END=""
TABLES="all"
DUMP_MODE=""
DUMP_DIR="$PROJECT_ROOT/data/sync"

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
        --tables)
            TABLES="$2"
            shift 2
            ;;
        --dump)
            DUMP_MODE="1"
            shift
            ;;
        --dump-dir)
            DUMP_DIR="$2"
            shift 2
            ;;
        --help|-h)
            head -18 "$0" | tail -17
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

load_state

if [[ -z "$DUMP_MODE" && -z "${DATABASE_URL:-}" ]]; then
    echo -e "${RED}ERROR: DATABASE_URL required for DB sync (or use --dump for CSV)${NC}"
    exit 1
fi

# =============================================================================
# Time range
# =============================================================================

if [[ -n "$START" && -n "$END" ]]; then
    TIME_FILTER="timestamp >= '$START'::timestamptz AND timestamp < '$END'::timestamptz"
    RANGE_DESC="$START to $END"
else
    TIME_FILTER="timestamp >= NOW() - INTERVAL '${HOURS} hours'"
    RANGE_DESC="last ${HOURS}h"
fi

# =============================================================================
# Table list
# =============================================================================

declare -a SYNC_TABLES

case "$TABLES" in
    signals)
        SYNC_TABLES=("signal_snapshots")
        ;;
    raw)
        SYNC_TABLES=("orderbook_snapshots" "funding_rates" "liquidations")
        ;;
    all)
        SYNC_TABLES=("signal_snapshots" "orderbook_snapshots" "funding_rates" "liquidations" "directional_trades" "directional_sessions")
        ;;
    *)
        # Allow comma-separated table names
        IFS=',' read -ra SYNC_TABLES <<< "$TABLES"
        ;;
esac

# =============================================================================
# Sync
# =============================================================================

echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║${NC}        ${WHITE}Database Sync: EC2 → Local${NC}                               ${CYAN}║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${DIM}Remote:${NC}  $PUBLIC_IP"
echo -e "  ${DIM}Range:${NC}   $RANGE_DESC"
echo -e "  ${DIM}Tables:${NC}  ${SYNC_TABLES[*]}"
if [[ -n "$DUMP_MODE" ]]; then
    echo -e "  ${DIM}Mode:${NC}    CSV dump → $DUMP_DIR"
else
    echo -e "  ${DIM}Mode:${NC}    DB insert (upsert)"
fi
echo ""

# Ensure local migrations are up to date
if [[ -z "$DUMP_MODE" ]]; then
    echo -e "${DIM}Running local migrations...${NC}"
    "$SCRIPT_DIR/migrate.sh" 2>&1 | grep -E '\[APPLY\]' || echo -e "  ${DIM}All migrations applied${NC}"
    echo ""
fi

# Create dump dir if needed
if [[ -n "$DUMP_MODE" ]]; then
    mkdir -p "$DUMP_DIR"
fi

TOTAL_ROWS=0

for table in "${SYNC_TABLES[@]}"; do
    echo -e "${WHITE}Syncing: ${CYAN}$table${NC}"

    # Get remote row count
    REMOTE_COUNT=$(remote_ssh "psql \$DATABASE_URL -tAc \"SELECT COUNT(*) FROM $table WHERE $TIME_FILTER;\"" 2>/dev/null || echo "0")
    REMOTE_COUNT=$(echo "$REMOTE_COUNT" | tr -d '[:space:]')

    if [[ "$REMOTE_COUNT" == "0" || -z "$REMOTE_COUNT" ]]; then
        echo -e "  ${DIM}No rows in range, skipping${NC}"
        continue
    fi

    echo -e "  ${DIM}Remote rows: $REMOTE_COUNT${NC}"

    if [[ -n "$DUMP_MODE" ]]; then
        # Dump to CSV
        CSV_FILE="$DUMP_DIR/${table}_$(date +%Y%m%d_%H%M%S).csv"
        echo -e "  ${DIM}Dumping to $CSV_FILE...${NC}"
        remote_ssh "psql \$DATABASE_URL -c \"COPY (SELECT * FROM $table WHERE $TIME_FILTER ORDER BY timestamp) TO STDOUT WITH CSV HEADER;\"" > "$CSV_FILE" 2>/dev/null
        ROWS=$(wc -l < "$CSV_FILE")
        ROWS=$((ROWS - 1))  # subtract header
        echo -e "  ${GREEN}Dumped $ROWS rows${NC}"
    else
        # Pipe through COPY for fast transfer
        echo -e "  ${DIM}Transferring...${NC}"
        remote_ssh "psql \$DATABASE_URL -c \"COPY (SELECT * FROM $table WHERE $TIME_FILTER ORDER BY timestamp) TO STDOUT WITH (FORMAT binary);\"" 2>/dev/null \
            | psql "$DATABASE_URL" -c "COPY $table FROM STDIN WITH (FORMAT binary);" 2>/dev/null

        # Check how many made it
        LOCAL_COUNT=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM $table WHERE $TIME_FILTER;" 2>/dev/null || echo "?")
        LOCAL_COUNT=$(echo "$LOCAL_COUNT" | tr -d '[:space:]')
        ROWS="$LOCAL_COUNT"
        echo -e "  ${GREEN}Synced (local now has $LOCAL_COUNT rows in range)${NC}"
    fi

    TOTAL_ROWS=$((TOTAL_ROWS + ${ROWS:-0}))
done

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
echo -e "${WHITE}Sync complete: $TOTAL_ROWS total rows${NC}"

if [[ -z "$DUMP_MODE" ]]; then
    echo ""
    echo -e "${DIM}Next steps:${NC}"
    echo -e "  ${DIM}1. Calculate forward returns:${NC}"
    echo -e "     cargo run -p algo-trade-cli -- calculate-returns --start <start> --end <end> --price-source orderbook"
    echo -e "  ${DIM}2. Validate signals:${NC}"
    echo -e "     cargo run -p algo-trade-cli -- validate-signals --start <start> --end <end>"
fi
echo ""
