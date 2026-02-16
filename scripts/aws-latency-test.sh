#!/bin/bash
#
# AWS Deploy - Deploy algo-trade bot to eu-west-2 (London) for Polymarket trading
#
# Usage:
#   ./scripts/aws-latency-test.sh deploy              # Build, provision on-demand EC2, deploy binary
#   ./scripts/aws-latency-test.sh deploy --spot       # Same but with spot instance (cheaper, can be reclaimed)
#   ./scripts/aws-latency-test.sh run [bot-args...]    # Run bot on remote instance
#   ./scripts/aws-latency-test.sh live [duration]      # Live trade + observer (all pairs)
#   ./scripts/aws-latency-test.sh stop                 # Kill all running bot processes
#   ./scripts/aws-latency-test.sh preflight            # Check auth, balance, markets
#   ./scripts/aws-latency-test.sh ssh                  # SSH into instance
#   ./scripts/aws-latency-test.sh logs [all|live|observer]  # Tail remote logs
#   ./scripts/aws-latency-test.sh redeploy             # Rebuild and upload binary
#   ./scripts/aws-latency-test.sh latency              # Measure latency to Polymarket endpoints
#   ./scripts/aws-latency-test.sh status               # Show instance status
#   ./scripts/aws-latency-test.sh db-check [interval]   # Show recent DB activity (default: 1h)
#   ./scripts/aws-latency-test.sh db-dump              # Download a pg_dump to local machine
#   ./scripts/aws-latency-test.sh db-sync              # Import remote dump into local Postgres
#   ./scripts/aws-latency-test.sh db-shell             # Open psql on the remote database
#   ./scripts/aws-latency-test.sh teardown             # Terminate and clean up everything
#
# Prerequisites:
#   - AWS CLI installed and configured (aws configure)
#   - .env file with POLYMARKET_PRIVATE_KEY
#
# Examples:
#   ./scripts/aws-latency-test.sh deploy
#   ./scripts/aws-latency-test.sh run --mode observe --duration 1h --no-persist --verbose
#   ./scripts/aws-latency-test.sh run --mode live --bet-size 2.5 --duration 30m --verbose
#   ./scripts/aws-latency-test.sh teardown
#

set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

REGION="eu-west-1"
INSTANCE_TYPE="t3.micro"
KEY_NAME="algo-trade-latency-test"
SG_NAME="algo-trade-latency-sg"
INSTANCE_TAG="algo-trade-latency"

DUMPS_DIR="$PROJECT_ROOT/data/aws-dumps"

# =============================================================================
# Helpers (provisioning-specific)
# =============================================================================

save_state() {
    cat > "$STATE_FILE" <<EOF
INSTANCE_ID=$INSTANCE_ID
KEY_NAME=$KEY_NAME
SECURITY_GROUP_ID=$SECURITY_GROUP_ID
PUBLIC_IP=$PUBLIC_IP
ALLOCATION_ID=$ALLOCATION_ID
REGION=$REGION
INSTANCE_MARKET=${INSTANCE_MARKET:-on-demand}
EOF
}

wait_for_ssh() {
    local max_attempts=30
    local attempt=0
    info "Waiting for SSH to become available..."
    while [[ $attempt -lt $max_attempts ]]; do
        if remote_ssh "true" 2>/dev/null; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 5
    done
    error "SSH not available after ${max_attempts} attempts"
    return 1
}

# =============================================================================
# deploy
# =============================================================================

cmd_deploy() {
    # Parse deploy options: --spot for spot instance, default is on-demand
    INSTANCE_MARKET="on-demand"
    for arg in "$@"; do
        case "$arg" in
            --spot) INSTANCE_MARKET="spot" ;;
        esac
    done

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}AWS Deploy — ${INSTANCE_MARKET}${NC}                                      ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    # Check prerequisites
    if ! command -v aws &>/dev/null; then
        error "AWS CLI not found. Install: https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html"
        exit 1
    fi

    if ! aws sts get-caller-identity &>/dev/null; then
        error "AWS CLI not configured. Run: aws configure"
        exit 1
    fi

    if [[ ! -f "$PROJECT_ROOT/.env" ]]; then
        error ".env file not found at $PROJECT_ROOT/.env"
        exit 1
    fi

    if [[ -f "$STATE_FILE" ]]; then
        warn "Existing deployment found. Run 'teardown' first or 'status' to check."
        exit 1
    fi

    # Step 1: Build release binary locally
    info "Building release binary locally..."
    (
        cd "$PROJECT_ROOT"
        set -a
        # shellcheck disable=SC1091
        source .env
        set +a
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
    info "Binary built: $binary ($binary_size)"

    # Step 2: Look up latest Ubuntu 24.04 AMI
    info "Looking up Ubuntu 24.04 AMI in $REGION..."
    local ami_id
    ami_id=$(aws ssm get-parameters \
        --names /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id \
        --region "$REGION" \
        --query 'Parameters[0].Value' \
        --output text 2>/dev/null || true)

    if [[ -z "$ami_id" || "$ami_id" == "None" ]]; then
        # Fallback: search for Ubuntu 24.04 AMI directly
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

    # Step 3: Create key pair
    info "Creating EC2 key pair..."
    # Delete existing key pair if it exists (from a failed previous deploy)
    aws ec2 delete-key-pair --key-name "$KEY_NAME" --region "$REGION" 2>/dev/null || true
    rm -f "$KEY_FILE"

    aws ec2 create-key-pair \
        --key-name "$KEY_NAME" \
        --region "$REGION" \
        --query 'KeyMaterial' \
        --output text > "$KEY_FILE"
    chmod 600 "$KEY_FILE"
    dim "Key: $KEY_NAME -> $KEY_FILE"

    # Step 4: Create security group (SSH from current IP only)
    info "Creating security group..."
    local my_ip
    my_ip=$(curl -s --max-time 5 ifconfig.me || curl -s --max-time 5 api.ipify.org || echo "0.0.0.0")

    # Get default VPC
    local vpc_id
    vpc_id=$(aws ec2 describe-vpcs \
        --region "$REGION" \
        --filters "Name=isDefault,Values=true" \
        --query 'Vpcs[0].VpcId' \
        --output text)

    # Reuse existing SG or create a new one
    SECURITY_GROUP_ID=$(aws ec2 describe-security-groups \
        --region "$REGION" \
        --filters "Name=group-name,Values=$SG_NAME" \
        --query 'SecurityGroups[0].GroupId' \
        --output text 2>/dev/null)

    if [[ -z "$SECURITY_GROUP_ID" || "$SECURITY_GROUP_ID" == "None" ]]; then
        SECURITY_GROUP_ID=$(aws ec2 create-security-group \
            --group-name "$SG_NAME" \
            --description "SSH access for algo-trade latency test" \
            --vpc-id "$vpc_id" \
            --region "$REGION" \
            --query 'GroupId' \
            --output text)
    fi

    # Update ingress rule for current IP (remove old rules first)
    aws ec2 revoke-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp --port 22 --cidr "0.0.0.0/0" \
        --region "$REGION" 2>/dev/null || true
    aws ec2 authorize-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp \
        --port 22 \
        --cidr "${my_ip}/32" \
        --region "$REGION" >/dev/null 2>&1 || true

    dim "Security group: $SECURITY_GROUP_ID (SSH from $my_ip)"

    # Step 5: Launch instance
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

    # Step 6: Wait for instance to be running
    info "Waiting for instance to start..."
    aws ec2 wait instance-running \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION"

    # Allocate Elastic IP (static — survives stop/start, needed for whitelisting)
    info "Allocating Elastic IP..."
    ALLOCATION_ID=$(aws ec2 allocate-address \
        --domain vpc \
        --region "$REGION" \
        --query 'AllocationId' \
        --output text)

    PUBLIC_IP=$(aws ec2 describe-addresses \
        --allocation-ids "$ALLOCATION_ID" \
        --region "$REGION" \
        --query 'Addresses[0].PublicIp' \
        --output text)

    aws ec2 associate-address \
        --instance-id "$INSTANCE_ID" \
        --allocation-id "$ALLOCATION_ID" \
        --region "$REGION" >/dev/null

    dim "Elastic IP: $PUBLIC_IP (Allocation: $ALLOCATION_ID)"

    # Wait for status checks
    info "Waiting for instance status checks..."
    aws ec2 wait instance-status-ok \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION"

    # Save state now (so teardown works even if next steps fail)
    save_state
    info "State saved to $STATE_FILE"

    # Step 7: Wait for SSH and install deps
    wait_for_ssh

    info "Installing runtime dependencies..."
    remote_ssh "sudo apt-get update -qq && sudo apt-get install -y -qq libssl3t64 ca-certificates >/dev/null 2>&1"

    # Step 8: Install PostgreSQL + TimescaleDB
    info "Installing PostgreSQL + TimescaleDB..."
    remote_ssh 'bash -s' <<'SETUP_PG'
set -euo pipefail

# Install Postgres 16
sudo apt-get install -y -qq postgresql postgresql-client >/dev/null 2>&1

# Install TimescaleDB from official apt repo
CODENAME=$(lsb_release -cs)
echo "deb https://packagecloud.io/timescale/timescaledb/ubuntu/ ${CODENAME} main" | sudo tee /etc/apt/sources.list.d/timescaledb.list >/dev/null
curl -sL https://packagecloud.io/timescale/timescaledb/gpgkey | sudo gpg --dearmor -o /etc/apt/trusted.gpg.d/timescaledb.gpg
sudo apt-get update -qq >/dev/null 2>&1
sudo apt-get install -y -qq timescaledb-2-postgresql-16 >/dev/null 2>&1 || echo "TimescaleDB package install returned non-zero (may still be OK)"

# Ensure shared_preload_libraries includes timescaledb
# timescaledb-tune handles this + memory settings, but verify it works
if sudo timescaledb-tune --quiet --yes 2>/dev/null; then
    echo "timescaledb-tune applied"
else
    echo "timescaledb-tune failed, setting shared_preload_libraries manually"
    sudo sed -i "s/^#*shared_preload_libraries.*/shared_preload_libraries = 'timescaledb'/" /etc/postgresql/16/main/postgresql.conf
    # Check if the line exists at all
    if ! grep -q "^shared_preload_libraries.*timescaledb" /etc/postgresql/16/main/postgresql.conf; then
        echo "shared_preload_libraries = 'timescaledb'" | sudo tee -a /etc/postgresql/16/main/postgresql.conf >/dev/null
    fi
fi

# Tune Postgres for low-memory (t3.micro = 1GB RAM)
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

# Create database and user
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

# Create TimescaleDB extension as superuser (algo can't do this)
sudo -u postgres psql -d algo_trade -c "CREATE EXTENSION IF NOT EXISTS timescaledb;"

# Allow local password auth
sudo sed -i 's/local\s\+all\s\+all\s\+peer/local   all             all                                     md5/' /etc/postgresql/16/main/pg_hba.conf
if ! grep -q "host.*all.*all.*127.0.0.1/32.*md5" /etc/postgresql/16/main/pg_hba.conf; then
    echo "host    all    all    127.0.0.1/32    md5" | sudo tee -a /etc/postgresql/16/main/pg_hba.conf >/dev/null
fi
sudo systemctl reload postgresql

echo "PostgreSQL + TimescaleDB installed and configured"
SETUP_PG

    # Step 9: Deploy migrations
    info "Deploying database migrations..."
    sync_and_migrate

    # Step 10: Set up daily backup cron
    info "Setting up daily database backup..."
    remote_ssh 'bash -s' <<'SETUP_BACKUP'
mkdir -p ~/backups
cat > ~/backup-db.sh <<'BKSCRIPT'
#!/bin/bash
export PGPASSWORD="algo_trade_local"
DUMP_FILE=~/backups/algo_trade_$(date +%Y%m%d_%H%M%S).sql.gz
pg_dump -h localhost -U algo algo_trade | gzip > "$DUMP_FILE"
# Keep only last 7 days of backups
find ~/backups -name "algo_trade_*.sql.gz" -mtime +7 -delete
BKSCRIPT
chmod +x ~/backup-db.sh

# Run daily at 04:00 UTC
(crontab -l 2>/dev/null | grep -v backup-db; echo "0 4 * * * ~/backup-db.sh") | crontab -
SETUP_BACKUP

    # Step 11: Inject DATABASE_URL into .env
    info "Deploying binary ($binary_size)..."
    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    info "Deploying .env (with DATABASE_URL)..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    # Append DATABASE_URL if not already present
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Step 12: Quick connectivity test
    info "Testing Polymarket API connectivity from $REGION..."
    local latency
    latency=$(remote_ssh "curl -s -o /dev/null -w '%{time_total}' https://clob.polymarket.com/time" 2>/dev/null || echo "failed")
    dim "CLOB API round-trip: ${latency}s"

    # Verify DB
    local table_count
    table_count=$(remote_ssh "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade -tAc \"SELECT count(*) FROM information_schema.tables WHERE table_schema='public'\"" 2>/dev/null || echo "?")
    dim "Database: algo_trade ($table_count tables)"

    # Done
    echo ""
    echo -e "${GREEN}═══════════════════════════════════════════════════════════════════${NC}"
    echo -e "${WHITE}Deployment complete!${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}  $INSTANCE_ID"
    echo -e "  ${DIM}Region:${NC}    $REGION"
    echo -e "  ${DIM}IP:${NC}        $PUBLIC_IP"
    echo -e "  ${DIM}Type:${NC}      $INSTANCE_TYPE (${INSTANCE_MARKET:-on-demand})"
    echo -e "  ${DIM}Database:${NC}  algo_trade (local Postgres + TimescaleDB)"
    echo ""
    echo -e "${WHITE}Quick start:${NC}"
    echo -e "  ${CYAN}./scripts/aws-latency-test.sh live${NC}           # Live trade with persistence"
    echo ""
    echo -e "${WHITE}Data commands:${NC}"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh db-shell${NC}       # Remote psql"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh db-dump${NC}        # Download pg_dump"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh db-sync${NC}        # Import into local Postgres"
    echo ""
    echo -e "${WHITE}Other commands:${NC}"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh ssh${NC}            # SSH into instance"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh redeploy${NC}       # Rebuild + sync migrations"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh teardown${NC}       # Destroy everything"
    echo ""
}

# =============================================================================
# run
# =============================================================================

cmd_run() {
    load_state

    # Default args if none provided
    local args=("$@")
    if [[ ${#args[@]} -eq 0 ]]; then
        args=(
            --pair btc,eth
            --combination coin1down_coin2up
            --mode observe
            --duration 1h
            --no-persist
            --min-spread 0.01
            --min-win-prob 0.50
            --max-loss-prob 0.99
            --max-position 1000000
            --kelly-fraction 0.25
            --stats-interval-secs 1
            --verbose
        )
        info "Using default observe mode args"
    fi

    echo -e "${CYAN}Running on ${PUBLIC_IP} (${REGION}):${NC}"
    dim "algo-trade cross-market-auto ${args[*]}"
    echo ""

    # Run with .env sourced, in foreground
    remote_ssh "set -a && source ~/.env && set +a && RUST_LOG=info ~/algo-trade cross-market-auto ${args[*]}"
}

# =============================================================================
# live
# =============================================================================

cmd_live() {
    load_state

    local duration="${1:-30m}"

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}Live Trading + Observer — ${REGION} (${PUBLIC_IP})${NC}              ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${WHITE}Live bot:${NC}"
    echo -e "  ${DIM}Pairs:${NC}      BTC/ETH, BTC/SOL, ETH/XRP (excl ETH/SOL, SOL/XRP)"
    echo -e "  ${DIM}Hours:${NC}      24/7 (collecting data)"
    echo -e "  ${DIM}Combo:${NC}      coin1up_coin2down (direction-neutral)"
    echo -e "  ${DIM}Shares:${NC}     5 per leg"
    echo -e "  ${DIM}Spread:${NC}     >= 0.10, reject symmetric ratio > 0.75"
    echo -e "  ${DIM}Entry:${NC}      10-4 mins before close"
    echo -e "  ${DIM}Max/window:${NC} \$20 / 2 trades"
    echo -e "  ${DIM}Duration:${NC}   ${duration}"
    echo ""
    echo -e "  ${WHITE}Observer:${NC}"
    echo -e "  ${DIM}Mode:${NC}       observe (all pairs, all combos)"
    echo -e "  ${DIM}Persists:${NC}   ALL opportunities to DB"
    echo -e "  ${DIM}Duration:${NC}   ${duration}"
    echo ""

    local live_args=(
        --pair all
        --exclude-pair eth,sol
        --exclude-pair sol,xrp
        --combination coin1up_coin2down
        --mode live
        --duration "$duration"
        --shares-per-leg 5
        --min-spread 0.10
        --min-win-prob 0.70
        --max-loss-prob 0.50
        --max-position 20
        --max-trades-per-window 2
        --kelly-fraction 0.25
        --entry-start-mins 10
        --entry-end-mins 4
        --max-leg-ratio 0.75
        --stats-interval-secs 1
        --persist
    )

    local observe_args=(
        --mode observe
        --duration "$duration"
        --persist
        --stats-interval-secs 30
    )

    dim "Observer: algo-trade cross-market-auto ${observe_args[*]}"
    dim "Live:     algo-trade cross-market-auto ${live_args[*]}"
    echo ""

    # Start observer in background first (no PTY needed, logs to file)
    # Use nohup + disown via bash -c so the process survives SSH disconnect
    info "Starting observer (background)..."
    remote_ssh "bash -c '
        set -a && source ~/.env && set +a
        nohup env RUST_LOG=info ~/algo-trade cross-market-auto ${observe_args[*]} \
            > /tmp/observer.log 2>&1 &
        echo \$! > /tmp/observer.pid
        disown
        sleep 2
        if kill -0 \$(cat /tmp/observer.pid) 2>/dev/null; then
            echo \"Observer started (PID \$(cat /tmp/observer.pid))\"
        else
            echo \"ERROR: Observer failed to start\"
            tail -5 /tmp/observer.log 2>/dev/null
        fi
    '"

    # Start live bot in foreground with PTY for dashboard
    info "Starting live bot (foreground)..."
    echo ""
    ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
        "set -a && source ~/.env && set +a && RUST_LOG=info,algo_trade_polymarket::arbitrage::sdk_client=debug,algo_trade_polymarket::arbitrage::live_executor=debug,algo_trade_polymarket::arbitrage::execution=debug ~/algo-trade cross-market-auto ${live_args[*]}"

    # When live bot exits (Ctrl+C or duration), also stop the observer
    echo ""
    info "Live bot exited. Stopping observer..."
    remote_ssh "if [[ -f /tmp/observer.pid ]]; then kill \$(cat /tmp/observer.pid) 2>/dev/null && echo 'Observer stopped' || echo 'Observer already exited'; rm -f /tmp/observer.pid; fi"
}

# =============================================================================
# stop — kill any running bot/observer processes
# =============================================================================

cmd_stop() {
    load_state

    info "Stopping all bot processes on $PUBLIC_IP..."

    remote_ssh "
        stopped=0
        # Stop observer
        if [[ -f /tmp/observer.pid ]]; then
            pid=\$(cat /tmp/observer.pid)
            if kill \$pid 2>/dev/null; then
                echo 'Stopped observer (PID '\$pid')'
                stopped=\$((stopped + 1))
            fi
            rm -f /tmp/observer.pid
        fi
        # Stop any remaining algo-trade processes
        for pid in \$(pgrep -f 'algo-trade cross-market-auto' 2>/dev/null); do
            kill \$pid 2>/dev/null && echo \"Stopped algo-trade PID \$pid\" && stopped=\$((stopped + 1))
        done
        if [[ \$stopped -eq 0 ]]; then
            echo 'No running processes found'
        fi
    "
}

# =============================================================================
# preflight
# =============================================================================

cmd_preflight() {
    load_state

    info "Running preflight checks on $PUBLIC_IP..."
    echo ""
    ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
        "set -a && source ~/.env && set +a && ~/algo-trade preflight --coins btc,eth --verbose"
}

# =============================================================================
# ssh
# =============================================================================

cmd_ssh() {
    load_state
    info "Connecting to $PUBLIC_IP..."
    ssh $SSH_OPTS -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP"
}

# =============================================================================
# logs
# =============================================================================

cmd_logs() {
    load_state

    local target="${1:-all}"

    case "$target" in
        observer)
            info "Tailing observer logs on $PUBLIC_IP..."
            remote_ssh "tail -f /tmp/observer.log 2>/dev/null || echo 'No observer log found.'"
            ;;
        live)
            info "Tailing live bot logs on $PUBLIC_IP..."
            remote_ssh "tail -f /tmp/cross_market_auto.log 2>/dev/null || echo 'No live bot log found.'"
            ;;
        all|*)
            info "Tailing all logs on $PUBLIC_IP (Ctrl+C to stop)..."
            remote_ssh "tail -f /tmp/cross_market_auto.log /tmp/observer.log 2>/dev/null || echo 'No log files found.'"
            ;;
    esac
}

# =============================================================================
# redeploy
# =============================================================================

cmd_redeploy() {
    load_state

    info "Building release binary..."
    (
        cd "$PROJECT_ROOT"
        set -a
        # shellcheck disable=SC1091
        source .env
        set +a
        if ! cargo build -p algo-trade-cli --release 2>&1 | tail -5; then
            error "Build failed"
            exit 1
        fi
    )

    local binary="$PROJECT_ROOT/target/release/algo-trade"
    local binary_size
    binary_size=$(du -h "$binary" | cut -f1)
    info "Binary built ($binary_size), uploading to $PUBLIC_IP..."

    # Stop any running processes first — Linux holds file handles on running
    # binaries, so scp will fail with "dest open: Failure" if the old binary
    # is still executing.
    info "Stopping any running bot processes before upload..."
    remote_ssh "pkill -f algo-trade 2>/dev/null; sleep 1; rm -f /tmp/observer.pid" || true

    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    # Sync .env (preserve remote DATABASE_URL)
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Sync and apply any new migrations
    sync_and_migrate

    info "Redeployed to $PUBLIC_IP"
}

# =============================================================================
# latency
# =============================================================================

cmd_latency() {
    load_state

    local rounds="${1:-5}"

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}Polymarket Endpoint Latency (${REGION})${NC}                      ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    # Endpoints to test: name|method|url
    local endpoints=(
        "CLOB Time|GET|https://clob.polymarket.com/time"
        "CLOB Prices|GET|https://clob.polymarket.com/prices"
        "CLOB Book|GET|https://clob.polymarket.com/book"
        "CLOB Order|POST|https://clob.polymarket.com/order"
        "Gamma API|GET|https://gamma-api.polymarket.com/markets?limit=1"
    )

    # Build the remote script
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

    # Calculate averages using awk
    avg_conn=$(printf "%s\n" "${connects[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    avg_fb=$(printf "%s\n" "${first_bytes[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    avg_total=$(printf "%s\n" "${totals[@]}" | awk "{s+=\$1} END {printf \"%.1f\", s/NR*1000}")
    min_total=$(printf "%s\n" "${totals[@]}" | awk "BEGIN{m=999} {if(\$1<m)m=\$1} END {printf \"%.1f\", m*1000}")
    max_total=$(printf "%s\n" "${totals[@]}" | awk "BEGIN{m=0} {if(\$1>m)m=\$1} END {printf \"%.1f\", m*1000}")

    printf "  %-16s  tcp:%-6s  tls→fb:%-7s  total:%-6s  (min:%-5s max:%-5s)  [%s]\n" \
        "$name" "${avg_conn}ms" "${avg_fb}ms" "${avg_total}ms" "${min_total}ms" "${max_total}ms" "$http"
}
'

    # Add each endpoint test to the remote script
    for ep in "${endpoints[@]}"; do
        IFS='|' read -r name method url <<< "$ep"
        remote_script+="run_test \"$name\" \"$method\" \"$url\""$'\n'
    done

    # Latency breakdown for CLOB
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
printf "    dns: %sms → tcp: %sms → tls: %sms (+%sms) → server: %sms (+%sms) → total: %sms\n" \
    "$dns" "$tcp" "$tls" "$tls_only" "$server" "$server_only" "$total"
echo ""
echo "  With connection reuse (reqwest::Client), tcp+tls is paid once."
echo "  Subsequent requests: ~${server_only}ms (server processing only)"
'

    info "Running $rounds rounds per endpoint from $PUBLIC_IP..."
    echo ""
    remote_ssh "$remote_script"
    echo ""
}

# =============================================================================
# status
# =============================================================================

cmd_status() {
    load_state

    echo -e "${CYAN}AWS Latency Test Instance${NC}"
    echo ""

    local state
    state=$(aws ec2 describe-instances \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION" \
        --query 'Reservations[0].Instances[0].State.Name' \
        --output text 2>/dev/null || echo "unknown")

    local launch_time
    launch_time=$(aws ec2 describe-instances \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION" \
        --query 'Reservations[0].Instances[0].LaunchTime' \
        --output text 2>/dev/null || echo "unknown")

    echo -e "  ${DIM}Instance:${NC}    $INSTANCE_ID"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo -e "  ${DIM}IP:${NC}          $PUBLIC_IP"
    echo -e "  ${DIM}State:${NC}       $state"
    echo -e "  ${DIM}Launched:${NC}    $launch_time"
    echo -e "  ${DIM}Type:${NC}        $INSTANCE_TYPE"
    echo ""

    if [[ "$state" == "running" ]]; then
        echo -e "${DIM}SSH: ssh -i $KEY_FILE $SSH_USER@$PUBLIC_IP${NC}"
    fi
}

# =============================================================================
# db-check
# =============================================================================

cmd_db_check() {
    load_state

    local interval="${1:-1 hour}"

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}Database Activity — last ${interval}${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    remote_ssh "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade" <<EOSQL
-- Row counts per table (recent activity)
\echo '=== Row Counts ==='
SELECT
    relname AS table,
    n_live_tup AS total_rows
FROM pg_stat_user_tables
WHERE n_live_tup > 0
ORDER BY n_live_tup DESC;

-- Recent inserts per table in the time window
\echo ''
\echo '=== Rows Inserted (last ${interval}) ==='
SELECT 'window_settlements' AS table,
       count(*) AS rows,
       count(DISTINCT coin) AS coins,
       min(window_start) AS earliest,
       max(window_start) AS latest
FROM window_settlements
WHERE settled_at > NOW() - INTERVAL '${interval}'
UNION ALL
SELECT 'clob_price_snapshots',
       count(*), count(DISTINCT coin),
       min(timestamp), max(timestamp)
FROM clob_price_snapshots
WHERE timestamp > NOW() - INTERVAL '${interval}'
UNION ALL
SELECT 'chainlink_window_prices',
       count(*), count(DISTINCT coin),
       min(window_start), max(window_start)
FROM chainlink_window_prices
WHERE last_polled_at > NOW() - INTERVAL '${interval}'
UNION ALL
SELECT 'cross_market_opportunities',
       count(*), count(DISTINCT coin1),
       min(timestamp), max(timestamp)
FROM cross_market_opportunities
WHERE timestamp > NOW() - INTERVAL '${interval}'
ORDER BY rows DESC;

-- Window settlements detail
\echo ''
\echo '=== Window Settlements (last ${interval}) ==='
SELECT
    window_start,
    coin,
    outcome,
    settlement_source,
    chainlink_start_price,
    chainlink_end_price
FROM window_settlements
WHERE settled_at > NOW() - INTERVAL '${interval}'
ORDER BY window_start DESC, coin;

-- Executed trades detail
\echo ''
\echo '=== Executed Trades (last ${interval}) ==='
SELECT
    timestamp AS detected_at,
    coin1 || '/' || coin2 AS pair,
    combination,
    leg1_direction || ' @' || ROUND(leg1_price, 3) AS leg1,
    leg2_direction || ' @' || ROUND(leg2_price, 3) AS leg2,
    ROUND(total_cost, 3) AS cost,
    ROUND(spread, 3) AS spread,
    CASE WHEN leg1_fill_price IS NOT NULL THEN ROUND(leg1_fill_price, 3)::TEXT ELSE 'REJECTED' END AS leg1_fill,
    CASE WHEN leg2_fill_price IS NOT NULL THEN ROUND(leg2_fill_price, 3)::TEXT ELSE 'REJECTED' END AS leg2_fill,
    ROUND(slippage, 4) AS slip,
    status,
    trade_result,
    ROUND(actual_pnl, 4) AS pnl
FROM cross_market_opportunities
WHERE executed = true
  AND timestamp > NOW() - INTERVAL '${interval}'
ORDER BY timestamp DESC;

-- Settled trades summary
\echo ''
\echo '=== Trade Summary ==='
SELECT
    COUNT(*) FILTER (WHERE executed = true) AS total_trades,
    COUNT(*) FILTER (WHERE executed = true AND leg1_fill_price IS NOT NULL AND leg2_fill_price IS NOT NULL) AS both_filled,
    COUNT(*) FILTER (WHERE executed = true AND (leg1_fill_price IS NULL OR leg2_fill_price IS NULL)) AS partial_fills,
    COUNT(*) FILTER (WHERE status = 'settled') AS settled,
    COUNT(*) FILTER (WHERE trade_result = 'WIN') AS wins,
    COUNT(*) FILTER (WHERE trade_result = 'LOSE') AS losses,
    ROUND(SUM(actual_pnl) FILTER (WHERE status = 'settled'), 4) AS total_pnl
FROM cross_market_opportunities
WHERE timestamp > NOW() - INTERVAL '${interval}';

-- Sessions
\echo ''
\echo '=== Active Sessions ==='
SELECT
    session_id,
    started_at,
    status,
    total_opportunities
FROM cross_market_sessions
ORDER BY started_at DESC
LIMIT 5;
EOSQL
}

# =============================================================================
# db-dump
# =============================================================================

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

    # Show row counts for key tables
    info "Remote table row counts:"
    remote_ssh "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade -c \"
        SELECT relname AS table, n_live_tup AS rows
        FROM pg_stat_user_tables
        WHERE n_live_tup > 0
        ORDER BY n_live_tup DESC;
    \"" 2>/dev/null || true
}

# =============================================================================
# db-sync
# =============================================================================

cmd_db_sync() {
    load_state

    # Find the latest dump
    local latest_dump
    latest_dump=$(ls -t "$DUMPS_DIR"/algo_trade_*.sql.gz 2>/dev/null | head -1)

    if [[ -z "$latest_dump" ]]; then
        warn "No dumps found in $DUMPS_DIR. Run 'db-dump' first."
        exit 1
    fi

    info "Latest dump: $latest_dump"
    local dump_size
    dump_size=$(du -h "$latest_dump" | cut -f1)
    dim "Size: $dump_size"

    # Check if local Postgres is accessible
    local local_db_url="${DATABASE_URL:-${LOCAL_DATABASE_URL:-}}"
    if [[ -z "$local_db_url" ]]; then
        # Try to read from .env
        if [[ -f "$PROJECT_ROOT/.env" ]]; then
            local_db_url=$(grep '^DATABASE_URL=' "$PROJECT_ROOT/.env" | cut -d= -f2- || true)
        fi
    fi

    if [[ -z "$local_db_url" ]]; then
        error "No DATABASE_URL found. Set it in .env or environment."
        exit 1
    fi

    info "Importing into local database..."
    dim "URL: $local_db_url"

    # Extract host/port/db/user from the URL for psql commands
    # Format: postgres://user:pass@host:port/dbname
    local db_user db_pass db_host db_port db_name
    db_user=$(echo "$local_db_url" | sed -n 's|.*://\([^:]*\):.*|\1|p')
    db_pass=$(echo "$local_db_url" | sed -n 's|.*://[^:]*:\([^@]*\)@.*|\1|p')
    db_host=$(echo "$local_db_url" | sed -n 's|.*@\([^:]*\):.*|\1|p')
    db_port=$(echo "$local_db_url" | sed -n 's|.*:\([0-9]*\)/.*|\1|p')
    db_name=$(echo "$local_db_url" | sed -n 's|.*/\([^?]*\).*|\1|p')

    info "Dropping and recreating local database '$db_name'..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d postgres \
        -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$db_name' AND pid <> pg_backend_pid();" \
        -c "DROP DATABASE IF EXISTS $db_name;" \
        -c "CREATE DATABASE $db_name;" \
        2>&1 | tail -3

    # Restore: TimescaleDB requires pre_restore / post_restore for proper hypertable import
    info "Restoring from dump..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "CREATE EXTENSION IF NOT EXISTS timescaledb;" 2>/dev/null
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "SELECT timescaledb_pre_restore();" 2>/dev/null
    gunzip -c "$latest_dump" | PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" 2>&1 | grep -cE 'ERROR' | xargs -I{} echo "  {} errors during restore"
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "SELECT timescaledb_post_restore();" 2>/dev/null

    # Verify
    info "Verifying local data..."
    PGPASSWORD="$db_pass" psql -h "$db_host" -p "$db_port" -U "$db_user" -d "$db_name" -c "
        SELECT relname AS table, n_live_tup AS rows
        FROM pg_stat_user_tables
        WHERE n_live_tup > 0
        ORDER BY n_live_tup DESC;
    " 2>/dev/null || true

    info "Import complete. Local database synced from $(basename "$latest_dump")"
}

# =============================================================================
# db-shell
# =============================================================================

cmd_db_shell() {
    load_state

    info "Opening psql on $PUBLIC_IP..."
    ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
        "PGPASSWORD=algo_trade_local psql -h localhost -U algo -d algo_trade"
}

# =============================================================================
# teardown
# =============================================================================

cmd_teardown() {
    load_state

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}AWS Latency Test - Teardown${NC}                              ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    echo -e "  ${DIM}Instance:${NC}  $INSTANCE_ID"
    echo -e "  ${DIM}Elastic IP:${NC} ${PUBLIC_IP:-unknown} (${ALLOCATION_ID:-unknown})"
    echo -e "  ${DIM}SG:${NC}        $SECURITY_GROUP_ID"
    echo -e "  ${DIM}Key:${NC}       $KEY_NAME"
    echo ""

    read -rp "Type 'yes' to destroy all resources: " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "Aborted."
        exit 1
    fi
    echo ""

    # Cancel spot instance request if applicable (persistent spot will relaunch otherwise)
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
            dim "Cancelled spot request: $spot_req"
        fi
    else
        # Safety: check for spot request even on on-demand (in case state file is stale)
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
            dim "Cancelled spot request: $spot_req"
        fi
    fi

    # Terminate instance
    info "Terminating instance $INSTANCE_ID..."
    aws ec2 terminate-instances \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION" >/dev/null 2>&1 || true

    info "Waiting for termination..."
    aws ec2 wait instance-terminated \
        --instance-ids "$INSTANCE_ID" \
        --region "$REGION" 2>/dev/null || true

    # Release Elastic IP
    if [[ -n "${ALLOCATION_ID:-}" ]]; then
        info "Releasing Elastic IP $ALLOCATION_ID..."
        aws ec2 release-address \
            --allocation-id "$ALLOCATION_ID" \
            --region "$REGION" 2>/dev/null || true
    fi

    # Delete security group (retry — takes a moment after termination)
    info "Deleting security group $SECURITY_GROUP_ID..."
    local sg_attempts=0
    while [[ $sg_attempts -lt 10 ]]; do
        if aws ec2 delete-security-group \
            --group-id "$SECURITY_GROUP_ID" \
            --region "$REGION" 2>/dev/null; then
            break
        fi
        sg_attempts=$((sg_attempts + 1))
        sleep 5
    done

    # Delete key pair
    info "Deleting key pair $KEY_NAME..."
    aws ec2 delete-key-pair --key-name "$KEY_NAME" --region "$REGION" 2>/dev/null || true

    # Clean up local files
    rm -f "$KEY_FILE" "$STATE_FILE"

    echo ""
    info "All resources cleaned up."
}

# =============================================================================
# Main
# =============================================================================

case "${1:-help}" in
    deploy)
        shift
        cmd_deploy "$@"
        ;;
    run)
        shift
        cmd_run "$@"
        ;;
    live)
        shift
        cmd_live "$@"
        ;;
    stop)
        cmd_stop
        ;;
    preflight)
        cmd_preflight
        ;;
    ssh)
        cmd_ssh
        ;;
    logs)
        shift
        cmd_logs "$@"
        ;;
    redeploy)
        cmd_redeploy
        ;;
    latency)
        shift
        cmd_latency "$@"
        ;;
    status)
        cmd_status
        ;;
    db-check)
        shift
        cmd_db_check "$@"
        ;;
    db-dump)
        cmd_db_dump
        ;;
    db-sync)
        cmd_db_sync
        ;;
    db-shell)
        cmd_db_shell
        ;;
    teardown)
        cmd_teardown
        ;;
    help|--help|-h)
        head -23 "$0" | tail -22
        ;;
    *)
        error "Unknown command: $1"
        echo "Usage: $0 {deploy|run|live|stop|preflight|ssh|logs|redeploy|latency|status|db-check|db-dump|db-sync|db-shell|teardown}"
        exit 1
        ;;
esac
