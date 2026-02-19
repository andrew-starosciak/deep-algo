#!/bin/bash
#
# IB Options Trading System — us-east-1
#
# Unified script for EC2 deployment and local development of the
# OpenClaw-orchestrated LLM-driven options trading system.
#
# Usage:
#   ./scripts/ib-options.sh <command>
#
# EC2 Commands:
#   deploy [--spot]          Provision EC2 + Postgres + Python + IB Gateway
#   redeploy                 Sync code + migrate + pip install + restart
#   sync                     Upload Python code only
#   teardown                 Terminate EC2 + cleanup AWS
#   ssh                      SSH into instance
#   start <svc|all>          Start EC2 service (gateway|scheduler|dashboard)
#   stop <svc|all>           Stop service(s)
#   restart <svc|all>        Restart
#   status                   Service health + DB stats + migrations
#   logs <svc>               Tail logs (manager|scheduler|premarket|weekly|dashboard|gateway)
#   setup-dashboard          One-time nginx + systemd setup
#   deploy-dashboard         Build frontend + upload + restart
#   setup-cron               Install cron jobs
#   db <query>               Run SQL on EC2
#
# Local Commands:
#   local                    Migrate + start + validate (default)
#   local start              Start all local services
#   local stop               Stop
#   local restart             Stop + start
#   local status             Health check
#   local logs <svc>         Tail local logs (scheduler|dashboard-api|dashboard-ui)
#   local migrate            Run migrations locally
#

set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# State files for this deployment
EC2_STATE_FILE="$SCRIPT_DIR/.ib-options.state"
EC2_KEY_FILE="$SCRIPT_DIR/.ib-options-key.pem"
export EC2_STATE_FILE EC2_KEY_FILE

# Migrate old state file names
if [[ ! -f "$EC2_STATE_FILE" && -f "$SCRIPT_DIR/.ib-options-deploy.state" ]]; then
    mv "$SCRIPT_DIR/.ib-options-deploy.state" "$EC2_STATE_FILE"
    echo "Migrated .ib-options-deploy.state -> .ib-options.state"
fi
if [[ ! -f "$EC2_KEY_FILE" && -f "$SCRIPT_DIR/.ib-options-deploy-key.pem" ]]; then
    mv "$SCRIPT_DIR/.ib-options-deploy-key.pem" "$EC2_KEY_FILE"
    echo "Migrated .ib-options-deploy-key.pem -> .ib-options-key.pem"
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/ec2-common.sh"

REGION="us-east-1"
INSTANCE_TYPE="t3.small"
KEY_NAME="ib-options-deploy"
SG_NAME="ib-options-sg"
INSTANCE_TAG="ib-options-trading"
PYTHON_VERSION="3.11"

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
    echo -e "${CYAN}|${NC}        ${WHITE}IB Options — EC2 Deploy${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}    $INSTANCE_TYPE ($INSTANCE_MARKET)"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo ""

    if [[ -f "$STATE_FILE" ]]; then
        warn "Deployment state file exists. Run 'teardown' first."
        load_state
        info "Existing deployment: $PUBLIC_IP ($INSTANCE_ID)"
        exit 1
    fi

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
    AMI_ID=$(aws ec2 describe-images \
        --region "$REGION" \
        --owners 099720109477 \
        --filters "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" \
        --query 'sort_by(Images, &CreationDate)[-1].ImageId' \
        --output text)

    if [[ "$INSTANCE_MARKET" == "spot" ]]; then
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

    PUBLIC_IP=$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$INSTANCE_ID" \
        --query 'Reservations[0].Instances[0].PublicIpAddress' \
        --output text)

    ALLOCATION_ID=""
    info "Instance running at $PUBLIC_IP"
    save_state
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
    remote_ssh "sed -i '/^DATABASE_URL=/d' ~/.env && echo 'DATABASE_URL=${REMOTE_DATABASE_URL}' >> ~/.env"

    sync_and_migrate
    remote_ssh "mkdir -p ~/logs"

    echo ""
    info "IB Options Trading System deployed to $PUBLIC_IP"
    echo ""
    echo -e "  ${DIM}Next steps:${NC}"
    echo -e "    ./scripts/ib-options.sh setup-cron"
    echo -e "    ./scripts/ib-options.sh start scheduler"
    echo -e "    ./scripts/ib-options.sh ssh"
    echo ""
}

# =============================================================================
# EC2: sync
# =============================================================================

cmd_sync() {
    load_state

    info "Syncing Python code to $PUBLIC_IP..."
    (
        cd "$PROJECT_ROOT"
        tar -czf /tmp/ib-options-python.tar.gz \
            -C python \
            --exclude='__pycache__' \
            --exclude='*.pyc' \
            --exclude='.pytest_cache' \
            .
    )

    remote_scp /tmp/ib-options-python.tar.gz "$SSH_USER@$PUBLIC_IP:~/python.tar.gz"
    remote_ssh "
        rm -rf ~/python-new
        mkdir -p ~/python-new
        tar -xzf ~/python.tar.gz -C ~/python-new
        rm ~/python.tar.gz
        rm -rf ~/python-old
        mv ~/python ~/python-old 2>/dev/null || true
        mv ~/python-new ~/python
        rm -rf ~/python-old
    "
    rm /tmp/ib-options-python.tar.gz
    info "Code synced."
}

# =============================================================================
# EC2: redeploy
# =============================================================================

cmd_redeploy() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Redeploy OpenClaw to ${PUBLIC_IP}${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    cmd_sync
    sync_and_migrate

    info "Updating Python dependencies..."
    remote_ssh "
        source venv/bin/activate
        cd ~/python
        pip install -e . 2>&1 | tail -3
    "

    info "Restarting services..."
    remote_ssh "
        if systemctl is-active --quiet ib-options-scheduler; then
            sudo systemctl restart ib-options-scheduler
            echo '  Restarted ib-options-scheduler'
        fi
        if systemctl is-active --quiet ib-options-manager; then
            sudo systemctl restart ib-options-manager
            echo '  Restarted ib-options-manager'
        fi
        if systemctl is-active --quiet dashboard-api; then
            sudo systemctl restart dashboard-api
            echo '  Restarted dashboard-api'
        fi
        sleep 2
    "

    info "Service status:"
    remote_ssh "
        systemctl is-active ib-options-scheduler 2>/dev/null && echo '  scheduler: running' || echo '  scheduler: not running'
        systemctl is-active ib-options-manager 2>/dev/null && echo '  manager: running' || echo '  manager: not running'
        systemctl is-active dashboard-api 2>/dev/null && echo '  dashboard-api: running' || echo '  dashboard-api: not running'
    "

    echo ""
    info "Redeploy complete."
    echo ""
}

# =============================================================================
# EC2: start / stop / restart
# =============================================================================

_ec2_service_action() {
    local action="$1"
    local svc="${2:-all}"

    load_state

    case "$svc" in
        gateway)
            case "$action" in
                start) _start_gateway ;;
                stop)  remote_ssh "docker rm -f ib-gateway 2>/dev/null || true"; info "IB Gateway stopped." ;;
                restart) remote_ssh "docker rm -f ib-gateway 2>/dev/null || true"; _start_gateway ;;
            esac
            ;;
        scheduler)
            case "$action" in
                start) _start_systemd_service "ib-options-scheduler" ;;
                stop)  _stop_systemd_service "ib-options-scheduler" ;;
                restart) _restart_systemd_service "ib-options-scheduler" ;;
            esac
            ;;
        manager)
            case "$action" in
                start) _start_systemd_service "ib-options-manager" ;;
                stop)  _stop_systemd_service "ib-options-manager" ;;
                restart) _restart_systemd_service "ib-options-manager" ;;
            esac
            ;;
        dashboard)
            case "$action" in
                start)
                    _start_systemd_service "dashboard-api"
                    remote_ssh "sudo systemctl reload nginx 2>/dev/null || true"
                    ;;
                stop)
                    _stop_systemd_service "dashboard-api"
                    ;;
                restart)
                    _restart_systemd_service "dashboard-api"
                    remote_ssh "sudo systemctl reload nginx 2>/dev/null || true"
                    ;;
            esac
            ;;
        all)
            case "$action" in
                start)
                    _start_systemd_service "ib-options-scheduler"
                    _start_systemd_service "ib-options-manager"
                    _start_systemd_service "dashboard-api"
                    ;;
                stop)
                    remote_ssh "
                        sudo systemctl stop ib-options-manager || true
                        sudo systemctl stop ib-options-scheduler || true
                        sudo systemctl stop dashboard-api || true
                    "
                    info "All services stopped."
                    ;;
                restart)
                    remote_ssh "
                        sudo systemctl restart ib-options-scheduler 2>/dev/null || true
                        sudo systemctl restart ib-options-manager 2>/dev/null || true
                        sudo systemctl restart dashboard-api 2>/dev/null || true
                        sleep 2
                    "
                    info "All services restarted."
                    ;;
            esac
            ;;
        *)
            error "Unknown service: '$svc'"
            echo "Available: gateway, scheduler, manager, dashboard, all"
            exit 1
            ;;
    esac
}

_start_systemd_service() {
    local unit="$1"
    remote_ssh "
        sudo systemctl start $unit 2>/dev/null
        sleep 2
        sudo systemctl status $unit --no-pager | head -5
    "
}

_stop_systemd_service() {
    local unit="$1"
    remote_ssh "sudo systemctl stop $unit 2>/dev/null || true"
    info "Stopped $unit."
}

_restart_systemd_service() {
    local unit="$1"
    remote_ssh "
        sudo systemctl restart $unit 2>/dev/null
        sleep 2
        sudo systemctl status $unit --no-pager | head -5
    "
}

_start_gateway() {
    if ! grep -q "IBKR_USERNAME" "$PROJECT_ROOT/.env" 2>/dev/null; then
        error "IBKR_USERNAME not found in .env"
        exit 1
    fi

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
            -e RELOGIN_AFTER_TWOFA_TIMEOUT=yes \
            -e READ_ONLY_API=no \
            -e BYPASS_WARNING=yes \
            -e VNC_SERVER_PASSWORD=ibgateway \
            -e AUTO_RESTART_TIME='02:00 AM' \
            -e EXISTING_SESSION_DETECTED_ACTION=primaryoverride \
            -p 127.0.0.1:4001:4003 \
            -p 127.0.0.1:4002:4004 \
            -p 127.0.0.1:5900:5900 \
            --ulimit nofile=10000:10000 \
            --restart unless-stopped \
            --health-cmd=\"timeout 1 bash -c '</dev/tcp/0.0.0.0/4004' && echo up || exit 1\" \
            --health-interval=10s \
            --health-start-period=60s \
            --health-retries=5 \
            ghcr.io/gnzsnz/ib-gateway:latest
    "

    info "IB Gateway starting... Check IBKR Mobile for 2FA!"
    info "Waiting for health check..."
    sleep 10

    for i in {1..12}; do
        health_status=$(remote_ssh "docker inspect --format='{{.State.Health.Status}}' ib-gateway 2>/dev/null || echo none")
        if [ "$health_status" = "healthy" ]; then
            info "IB Gateway is healthy!"
            break
        elif [ "$health_status" = "unhealthy" ]; then
            warn "IB Gateway health check failed."
            break
        fi
        info "Health: $health_status ($i/12)"
        sleep 5
    done
}

# =============================================================================
# EC2: setup-cron
# =============================================================================

cmd_setup_cron() {
    load_state

    info "Setting up cron jobs on $PUBLIC_IP..."
    remote_ssh "mkdir -p ~/scripts"

    remote_ssh "cat > ~/scripts/run-premarket.sh <<'SCRIPT'
#!/bin/bash
set -a && source ~/.env && set +a
cd ~/python
~/venv/bin/python -m openclaw --db-url \$DATABASE_URL run trade-thesis --ticker AAPL
SCRIPT
chmod +x ~/scripts/run-premarket.sh"

    remote_ssh "cat > ~/scripts/run-weekly-deep-dive.sh <<'SCRIPT'
#!/bin/bash
set -a && source ~/.env && set +a
cd ~/python
for ticker in AAPL NVDA MSFT GOOGL META AMZN TSLA AMD; do
    ~/venv/bin/python -m openclaw research \$ticker >> ~/logs/weekly-\$(date +%Y%m%d).log 2>&1
done
SCRIPT
chmod +x ~/scripts/run-weekly-deep-dive.sh"

    remote_ssh "cat > ~/crontab <<'CRON'
# IB Options Trading Workflows (ET = UTC-5 or UTC-4 during DST)
0 13 * * 1-5 /home/ubuntu/scripts/run-premarket.sh >> /home/ubuntu/logs/premarket.log 2>&1
0 15 * * 6 /home/ubuntu/scripts/run-weekly-deep-dive.sh >> /home/ubuntu/logs/weekly.log 2>&1
0 2 * * * find /home/ubuntu/logs -name '*.log' -mtime +7 -delete
CRON
crontab ~/crontab"

    info "Cron jobs installed."
}

# =============================================================================
# EC2: setup-dashboard / deploy-dashboard
# =============================================================================

cmd_setup_dashboard() {
    load_state

    info "Setting up dashboard on $PUBLIC_IP..."
    remote_ssh "sudo apt-get install -y nginx 2>&1 | tail -3"

    remote_scp "$SCRIPT_DIR/dashboard/nginx.conf" "$SSH_USER@$PUBLIC_IP:/tmp/dashboard-nginx.conf"
    remote_ssh "
        sudo rm -f /etc/nginx/sites-enabled/default
        sudo cp /tmp/dashboard-nginx.conf /etc/nginx/sites-available/dashboard
        sudo ln -sf /etc/nginx/sites-available/dashboard /etc/nginx/sites-enabled/dashboard
        sudo nginx -t
        sudo systemctl enable nginx
        sudo systemctl restart nginx
    "

    remote_scp "$SCRIPT_DIR/dashboard/dashboard-api.service" "$SSH_USER@$PUBLIC_IP:/tmp/dashboard-api.service"
    remote_ssh "
        sudo cp /tmp/dashboard-api.service /etc/systemd/system/dashboard-api.service
        sudo systemctl daemon-reload
        sudo systemctl enable dashboard-api
    "

    remote_ssh "mkdir -p ~/dashboard-frontend"

    remote_ssh "
        if ! grep -q '^DASHBOARD_TOKEN=' ~/.env 2>/dev/null; then
            TOKEN=\$(python3 -c 'import secrets; print(secrets.token_hex(32))')
            echo \"DASHBOARD_TOKEN=\$TOKEN\" >> ~/.env
            echo \"Generated DASHBOARD_TOKEN: \$TOKEN\"
        else
            echo 'DASHBOARD_TOKEN already set'
        fi
    "

    info "Opening port 80..."
    aws ec2 authorize-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp \
        --port 80 \
        --cidr 0.0.0.0/0 \
        --region "$REGION" 2>/dev/null || info "Port 80 already open"

    echo ""
    info "Dashboard ready. Next: ./scripts/ib-options.sh deploy-dashboard"
    echo ""
}

cmd_deploy_dashboard() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}Deploy Dashboard to ${PUBLIC_IP}${NC}"
    echo -e "${CYAN}+==================================================================+${NC}"
    echo ""

    info "Building frontend..."
    (
        cd "$PROJECT_ROOT/dashboard"
        NEXT_PUBLIC_API_URL="" npm run build 2>&1 | tail -5
    )

    info "Uploading frontend..."
    (
        cd "$PROJECT_ROOT/dashboard"
        tar -czf /tmp/dashboard-frontend.tar.gz -C out .
    )
    remote_scp /tmp/dashboard-frontend.tar.gz "$SSH_USER@$PUBLIC_IP:~/dashboard-frontend.tar.gz"
    remote_ssh "
        rm -rf ~/dashboard-frontend/*
        tar -xzf ~/dashboard-frontend.tar.gz -C ~/dashboard-frontend
        rm ~/dashboard-frontend.tar.gz
    "
    rm /tmp/dashboard-frontend.tar.gz

    cmd_sync

    info "Updating Python dependencies..."
    remote_ssh "
        source venv/bin/activate
        cd ~/python
        pip install -e . 2>&1 | tail -3
    "

    info "Restarting dashboard-api..."
    remote_ssh "
        sudo systemctl restart dashboard-api
        sleep 2
        sudo systemctl status dashboard-api --no-pager | head -5
    "
    remote_ssh "sudo systemctl reload nginx"

    echo ""
    info "Dashboard deployed to http://$PUBLIC_IP"
    echo ""
}

# =============================================================================
# EC2: status
# =============================================================================

cmd_status() {
    load_state

    echo -e "${CYAN}+==================================================================+${NC}"
    echo -e "${CYAN}|${NC}        ${WHITE}IB Options — Status${NC}  (${DIM}${PUBLIC_IP}${NC})"
    echo -e "${CYAN}+==================================================================+${NC}"
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

    info "Dashboard API:"
    remote_ssh "sudo systemctl status dashboard-api --no-pager 2>/dev/null || echo 'Not running'"
    echo ""

    info "Nginx:"
    remote_ssh "sudo systemctl status nginx --no-pager 2>/dev/null || echo 'Not running'"
    echo ""

    info "Cron jobs:"
    remote_ssh "crontab -l 2>/dev/null | grep -v '^#' || echo 'No cron jobs'"
    echo ""

    info "Recent log activity:"
    remote_ssh "ls -lht ~/logs/*.log 2>/dev/null | head -5 || echo 'No logs found'"
}

# =============================================================================
# EC2: logs
# =============================================================================

cmd_logs() {
    load_state

    local log_type="${1:-scheduler}"
    local log_file

    case "$log_type" in
        manager)    log_file="~/logs/manager.log" ;;
        scheduler)  log_file="~/logs/scheduler.log" ;;
        premarket)  log_file="~/logs/premarket.log" ;;
        weekly)     log_file="~/logs/weekly.log" ;;
        dashboard)  log_file="~/logs/dashboard-api.log" ;;
        gateway)    remote_ssh "docker logs -f ib-gateway 2>&1"; return ;;
        *)
            error "Unknown log type: $log_type"
            echo "Available: manager, scheduler, premarket, weekly, dashboard, gateway"
            exit 1
            ;;
    esac

    info "Tailing $log_type logs (Ctrl+C to stop)..."
    remote_ssh "tail -f $log_file 2>/dev/null || echo 'No log file found at $log_file'"
}

# =============================================================================
# EC2: db
# =============================================================================

cmd_db() {
    load_state

    local query="$*"
    if [[ -z "$query" ]]; then
        error "Usage: ib-options.sh db <sql-query>"
        exit 1
    fi
    remote_ssh "PGPASSWORD='algo_trade_local' psql -h localhost -U algo -d algo_trade -c \"$query\""
}

# =============================================================================
# EC2: teardown
# =============================================================================

cmd_teardown() {
    if [[ ! -f "$STATE_FILE" ]]; then
        error "No deployment state file found."
        exit 1
    fi

    load_state

    echo -e "${RED}+==================================================================+${NC}"
    echo -e "${RED}|${NC}        ${WHITE}WARNING: Tearing Down IB Options Trading System${NC}"
    echo -e "${RED}+==================================================================+${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}    $PUBLIC_IP ($INSTANCE_ID)"
    echo -e "  ${DIM}Region:${NC}      $REGION"
    echo ""

    read -p "Are you sure you want to terminate and delete everything? (yes/no): " confirm
    if [[ "$confirm" != "yes" ]]; then
        info "Aborted."
        exit 0
    fi

    _ec2_service_action stop all || true

    info "Terminating instance $INSTANCE_ID..."
    aws ec2 terminate-instances --region "$REGION" --instance-ids "$INSTANCE_ID" >/dev/null
    aws ec2 wait instance-terminated --region "$REGION" --instance-ids "$INSTANCE_ID" 2>/dev/null || true

    info "Deleting security group..."
    aws ec2 delete-security-group --region "$REGION" --group-id "$SECURITY_GROUP_ID" 2>/dev/null || true

    info "Deleting key pair..."
    aws ec2 delete-key-pair --region "$REGION" --key-name "$KEY_NAME" 2>/dev/null || true
    rm -f "$KEY_FILE"
    rm -f "$STATE_FILE"

    info "Teardown complete."
}

# =============================================================================
# Local: all subcommands
# =============================================================================

_local_dispatch() {
    local subcmd="${1:-deploy}"
    shift 2>/dev/null || true

    # Load .env for local commands
    if [[ -f "$PROJECT_ROOT/.env" ]]; then
        set -a
        # shellcheck disable=SC1091
        source "$PROJECT_ROOT/.env"
        set +a
    fi

    local LOG_DIR="$PROJECT_ROOT/logs"
    local PID_DIR="$PROJECT_ROOT/.pids"
    local DATABASE_URL="${DATABASE_URL:-postgres://postgres:changeme_secure_password@localhost:5432/algo_trade}"

    mkdir -p "$LOG_DIR" "$PID_DIR"

    # Local helper functions
    _pid_file() { echo "$PID_DIR/$1.pid"; }
    _read_pid() { local f; f="$(_pid_file "$1")"; [[ -f "$f" ]] && cat "$f" || echo ""; }
    _is_running() { local pid; pid="$(_read_pid "$1")"; [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; }

    _stop_svc() {
        local name="$1"
        local pid
        pid="$(_read_pid "$name")"
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in {1..10}; do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
            kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
            echo -e "  ${GREEN}Stopped${NC} $name (pid $pid)"
        fi
        rm -f "$(_pid_file "$name")"
    }

    _wait_for_port() {
        local port="$1" label="$2" timeout="${3:-15}"
        local elapsed=0
        while ! (echo > /dev/tcp/127.0.0.1/"$port") 2>/dev/null; do
            sleep 1
            elapsed=$((elapsed + 1))
            if [[ $elapsed -ge $timeout ]]; then
                echo -e "  ${RED}$label not responding on port $port${NC}"
                return 1
            fi
        done
    }

    case "$subcmd" in
        deploy|up|"")
            _local_migrate
            _local_start
            _local_status
            ;;
        start)   _local_start ;;
        stop|down) _local_stop ;;
        restart) _local_stop; _local_start; _local_status ;;
        status|health) _local_status ;;
        logs)    _local_logs "$@" ;;
        migrate) _local_migrate ;;
        *)
            echo "Local commands: start, stop, restart, status, logs <svc>, migrate"
            exit 1
            ;;
    esac
}

_local_migrate() {
    echo ""
    echo -e "  ${CYAN}--- Migrations ---${NC}"

    local DATABASE_URL="${DATABASE_URL:-postgres://postgres:changeme_secure_password@localhost:5432/algo_trade}"

    if ! psql "$DATABASE_URL" -c "SELECT 1" &>/dev/null; then
        echo -e "  ${RED}Cannot connect to PostgreSQL${NC}"
        return 1
    fi
    echo -e "  ${GREEN}PostgreSQL connected${NC}"

    psql "$DATABASE_URL" -q -c "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            filename TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ DEFAULT NOW()
        );
    "

    local applied=0
    for migration in "$SCRIPT_DIR"/migrations/V*.sql; do
        [[ -f "$migration" ]] || continue
        local fname
        fname="$(basename "$migration")"
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
                return 1
            fi
        fi
    done

    if [[ $applied -eq 0 ]]; then
        echo -e "  ${GREEN}All migrations up to date${NC}"
    else
        echo -e "  ${GREEN}Applied $applied migration(s)${NC}"
    fi
}

_local_start() {
    local LOG_DIR="$PROJECT_ROOT/logs"
    local PID_DIR="$PROJECT_ROOT/.pids"
    local DATABASE_URL="${DATABASE_URL:-postgres://postgres:changeme_secure_password@localhost:5432/algo_trade}"

    echo ""
    echo -e "  ${CYAN}--- Starting services ---${NC}"

    # 1. TimescaleDB
    echo ""
    echo -e "  ${DIM}[1/5] TimescaleDB${NC}"
    local db_container
    db_container=$(docker ps --filter "name=algo-trade-db" --format '{{.Names}}' 2>/dev/null || true)
    if [[ -n "$db_container" ]]; then
        echo -e "  ${GREEN}Already running${NC}"
    elif docker start algo-trade-db &>/dev/null; then
        echo -e "  ${GREEN}Started existing container${NC}"
    else
        (cd "$PROJECT_ROOT" && docker compose up -d timescaledb 2>&1 | tail -1)
        echo -e "  ${GREEN}Started via docker compose${NC}"
    fi
    _wait_for_port 5432 "TimescaleDB" 20 || return 1

    # 2. IB Gateway
    echo -e "  ${DIM}[2/5] IB Gateway${NC}"
    local gw_container
    gw_container=$(docker ps --filter "name=ib-gateway" --format '{{.Names}}' 2>/dev/null || true)
    if [[ -n "$gw_container" ]]; then
        echo -e "  ${GREEN}Already running${NC}"
    elif docker start ib-gateway &>/dev/null; then
        echo -e "  ${GREEN}Started existing container${NC}"
    else
        echo -e "  ${YELLOW}No ib-gateway container — start manually or via docker compose${NC}"
    fi

    # Python deps
    echo -e "  ${DIM}[deps] Python packages${NC}"
    (
        cd "$PROJECT_ROOT/python"
        source .venv/bin/activate
        pip install -e . -q 2>&1 | grep -v "already satisfied" | tail -3
        deactivate 2>/dev/null || true
    )
    echo -e "  ${GREEN}Python environment ready${NC}"

    # 3. OpenClaw Scheduler
    echo -e "  ${DIM}[3/5] OpenClaw Scheduler${NC}"
    if _is_running scheduler; then
        echo -e "  ${GREEN}Already running (pid $(_read_pid scheduler))${NC}"
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

        echo "$SCHED_PID" > "$PID_DIR/scheduler.pid"
        sleep 2
        if kill -0 "$SCHED_PID" 2>/dev/null; then
            echo -e "  ${GREEN}Started (pid $SCHED_PID, mode=$ib_mode)${NC}"
        else
            echo -e "  ${RED}Failed to start${NC}"
        fi
    fi

    # 4. Dashboard API
    echo -e "  ${DIM}[4/5] Dashboard API${NC}"
    if _is_running dashboard-api; then
        echo -e "  ${GREEN}Already running (pid $(_read_pid dashboard-api))${NC}"
    elif (echo > /dev/tcp/127.0.0.1/8000) 2>/dev/null; then
        echo -e "  ${YELLOW}Port 8000 already in use${NC}"
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

        echo "$API_PID" > "$PID_DIR/dashboard-api.pid"
        sleep 2
        if kill -0 "$API_PID" 2>/dev/null; then
            echo -e "  ${GREEN}Started (pid $API_PID, port 8000)${NC}"
        else
            echo -e "  ${RED}Failed to start${NC}"
        fi
    fi

    # 5. Dashboard UI
    echo -e "  ${DIM}[5/5] Dashboard UI${NC}"
    if _is_running dashboard-ui; then
        echo -e "  ${GREEN}Already running (pid $(_read_pid dashboard-ui))${NC}"
    elif (echo > /dev/tcp/127.0.0.1/3000) 2>/dev/null; then
        echo -e "  ${GREEN}Already running on port 3000${NC}"
    else
        cd "$PROJECT_ROOT/dashboard"
        nohup npx next dev --port 3000 \
            >> "$LOG_DIR/dashboard-ui.log" 2>&1 &
        local UI_PID=$!
        cd "$PROJECT_ROOT"

        echo "$UI_PID" > "$PID_DIR/dashboard-ui.pid"
        sleep 3
        if kill -0 "$UI_PID" 2>/dev/null; then
            echo -e "  ${GREEN}Started (pid $UI_PID, port 3000)${NC}"
        else
            echo -e "  ${RED}Failed to start${NC}"
        fi
    fi

    echo ""
}

_local_stop() {
    echo ""
    echo -e "  ${CYAN}--- Stopping services ---${NC}"
    _stop_svc dashboard-ui
    _stop_svc dashboard-api
    _stop_svc scheduler
    echo -e "  ${DIM}Docker containers left running${NC}"
    echo ""
}

_local_status() {
    local LOG_DIR="$PROJECT_ROOT/logs"
    local PID_DIR="$PROJECT_ROOT/.pids"
    local DATABASE_URL="${DATABASE_URL:-postgres://postgres:changeme_secure_password@localhost:5432/algo_trade}"

    echo ""
    echo -e "  ${CYAN}+============================================================+${NC}"
    echo -e "  ${CYAN}|${NC}        OpenClaw Local Services — Status                  ${CYAN}|${NC}"
    echo -e "  ${CYAN}+============================================================+${NC}"
    echo ""

    # TimescaleDB
    echo -n "  TimescaleDB        "
    if docker ps --filter "name=algo-trade-db" --format '{{.Status}}' 2>/dev/null | grep -q "Up"; then
        echo -e "${GREEN}running${NC}  ($(docker ps --filter "name=algo-trade-db" --format '{{.Status}}'))"
    else
        echo -e "${RED}stopped${NC}"
    fi

    # IB Gateway
    echo -n "  IB Gateway         "
    if docker ps --filter "name=ib-gateway" --format '{{.Status}}' 2>/dev/null | grep -q "Up"; then
        echo -e "${GREEN}running${NC}  ($(docker ps --filter "name=ib-gateway" --format '{{.Status}}'))"
    else
        echo -e "${YELLOW}stopped${NC}  (scheduler will use sim mode)"
    fi

    # Scheduler
    echo -n "  Scheduler          "
    if _is_running scheduler; then
        echo -e "${GREEN}running${NC}  (pid $(_read_pid scheduler))"
    else
        echo -e "${RED}stopped${NC}"
    fi

    # Dashboard API
    echo -n "  Dashboard API      "
    if _is_running dashboard-api; then
        if curl -sf http://127.0.0.1:8000/api/health >/dev/null 2>&1; then
            echo -e "${GREEN}running${NC}  (pid $(_read_pid dashboard-api), http://localhost:8000)"
        else
            echo -e "${YELLOW}starting${NC}  (pid $(_read_pid dashboard-api))"
        fi
    else
        echo -e "${RED}stopped${NC}"
    fi

    # Dashboard UI
    echo -n "  Dashboard UI       "
    if _is_running dashboard-ui; then
        echo -e "${GREEN}running${NC}  (pid $(_read_pid dashboard-ui), http://localhost:3000)"
    elif (echo > /dev/tcp/127.0.0.1/3000) 2>/dev/null; then
        echo -e "${GREEN}running${NC}  (http://localhost:3000)"
    else
        echo -e "${RED}stopped${NC}"
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

    local latest_applied latest_available
    latest_applied=$(psql "$DATABASE_URL" -tAc "SELECT filename FROM schema_migrations ORDER BY filename DESC LIMIT 1" 2>/dev/null || echo "?")
    latest_available=$(ls "$SCRIPT_DIR"/migrations/V*.sql 2>/dev/null | sort | tail -1 | xargs basename 2>/dev/null || echo "?")
    echo -e "  ${DIM}Migrations:${NC} applied=$latest_applied | available=$latest_available"
    echo ""
}

_local_logs() {
    local LOG_DIR="$PROJECT_ROOT/logs"
    local service="${1:-scheduler}"
    local log_file

    case "$service" in
        scheduler)         log_file="$LOG_DIR/scheduler.log" ;;
        dashboard-api|api) log_file="$LOG_DIR/dashboard-api.log" ;;
        dashboard-ui|ui)   log_file="$LOG_DIR/dashboard-ui.log" ;;
        *)
            echo "Usage: ib-options.sh local logs {scheduler|dashboard-api|dashboard-ui}"
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
# Main dispatcher
# =============================================================================

case "${1:-}" in
    deploy)         shift; cmd_deploy "$@" ;;
    redeploy)       cmd_redeploy ;;
    sync)           cmd_sync ;;
    teardown)       cmd_teardown ;;
    ssh)            load_state; ec2_ssh_interactive ;;
    start)          shift; _ec2_service_action start "${1:-all}" ;;
    stop)           shift; _ec2_service_action stop "${1:-all}" ;;
    restart)        shift; _ec2_service_action restart "${1:-all}" ;;
    status)         cmd_status ;;
    logs)           shift; cmd_logs "$@" ;;
    setup-dashboard) cmd_setup_dashboard ;;
    deploy-dashboard) cmd_deploy_dashboard ;;
    setup-cron)     cmd_setup_cron ;;
    db)             shift; cmd_db "$@" ;;
    local)          shift; _local_dispatch "$@" ;;
    *)
        echo "IB Options Trading System"
        echo ""
        echo "Usage: $0 <command>"
        echo ""
        echo "EC2 Commands:"
        echo "  deploy [--spot]          Provision EC2 + Postgres + Python + IB Gateway"
        echo "  redeploy                 Sync code + migrate + pip install + restart"
        echo "  sync                     Upload Python code only"
        echo "  teardown                 Terminate EC2 + cleanup AWS"
        echo "  ssh                      SSH into instance"
        echo "  start <svc|all>          Start service (gateway|scheduler|manager|dashboard|all)"
        echo "  stop <svc|all>           Stop service(s)"
        echo "  restart <svc|all>        Restart service(s)"
        echo "  status                   Service health + DB stats"
        echo "  logs <svc>               Tail logs (manager|scheduler|premarket|weekly|dashboard|gateway)"
        echo "  setup-dashboard          One-time nginx + systemd setup"
        echo "  deploy-dashboard         Build frontend + upload + restart"
        echo "  setup-cron               Install cron jobs"
        echo "  db <query>               Run SQL on EC2"
        echo ""
        echo "Local Commands:"
        echo "  local                    Migrate + start + validate (default)"
        echo "  local start              Start all local services"
        echo "  local stop               Stop all"
        echo "  local restart            Stop + start"
        echo "  local status             Health check"
        echo "  local logs <svc>         Tail logs (scheduler|dashboard-api|dashboard-ui)"
        echo "  local migrate            Run migrations only"
        echo ""
        exit 1
        ;;
esac
