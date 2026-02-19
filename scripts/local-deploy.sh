#!/bin/bash
#
# Local deployment — migrate, start all services, validate health.
#
# Usage:
#   ./scripts/local-deploy.sh              # Full deploy: migrate + start + validate
#   ./scripts/local-deploy.sh start        # Start all services (skip migrate)
#   ./scripts/local-deploy.sh stop         # Stop all services
#   ./scripts/local-deploy.sh restart      # Stop + start
#   ./scripts/local-deploy.sh status       # Health check all services
#   ./scripts/local-deploy.sh logs [svc]   # Tail logs (scheduler|dashboard-api|dashboard-ui)
#   ./scripts/local-deploy.sh migrate      # Run migrations only
#
# Services managed:
#   1. PostgreSQL/TimescaleDB (Docker)
#   2. IB Gateway (Docker)
#   3. OpenClaw Scheduler (Python — includes position manager)
#   4. Dashboard API (FastAPI/uvicorn)
#   5. Dashboard UI (Next.js dev server)
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_DIR="$PROJECT_ROOT/logs"
PID_DIR="$PROJECT_ROOT/.pids"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
DIM='\033[0;2m'
NC='\033[0m'

# Load .env
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    source "$PROJECT_ROOT/.env"
    set +a
fi

DATABASE_URL="${DATABASE_URL:-postgres://postgres:changeme_secure_password@localhost:5432/algo_trade}"

# =============================================================================
# Helpers
# =============================================================================

info()  { echo -e "  ${GREEN}✓${NC} $*"; }
warn()  { echo -e "  ${YELLOW}!${NC} $*"; }
fail()  { echo -e "  ${RED}✗${NC} $*"; }
header() {
    echo ""
    echo -e "  ${CYAN}━━━ $* ━━━${NC}"
}

ensure_dirs() {
    mkdir -p "$LOG_DIR" "$PID_DIR"
}

pid_file() { echo "$PID_DIR/$1.pid"; }

save_pid() {
    echo "$2" > "$(pid_file "$1")"
}

read_pid() {
    local f
    f="$(pid_file "$1")"
    [[ -f "$f" ]] && cat "$f" || echo ""
}

is_running() {
    local pid
    pid="$(read_pid "$1")"
    [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

stop_service() {
    local name="$1"
    local pid
    pid="$(read_pid "$name")"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        # Wait up to 5 seconds for graceful shutdown
        for _ in {1..10}; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.5
        done
        # Force kill if still alive
        kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
        info "Stopped $name (pid $pid)"
    fi
    rm -f "$(pid_file "$name")"
}

wait_for_port() {
    local port="$1" label="$2" timeout="${3:-15}"
    local elapsed=0
    while ! (echo > /dev/tcp/127.0.0.1/"$port") 2>/dev/null; do
        sleep 1
        elapsed=$((elapsed + 1))
        if [[ $elapsed -ge $timeout ]]; then
            fail "$label not responding on port $port after ${timeout}s"
            return 1
        fi
    done
    return 0
}

# =============================================================================
# migrate — Apply pending SQL migrations
# =============================================================================

cmd_migrate() {
    header "Migrations"

    # Check Postgres is reachable
    if ! psql "$DATABASE_URL" -c "SELECT 1" &>/dev/null; then
        fail "Cannot connect to PostgreSQL at $DATABASE_URL"
        return 1
    fi
    info "PostgreSQL connected"

    # Ensure schema_migrations table exists
    psql "$DATABASE_URL" -q -c "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            filename TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ DEFAULT NOW()
        );
    "

    # Find and apply pending migrations
    local applied=0
    for migration in "$SCRIPT_DIR"/migrations/V*.sql; do
        [[ -f "$migration" ]] || continue
        local fname
        fname="$(basename "$migration")"

        # Check if already applied
        local exists
        exists=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM schema_migrations WHERE filename = '$fname'")
        if [[ "$exists" == "0" ]]; then
            echo -n "  Applying $fname... "
            if psql "$DATABASE_URL" -q -f "$migration" 2>/dev/null; then
                psql "$DATABASE_URL" -q -c "INSERT INTO schema_migrations (filename) VALUES ('$fname')"
                echo -e "${GREEN}done${NC}"
                applied=$((applied + 1))
            else
                echo -e "${RED}FAILED${NC}"
                fail "Migration $fname failed — fix and retry"
                return 1
            fi
        fi
    done

    if [[ $applied -eq 0 ]]; then
        info "All migrations up to date"
    else
        info "Applied $applied migration(s)"
    fi
}

# =============================================================================
# start — Start all services
# =============================================================================

cmd_start() {
    ensure_dirs

    header "Starting services"

    # 1. Docker: TimescaleDB
    echo ""
    echo -e "  ${DIM}[1/5] TimescaleDB${NC}"
    local db_container
    db_container=$(docker ps --filter "name=algo-trade-db" --format '{{.Names}}' 2>/dev/null || true)
    if [[ -n "$db_container" ]]; then
        info "Already running"
    else
        # Try to start existing stopped container, else start from compose
        if docker start algo-trade-db &>/dev/null; then
            info "Started existing container"
        else
            (cd "$PROJECT_ROOT" && docker compose up -d timescaledb 2>&1 | tail -1)
            info "Started via docker compose"
        fi
    fi

    # Wait for DB to be ready
    if ! wait_for_port 5432 "TimescaleDB" 20; then
        return 1
    fi

    # 2. Docker: IB Gateway
    echo -e "  ${DIM}[2/5] IB Gateway${NC}"
    local gw_container
    gw_container=$(docker ps --filter "name=ib-gateway" --format '{{.Names}}' 2>/dev/null || true)
    if [[ -n "$gw_container" ]]; then
        info "Already running"
    elif docker start ib-gateway &>/dev/null; then
        info "Started existing container"
    else
        warn "No ib-gateway container found — start manually or run:"
        warn "  docker compose --profile ib up -d ib-gateway"
        warn "  (Scheduler will start in sim mode without IB)"
    fi

    # Ensure Python deps are installed
    echo -e "  ${DIM}[deps] Python packages${NC}"
    (
        cd "$PROJECT_ROOT/python"
        source .venv/bin/activate
        pip install -e . -q 2>&1 | grep -v "already satisfied" | tail -3
        deactivate 2>/dev/null || true
    )
    info "Python environment ready"

    # 3. OpenClaw Scheduler (includes position manager)
    echo -e "  ${DIM}[3/5] OpenClaw Scheduler${NC}"
    if is_running scheduler; then
        info "Already running (pid $(read_pid scheduler))"
    else
        local ib_mode="sim"
        if docker ps --filter "name=ib-gateway" --format '{{.Names}}' 2>/dev/null | grep -q ib-gateway; then
            ib_mode="paper"
        fi

        cd "$PROJECT_ROOT/python"
        source .venv/bin/activate
        DATABASE_URL="$DATABASE_URL" \
        nohup python -m openclaw \
            --db-url "$DATABASE_URL" \
            scheduler \
            --mode "$ib_mode" \
            --auto-approve \
            >> "$LOG_DIR/scheduler.log" 2>&1 &
        local SCHED_PID=$!
        deactivate 2>/dev/null || true
        cd "$PROJECT_ROOT"

        save_pid scheduler "$SCHED_PID"
        sleep 2
        if kill -0 "$SCHED_PID" 2>/dev/null; then
            info "Started (pid $SCHED_PID, mode=$ib_mode)"
        else
            fail "Failed to start — check $LOG_DIR/scheduler.log"
        fi
    fi

    # 4. Dashboard API (FastAPI)
    echo -e "  ${DIM}[4/5] Dashboard API${NC}"
    if is_running dashboard-api; then
        info "Already running (pid $(read_pid dashboard-api))"
    elif (echo > /dev/tcp/127.0.0.1/8000) 2>/dev/null; then
        warn "Port 8000 already in use — skipping"
    else
        cd "$PROJECT_ROOT/python"
        source .venv/bin/activate
        DATABASE_URL="$DATABASE_URL" \
        nohup python -m uvicorn dashboard.app:app \
            --host 127.0.0.1 \
            --port 8000 \
            >> "$LOG_DIR/dashboard-api.log" 2>&1 &
        local API_PID=$!
        deactivate 2>/dev/null || true
        cd "$PROJECT_ROOT"

        save_pid dashboard-api "$API_PID"
        sleep 2
        if kill -0 "$API_PID" 2>/dev/null; then
            info "Started (pid $API_PID, port 8000)"
        else
            fail "Failed to start — check $LOG_DIR/dashboard-api.log"
        fi
    fi

    # 5. Dashboard UI (Next.js)
    echo -e "  ${DIM}[5/5] Dashboard UI${NC}"
    if is_running dashboard-ui; then
        info "Already running (pid $(read_pid dashboard-ui))"
    elif (echo > /dev/tcp/127.0.0.1/3000) 2>/dev/null; then
        info "Already running on port 3000 (external)"
    else
        cd "$PROJECT_ROOT/dashboard"
        nohup npx next dev --port 3000 \
            >> "$LOG_DIR/dashboard-ui.log" 2>&1 &
        local UI_PID=$!
        cd "$PROJECT_ROOT"

        save_pid dashboard-ui "$UI_PID"
        sleep 3
        if kill -0 "$UI_PID" 2>/dev/null; then
            info "Started (pid $UI_PID, port 3000)"
        else
            fail "Failed to start — check $LOG_DIR/dashboard-ui.log"
        fi
    fi

    echo ""
}

# =============================================================================
# stop — Stop all managed services
# =============================================================================

cmd_stop() {
    header "Stopping services"
    stop_service dashboard-ui
    stop_service dashboard-api
    stop_service scheduler
    # Docker services are left running (persistent data)
    info "Docker containers (TimescaleDB, IB Gateway) left running"
    echo ""
}

# =============================================================================
# status — Health check
# =============================================================================

cmd_status() {
    echo ""
    echo -e "  ${CYAN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "  ${CYAN}║${NC}        OpenClaw Local Services — Status                    ${CYAN}║${NC}"
    echo -e "  ${CYAN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    local all_ok=true

    # TimescaleDB
    echo -n "  TimescaleDB        "
    if docker ps --filter "name=algo-trade-db" --format '{{.Status}}' 2>/dev/null | grep -q "Up"; then
        local db_status
        db_status=$(docker ps --filter "name=algo-trade-db" --format '{{.Status}}')
        echo -e "${GREEN}running${NC}  ($db_status)"
    else
        echo -e "${RED}stopped${NC}"
        all_ok=false
    fi

    # IB Gateway
    echo -n "  IB Gateway         "
    if docker ps --filter "name=ib-gateway" --format '{{.Status}}' 2>/dev/null | grep -q "Up"; then
        local gw_status
        gw_status=$(docker ps --filter "name=ib-gateway" --format '{{.Status}}')
        echo -e "${GREEN}running${NC}  ($gw_status)"
    else
        echo -e "${YELLOW}stopped${NC}  (scheduler will use sim mode)"
    fi

    # Scheduler
    echo -n "  Scheduler          "
    if is_running scheduler; then
        echo -e "${GREEN}running${NC}  (pid $(read_pid scheduler))"
    else
        echo -e "${RED}stopped${NC}"
        all_ok=false
    fi

    # Dashboard API
    echo -n "  Dashboard API      "
    if is_running dashboard-api; then
        # Health check
        if curl -sf http://127.0.0.1:8000/api/health >/dev/null 2>&1; then
            echo -e "${GREEN}running${NC}  (pid $(read_pid dashboard-api), http://localhost:8000)"
        else
            echo -e "${YELLOW}starting${NC}  (pid $(read_pid dashboard-api), not yet healthy)"
        fi
    else
        echo -e "${RED}stopped${NC}"
        all_ok=false
    fi

    # Dashboard UI
    echo -n "  Dashboard UI       "
    if is_running dashboard-ui; then
        echo -e "${GREEN}running${NC}  (pid $(read_pid dashboard-ui), http://localhost:3000)"
    elif (echo > /dev/tcp/127.0.0.1/3000) 2>/dev/null; then
        echo -e "${GREEN}running${NC}  (http://localhost:3000)"
    else
        echo -e "${RED}stopped${NC}"
        all_ok=false
    fi

    # DB stats
    echo ""
    if psql "$DATABASE_URL" -c "SELECT 1" &>/dev/null 2>&1; then
        local tbl_count pos_count rec_count wl_count
        tbl_count=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public'" 2>/dev/null || echo "?")
        pos_count=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM options_positions WHERE status = 'open'" 2>/dev/null || echo "?")
        rec_count=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM trade_recommendations WHERE status IN ('pending_review','approved','submitted')" 2>/dev/null || echo "?")
        wl_count=$(psql "$DATABASE_URL" -tAc "SELECT COUNT(*) FROM options_watchlist" 2>/dev/null || echo "?")

        echo -e "  ${DIM}Database:${NC} $tbl_count tables | $wl_count watchlist | $pos_count open positions | $rec_count pending recs"
    fi

    # Migrations
    local latest_applied latest_available
    latest_applied=$(psql "$DATABASE_URL" -tAc "SELECT filename FROM schema_migrations ORDER BY filename DESC LIMIT 1" 2>/dev/null || echo "?")
    latest_available=$(ls "$SCRIPT_DIR"/migrations/V*.sql 2>/dev/null | sort | tail -1 | xargs basename 2>/dev/null || echo "?")
    echo -e "  ${DIM}Migrations:${NC} latest applied=$latest_applied | latest available=$latest_available"

    echo ""
    if $all_ok; then
        echo -e "  ${GREEN}All services healthy${NC}"
    else
        echo -e "  ${YELLOW}Some services need attention${NC}"
    fi
    echo ""
}

# =============================================================================
# logs — Tail service logs
# =============================================================================

cmd_logs() {
    local service="${1:-scheduler}"
    local log_file

    case "$service" in
        scheduler)     log_file="$LOG_DIR/scheduler.log" ;;
        dashboard-api|api) log_file="$LOG_DIR/dashboard-api.log" ;;
        dashboard-ui|ui)   log_file="$LOG_DIR/dashboard-ui.log" ;;
        *)
            echo "Usage: $0 logs {scheduler|dashboard-api|dashboard-ui}"
            exit 1
            ;;
    esac

    if [[ -f "$log_file" ]]; then
        tail -f "$log_file"
    else
        echo "No log file at $log_file"
    fi
}

# =============================================================================
# Main
# =============================================================================

cmd="${1:-deploy}"
shift 2>/dev/null || true

case "$cmd" in
    deploy|up)
        cmd_migrate
        cmd_start
        cmd_status
        ;;
    start)
        cmd_start
        cmd_status
        ;;
    stop|down)
        cmd_stop
        ;;
    restart)
        cmd_stop
        cmd_start
        cmd_status
        ;;
    status|health)
        cmd_status
        ;;
    logs)
        cmd_logs "$@"
        ;;
    migrate)
        cmd_migrate
        ;;
    *)
        echo "OpenClaw Local Deploy"
        echo ""
        echo "Usage: $0 {command}"
        echo ""
        echo "Commands:"
        echo "  deploy    Full deploy: migrate + start + validate (default)"
        echo "  start     Start all services (skip migrations)"
        echo "  stop      Stop all managed services"
        echo "  restart   Stop + start"
        echo "  status    Health check all services"
        echo "  logs      Tail logs (scheduler|dashboard-api|dashboard-ui)"
        echo "  migrate   Run pending SQL migrations only"
        echo ""
        exit 1
        ;;
esac
