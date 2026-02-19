#!/bin/bash
#
# EC2 Common Library — shared SSH, deploy, and management functions
#
# Source this from bot runner scripts to get EC2 subcommand support.
# Requires SCRIPT_DIR and PROJECT_ROOT to be set before sourcing.
#
# Provides:
#   - Colors, logging helpers (info, warn, error, dim)
#   - SSH/SCP helpers (remote_ssh, remote_scp)
#   - load_state() — load EC2 deployment state
#   - sync_and_migrate() — upload and apply SQL migrations
#   - ec2_redeploy() — build + upload binary + .env + migrate
#   - ec2_start() — start bot on EC2 in background
#   - ec2_stop() — stop bot on EC2
#   - ec2_logs() — tail remote logs
#   - ec2_ssh_interactive() — interactive SSH session
#   - ec2_dispatch() — route subcommands (redeploy/start/stop/logs/ssh)
#

# Source guard — prevent double-sourcing
[[ -n "${_EC2_COMMON_SOURCED:-}" ]] && return
_EC2_COMMON_SOURCED=1

# =============================================================================
# Colors & logging
# =============================================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[x]${NC} $*" >&2; }
dim()   { echo -e "${DIM}    $*${NC}"; }

# =============================================================================
# SSH / state constants
# =============================================================================

STATE_FILE="${SCRIPT_DIR:?SCRIPT_DIR must be set before sourcing ec2-common.sh}/.aws-latency-test.state"
KEY_FILE="$SCRIPT_DIR/.aws-latency-key.pem"

SSH_USER="ubuntu"
SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ServerAliveInterval=15 -o ServerAliveCountMax=3"

REMOTE_DB_USER="algo"
REMOTE_DB_PASS="algo_trade_local"
REMOTE_DB_NAME="algo_trade"
REMOTE_DATABASE_URL="postgres://${REMOTE_DB_USER}:${REMOTE_DB_PASS}@localhost/${REMOTE_DB_NAME}"

# =============================================================================
# Core helpers
# =============================================================================

load_state() {
    if [[ ! -f "$STATE_FILE" ]]; then
        error "No EC2 deployment found. Run './scripts/aws-latency-test.sh deploy' first."
        exit 1
    fi
    # shellcheck disable=SC1090
    source "$STATE_FILE"
}

remote_ssh() {
    ssh $SSH_OPTS -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" "$@"
}

remote_scp() {
    scp $SSH_OPTS -i "$KEY_FILE" "$@"
}

sync_and_migrate() {
    info "Syncing migrations..."
    remote_ssh "mkdir -p ~/migrations"
    remote_scp "${PROJECT_ROOT:?PROJECT_ROOT must be set}/scripts/migrations/"*.sql "$SSH_USER@$PUBLIC_IP:~/migrations/"

    info "Running new migrations..."
    remote_ssh "bash -s" <<'RUN_MIGRATIONS'
set -euo pipefail
export PGPASSWORD="algo_trade_local"
DB_ARGS="-h localhost -U algo -d algo_trade"

# Create tracking table if it doesn't exist
psql $DB_ARGS -c "
CREATE TABLE IF NOT EXISTS schema_migrations (
    filename TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);" 2>/dev/null

applied=0
skipped=0
for f in $(ls ~/migrations/*.sql | sort); do
    fname=$(basename "$f")
    already=$(psql $DB_ARGS -tAc "SELECT 1 FROM schema_migrations WHERE filename = '$fname'" 2>/dev/null)
    if [[ "$already" == "1" ]]; then
        skipped=$((skipped + 1))
        continue
    fi
    echo "  Applying $fname..."
    if psql $DB_ARGS -f "$f" 2>&1 | tail -3; then
        psql $DB_ARGS -c "INSERT INTO schema_migrations (filename) VALUES ('$fname');" 2>/dev/null
        applied=$((applied + 1))
    else
        echo "  ERROR applying $fname — stopping"
        exit 1
    fi
done
echo "Migrations: $applied applied, $skipped already up-to-date"
RUN_MIGRATIONS
}

# =============================================================================
# Generic EC2 bot operations
# =============================================================================

# ec2_redeploy BOT_NAME PID_FILE PROCESS_PATTERN
#   Build release binary, stop remote process, upload, sync .env, migrate.
ec2_redeploy() {
    local bot_name="$1"
    local pid_file="$2"
    local process_pattern="$3"

    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Redeploy ${bot_name} to EC2 (${PUBLIC_IP})${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    # Build release binary
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

    # Stop running process on EC2
    info "Stopping running ${bot_name}..."
    remote_ssh "
        if [[ -f ${pid_file} ]]; then
            pid=\$(cat ${pid_file})
            if kill \$pid 2>/dev/null; then
                echo '  Stopped ${bot_name} (PID '\$pid')'
                sleep 1
            fi
            rm -f ${pid_file}
        fi
        pkill -f '${process_pattern}' 2>/dev/null || true
        sleep 1
    " || true

    # Upload binary (remove first — can't overwrite a running binary)
    info "Uploading binary to $PUBLIC_IP..."
    remote_ssh "rm -f ~/algo-trade"
    if ! remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"; then
        error "Failed to upload binary"
        exit 1
    fi
    remote_ssh "chmod +x ~/algo-trade"

    # Sync .env (preserve remote DATABASE_URL)
    info "Syncing .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Sync and apply migrations
    sync_and_migrate

    echo ""
    info "Redeployed ${bot_name} to $PUBLIC_IP"
    echo ""
}

# ec2_start BOT_NAME PID_FILE LOG_FILE CLI_CMD CMD_ARGS
#   Start bot on EC2 in background via nohup.
ec2_start() {
    local bot_name="$1"
    local pid_file="$2"
    local log_file="$3"
    local cli_cmd="$4"
    local cmd_args="$5"

    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Start ${bot_name} on EC2 (${PUBLIC_IP})${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""
    echo -e "  ${DIM}Command:${NC}  algo-trade ${cli_cmd} ${cmd_args}"
    echo ""

    # Check if already running
    local already_running
    already_running=$(remote_ssh "
        if [[ -f ${pid_file} ]] && kill -0 \$(cat ${pid_file}) 2>/dev/null; then
            echo yes
        else
            echo no
        fi
    ")
    if [[ "$already_running" == *"yes" ]]; then
        warn "${bot_name} is already running. Use 'stop' first."
        exit 1
    fi

    # Start in background with nohup
    info "Starting ${bot_name}..."
    remote_ssh "bash -c '
        set -a && source ~/.env && set +a
        [[ -f ${log_file} ]] && mv ${log_file} ${log_file}.prev
        nohup env RUST_LOG=info ~/algo-trade ${cli_cmd} ${cmd_args} \
            > ${log_file} 2>&1 &
        echo \$! > ${pid_file}
        disown
        sleep 2
        if kill -0 \$(cat ${pid_file}) 2>/dev/null; then
            echo \"${bot_name} started (PID \$(cat ${pid_file}))\"
        else
            echo \"ERROR: ${bot_name} failed to start\"
            tail -10 ${log_file} 2>/dev/null
            exit 1
        fi
    '"

    echo ""
    info "${bot_name} running on $PUBLIC_IP"
    echo ""
    echo -e "  ${DIM}Tail logs:${NC}   $(basename "$0") logs"
    echo -e "  ${DIM}Stop:${NC}        $(basename "$0") stop"
    echo ""
}

# ec2_stop BOT_NAME PID_FILE PROCESS_PATTERN
#   Stop bot on EC2.
ec2_stop() {
    local bot_name="$1"
    local pid_file="$2"
    local process_pattern="$3"

    load_state

    info "Stopping ${bot_name} on $PUBLIC_IP..."

    remote_ssh "
        stopped=0
        if [[ -f ${pid_file} ]]; then
            pid=\$(cat ${pid_file})
            if kill \$pid 2>/dev/null; then
                echo '  Stopped ${bot_name} (PID '\$pid')'
                stopped=\$((stopped + 1))
            fi
            rm -f ${pid_file}
        fi
        for pid in \$(pgrep -f '${process_pattern}' 2>/dev/null); do
            kill \$pid 2>/dev/null && echo \"  Stopped PID \$pid\" && stopped=\$((stopped + 1))
        done
        if [[ \$stopped -eq 0 ]]; then
            echo '  No running ${bot_name} found'
        fi
    "
}

# ec2_logs LOG_FILE
#   Tail remote log file.
ec2_logs() {
    local log_file="$1"

    load_state

    info "Tailing logs on $PUBLIC_IP (Ctrl+C to stop)..."
    echo ""
    remote_ssh "tail -f ${log_file} 2>/dev/null || echo 'No log file found at ${log_file}.'"
}

# ec2_ssh_interactive
#   Open interactive SSH session.
ec2_ssh_interactive() {
    load_state

    info "Connecting to $PUBLIC_IP..."
    ssh $SSH_OPTS -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP"
}

# =============================================================================
# Subcommand dispatcher
# =============================================================================

# ec2_dispatch BOT_NAME PID_FILE LOG_FILE PROCESS_PATTERN CLI_CMD BUILD_ARGS_FN "$@"
#   Routes redeploy/start/stop/logs/ssh subcommands.
#   Returns 0 if a subcommand was handled, 1 if not (fall through to local run).
#   BUILD_ARGS_FN is the name of a function that takes start subcommand args
#   and echoes the CLI args string for the remote binary.
ec2_dispatch() {
    local bot_name="$1"
    local pid_file="$2"
    local log_file="$3"
    local process_pattern="$4"
    local cli_cmd="$5"
    local build_args_fn="$6"
    shift 6

    local subcmd="${1:-}"

    case "$subcmd" in
        redeploy)
            ec2_redeploy "$bot_name" "$pid_file" "$process_pattern"
            return 0
            ;;
        start)
            shift
            local cmd_args
            cmd_args=$("$build_args_fn" "$@")
            ec2_start "$bot_name" "$pid_file" "$log_file" "$cli_cmd" "$cmd_args"
            return 0
            ;;
        stop)
            ec2_stop "$bot_name" "$pid_file" "$process_pattern"
            return 0
            ;;
        logs)
            ec2_logs "$log_file"
            return 0
            ;;
        ssh)
            ec2_ssh_interactive
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}
