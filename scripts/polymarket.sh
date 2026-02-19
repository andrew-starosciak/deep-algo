#!/bin/bash
#
# Polymarket Trading System — eu-west-1
#
# Unified script for EC2 deployment and service management of the
# Polymarket 15-min crypto binary options trading system.
#
# Usage:
#   ./scripts/polymarket.sh <command>
#
# EC2 Commands:
#   deploy [--spot]          Provision EC2 + Postgres + TimescaleDB
#   redeploy                 Build + upload + migrate + restart
#   teardown                 Terminate + cleanup
#   ssh                      SSH into instance
#   start <svc> [opts]       Start service (with optional override args)
#   stop <svc|all>           Stop service(s)
#   restart <svc> [opts]     Restart
#   status                   All services + DB stats + system health
#   logs <svc>               Tail logs
#   db <query>               Run SQL on EC2
#   db-dump                  Download pg_dump
#   db-sync                  Import dump into local
#   latency [rounds]         Polymarket API latency test
#   preflight                Auth/balance/market checks
#   settle [opts]            Settle unsettled trades
#
# Local Commands:
#   local <svc> [opts]       Run service locally (foreground, cargo run)
#
# Services:
#   collector                Data collection (orderbook, funding, CLOB, settlements)
#   timing                   CLOB first-move timing strategy
#   cross-market             Cross-market correlation arbitrage
#   directional              Single-leg directional trading
#

set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# State files for this deployment
EC2_STATE_FILE="$SCRIPT_DIR/.polymarket.state"
EC2_KEY_FILE="$SCRIPT_DIR/.polymarket-key.pem"
export EC2_STATE_FILE EC2_KEY_FILE

# Migrate old state file names
if [[ ! -f "$EC2_STATE_FILE" && -f "$SCRIPT_DIR/.aws-latency-test.state" ]]; then
    mv "$SCRIPT_DIR/.aws-latency-test.state" "$EC2_STATE_FILE"
    echo "Migrated .aws-latency-test.state -> .polymarket.state"
fi
if [[ ! -f "$EC2_KEY_FILE" && -f "$SCRIPT_DIR/.aws-latency-key.pem" ]]; then
    mv "$SCRIPT_DIR/.aws-latency-key.pem" "$EC2_KEY_FILE"
    echo "Migrated .aws-latency-key.pem -> .polymarket-key.pem"
fi

# Auto-source .env
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

REGION="eu-west-1"
INSTANCE_TYPE="t3.micro"
KEY_NAME="algo-trade-latency-test"
SG_NAME="algo-trade-latency-sg"
INSTANCE_TAG="algo-trade-latency"
DUMPS_DIR="$PROJECT_ROOT/data/aws-dumps"

# =============================================================================
# Service registry
# =============================================================================

# Format: NAME|PID_FILE|LOG_FILE|PROCESS_PATTERN|CLI_CMD|DEFAULT_ARGS
declare -A SERVICES
SERVICES[collector]="collector|/tmp/collector.pid|/tmp/collector.log|algo-trade collect-signals|collect-signals|--duration 365d --coins btc,eth,sol,xrp --sources all --signals"
SERVICES[timing]="clob-timing|/tmp/clob-timing.pid|/tmp/clob-timing.log|algo-trade clob-timing|clob-timing|--mode live --duration 24h --coins btc,eth,sol,xrp --min-displacement 0.15 --bet-size 5 --exclude-hours 4,9,14 --max-position 20 --max-trades-per-window 4 --verbose --persist"
SERVICES[cross-market]="cross-market-auto|/tmp/cross_market_auto.pid|/tmp/cross_market_auto.log|algo-trade cross-market-auto|cross-market-auto|--mode paper --duration 1h --pair all --persist --verbose"
SERVICES[directional]="directional|/tmp/directional.pid|/tmp/directional.log|algo-trade directional-auto|directional-auto|--mode paper --duration 1h --coins btc,eth,sol,xrp --persist"

_svc_field() { echo "${SERVICES[$1]}" | cut -d'|' -f"$2"; }
_svc_name()    { _svc_field "$1" 1; }
_svc_pid()     { _svc_field "$1" 2; }
_svc_log()     { _svc_field "$1" 3; }
_svc_pattern() { _svc_field "$1" 4; }
_svc_cmd()     { _svc_field "$1" 5; }
_svc_args()    { _svc_field "$1" 6; }

_validate_service() {
    if [[ -z "${SERVICES[$1]:-}" ]]; then
        error "Unknown service: '$1'"
        echo ""
        echo "Available services:"
        for key in "${!SERVICES[@]}"; do
            echo "  $key  ($(_svc_name "$key"))"
        done
        exit 1
    fi
}

# =============================================================================
# EC2: deploy
# =============================================================================

cmd_deploy() {
    INSTANCE_MARKET="on-demand"
    for arg in "$@"; do
        case "$arg" in
            --spot) INSTANCE_MARKET="spot" ;;
        esac
    done

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Polymarket — EC2 Deploy (${INSTANCE_MARKET})${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    if ! command -v aws &>/dev/null; then
        error "AWS CLI not found."
        exit 1
    fi

    if ! aws sts get-caller-identity &>/dev/null; then
        error "AWS CLI not configured. Run: aws configure"
        exit 1
    fi

    if [[ ! -f "$PROJECT_ROOT/.env" ]]; then
        error ".env file not found"
        exit 1
    fi

    if [[ -f "$STATE_FILE" ]]; then
        warn "Existing deployment found. Run 'teardown' first."
        exit 1
    fi

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

    # Look up Ubuntu 24.04 AMI
    info "Looking up Ubuntu 24.04 AMI in $REGION..."
    local ami_id
    ami_id=$(aws ssm get-parameters \
        --names /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id \
        --region "$REGION" \
        --query 'Parameters[0].Value' \
        --output text 2>/dev/null || true)

    if [[ -z "$ami_id" || "$ami_id" == "None" ]]; then
        info "SSM lookup failed, searching AMIs directly..."
        ami_id=$(aws ec2 describe-images \
            --region "$REGION" \
            --owners 099720109477 \
            --filters "Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*" \
                      "Name=state,Values=available" \
            --query 'sort_by(Images, &CreationDate)[-1].ImageId' \
            --output text)
    fi

    if [[ -z "$ami_id" || "$ami_id" == "None" ]]; then
        error "Could not find Ubuntu 24.04 AMI in $REGION"
        exit 1
    fi
    dim "AMI: $ami_id"

    # Create key pair
    info "Creating EC2 key pair..."
    aws ec2 delete-key-pair --key-name "$KEY_NAME" --region "$REGION" 2>/dev/null || true
    rm -f "$KEY_FILE"
    aws ec2 create-key-pair \
        --key-name "$KEY_NAME" \
        --region "$REGION" \
        --query 'KeyMaterial' \
        --output text > "$KEY_FILE"
    chmod 600 "$KEY_FILE"

    # Create security group
    info "Creating security group..."
    local my_ip
    my_ip=$(curl -s --max-time 5 ifconfig.me || curl -s --max-time 5 api.ipify.org || echo "0.0.0.0")

    local vpc_id
    vpc_id=$(aws ec2 describe-vpcs --region "$REGION" --filters "Name=isDefault,Values=true" --query 'Vpcs[0].VpcId' --output text)

    SECURITY_GROUP_ID=$(aws ec2 describe-security-groups \
        --region "$REGION" \
        --filters "Name=group-name,Values=$SG_NAME" \
        --query 'SecurityGroups[0].GroupId' \
        --output text 2>/dev/null)

    if [[ -z "$SECURITY_GROUP_ID" || "$SECURITY_GROUP_ID" == "None" ]]; then
        SECURITY_GROUP_ID=$(aws ec2 create-security-group \
            --group-name "$SG_NAME" \
            --description "SSH access for Polymarket trading" \
            --vpc-id "$vpc_id" \
            --region "$REGION" \
            --query 'GroupId' \
            --output text)
    fi

    aws ec2 revoke-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp --port 22 --cidr "0.0.0.0/0" \
        --region "$REGION" 2>/dev/null || true
    aws ec2 authorize-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp --port 22 --cidr "${my_ip}/32" \
        --region "$REGION" >/dev/null 2>&1 || true

    dim "Security group: $SECURITY_GROUP_ID (SSH from $my_ip)"

    # Launch instance
    info "Launching $INSTANCE_TYPE $INSTANCE_MARKET instance in $REGION..."
    if [[ "$INSTANCE_MARKET" == "spot" ]]; then
        INSTANCE_ID=$(aws ec2 run-instances \
            --image-id "$ami_id" \
            --instance-type "$INSTANCE_TYPE" \
            --key-name "$KEY_NAME" \
            --security-group-ids "$SECURITY_GROUP_ID" \
            --region "$REGION" \
            --instance-market-options '{"MarketType":"spot","SpotOptions":{"SpotInstanceType":"persistent","InstanceInterruptionBehavior":"stop"}}' \
            --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":8,"VolumeType":"gp3"}}]' \
            --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$INSTANCE_TAG}]" \
            --query 'Instances[0].InstanceId' \
            --output text)
    else
        INSTANCE_ID=$(aws ec2 run-instances \
            --image-id "$ami_id" \
            --instance-type "$INSTANCE_TYPE" \
            --key-name "$KEY_NAME" \
            --security-group-ids "$SECURITY_GROUP_ID" \
            --region "$REGION" \
            --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":8,"VolumeType":"gp3"}}]' \
            --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$INSTANCE_TAG}]" \
            --query 'Instances[0].InstanceId' \
            --output text)
    fi

    dim "Instance: $INSTANCE_ID"

    info "Waiting for instance to start..."
    aws ec2 wait instance-running --instance-ids "$INSTANCE_ID" --region "$REGION"

    # Allocate Elastic IP
    info "Allocating Elastic IP..."
    ALLOCATION_ID=$(aws ec2 allocate-address --domain vpc --region "$REGION" --query 'AllocationId' --output text)
    PUBLIC_IP=$(aws ec2 describe-addresses --allocation-ids "$ALLOCATION_ID" --region "$REGION" --query 'Addresses[0].PublicIp' --output text)
    aws ec2 associate-address --instance-id "$INSTANCE_ID" --allocation-id "$ALLOCATION_ID" --region "$REGION" >/dev/null

    dim "Elastic IP: $PUBLIC_IP ($ALLOCATION_ID)"

    info "Waiting for status checks..."
    aws ec2 wait instance-status-ok --instance-ids "$INSTANCE_ID" --region "$REGION"

    save_state
    info "State saved to $STATE_FILE"

    wait_for_ssh

    info "Installing runtime dependencies..."
    remote_ssh "sudo apt-get update -qq && sudo apt-get install -y -qq libssl3t64 ca-certificates >/dev/null 2>&1"

    # Install PostgreSQL + TimescaleDB
    info "Installing PostgreSQL + TimescaleDB..."
    remote_ssh 'bash -s' <<'SETUP_PG'
set -euo pipefail
sudo apt-get install -y -qq postgresql postgresql-client >/dev/null 2>&1

CODENAME=$(lsb_release -cs)
echo "deb https://packagecloud.io/timescale/timescaledb/ubuntu/ ${CODENAME} main" | sudo tee /etc/apt/sources.list.d/timescaledb.list >/dev/null
curl -sL https://packagecloud.io/timescale/timescaledb/gpgkey | sudo gpg --dearmor -o /etc/apt/trusted.gpg.d/timescaledb.gpg
sudo apt-get update -qq >/dev/null 2>&1
sudo apt-get install -y -qq timescaledb-2-postgresql-16 >/dev/null 2>&1 || echo "TimescaleDB install returned non-zero (may still be OK)"

if sudo timescaledb-tune --quiet --yes 2>/dev/null; then
    echo "timescaledb-tune applied"
else
    echo "timescaledb-tune failed, setting shared_preload_libraries manually"
    sudo sed -i "s/^#*shared_preload_libraries.*/shared_preload_libraries = 'timescaledb'/" /etc/postgresql/16/main/postgresql.conf
    if ! grep -q "^shared_preload_libraries.*timescaledb" /etc/postgresql/16/main/postgresql.conf; then
        echo "shared_preload_libraries = 'timescaledb'" | sudo tee -a /etc/postgresql/16/main/postgresql.conf >/dev/null
    fi
fi

sudo tee /etc/postgresql/16/main/conf.d/low-memory.conf >/dev/null <<PGCONF
shared_buffers = 128MB
effective_cache_size = 384MB
work_mem = 4MB
maintenance_work_mem = 64MB
max_connections = 20
wal_buffers = 4MB
checkpoint_completion_target = 0.9
random_page_cost = 1.1
max_wal_size = 256MB
PGCONF

sudo systemctl restart postgresql
sudo systemctl enable postgresql

sudo -u postgres psql -c "DO \$\$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'algo') THEN
        CREATE ROLE algo WITH LOGIN PASSWORD 'algo_trade_local';
    END IF;
END
\$\$;"
sudo -u postgres createdb -O algo algo_trade 2>/dev/null || true
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE algo_trade TO algo;"
sudo -u postgres psql -d algo_trade -c "GRANT ALL ON SCHEMA public TO algo;"
sudo -u postgres psql -d algo_trade -c "CREATE EXTENSION IF NOT EXISTS timescaledb;"

sudo sed -i 's/local\s\+all\s\+all\s\+peer/local   all             all                                     md5/' /etc/postgresql/16/main/pg_hba.conf
if ! grep -q "host.*all.*all.*127.0.0.1/32.*md5" /etc/postgresql/16/main/pg_hba.conf; then
    echo "host    all    all    127.0.0.1/32    md5" | sudo tee -a /etc/postgresql/16/main/pg_hba.conf >/dev/null
fi
sudo systemctl reload postgresql
echo "PostgreSQL + TimescaleDB ready"
SETUP_PG

    # Migrations
    info "Deploying database migrations..."
    sync_and_migrate

    # Daily backup cron
    info "Setting up daily database backup..."
    remote_ssh 'bash -s' <<'SETUP_BACKUP'
mkdir -p ~/backups
cat > ~/backup-db.sh <<'BKSCRIPT'
#!/bin/bash
export PGPASSWORD="algo_trade_local"
DUMP_FILE=~/backups/algo_trade_$(date +%Y%m%d_%H%M%S).sql.gz
pg_dump -h localhost -U algo algo_trade | gzip > "$DUMP_FILE"
find ~/backups -name "algo_trade_*.sql.gz" -mtime +7 -delete
BKSCRIPT
chmod +x ~/backup-db.sh
(crontab -l 2>/dev/null | grep -v backup-db; echo "0 4 * * * ~/backup-db.sh") | crontab -
SETUP_BACKUP

    # Deploy binary + .env
    info "Deploying binary ($binary_size)..."
    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    info "Deploying .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Connectivity test
    info "Testing Polymarket API connectivity..."
    local latency
    latency=$(remote_ssh "curl -s -o /dev/null -w '%{time_total}' https://clob.polymarket.com/time" 2>/dev/null || echo "failed")
    dim "CLOB API round-trip: ${latency}s"

    local table_count
    table_count=$(remote_ssh "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade -tAc \"SELECT count(*) FROM information_schema.tables WHERE table_schema='public'\"" 2>/dev/null || echo "?")
    dim "Database: algo_trade ($table_count tables)"

    echo ""
    echo -e "${GREEN}Deployment complete!${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}  $INSTANCE_ID"
    echo -e "  ${DIM}Region:${NC}    $REGION"
    echo -e "  ${DIM}IP:${NC}        $PUBLIC_IP"
    echo -e "  ${DIM}Type:${NC}      $INSTANCE_TYPE ($INSTANCE_MARKET)"
    echo ""
    echo -e "  ${DIM}Start:${NC}     ./scripts/polymarket.sh start collector"
    echo -e "  ${DIM}Status:${NC}    ./scripts/polymarket.sh status"
    echo -e "  ${DIM}SSH:${NC}       ./scripts/polymarket.sh ssh"
    echo ""
}

# =============================================================================
# EC2: redeploy
# =============================================================================

cmd_redeploy() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Redeploy to EC2${NC}  (${DIM}${PUBLIC_IP}${NC})"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

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

    info "Uploading binary..."
    remote_ssh "rm -f ~/algo-trade"
    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    info "Syncing .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    sync_and_migrate

    echo ""
    info "Redeployed to $PUBLIC_IP"
    echo ""

    # Restart collector and timing (the persistent services)
    for svc in collector timing; do
        info "Starting $svc..."
        ec2_start "$(_svc_name "$svc")" "$(_svc_pid "$svc")" "$(_svc_log "$svc")" "$(_svc_cmd "$svc")" "$(_svc_args "$svc")"
    done

    echo ""
    info "Check with: ./scripts/polymarket.sh status"
    echo ""
}

# =============================================================================
# EC2: start / stop / restart
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

    if [[ "$svc" == "all" ]]; then
        load_state
        for key in "${!SERVICES[@]}"; do
            info "Stopping $key..."
            ec2_stop "$(_svc_name "$key")" "$(_svc_pid "$key")" "$(_svc_pattern "$key")"
        done
        return
    fi

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

# =============================================================================
# EC2: status
# =============================================================================

cmd_status() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Polymarket — Status${NC}  (${DIM}${PUBLIC_IP}${NC})"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    # Build status script for single SSH call
    local status_script='
export PGPASSWORD="algo_trade_local"
DB_ARGS="-h localhost -U algo -d algo_trade"
'

    for svc in collector timing cross-market directional; do
        [[ -z "${SERVICES[$svc]:-}" ]] && continue
        local pid_file log_file pattern
        pid_file=$(_svc_pid "$svc")
        log_file=$(_svc_log "$svc")
        pattern=$(_svc_pattern "$svc")

        status_script+="
echo \"SVC:${svc}\"
if [[ -f ${pid_file} ]]; then
    pid=\$(cat ${pid_file})
    if kill -0 \$pid 2>/dev/null; then
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
    orphan=\$(pgrep -f '${pattern}' 2>/dev/null | head -1)
    if [[ -n \"\$orphan\" ]]; then
        echo \"STATUS:running (orphan)\"
        echo \"PID:\$orphan\"
    else
        echo \"STATUS:stopped\"
    fi
fi
if [[ -f ${log_file} ]]; then
    log_mod=\$(stat -c %Y ${log_file} 2>/dev/null || echo 0)
    now=\$(date +%s)
    log_age=\$((now - log_mod))
    if [[ \$log_age -lt 60 ]]; then echo \"LOG_AGE:\${log_age}s ago\"
    elif [[ \$log_age -lt 3600 ]]; then echo \"LOG_AGE:\$((log_age / 60))m ago\"
    else echo \"LOG_AGE:\$((log_age / 3600))h ago\"; fi
    last_line=\$(tail -1 ${log_file} 2>/dev/null | cut -c1-80)
    echo \"LAST_LOG:\$last_line\"
    log_size=\$(du -h ${log_file} 2>/dev/null | cut -f1)
    echo \"LOG_SIZE:\$log_size\"
else
    echo \"LOG_AGE:no log\"
fi
"
    done

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

    for svc in collector timing cross-market directional; do
        local status="${svc_data["${svc}_status"]:-unknown}"
        local pid="${svc_data["${svc}_pid"]:-}"
        local uptime="${svc_data["${svc}_uptime"]:-}"
        local log_age="${svc_data["${svc}_log_age"]:-}"
        local last_log="${svc_data["${svc}_last_log"]:-}"
        local log_size="${svc_data["${svc}_log_size"]:-}"

        local status_display
        case "$status" in
            running*)  status_display="${GREEN}RUNNING${NC}" ;;
            dead*)     status_display="${RED}DEAD${NC}" ;;
            stopped*)  status_display="${DIM}STOPPED${NC}" ;;
            *)         status_display="${YELLOW}${status}${NC}" ;;
        esac

        printf "  ${WHITE}%-15s${NC}" "$svc"
        printf "  %b" "$status_display"

        if [[ "$status" == running* ]]; then
            [[ -n "$pid" ]] && printf "  ${DIM}PID ${pid}${NC}"
            [[ -n "$uptime" ]] && printf "  ${DIM}up ${uptime}${NC}"
        fi
        [[ -n "$log_age" && "$log_age" != "no log" ]] && printf "  ${DIM}log ${log_age}${NC}"
        [[ -n "$log_size" ]] && printf "  ${DIM}(${log_size})${NC}"
        echo ""

        if [[ -n "$last_log" ]]; then
            echo -e "                  ${DIM}${last_log}${NC}"
        fi
    done

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
# EC2: logs
# =============================================================================

cmd_logs() {
    local svc="$1"
    _validate_service "$svc"
    ec2_logs "$(_svc_log "$svc")"
}

# =============================================================================
# EC2: db / db-dump / db-sync
# =============================================================================

cmd_db() {
    load_state

    local query="$*"
    if [[ -z "$query" ]]; then
        # Interactive psql
        ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
            "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade"
    else
        remote_ssh "PGPASSWORD='algo_trade_local' psql -h localhost -U algo -d algo_trade -c \"$query\""
    fi
}

cmd_db_dump() {
    load_state

    mkdir -p "$DUMPS_DIR"

    local timestamp
    timestamp=$(date +%Y%m%d_%H%M%S)
    local dump_file="$DUMPS_DIR/algo_trade_${timestamp}.sql.gz"

    info "Creating database dump on $PUBLIC_IP..."
    remote_ssh "PGPASSWORD=algo_trade_local pg_dump -h localhost -U algo algo_trade | gzip > /tmp/algo_trade_dump.sql.gz"

    info "Downloading dump..."
    remote_scp "$SSH_USER@$PUBLIC_IP:/tmp/algo_trade_dump.sql.gz" "$dump_file"
    remote_ssh "rm -f /tmp/algo_trade_dump.sql.gz"

    local dump_size
    dump_size=$(du -h "$dump_file" | cut -f1)
    info "Dump saved: $dump_file ($dump_size)"

    info "Remote table row counts:"
    remote_ssh "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade -c \"
        SELECT relname AS table, n_live_tup AS rows
        FROM pg_stat_user_tables
        WHERE n_live_tup > 0
        ORDER BY n_live_tup DESC;
    \"" 2>/dev/null || true
}

cmd_db_sync() {
    load_state

    local latest_dump
    latest_dump=$(ls -t "$DUMPS_DIR"/algo_trade_*.sql.gz 2>/dev/null | head -1)

    if [[ -z "$latest_dump" ]]; then
        warn "No dumps found in $DUMPS_DIR. Run 'db-dump' first."
        exit 1
    fi

    info "Latest dump: $latest_dump"
    dim "Size: $(du -h "$latest_dump" | cut -f1)"

    local local_db_url="${DATABASE_URL:-${LOCAL_DATABASE_URL:-}}"
    if [[ -z "$local_db_url" ]]; then
        if [[ -f "$PROJECT_ROOT/.env" ]]; then
            local_db_url=$(grep '^DATABASE_URL=' "$PROJECT_ROOT/.env" | cut -d= -f2- || true)
        fi
    fi

    if [[ -z "$local_db_url" ]]; then
        error "No DATABASE_URL found."
        exit 1
    fi

    info "Importing into local database..."

    local db_user db_pass db_host db_port db_name
    db_user=$(echo "$local_db_url" | sed -n 's|.*://\([^:]*\):.*|\1|p')
    db_pass=$(echo "$local_db_url" | sed -n 's|.*://[^:]*:\([^@]*\)@.*|\1|p')
    db_host=$(echo "$local_db_url" | sed -n 's|.*@\([^:]*\):.*|\1|p')
    db_port=$(echo "$local_db_url" | sed -n 's|.*:\([0-9]*\)/.*|\1|p')
    db_name=$(echo "$local_db_url" | sed -n 's|.*/\([^?]*\).*|\1|p')

    info "Dropping and recreating '$db_name'..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d postgres \
        -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$db_name' AND pid <> pg_backend_pid();" \
        -c "DROP DATABASE IF EXISTS $db_name;" \
        -c "CREATE DATABASE $db_name;" \
        2>&1 | tail -3

    info "Restoring from dump..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "CREATE EXTENSION IF NOT EXISTS timescaledb;" 2>/dev/null
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "SELECT timescaledb_pre_restore();" 2>/dev/null
    gunzip -c "$latest_dump" | PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" 2>&1 | grep -cE 'ERROR' | xargs -I{} echo "  {} errors during restore"
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "SELECT timescaledb_post_restore();" 2>/dev/null

    info "Verifying..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "
        SELECT relname AS table, n_live_tup AS rows
        FROM pg_stat_user_tables
        WHERE n_live_tup > 0
        ORDER BY n_live_tup DESC;
    " 2>/dev/null || true

    info "Import complete."
}

# =============================================================================
# EC2: latency
# =============================================================================

cmd_latency() {
    load_state

    local rounds="${1:-5}"

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Polymarket Latency (${REGION})${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    local endpoints=(
        "CLOB Time|GET|https://clob.polymarket.com/time"
        "CLOB Prices|GET|https://clob.polymarket.com/prices"
        "CLOB Book|GET|https://clob.polymarket.com/book"
        "CLOB Order|POST|https://clob.polymarket.com/order"
        "Gamma API|GET|https://gamma-api.polymarket.com/markets?limit=1"
    )

    local remote_script='
rounds='"$rounds"'
fmt="connect:%{time_connect} tls:%{time_appconnect} firstbyte:%{time_starttransfer} total:%{time_total} http:%{http_code}\n"

run_test() {
    local name="$1" method="$2" url="$3"
    local totals=() connects=() first_bytes=()
    local curl_args=(-s -o /dev/null -w "$fmt" --max-time 10)
    if [[ "$method" == "POST" ]]; then
        curl_args+=(-X POST -H "Content-Type: application/json" -d "{}")
    fi

    for i in $(seq 1 $rounds); do
        result=$(curl "${curl_args[@]}" "$url" 2>/dev/null)
        total=$(echo "$result" | grep -oP "total:\K[0-9.]+")
        conn=$(echo "$result" | grep -oP "connect:\K[0-9.]+")
        fb=$(echo "$result" | grep -oP "firstbyte:\K[0-9.]+")
        http=$(echo "$result" | grep -oP "http:\K[0-9]+")
        totals+=("$total")
        connects+=("$conn")
        first_bytes+=("$fb")
    done

    avg_conn=$(printf "%s\n" "${connects[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    avg_fb=$(printf "%s\n" "${first_bytes[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    avg_total=$(printf "%s\n" "${totals[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    min_total=$(printf "%s\n" "${totals[@]}" | awk "BEGIN{m=999} {if(\$1<m)m=\$1} END {printf \"%.1f\", m*1000}")
    max_total=$(printf "%s\n" "${totals[@]}" | awk "BEGIN{m=0} {if(\$1>m)m=\$1} END {printf \"%.1f\", m*1000}")

    printf "  %-16s  tcp:%-6s  tls->fb:%-7s  total:%-6s  (min:%-5s max:%-5s)  [%s]\n" \
        "$name" "${avg_conn}ms" "${avg_fb}ms" "${avg_total}ms" "${min_total}ms" "${max_total}ms" "$http"
}
'

    for ep in "${endpoints[@]}"; do
        IFS='|' read -r name method url <<< "$ep"
        remote_script+="run_test \"$name\" \"$method\" \"$url\""$'\n'
    done

    remote_script+='
echo ""
echo "  CLOB latency breakdown:"
breakdown=$(curl -s -o /dev/null -w "dns:%{time_namelookup} tcp:%{time_connect} tls:%{time_appconnect} server:%{time_starttransfer} total:%{time_total}" --max-time 10 https://clob.polymarket.com/time)
dns=$(echo "$breakdown" | grep -oP "dns:\K[0-9.]+" | awk "{printf \"%.1f\", \$1*1000}")
tcp=$(echo "$breakdown" | grep -oP "tcp:\K[0-9.]+" | awk "{printf \"%.1f\", \$1*1000}")
tls=$(echo "$breakdown" | grep -oP "tls:\K[0-9.]+" | awk "{printf \"%.1f\", \$1*1000}")
server=$(echo "$breakdown" | grep -oP "server:\K[0-9.]+" | awk "{printf \"%.1f\", \$1*1000}")
total=$(echo "$breakdown" | grep -oP "total:\K[0-9.]+" | awk "{printf \"%.1f\", \$1*1000}")
tls_only=$(echo "$tls $tcp" | awk "{printf \"%.1f\", \$1-\$2}")
server_only=$(echo "$server $tls" | awk "{printf \"%.1f\", \$1-\$2}")
printf "    dns: %sms -> tcp: %sms -> tls: %sms (+%sms) -> server: %sms (+%sms) -> total: %sms\n" \
    "$dns" "$tcp" "$tls" "$tls_only" "$server" "$server_only" "$total"
echo ""
echo "  With connection reuse, tcp+tls is paid once."
echo "  Subsequent requests: ~${server_only}ms (server processing only)"
'

    info "Running $rounds rounds per endpoint from $PUBLIC_IP..."
    echo ""
    remote_ssh "$remote_script"
    echo ""
}

# =============================================================================
# EC2: preflight
# =============================================================================

cmd_preflight() {
    load_state

    info "Running preflight checks on $PUBLIC_IP..."
    echo ""
    ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
        "set -a && source ~/.env && set +a && ~/algo-trade preflight --coins btc,eth --verbose"
}

# =============================================================================
# EC2: settle
# =============================================================================

cmd_settle() {
    load_state

    local settle_args=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)       settle_args+=" --dry-run"; shift ;;
            --coin)          settle_args+=" --coin $2"; shift 2 ;;
            --session)       settle_args+=" --session $2"; shift 2 ;;
            --max-age-hours) settle_args+=" --max-age-hours $2"; shift 2 ;;
            --no-redeem)     settle_args+=" --no-redeem"; shift ;;
            --verbose|-v)    settle_args+=" --verbose"; shift ;;
            *)               error "Unknown settle option: $1"; exit 1 ;;
        esac
    done

    info "Running settlement on EC2..."
    remote_ssh "bash -c 'set -a && source ~/.env && set +a && RUST_LOG=info ~/algo-trade directional-settle${settle_args}'"
}

# =============================================================================
# EC2: teardown
# =============================================================================

cmd_teardown() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Polymarket — Teardown${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}  $INSTANCE_ID"
    echo -e "  ${DIM}Elastic IP:${NC} ${PUBLIC_IP:-unknown} (${ALLOCATION_ID:-unknown})"
    echo -e "  ${DIM}SG:${NC}        $SECURITY_GROUP_ID"
    echo ""

    read -rp "Type 'yes' to destroy all resources: " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "Aborted."
        exit 1
    fi
    echo ""

    # Cancel spot request if applicable
    if [[ "${INSTANCE_MARKET:-on-demand}" == "spot" ]]; then
        info "Cancelling spot instance request..."
        local spot_req
        spot_req=$(aws ec2 describe-spot-instance-requests \
            --region "$REGION" \
            --filters "Name=instance-id,Values=$INSTANCE_ID" \
            --query 'SpotInstanceRequests[0].SpotInstanceRequestId' \
            --output text 2>/dev/null || echo "None")
        if [[ -n "$spot_req" && "$spot_req" != "None" ]]; then
            aws ec2 cancel-spot-instance-requests \
                --spot-instance-request-ids "$spot_req" \
                --region "$REGION" >/dev/null 2>&1 || true
        fi
    else
        local spot_req
        spot_req=$(aws ec2 describe-spot-instance-requests \
            --region "$REGION" \
            --filters "Name=instance-id,Values=$INSTANCE_ID" \
            --query 'SpotInstanceRequests[0].SpotInstanceRequestId' \
            --output text 2>/dev/null || echo "None")
        if [[ -n "$spot_req" && "$spot_req" != "None" ]]; then
            info "Found lingering spot request, cancelling..."
            aws ec2 cancel-spot-instance-requests \
                --spot-instance-request-ids "$spot_req" \
                --region "$REGION" >/dev/null 2>&1 || true
        fi
    fi

    info "Terminating instance $INSTANCE_ID..."
    aws ec2 terminate-instances --instance-ids "$INSTANCE_ID" --region "$REGION" >/dev/null 2>&1 || true

    info "Waiting for termination..."
    aws ec2 wait instance-terminated --instance-ids "$INSTANCE_ID" --region "$REGION" 2>/dev/null || true

    if [[ -n "${ALLOCATION_ID:-}" ]]; then
        info "Releasing Elastic IP $ALLOCATION_ID..."
        aws ec2 release-address --allocation-id "$ALLOCATION_ID" --region "$REGION" 2>/dev/null || true
    fi

    info "Deleting security group..."
    local sg_attempts=0
    while [[ $sg_attempts -lt 10 ]]; do
        if aws ec2 delete-security-group --group-id "$SECURITY_GROUP_ID" --region "$REGION" 2>/dev/null; then
            break
        fi
        sg_attempts=$((sg_attempts + 1))
        sleep 5
    done

    info "Deleting key pair..."
    aws ec2 delete-key-pair --key-name "$KEY_NAME" --region "$REGION" 2>/dev/null || true

    rm -f "$KEY_FILE" "$STATE_FILE"

    echo ""
    info "All resources cleaned up."
}

# =============================================================================
# Local: run service in foreground
# =============================================================================

cmd_local() {
    local svc="${1:-}"
    shift 2>/dev/null || true

    if [[ -z "$svc" ]]; then
        error "Usage: polymarket.sh local <service> [opts]"
        echo "Services: collector, timing, cross-market, directional"
        exit 1
    fi

    _validate_service "$svc"

    local cli_cmd
    cli_cmd=$(_svc_cmd "$svc")

    local args
    if [[ $# -gt 0 ]]; then
        args="$*"
    else
        args=$(_svc_args "$svc")
    fi

    if [[ -z "${DATABASE_URL:-}" ]]; then
        error "DATABASE_URL required"
        exit 1
    fi

    info "Running $svc locally (foreground)..."
    dim "algo-trade $cli_cmd $args"
    echo ""

    export RUST_LOG="${RUST_LOG:-info}"
    # shellcheck disable=SC2086
    cargo run -p algo-trade-cli --release -- $cli_cmd $args
}

# =============================================================================
# Main dispatcher
# =============================================================================

subcmd="${1:-}"
shift 2>/dev/null || true

case "$subcmd" in
    deploy)           cmd_deploy "$@" ;;
    redeploy)         cmd_redeploy ;;
    teardown)         cmd_teardown ;;
    ssh)              load_state; ec2_ssh_interactive ;;
    start)            cmd_start "${1:?Usage: polymarket.sh start <service>}" "${@:2}" ;;
    stop)             cmd_stop "${1:?Usage: polymarket.sh stop <service|all>}" ;;
    restart)          cmd_restart "${1:?Usage: polymarket.sh restart <service>}" "${@:2}" ;;
    status)           cmd_status ;;
    logs)             cmd_logs "${1:?Usage: polymarket.sh logs <service>}" ;;
    db)               cmd_db "$@" ;;
    db-dump)          cmd_db_dump ;;
    db-sync)          cmd_db_sync ;;
    latency)          cmd_latency "$@" ;;
    preflight)        cmd_preflight ;;
    settle)           cmd_settle "$@" ;;
    local)            cmd_local "$@" ;;
    help|--help|-h|"")
        echo "Polymarket Trading System"
        echo ""
        echo "Usage: ./scripts/polymarket.sh <command>"
        echo ""
        echo "EC2 Commands:"
        echo "  deploy [--spot]          Provision EC2 + Postgres + TimescaleDB"
        echo "  redeploy                 Build + upload binary + migrate + restart"
        echo "  teardown                 Terminate + cleanup"
        echo "  ssh                      SSH into instance"
        echo "  start <svc> [opts]       Start service (with optional override args)"
        echo "  stop <svc|all>           Stop service(s)"
        echo "  restart <svc> [opts]     Restart"
        echo "  status                   All services + DB stats + system health"
        echo "  logs <svc>               Tail logs"
        echo "  db [query]               Run SQL (or interactive psql if no query)"
        echo "  db-dump                  Download pg_dump"
        echo "  db-sync                  Import dump into local"
        echo "  latency [rounds]         Polymarket API latency test"
        echo "  preflight                Auth/balance/market checks"
        echo "  settle [opts]            Settle unsettled trades"
        echo ""
        echo "Local Commands:"
        echo "  local <svc> [opts]       Run service locally (foreground, cargo run)"
        echo ""
        echo "Services:"
        echo "  collector                Data collection (orderbook, funding, CLOB, settlements)"
        echo "  timing                   CLOB first-move timing strategy"
        echo "  cross-market             Cross-market correlation arbitrage"
        echo "  directional              Single-leg directional trading"
        echo ""
        ;;
    *)
        error "Unknown command: '$subcmd'"
        echo "Run: ./scripts/polymarket.sh help"
        exit 1
        ;;
esac
