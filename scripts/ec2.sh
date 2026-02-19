#!/bin/bash
#
# EC2 Unified Service Manager
#
# Single entry point for managing all services on the EC2 trading instance.
#
# Usage:
#   ./scripts/ec2.sh status                    # Show all services
#   ./scripts/ec2.sh logs <service>            # Tail logs (collector, timing, etc.)
#   ./scripts/ec2.sh start <service> [opts]    # Start a service
#   ./scripts/ec2.sh stop <service>            # Stop a service
#   ./scripts/ec2.sh restart <service> [opts]  # Restart a service
#   ./scripts/ec2.sh stop-all                  # Stop all services
#   ./scripts/ec2.sh redeploy                  # Build + upload binary + migrate
#   ./scripts/ec2.sh ssh                       # SSH into EC2
#   ./scripts/ec2.sh db <query>                # Run a psql query on EC2
#
# Services:
#   collector    Data collector (orderbook, funding, CLOB prices, settlements, ...)
#   timing       CLOB first-move timing strategy bot
#
# Examples:
#   ./scripts/ec2.sh status
#   ./scripts/ec2.sh logs timing
#   ./scripts/ec2.sh start collector --duration 365d
#   ./scripts/ec2.sh restart timing
#   ./scripts/ec2.sh redeploy
#   ./scripts/ec2.sh db "SELECT count(*) FROM clob_price_snapshots WHERE timestamp > now() - interval '15 min'"
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Auto-source .env
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

# =============================================================================
# Service registry
# =============================================================================

# Each service: NAME|PID_FILE|LOG_FILE|PROCESS_PATTERN|CLI_CMD|DEFAULT_ARGS
declare -A SERVICES
SERVICES[collector]="collector|/tmp/collector.pid|/tmp/collector.log|algo-trade collect-signals|collect-signals|--duration 365d --coins btc,eth,sol,xrp --sources all --signals"
SERVICES[timing]="clob-timing|/tmp/clob-timing.pid|/tmp/clob-timing.log|algo-trade clob-timing|clob-timing|--mode live --duration 24h --coins btc,eth,sol,xrp --min-displacement 0.15 --bet-size 5 --exclude-hours 4,9,14 --max-position 20 --max-trades-per-window 4 --verbose --persist"

_svc_field() {
    local svc="$1" idx="$2"
    echo "${SERVICES[$svc]}" | cut -d'|' -f"$idx"
}

_svc_name()    { _svc_field "$1" 1; }
_svc_pid()     { _svc_field "$1" 2; }
_svc_log()     { _svc_field "$1" 3; }
_svc_pattern() { _svc_field "$1" 4; }
_svc_cmd()     { _svc_field "$1" 5; }
_svc_args()    { _svc_field "$1" 6; }

_validate_service() {
    local svc="$1"
    if [[ -z "${SERVICES[$svc]:-}" ]]; then
        error "Unknown service: '$svc'"
        echo ""
        echo "Available services:"
        for key in "${!SERVICES[@]}"; do
            echo "  $key  ($(_svc_name "$key"))"
        done
        exit 1
    fi
}

# =============================================================================
# status — show all services at a glance
# =============================================================================

cmd_status() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}EC2 Service Status${NC}  (${DIM}${PUBLIC_IP}${NC})"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    # Collect all status info in a single SSH call
    local status_script='
export PGPASSWORD="algo_trade_local"
DB_ARGS="-h localhost -U algo -d algo_trade"
'

    for svc in collector timing; do
        local pid_file
        pid_file=$(_svc_pid "$svc")
        local log_file
        log_file=$(_svc_log "$svc")
        local pattern
        pattern=$(_svc_pattern "$svc")

        status_script+="
echo \"SVC:${svc}\"

# Check PID
if [[ -f ${pid_file} ]]; then
    pid=\$(cat ${pid_file})
    if kill -0 \$pid 2>/dev/null; then
        # Get uptime from /proc
        if [[ -d /proc/\$pid ]]; then
            start_time=\$(stat -c %Y /proc/\$pid 2>/dev/null || echo 0)
            now=\$(date +%s)
            uptime_secs=\$((now - start_time))
            days=\$((uptime_secs / 86400))
            hours=\$(( (uptime_secs % 86400) / 3600 ))
            mins=\$(( (uptime_secs % 3600) / 60 ))
            if [[ \$days -gt 0 ]]; then
                uptime_str=\"\${days}d\${hours}h\${mins}m\"
            elif [[ \$hours -gt 0 ]]; then
                uptime_str=\"\${hours}h\${mins}m\"
            else
                uptime_str=\"\${mins}m\"
            fi
        else
            uptime_str=\"?\"
        fi
        echo \"STATUS:running\"
        echo \"PID:\$pid\"
        echo \"UPTIME:\$uptime_str\"
    else
        echo \"STATUS:dead\"
        echo \"PID:\$pid\"
    fi
else
    # Check for orphan process
    orphan=\$(pgrep -f '${pattern}' 2>/dev/null | head -1)
    if [[ -n \"\$orphan\" ]]; then
        echo \"STATUS:running (orphan)\"
        echo \"PID:\$orphan\"
    else
        echo \"STATUS:stopped\"
    fi
fi

# Last log activity
if [[ -f ${log_file} ]]; then
    log_mod=\$(stat -c %Y ${log_file} 2>/dev/null || echo 0)
    now=\$(date +%s)
    log_age=\$((now - log_mod))
    if [[ \$log_age -lt 60 ]]; then
        echo \"LOG_AGE:\${log_age}s ago\"
    elif [[ \$log_age -lt 3600 ]]; then
        echo \"LOG_AGE:\$((log_age / 60))m ago\"
    else
        echo \"LOG_AGE:\$((log_age / 3600))h ago\"
    fi
    last_line=\$(tail -1 ${log_file} 2>/dev/null | cut -c1-80)
    echo \"LAST_LOG:\$last_line\"
    log_size=\$(du -h ${log_file} 2>/dev/null | cut -f1)
    echo \"LOG_SIZE:\$log_size\"
else
    echo \"LOG_AGE:no log\"
fi
"
    done

    # Add DB checks
    status_script+='
echo "SVC:_db"
clob_count=$(psql $DB_ARGS -tAc "SELECT count(*) FROM clob_price_snapshots WHERE timestamp > now() - interval '\''15 min'\''" 2>/dev/null || echo "?")
echo "CLOB_15M:$clob_count"
settle_count=$(psql $DB_ARGS -tAc "SELECT count(*) FROM window_settlements WHERE settled_at > now() - interval '\''1 hour'\''" 2>/dev/null || echo "?")
echo "SETTLE_1H:$settle_count"
timing_count=$(psql $DB_ARGS -tAc "SELECT count(*) FROM directional_trades WHERE created_at > now() - interval '\''1 hour'\''" 2>/dev/null || echo "?")
echo "TIMING_1H:$timing_count"
disk_free=$(df -h / 2>/dev/null | tail -1 | awk '\''{print $4}'\'' || echo "?")
echo "DISK_FREE:$disk_free"
mem_free=$(free -h 2>/dev/null | awk '\''/^Mem:/{print $7}'\'' || echo "?")
echo "MEM_AVAIL:$mem_free"
'

    local raw_output
    raw_output=$(remote_ssh "bash -s" <<< "$status_script" 2>/dev/null)

    # Parse and display
    local current_svc=""
    declare -A svc_data

    while IFS= read -r line; do
        case "$line" in
            SVC:*)       current_svc="${line#SVC:}" ;;
            STATUS:*)    svc_data["${current_svc}_status"]="${line#STATUS:}" ;;
            PID:*)       svc_data["${current_svc}_pid"]="${line#PID:}" ;;
            UPTIME:*)    svc_data["${current_svc}_uptime"]="${line#UPTIME:}" ;;
            LOG_AGE:*)   svc_data["${current_svc}_log_age"]="${line#LOG_AGE:}" ;;
            LAST_LOG:*)  svc_data["${current_svc}_last_log"]="${line#LAST_LOG:}" ;;
            LOG_SIZE:*)  svc_data["${current_svc}_log_size"]="${line#LOG_SIZE:}" ;;
            CLOB_15M:*)  svc_data["_db_clob_15m"]="${line#CLOB_15M:}" ;;
            SETTLE_1H:*) svc_data["_db_settle_1h"]="${line#SETTLE_1H:}" ;;
            TIMING_1H:*) svc_data["_db_timing_1h"]="${line#TIMING_1H:}" ;;
            DISK_FREE:*) svc_data["_db_disk"]="${line#DISK_FREE:}" ;;
            MEM_AVAIL:*) svc_data["_db_mem"]="${line#MEM_AVAIL:}" ;;
        esac
    done <<< "$raw_output"

    # Render services
    for svc in collector timing; do
        local status="${svc_data["${svc}_status"]:-unknown}"
        local pid="${svc_data["${svc}_pid"]:-}"
        local uptime="${svc_data["${svc}_uptime"]:-}"
        local log_age="${svc_data["${svc}_log_age"]:-}"
        local last_log="${svc_data["${svc}_last_log"]:-}"
        local log_size="${svc_data["${svc}_log_size"]:-}"

        # Status color
        local status_display
        case "$status" in
            running*)  status_display="${GREEN}RUNNING${NC}" ;;
            dead*)     status_display="${RED}DEAD${NC}" ;;
            stopped*)  status_display="${DIM}STOPPED${NC}" ;;
            *)         status_display="${YELLOW}${status}${NC}" ;;
        esac

        # Service label
        local label
        case "$svc" in
            collector) label="collector  " ;;
            timing)    label="timing     " ;;
        esac

        printf "  ${WHITE}%-12s${NC}" "$svc"
        printf "  %b" "$status_display"

        if [[ "$status" == running* ]]; then
            [[ -n "$pid" ]] && printf "  ${DIM}PID ${pid}${NC}"
            [[ -n "$uptime" ]] && printf "  ${DIM}up ${uptime}${NC}"
        fi
        [[ -n "$log_age" && "$log_age" != "no log" ]] && printf "  ${DIM}log ${log_age}${NC}"
        [[ -n "$log_size" ]] && printf "  ${DIM}(${log_size})${NC}"
        echo ""

        if [[ -n "$last_log" ]]; then
            echo -e "              ${DIM}${last_log}${NC}"
        fi
    done

    # DB & system stats
    echo ""
    echo -e "  ${WHITE}Database:${NC}"
    echo -e "    ${DIM}CLOB snapshots (15m):${NC}  ${svc_data["_db_clob_15m"]:-?}"
    echo -e "    ${DIM}Settlements (1h):${NC}      ${svc_data["_db_settle_1h"]:-?}"
    echo -e "    ${DIM}Timing trades (1h):${NC}    ${svc_data["_db_timing_1h"]:-?}"

    echo ""
    echo -e "  ${WHITE}System:${NC}"
    echo -e "    ${DIM}Disk free:${NC}  ${svc_data["_db_disk"]:-?}"
    echo -e "    ${DIM}Memory:${NC}     ${svc_data["_db_mem"]:-?}"

    echo ""
}

# =============================================================================
# start / stop / restart / logs
# =============================================================================

cmd_start() {
    local svc="$1"; shift
    _validate_service "$svc"

    local args
    if [[ $# -gt 0 ]]; then
        args="$*"
    else
        args=$(_svc_args "$svc")
    fi

    ec2_start "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_log "$svc")" "$(_svc_cmd "$svc")" "$args"
}

cmd_stop() {
    local svc="$1"
    _validate_service "$svc"
    load_state
    ec2_stop "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_pattern "$svc")"
}

cmd_restart() {
    local svc="$1"; shift
    _validate_service "$svc"
    load_state

    info "Restarting $svc..."
    ec2_stop "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_pattern "$svc")"
    sleep 2

    local args
    if [[ $# -gt 0 ]]; then
        args="$*"
    else
        args=$(_svc_args "$svc")
    fi

    ec2_start "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_log "$svc")" "$(_svc_cmd "$svc")" "$args"
}

cmd_stop_all() {
    load_state

    for svc in "${!SERVICES[@]}"; do
        info "Stopping $svc..."
        ec2_stop "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_pattern "$svc")"
    done
}

cmd_logs() {
    local svc="$1"
    _validate_service "$svc"
    ec2_logs "$(_svc_log "$svc")"
}

cmd_redeploy() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Redeploy to EC2${NC}  (${DIM}${PUBLIC_IP}${NC})"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    # Build
    info "Building release binary..."
    (
        cd "$PROJECT_ROOT"
        if ! cargo build -p algo-trade-cli --release 2>&1 | tail -5; then
            error "Build failed"
            exit 1
        fi
    )

    local binary="$PROJECT_ROOT/target/release/algo-trade"
    if [[ ! -f "$binary" ]]; then
        error "Binary not found at $binary"
        exit 1
    fi
    local binary_size
    binary_size=$(du -h "$binary" | cut -f1)
    info "Binary built ($binary_size)"

    # Stop all services
    info "Stopping all services..."
    for svc in "${!SERVICES[@]}"; do
        remote_ssh "
            pid_file=$(_svc_pid "$svc")
            if [[ -f \$pid_file ]]; then
                pid=\$(cat \$pid_file)
                kill \$pid 2>/dev/null && echo '  Stopped $svc (PID '\$pid')'
                rm -f \$pid_file
            fi
            pkill -f '$(_svc_pattern "$svc")' 2>/dev/null || true
        " || true
    done
    sleep 1

    # Upload binary
    info "Uploading binary..."
    remote_ssh "rm -f ~/algo-trade"
    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    # Sync .env
    info "Syncing .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Migrations
    sync_and_migrate

    echo ""
    info "Redeployed to $PUBLIC_IP"
    echo ""

    # Restart all services with default args
    for svc in collector timing; do
        info "Starting $svc..."
        ec2_start "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_log "$svc")" "$(_svc_cmd "$svc")" "$(_svc_args "$svc")"
    done

    echo ""
    info "All services restarted. Check with: ./scripts/ec2.sh status"
    echo ""
}

cmd_db() {
    load_state

    local query="$*"
    if [[ -z "$query" ]]; then
        error "Usage: ec2.sh db <sql-query>"
        exit 1
    fi

    remote_ssh "PGPASSWORD='algo_trade_local' psql -h localhost -U algo -d algo_trade -c \"$query\""
}

cmd_ssh() {
    ec2_ssh_interactive
}

# =============================================================================
# Help
# =============================================================================

cmd_help() {
    echo -e "${WHITE}EC2 Service Manager${NC}"
    echo ""
    echo "Usage: ./scripts/ec2.sh <command> [service] [options]"
    echo ""
    echo -e "${WHITE}Commands:${NC}"
    echo "  status                    Show all services and system health"
    echo "  logs <service>            Tail service logs"
    echo "  start <service> [opts]    Start a service (with optional custom args)"
    echo "  stop <service>            Stop a service"
    echo "  restart <service> [opts]  Restart a service"
    echo "  stop-all                  Stop all services"
    echo "  redeploy                  Build + upload binary + migrate (stops all)"
    echo "  ssh                       SSH into EC2 instance"
    echo "  db <query>                Run a SQL query on EC2"
    echo ""
    echo -e "${WHITE}Services:${NC}"
    echo "  collector    Data collection (orderbook, funding, CLOB prices, settlements, ...)"
    echo "  timing       CLOB first-move timing strategy bot"
    echo ""
    echo -e "${WHITE}Examples:${NC}"
    echo "  ./scripts/ec2.sh status"
    echo "  ./scripts/ec2.sh logs timing"
    echo "  ./scripts/ec2.sh start collector --duration 365d"
    echo "  ./scripts/ec2.sh restart timing"
    echo "  ./scripts/ec2.sh redeploy"
    echo "  ./scripts/ec2.sh db \"SELECT count(*) FROM directional_trades\""
    echo ""
}

# =============================================================================
# Dispatch
# =============================================================================

subcmd="${1:-}"
shift || true

case "$subcmd" in
    status)    cmd_status ;;
    logs)      cmd_logs "${1:?Usage: ec2.sh logs <service>}" ;;
    start)     cmd_start "${1:?Usage: ec2.sh start <service>}" "${@:2}" ;;
    stop)      cmd_stop "${1:?Usage: ec2.sh stop <service>}" ;;
    restart)   cmd_restart "${1:?Usage: ec2.sh restart <service>}" "${@:2}" ;;
    stop-all)  cmd_stop_all ;;
    redeploy)  cmd_redeploy ;;
    ssh)       cmd_ssh ;;
    db)        cmd_db "$@" ;;
    help|--help|-h|"")  cmd_help ;;
    *)
        error "Unknown command: '$subcmd'"
        echo ""
        cmd_help
        exit 1
        ;;
esac
