#!/bin/bash
#
# AWS Resource & Cost Report
#
# Usage:
#   ./scripts/aws-report.sh              # Full report (eu-west-1)
#   ./scripts/aws-report.sh costs        # Cost breakdown (last 7 days)
#   ./scripts/aws-report.sh resources    # Resource inventory only
#

set -euo pipefail

REGION="eu-west-1"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
DIM='\033[2m'
NC='\033[0m'

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
dim()   { echo -e "${DIM}    $*${NC}"; }

# =============================================================================
# resources
# =============================================================================

cmd_resources() {
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}AWS Resource Inventory (${REGION})${NC}                           ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    # EC2 Instances
    info "EC2 Instances"
    local instances
    instances=$(aws ec2 describe-instances \
        --region "$REGION" \
        --filters "Name=instance-state-name,Values=running,stopped,pending,stopping" \
        --query 'Reservations[*].Instances[*].[InstanceId,InstanceType,State.Name,LaunchTime,Tags[?Key==`Name`].Value|[0],InstanceLifecycle||`on-demand`]' \
        --output text 2>/dev/null || true)

    if [[ -n "$instances" ]]; then
        echo "$instances" | while IFS=$'\t' read -r id type state launched name lifecycle; do
            local cost=""
            if [[ "$state" == "running" ]]; then
                if [[ "$lifecycle" == "spot" ]]; then
                    cost="${DIM}(~\$2.30/mo spot)${NC}"
                else
                    cost="${DIM}(~\$7.60/mo on-demand)${NC}"
                fi
            elif [[ "$state" == "stopped" ]]; then
                cost="${DIM}(\$0 compute, EBS still charges)${NC}"
            fi
            echo -e "    ${WHITE}${id}${NC}  ${type}  ${state}  ${name:-unnamed}  ${lifecycle}  ${cost}"
            dim "Launched: ${launched}"
        done
    else
        dim "No active instances"
    fi
    echo ""

    # Elastic IPs
    info "Elastic IPs"
    local eips
    eips=$(aws ec2 describe-addresses \
        --region "$REGION" \
        --query 'Addresses[*].[PublicIp,AllocationId,InstanceId||`unattached`]' \
        --output text 2>/dev/null || true)

    if [[ -n "$eips" ]]; then
        echo "$eips" | while IFS=$'\t' read -r ip alloc instance; do
            local cost="${DIM}(~\$3.60/mo)${NC}"
            local status=""
            if [[ "$instance" == "unattached" ]]; then
                status="${YELLOW}UNATTACHED${NC} "
            fi
            echo -e "    ${WHITE}${ip}${NC}  ${alloc}  ${status}${cost}"
        done
    else
        dim "No elastic IPs"
    fi
    echo ""

    # EBS Volumes
    info "EBS Volumes"
    local volumes
    volumes=$(aws ec2 describe-volumes \
        --region "$REGION" \
        --query 'Volumes[*].[VolumeId,Size,State,VolumeType,Attachments[0].InstanceId||`detached`]' \
        --output text 2>/dev/null || true)

    if [[ -n "$volumes" ]]; then
        echo "$volumes" | while IFS=$'\t' read -r vid size state vtype instance; do
            local cost
            cost=$(echo "$size" | awk '{printf "~$%.2f/mo", $1 * 0.08}')
            local status=""
            if [[ "$instance" == "detached" ]]; then
                status="${YELLOW}DETACHED${NC} "
            fi
            echo -e "    ${WHITE}${vid}${NC}  ${size}GB ${vtype}  ${state}  ${status}${DIM}(${cost})${NC}"
        done
    else
        dim "No volumes"
    fi
    echo ""

    # Spot Requests
    info "Spot Instance Requests"
    local spots
    spots=$(aws ec2 describe-spot-instance-requests \
        --region "$REGION" \
        --filters "Name=state,Values=open,active" \
        --query 'SpotInstanceRequests[*].[SpotInstanceRequestId,State,InstanceId||`none`,Type]' \
        --output text 2>/dev/null || true)

    if [[ -n "$spots" ]]; then
        echo "$spots" | while IFS=$'\t' read -r rid state inst type; do
            echo -e "    ${WHITE}${rid}${NC}  ${state}  instance:${inst}  ${type}"
        done
    else
        dim "No active spot requests"
    fi
    echo ""

    # Security Groups (non-default)
    info "Security Groups (non-default)"
    local sgs
    sgs=$(aws ec2 describe-security-groups \
        --region "$REGION" \
        --query 'SecurityGroups[?GroupName!=`default`].[GroupName,GroupId,Description]' \
        --output text 2>/dev/null || true)

    if [[ -n "$sgs" ]]; then
        echo "$sgs" | while IFS=$'\t' read -r name gid desc; do
            echo -e "    ${WHITE}${name}${NC}  ${gid}  ${DIM}${desc}${NC}"
        done
    else
        dim "None (only default)"
    fi
    echo ""

    # Key Pairs
    info "Key Pairs"
    aws ec2 describe-key-pairs \
        --region "$REGION" \
        --query 'KeyPairs[*].[KeyName,KeyPairId]' \
        --output text 2>/dev/null | while IFS=$'\t' read -r name kid; do
        echo -e "    ${WHITE}${name}${NC}  ${kid}"
    done || dim "None"
    echo ""
}

# =============================================================================
# costs
# =============================================================================

cmd_costs() {
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}AWS Cost Report (Last 7 Days)${NC}                              ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    local end_date
    end_date=$(date -u +%Y-%m-%d)
    local start_date
    start_date=$(date -u -d "7 days ago" +%Y-%m-%d 2>/dev/null || date -u -v-7d +%Y-%m-%d)

    info "Daily costs by service ($start_date to $end_date)"
    echo ""

    aws ce get-cost-and-usage \
        --time-period "Start=${start_date},End=${end_date}" \
        --granularity DAILY \
        --metrics BlendedCost \
        --group-by Type=DIMENSION,Key=SERVICE \
        --query 'ResultsByTime[*].[TimePeriod.Start,Groups[*].[Keys[0],Metrics.BlendedCost.Amount]]' \
        --output json 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
totals = {}
for day_data in data:
    day = day_data[0]
    for group in day_data[1]:
        service = group[0]
        cost = float(group[1])
        if cost > 0.001:
            totals[service] = totals.get(service, 0) + cost

# Print by service
print('    {:<40} {:>10}'.format('Service', '7-Day Cost'))
print('    ' + '-' * 52)
grand_total = 0
for svc, cost in sorted(totals.items(), key=lambda x: -x[1]):
    print('    {:<40} \${:>9.2f}'.format(svc[:40], cost))
    grand_total += cost
print('    ' + '-' * 52)
print('    {:<40} \${:>9.2f}'.format('TOTAL', grand_total))
print()
monthly = grand_total / 7 * 30
print('    Projected monthly: ~\${:.2f}'.format(monthly))
" 2>/dev/null || warn "Cost Explorer not available (requires billing access or may take 24h for new accounts)"

    echo ""

    # Current month total
    local month_start
    month_start=$(date -u +%Y-%m-01)
    info "Current month total ($month_start to $end_date)"

    aws ce get-cost-and-usage \
        --time-period "Start=${month_start},End=${end_date}" \
        --granularity MONTHLY \
        --metrics BlendedCost \
        --query 'ResultsByTime[0].Total.BlendedCost.Amount' \
        --output text 2>/dev/null | while read -r amount; do
        echo -e "    ${WHITE}\$${amount}${NC}"
    done || dim "Not available"
    echo ""
}

# =============================================================================
# estimate
# =============================================================================

cmd_estimate() {
    echo -e "${CYAN}╔══════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}        ${WHITE}Monthly Cost Estimate (${REGION})${NC}                            ${CYAN}║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    local total="0"

    # Count running instances
    local instance_count
    instance_count=$(aws ec2 describe-instances \
        --region "$REGION" \
        --filters "Name=instance-state-name,Values=running" \
        --query 'length(Reservations[*].Instances[*][])' \
        --output text 2>/dev/null || echo "0")

    local spot_count
    spot_count=$(aws ec2 describe-instances \
        --region "$REGION" \
        --filters "Name=instance-state-name,Values=running" "Name=instance-lifecycle,Values=spot" \
        --query 'length(Reservations[*].Instances[*][])' \
        --output text 2>/dev/null || echo "0")

    local ondemand_count=$((instance_count - spot_count))

    if [[ "$spot_count" -gt 0 ]]; then
        local spot_cost
        spot_cost=$(echo "$spot_count" | awk '{printf "%.2f", $1 * 2.30}')
        echo -e "    ${WHITE}EC2 spot (t3.micro x${spot_count})${NC}          \$${spot_cost}/mo"
        total=$(echo "$total $spot_cost" | awk '{printf "%.2f", $1 + $2}')
    fi

    if [[ "$ondemand_count" -gt 0 ]]; then
        local od_cost
        od_cost=$(echo "$ondemand_count" | awk '{printf "%.2f", $1 * 7.60}')
        echo -e "    ${WHITE}EC2 on-demand (t3.micro x${ondemand_count})${NC}    \$${od_cost}/mo"
        total=$(echo "$total $od_cost" | awk '{printf "%.2f", $1 + $2}')
    fi

    # EBS total
    local ebs_gb
    ebs_gb=$(aws ec2 describe-volumes \
        --region "$REGION" \
        --query 'sum(Volumes[*].Size)' \
        --output text 2>/dev/null || echo "0")

    if [[ "$ebs_gb" != "0" && "$ebs_gb" != "None" ]]; then
        local ebs_cost
        ebs_cost=$(echo "$ebs_gb" | awk '{printf "%.2f", $1 * 0.08}')
        echo -e "    ${WHITE}EBS (${ebs_gb}GB gp3)${NC}                   \$${ebs_cost}/mo"
        total=$(echo "$total $ebs_cost" | awk '{printf "%.2f", $1 + $2}')
    fi

    # Elastic IPs
    local eip_count
    eip_count=$(aws ec2 describe-addresses \
        --region "$REGION" \
        --query 'length(Addresses)' \
        --output text 2>/dev/null || echo "0")

    if [[ "$eip_count" -gt 0 ]]; then
        local eip_cost
        eip_cost=$(echo "$eip_count" | awk '{printf "%.2f", $1 * 3.60}')
        echo -e "    ${WHITE}Public IPv4 (x${eip_count})${NC}                 \$${eip_cost}/mo"
        total=$(echo "$total $eip_cost" | awk '{printf "%.2f", $1 + $2}')
    fi

    echo -e "    ────────────────────────────────────"
    echo -e "    ${WHITE}Estimated total${NC}                  ${WHITE}\$${total}/mo${NC}"
    echo ""
}

# =============================================================================
# Main
# =============================================================================

cmd="${1:-all}"

case "$cmd" in
    all)
        cmd_resources
        cmd_estimate
        cmd_costs
        ;;
    resources)
        cmd_resources
        ;;
    costs)
        cmd_costs
        ;;
    estimate)
        cmd_estimate
        ;;
    help|--help|-h)
        head -10 "$0" | tail -9
        ;;
    *)
        echo "Usage: $0 {all|resources|costs|estimate}"
        exit 1
        ;;
esac
