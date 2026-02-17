#!/bin/bash
#
# IB Options Trading System — EC2 Deployment
#
# Deploys the OpenClaw-orchestrated LLM-driven options trading system to EC2.
# Provisions PostgreSQL, Python environment, cron jobs, and Discord/Telegram integration.
#
# Usage:
#   ./scripts/deploy-ib-options.sh deploy              # Provision EC2 + install all dependencies
#   ./scripts/deploy-ib-options.sh sync                # Upload latest Python code
#   ./scripts/deploy-ib-options.sh setup-cron          # Install cron jobs for scheduled workflows
#   ./scripts/deploy-ib-options.sh start-gateway       # Start IB Gateway container
#   ./scripts/deploy-ib-options.sh stop-gateway        # Stop IB Gateway container
#   ./scripts/deploy-ib-options.sh start-manager       # Start position manager daemon
#   ./scripts/deploy-ib-options.sh start-scheduler     # Start workflow scheduler daemon
#   ./scripts/deploy-ib-options.sh stop-all            # Stop all services
#   ./scripts/deploy-ib-options.sh ssh                 # SSH into instance
#   ./scripts/deploy-ib-options.sh logs [service]      # Tail logs (manager|scheduler|premarket|weekly)
#   ./scripts/deploy-ib-options.sh status              # Show service status
#   ./scripts/deploy-ib-options.sh teardown            # Terminate and clean up
#
# Prerequisites:
#   - AWS CLI installed and configured (aws configure)
#   - .env file with required keys (see .env.example)
#   - Discord webhook URL or Telegram bot token configured
#

set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

# Override state file to separate from Polymarket deployment
STATE_FILE="$SCRIPT_DIR/.ib-options-deploy.state"
KEY_FILE="$SCRIPT_DIR/.ib-options-deploy-key.pem"

REGION="us-east-1"
INSTANCE_TYPE="t3.small"  # Needs more resources for Python + Postgres
KEY_NAME="ib-options-deploy"
SG_NAME="ib-options-sg"
INSTANCE_TAG="ib-options-trading"

PYTHON_VERSION="3.11"

# =============================================================================
# Helpers
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
# deploy — Provision EC2 and install all dependencies
# =============================================================================

cmd_deploy() {
    INSTANCE_MARKET="on-demand"
    for arg in "$@"; do
        case "$arg" in
            --spot) INSTANCE_MARKET="spot" ;;
        esac
    done

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}IB Options Trading System — EC2 Deploy${NC}              ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}    $INSTANCE_TYPE ($INSTANCE_MARKET)"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo ""

    # Check if already deployed
    if [[ -f "$STATE_FILE" ]]; then
        warn "Deployment state file exists. If you want to redeploy, run 'teardown' first."
        load_state
        info "Existing deployment: $PUBLIC_IP (instance $INSTANCE_ID)"
        exit 1
    fi

    # Skip Rust build - IB Options system is Python-based (OpenClaw)
    # Rust components not needed for this deployment
    # info "Building Rust components..."
    # (
    #     cd "$PROJECT_ROOT"
    #     cargo build -p algo-trade-ib --release 2>&1 | tail -5 || true
    # )

    # Create SSH key pair
    info "Creating SSH key pair..."
    if ! aws ec2 describe-key-pairs --key-names "$KEY_NAME" --region "$REGION" &>/dev/null; then
        aws ec2 create-key-pair \
            --key-name "$KEY_NAME" \
            --region "$REGION" \
            --query 'KeyMaterial' \
            --output text > "$KEY_FILE"
        chmod 400 "$KEY_FILE"
    else
        warn "Key pair $KEY_NAME already exists, reusing."
    fi

    # Create security group
    info "Creating security group..."
    VPC_ID=$(aws ec2 describe-vpcs --region "$REGION" --filters "Name=isDefault,Values=true" --query 'Vpcs[0].VpcId' --output text)

    if ! SECURITY_GROUP_ID=$(aws ec2 describe-security-groups --region "$REGION" --filters "Name=group-name,Values=$SG_NAME" --query 'SecurityGroups[0].GroupId' --output text 2>/dev/null) || [[ "$SECURITY_GROUP_ID" == "None" ]]; then
        SECURITY_GROUP_ID=$(aws ec2 create-security-group \
            --group-name "$SG_NAME" \
            --description "IB Options Trading System" \
            --vpc-id "$VPC_ID" \
            --region "$REGION" \
            --query 'GroupId' \
            --output text)

        # Allow SSH
        aws ec2 authorize-security-group-ingress \
            --group-id "$SECURITY_GROUP_ID" \
            --protocol tcp \
            --port 22 \
            --cidr 0.0.0.0/0 \
            --region "$REGION"
    else
        info "Security group $SG_NAME already exists, reusing."
    fi

    # Launch instance
    info "Launching EC2 instance..."

    # Get latest Ubuntu 22.04 AMI
    AMI_ID=$(aws ec2 describe-images \
        --region "$REGION" \
        --owners 099720109477 \
        --filters "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" \
        --query 'sort_by(Images, &CreationDate)[-1].ImageId' \
        --output text)

    if [[ "$INSTANCE_MARKET" == "spot" ]]; then
        # Spot instance request
        SPOT_REQUEST_ID=$(aws ec2 request-spot-instances \
            --region "$REGION" \
            --instance-count 1 \
            --type "one-time" \
            --launch-specification "{
                \"ImageId\": \"$AMI_ID\",
                \"InstanceType\": \"$INSTANCE_TYPE\",
                \"KeyName\": \"$KEY_NAME\",
                \"SecurityGroupIds\": [\"$SECURITY_GROUP_ID\"],
                \"BlockDeviceMappings\": [{
                    \"DeviceName\": \"/dev/sda1\",
                    \"Ebs\": {\"VolumeSize\": 20, \"VolumeType\": \"gp3\"}
                }]
            }" \
            --query 'SpotInstanceRequests[0].SpotInstanceRequestId' \
            --output text)

        info "Waiting for spot request to fulfill..."
        aws ec2 wait spot-instance-request-fulfilled --region "$REGION" --spot-instance-request-ids "$SPOT_REQUEST_ID"

        INSTANCE_ID=$(aws ec2 describe-spot-instance-requests \
            --region "$REGION" \
            --spot-instance-request-ids "$SPOT_REQUEST_ID" \
            --query 'SpotInstanceRequests[0].InstanceId' \
            --output text)
    else
        # On-demand instance
        INSTANCE_ID=$(aws ec2 run-instances \
            --region "$REGION" \
            --image-id "$AMI_ID" \
            --instance-type "$INSTANCE_TYPE" \
            --key-name "$KEY_NAME" \
            --security-group-ids "$SECURITY_GROUP_ID" \
            --block-device-mappings "DeviceName=/dev/sda1,Ebs={VolumeSize=20,VolumeType=gp3}" \
            --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$INSTANCE_TAG}]" \
            --query 'Instances[0].InstanceId' \
            --output text)
    fi

    info "Instance $INSTANCE_ID created, waiting for running state..."
    aws ec2 wait instance-running --region "$REGION" --instance-ids "$INSTANCE_ID"

    # Get public IP
    PUBLIC_IP=$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$INSTANCE_ID" \
        --query 'Reservations[0].Instances[0].PublicIpAddress' \
        --output text)

    ALLOCATION_ID=""  # No elastic IP for now

    info "Instance running at $PUBLIC_IP"
    save_state

    # Wait for SSH
    wait_for_ssh

    # Install system dependencies
    info "Updating package lists..."
    remote_ssh "sudo apt-get update -y" 2>&1 | tail -3

    info "Installing system dependencies..."
    remote_ssh "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
        postgresql postgresql-contrib \
        python${PYTHON_VERSION} python${PYTHON_VERSION}-venv python3-pip \
        git curl build-essential libpq-dev \
        docker.io \
        htop vim" 2>&1 | tail -10

    # Add ubuntu to docker group
    remote_ssh "sudo usermod -aG docker ubuntu" 2>&1 | tail -3

    # Setup PostgreSQL
    info "Setting up PostgreSQL..."
    remote_ssh "sudo -u postgres psql <<'SQL'
CREATE USER algo WITH PASSWORD '${REMOTE_DB_PASS}';
CREATE DATABASE ${REMOTE_DB_NAME} OWNER algo;
GRANT ALL PRIVILEGES ON DATABASE ${REMOTE_DB_NAME} TO algo;
\c ${REMOTE_DB_NAME}
GRANT ALL ON SCHEMA public TO algo;
SQL
" 2>&1 | tail -5

    # Create Python virtualenv
    info "Setting up Python environment..."
    remote_ssh "
        cd ~
        python${PYTHON_VERSION} -m venv venv
        source venv/bin/activate
        pip install --upgrade pip wheel setuptools
    " 2>&1 | tail -3

    # Upload Python code
    info "Uploading Python code..."
    cmd_sync

    # Install Python dependencies
    info "Installing Python dependencies..."
    remote_ssh "
        source venv/bin/activate
        cd ~/python
        pip install -e . 2>&1 | tail -10
    "

    # Upload and configure .env
    info "Uploading .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"
    # Override DATABASE_URL for remote Postgres
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    # Sync and apply migrations
    sync_and_migrate

    # Create log directory
    remote_ssh "mkdir -p ~/logs"

    echo ""
    info "✅ IB Options Trading System deployed to $PUBLIC_IP"
    echo ""
    echo -e "  ${DIM}Next steps:${NC}"
    echo -e "    ./scripts/deploy-ib-options.sh setup-cron      # Install scheduled workflows"
    echo -e "    ./scripts/deploy-ib-options.sh start-manager   # Start position manager"
    echo -e "    ./scripts/deploy-ib-options.sh ssh             # SSH into instance"
    echo ""
}

# =============================================================================
# sync — Upload latest Python code
# =============================================================================

cmd_sync() {
    load_state

    info "Syncing Python code to $PUBLIC_IP..."

    # Create tarball of Python code
    (
        cd "$PROJECT_ROOT"
        tar -czf /tmp/ib-options-python.tar.gz \
            -C python \
            --exclude='__pycache__' \
            --exclude='*.pyc' \
            --exclude='.pytest_cache' \
            .
    )

    # Upload and extract
    remote_scp /tmp/ib-options-python.tar.gz "$SSH_USER@$PUBLIC_IP:~/python.tar.gz"
    remote_ssh "
        rm -rf ~/python
        mkdir -p ~/python
        tar -xzf ~/python.tar.gz -C ~/python
        rm ~/python.tar.gz
    "

    rm /tmp/ib-options-python.tar.gz
    info "Code synced."
}

# =============================================================================
# setup-cron — Install cron jobs for scheduled workflows
# =============================================================================

cmd_setup_cron() {
    load_state

    info "Setting up cron jobs on $PUBLIC_IP..."

    # Upload helper scripts
    remote_ssh "mkdir -p ~/scripts"

    # Create premarket script
    remote_ssh "cat > ~/scripts/run-premarket.sh <<'SCRIPT'
#!/bin/bash
set -a && source ~/.env && set +a
cd ~/python
~/venv/bin/python -m openclaw run trade-thesis --ticker AAPL --db-url \$DATABASE_URL
SCRIPT
chmod +x ~/scripts/run-premarket.sh"

    # Create weekly deep dive script
    remote_ssh "cat > ~/scripts/run-weekly-deep-dive.sh <<'SCRIPT'
#!/bin/bash
set -a && source ~/.env && set +a
cd ~/python
# Run research for watchlist tickers
for ticker in AAPL NVDA MSFT GOOGL META AMZN TSLA AMD; do
    ~/venv/bin/python -m openclaw research \$ticker >> ~/logs/weekly-\$(date +%Y%m%d).log 2>&1
done
SCRIPT
chmod +x ~/scripts/run-weekly-deep-dive.sh"

    # Create crontab
    remote_ssh "cat > ~/crontab <<'CRON'
# IB Options Trading Workflows (times in ET)
# Note: Cron uses UTC, adjust for ET (ET = UTC-5 or UTC-4 during DST)

# Pre-market research (8:00 AM ET = 13:00 UTC)
0 13 * * 1-5 /home/ubuntu/scripts/run-premarket.sh >> /home/ubuntu/logs/premarket.log 2>&1

# Weekend deep dive (Saturday 10:00 AM ET = 15:00 UTC)
0 15 * * 6 /home/ubuntu/scripts/run-weekly-deep-dive.sh >> /home/ubuntu/logs/weekly.log 2>&1

# Log rotation (keep last 7 days)
0 2 * * * find /home/ubuntu/logs -name '*.log' -mtime +7 -delete
CRON
crontab ~/crontab"

    info "Cron jobs installed. View with: ssh $PUBLIC_IP 'crontab -l'"
}

# =============================================================================
# start-manager — Start position manager daemon
# =============================================================================

cmd_start_manager() {
    load_state

    info "Starting position manager on $PUBLIC_IP..."

    # Create systemd service file
    remote_ssh "sudo tee /etc/systemd/system/ib-options-manager.service > /dev/null <<'SERVICE'
[Unit]
Description=IB Options Position Manager
After=network.target postgresql.service

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/home/ubuntu/python
EnvironmentFile=/home/ubuntu/.env
ExecStart=/home/ubuntu/venv/bin/python -m openclaw --db-url postgres://algo:algo_trade_local@localhost/algo_trade position-manager --mode paper --poll-interval 30
Restart=always
RestartSec=10
StandardOutput=append:/home/ubuntu/logs/manager.log
StandardError=append:/home/ubuntu/logs/manager.log

[Install]
WantedBy=multi-user.target
SERVICE
"

    remote_ssh "
        sudo systemctl daemon-reload
        sudo systemctl enable ib-options-manager
        sudo systemctl start ib-options-manager
        sleep 2
        sudo systemctl status ib-options-manager --no-pager
    "

    info "Position manager started. View logs: ./scripts/deploy-ib-options.sh logs manager"
}

# =============================================================================
# start-scheduler — Start workflow scheduler daemon
# =============================================================================

cmd_start_scheduler() {
    load_state

    info "Starting workflow scheduler on $PUBLIC_IP..."

    # Create systemd service file
    remote_ssh "sudo tee /etc/systemd/system/ib-options-scheduler.service > /dev/null <<'SERVICE'
[Unit]
Description=IB Options Workflow Scheduler
After=network.target postgresql.service

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/home/ubuntu/python
EnvironmentFile=/home/ubuntu/.env
ExecStart=/home/ubuntu/venv/bin/python -m openclaw --db-url postgres://algo:algo_trade_local@localhost/algo_trade scheduler
Restart=always
RestartSec=10
StandardOutput=append:/home/ubuntu/logs/scheduler.log
StandardError=append:/home/ubuntu/logs/scheduler.log

[Install]
WantedBy=multi-user.target
SERVICE
"

    remote_ssh "
        sudo systemctl daemon-reload
        sudo systemctl enable ib-options-scheduler
        sudo systemctl start ib-options-scheduler
        sleep 2
        sudo systemctl status ib-options-scheduler --no-pager
    "

    info "Workflow scheduler started. View logs: ./scripts/deploy-ib-options.sh logs scheduler"
}

# =============================================================================
# stop-all — Stop all services
# =============================================================================

cmd_stop_all() {
    load_state

    info "Stopping all services on $PUBLIC_IP..."

    remote_ssh "
        sudo systemctl stop ib-options-manager || true
        sudo systemctl stop ib-options-scheduler || true
        sudo systemctl disable ib-options-manager || true
        sudo systemctl disable ib-options-scheduler || true
    "

    info "All services stopped."
}

# =============================================================================
# start-gateway — Start IB Gateway container on EC2
# =============================================================================

cmd_start_gateway() {
    load_state

    info "Starting IB Gateway container on $PUBLIC_IP..."

    # Ensure .env has IB credentials
    if ! grep -q "IBKR_USERNAME" "$PROJECT_ROOT/.env" 2>/dev/null; then
        error "IBKR_USERNAME not found in .env — see secrets/ib_credentials.env.example"
        exit 1
    fi

    # Source credentials from .env
    local ibkr_username ibkr_password ib_trading_mode
    ibkr_username=$(grep "^IBKR_USERNAME=" "$PROJECT_ROOT/.env" | cut -d= -f2-)
    ibkr_password=$(grep "^IBKR_PASSWORD=" "$PROJECT_ROOT/.env" | cut -d= -f2-)
    ib_trading_mode=$(grep "^IB_TRADING_MODE=" "$PROJECT_ROOT/.env" | cut -d= -f2- || echo "paper")

    remote_ssh "
        docker rm -f ib-gateway 2>/dev/null || true
        docker run -d --name ib-gateway \
            -e TWS_USERID='${ibkr_username}' \
            -e TWS_PASSWORD='${ibkr_password}' \
            -e TRADING_MODE='${ib_trading_mode:-paper}' \
            -e TIME_ZONE=America/New_York \
            -e TWOFA_TIMEOUT_ACTION=restart \
            -e READ_ONLY_API=no \
            -e BYPASS_WARNING=yes \
            -p 4001:4001 -p 4002:4002 \
            --restart unless-stopped \
            ghcr.io/gnzsnz/ib-gateway:latest
    "

    info "IB Gateway started. Check: ./scripts/deploy-ib-options.sh status"
}

# =============================================================================
# stop-gateway — Stop IB Gateway container on EC2
# =============================================================================

cmd_stop_gateway() {
    load_state

    info "Stopping IB Gateway container on $PUBLIC_IP..."
    remote_ssh "docker rm -f ib-gateway 2>/dev/null || true"
    info "IB Gateway stopped."
}

# =============================================================================
# logs — Tail logs
# =============================================================================

cmd_logs() {
    load_state

    local log_type="${1:-manager}"
    local log_file

    case "$log_type" in
        manager)
            log_file="~/logs/manager.log"
            ;;
        scheduler)
            log_file="~/logs/scheduler.log"
            ;;
        premarket)
            log_file="~/logs/premarket.log"
            ;;
        weekly)
            log_file="~/logs/weekly.log"
            ;;
        *)
            error "Unknown log type: $log_type"
            echo "Available: manager, scheduler, premarket, weekly"
            exit 1
            ;;
    esac

    info "Tailing $log_type logs on $PUBLIC_IP (Ctrl+C to stop)..."
    echo ""
    remote_ssh "tail -f $log_file 2>/dev/null || echo 'No log file found at $log_file'"
}

# =============================================================================
# status — Show service status
# =============================================================================

cmd_status() {
    load_state

    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}IB Options Trading System — Status${NC}                  ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}    $PUBLIC_IP ($INSTANCE_ID)"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo ""

    info "IB Gateway:"
    remote_ssh "docker ps --filter name=ib-gateway --format '  {{.Status}}  ({{.Ports}})' 2>/dev/null || echo '  Not running'"
    echo ""

    info "Position Manager:"
    remote_ssh "sudo systemctl status ib-options-manager --no-pager || echo 'Not running'"
    echo ""

    info "Workflow Scheduler:"
    remote_ssh "sudo systemctl status ib-options-scheduler --no-pager || echo 'Not running'"
    echo ""

    info "Cron jobs:"
    remote_ssh "crontab -l 2>/dev/null | grep -v '^#' || echo 'No cron jobs'"
    echo ""

    info "Recent log activity:"
    remote_ssh "ls -lht ~/logs/*.log 2>/dev/null | head -5 || echo 'No logs found'"
}

# =============================================================================
# teardown — Terminate EC2 and clean up
# =============================================================================

cmd_teardown() {
    if [[ ! -f "$STATE_FILE" ]]; then
        error "No deployment state file found. Nothing to tear down."
        exit 1
    fi

    load_state

    echo -e "${RED}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${RED}║${NC}        ${WHITE}WARNING: Tearing Down IB Options Trading System${NC}    ${RED}║${NC}"
    echo -e "${RED}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}    $PUBLIC_IP ($INSTANCE_ID)"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo ""

    read -p "Are you sure you want to terminate and delete everything? (yes/no): " confirm
    if [[ "$confirm" != "yes" ]]; then
        info "Aborted."
        exit 0
    fi

    # Stop services first
    cmd_stop_all || true

    # Terminate instance
    info "Terminating instance $INSTANCE_ID..."
    aws ec2 terminate-instances --region "$REGION" --instance-ids "$INSTANCE_ID" >/dev/null
    aws ec2 wait instance-terminated --region "$REGION" --instance-ids "$INSTANCE_ID" 2>/dev/null || true

    # Delete security group
    info "Deleting security group..."
    aws ec2 delete-security-group --region "$REGION" --group-id "$SECURITY_GROUP_ID" 2>/dev/null || true

    # Delete key pair
    info "Deleting key pair..."
    aws ec2 delete-key-pair --region "$REGION" --key-name "$KEY_NAME" 2>/dev/null || true
    rm -f "$KEY_FILE"

    # Remove state file
    rm -f "$STATE_FILE"

    info "✅ Teardown complete."
}

# =============================================================================
# Main dispatcher
# =============================================================================

case "${1:-}" in
    deploy)
        shift
        cmd_deploy "$@"
        ;;
    sync)
        cmd_sync
        ;;
    setup-cron)
        cmd_setup_cron
        ;;
    start-gateway)
        cmd_start_gateway
        ;;
    stop-gateway)
        cmd_stop_gateway
        ;;
    start-manager)
        cmd_start_manager
        ;;
    start-scheduler)
        cmd_start_scheduler
        ;;
    stop-all)
        cmd_stop_all
        ;;
    ssh)
        load_state
        ec2_ssh_interactive
        ;;
    logs)
        shift
        cmd_logs "$@"
        ;;
    status)
        cmd_status
        ;;
    teardown)
        cmd_teardown
        ;;
    *)
        echo "IB Options Trading System — EC2 Deployment"
        echo ""
        echo "Usage: $0 {command}"
        echo ""
        echo "Commands:"
        echo "  deploy              Provision EC2 and install all dependencies"
        echo "  sync                Upload latest Python code"
        echo "  setup-cron          Install cron jobs for scheduled workflows"
        echo "  start-gateway       Start IB Gateway container"
        echo "  stop-gateway        Stop IB Gateway container"
        echo "  start-manager       Start position manager daemon"
        echo "  start-scheduler     Start workflow scheduler daemon"
        echo "  stop-all            Stop all services"
        echo "  ssh                 SSH into instance"
        echo "  logs [service]      Tail logs (manager|scheduler|premarket|weekly)"
        echo "  status              Show service status"
        echo "  teardown            Terminate and clean up"
        echo ""
        exit 1
        ;;
esac
