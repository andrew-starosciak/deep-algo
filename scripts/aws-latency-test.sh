#!/bin/bash
#
# AWS Deploy - Deploy algo-trade bot to eu-west-2 (London) for Polymarket trading
#
# Usage:
#   ./scripts/aws-latency-test.sh deploy              # Build, provision EC2, deploy binary
#   ./scripts/aws-latency-test.sh run [bot-args...]    # Run bot on remote instance
#   ./scripts/aws-latency-test.sh live [duration]      # Live trade with $1 FOK strategy
#   ./scripts/aws-latency-test.sh preflight            # Check auth, balance, markets
#   ./scripts/aws-latency-test.sh ssh                  # SSH into instance
#   ./scripts/aws-latency-test.sh logs                 # Tail remote logs
#   ./scripts/aws-latency-test.sh redeploy             # Rebuild and upload binary
#   ./scripts/aws-latency-test.sh latency              # Measure latency to Polymarket endpoints
#   ./scripts/aws-latency-test.sh status               # Show instance status
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

REGION="eu-west-1"
INSTANCE_TYPE="t3.micro"
KEY_NAME="algo-trade-latency-test"
SG_NAME="algo-trade-latency-sg"
INSTANCE_TAG="algo-trade-latency"

STATE_FILE="$SCRIPT_DIR/.aws-latency-test.state"
KEY_FILE="$SCRIPT_DIR/.aws-latency-key.pem"

SSH_USER="ubuntu"
SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

# =============================================================================
# Helpers
# =============================================================================

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[x]${NC} $*" >&2; }
dim()   { echo -e "${DIM}    $*${NC}"; }

load_state() {
    if [[ ! -f "$STATE_FILE" ]]; then
        error "No deployment found. Run 'deploy' first."
        exit 1
    fi
    # shellcheck disable=SC1090
    source "$STATE_FILE"
}

save_state() {
    cat > "$STATE_FILE" <<EOF
INSTANCE_ID=$INSTANCE_ID
KEY_NAME=$KEY_NAME
SECURITY_GROUP_ID=$SECURITY_GROUP_ID
PUBLIC_IP=$PUBLIC_IP
ALLOCATION_ID=$ALLOCATION_ID
REGION=$REGION
EOF
}

remote_ssh() {
    ssh $SSH_OPTS -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" "$@"
}

remote_scp() {
    scp $SSH_OPTS -i "$KEY_FILE" "$@"
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
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}AWS Latency Test - Deploy${NC}                                ${CYAN}║${NC}"
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

    SECURITY_GROUP_ID=$(aws ec2 create-security-group \
        --group-name "$SG_NAME" \
        --description "SSH access for algo-trade latency test" \
        --vpc-id "$vpc_id" \
        --region "$REGION" \
        --query 'GroupId' \
        --output text)

    aws ec2 authorize-security-group-ingress \
        --group-id "$SECURITY_GROUP_ID" \
        --protocol tcp \
        --port 22 \
        --cidr "${my_ip}/32" \
        --region "$REGION" >/dev/null

    dim "Security group: $SECURITY_GROUP_ID (SSH from $my_ip)"

    # Step 5: Launch instance
    info "Launching $INSTANCE_TYPE in $REGION..."
    INSTANCE_ID=$(aws ec2 run-instances \
        --image-id "$ami_id" \
        --instance-type "$INSTANCE_TYPE" \
        --key-name "$KEY_NAME" \
        --security-group-ids "$SECURITY_GROUP_ID" \
        --region "$REGION" \
        --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":20,"VolumeType":"gp3"}}]' \
        --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$INSTANCE_TAG}]" \
        --query 'Instances[0].InstanceId' \
        --output text)

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

    # Step 8: Deploy binary and config
    info "Deploying binary ($binary_size)..."
    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    info "Deploying .env..."
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"

    # Step 9: Quick connectivity test
    info "Testing Polymarket API connectivity from $REGION..."
    local latency
    latency=$(remote_ssh "curl -s -o /dev/null -w '%{time_total}' https://clob.polymarket.com/time" 2>/dev/null || echo "failed")
    dim "CLOB API round-trip: ${latency}s"

    # Done
    echo ""
    echo -e "${GREEN}═══════════════════════════════════════════════════════════════════${NC}"
    echo -e "${WHITE}Deployment complete!${NC}"
    echo ""
    echo -e "  ${DIM}Instance:${NC}  $INSTANCE_ID"
    echo -e "  ${DIM}Region:${NC}    $REGION"
    echo -e "  ${DIM}IP:${NC}        $PUBLIC_IP"
    echo -e "  ${DIM}Type:${NC}      $INSTANCE_TYPE"
    echo ""
    echo -e "${WHITE}Quick start:${NC}"
    echo -e "  ${CYAN}./scripts/aws-latency-test.sh run --mode observe --duration 1h --no-persist --verbose${NC}"
    echo ""
    echo -e "${WHITE}Other commands:${NC}"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh ssh${NC}        # SSH into instance"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh status${NC}     # Check instance status"
    echo -e "  ${DIM}./scripts/aws-latency-test.sh teardown${NC}   # Destroy everything"
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
    echo -e "${CYAN}║${NC}        ${WHITE}Live Trading — ${REGION} (${PUBLIC_IP})${NC}                      ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${DIM}Pair:${NC}       BTC/ETH"
    echo -e "  ${DIM}Combo:${NC}      coin1down_coin2up"
    echo -e "  ${DIM}Bet size:${NC}   \$1 per leg (FOK)"
    echo -e "  ${DIM}Max/window:${NC} \$10 / 10 trades"
    echo -e "  ${DIM}Duration:${NC}   ${duration}"
    echo ""

    local args=(
        --pair btc,eth
        --combination coin1down_coin2up
        --mode live
        --duration "$duration"
        --bet-size 1
        --min-spread 0.15
        --min-win-prob 0.85
        --max-loss-prob 0.50
        --max-position 10
        --max-trades-per-window 10
        --kelly-fraction 0.25
        --stats-interval-secs 1
    )

    dim "algo-trade cross-market-auto ${args[*]}"
    echo ""

    # Allocate a PTY so the dashboard renders correctly
    ssh $SSH_OPTS -t -i "$KEY_FILE" "$SSH_USER@$PUBLIC_IP" \
        "set -a && source ~/.env && set +a && RUST_LOG=info,algo_trade_polymarket::arbitrage::sdk_client=debug,algo_trade_polymarket::arbitrage::live_executor=debug,algo_trade_polymarket::arbitrage::execution=debug ~/algo-trade cross-market-auto ${args[*]}"
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
    info "Tailing logs on $PUBLIC_IP..."
    remote_ssh "tail -f /tmp/cross_market_auto.log 2>/dev/null || echo 'No log file found. Bot may not have run yet.'"
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

    remote_scp "$binary" "$SSH_USER@$PUBLIC_IP:~/algo-trade"
    remote_ssh "chmod +x ~/algo-trade"

    # Also sync .env in case it changed
    remote_scp "$PROJECT_ROOT/.env" "$SSH_USER@$PUBLIC_IP:~/.env"
    remote_ssh "chmod 600 ~/.env"

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
    preflight)
        cmd_preflight
        ;;
    ssh)
        cmd_ssh
        ;;
    logs)
        cmd_logs
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
    teardown)
        cmd_teardown
        ;;
    help|--help|-h)
        head -22 "$0" | tail -21
        ;;
    *)
        error "Unknown command: $1"
        echo "Usage: $0 {deploy|run|live|preflight|ssh|logs|redeploy|latency|status|teardown}"
        exit 1
        ;;
esac
