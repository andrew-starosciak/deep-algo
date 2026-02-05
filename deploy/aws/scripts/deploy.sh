#!/bin/bash
# Deploy trading bot to AWS EC2
set -e

REGION="${1:-us-east-1}"
INSTANCE_TYPE="${2:-c6i.large}"
KEY_NAME="${3:-trading-bot}"

echo "=============================================="
echo "Deploying Trading Bot to AWS"
echo "=============================================="
echo "Region: $REGION"
echo "Instance: $INSTANCE_TYPE"
echo ""

# Build release binary for Linux
echo "[1/5] Building release binary..."
cd "$(dirname "$0")/../../.."

# Cross-compile for Linux if on macOS
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "Cross-compiling for Linux x86_64..."
    cargo build --release --target x86_64-unknown-linux-gnu -p algo-trade-cli
    BINARY_PATH="target/x86_64-unknown-linux-gnu/release/algo-trade-cli"
else
    cargo build --release -p algo-trade-cli
    BINARY_PATH="target/release/algo-trade-cli"
fi

echo "Binary built: $BINARY_PATH"
echo ""

# Create deployment package
echo "[2/5] Creating deployment package..."
DEPLOY_DIR="deploy/aws/.deploy-package"
rm -rf "$DEPLOY_DIR"
mkdir -p "$DEPLOY_DIR"

cp "$BINARY_PATH" "$DEPLOY_DIR/algo-trade-cli"
cp deploy/aws/config/bot-config.toml "$DEPLOY_DIR/" 2>/dev/null || echo "No config file, will use defaults"
cp deploy/aws/scripts/start.sh "$DEPLOY_DIR/"
cp deploy/aws/scripts/setup-instance.sh "$DEPLOY_DIR/"

# Create tarball
tar -czvf deploy/aws/.deploy-package.tar.gz -C "$DEPLOY_DIR" .
echo ""

# Check for existing instance or create new one
echo "[3/5] Checking for existing instance..."
INSTANCE_ID=$(aws ec2 describe-instances \
    --region "$REGION" \
    --filters "Name=tag:Name,Values=trading-bot" "Name=instance-state-name,Values=running" \
    --query 'Reservations[0].Instances[0].InstanceId' \
    --output text 2>/dev/null)

if [ "$INSTANCE_ID" != "None" ] && [ -n "$INSTANCE_ID" ]; then
    echo "Found existing instance: $INSTANCE_ID"
else
    echo "No existing instance found. Create one with Terraform:"
    echo "  cd deploy/aws/terraform && terraform apply"
    exit 1
fi

# Get instance IP
INSTANCE_IP=$(aws ec2 describe-instances \
    --region "$REGION" \
    --instance-ids "$INSTANCE_ID" \
    --query 'Reservations[0].Instances[0].PublicIpAddress' \
    --output text)

echo "Instance IP: $INSTANCE_IP"
echo ""

# Upload deployment package
echo "[4/5] Uploading deployment package..."
scp -i ~/.ssh/${KEY_NAME}.pem \
    deploy/aws/.deploy-package.tar.gz \
    ec2-user@${INSTANCE_IP}:/tmp/

# Deploy and start
echo "[5/5] Deploying on instance..."
ssh -i ~/.ssh/${KEY_NAME}.pem ec2-user@${INSTANCE_IP} << 'EOF'
    set -e

    # Extract package
    sudo mkdir -p /opt/trading-bot
    sudo tar -xzvf /tmp/.deploy-package.tar.gz -C /opt/trading-bot
    sudo chown -R ec2-user:ec2-user /opt/trading-bot

    # Run setup if first deploy
    if [ ! -f /opt/trading-bot/.initialized ]; then
        chmod +x /opt/trading-bot/setup-instance.sh
        /opt/trading-bot/setup-instance.sh
        touch /opt/trading-bot/.initialized
    fi

    # Restart bot
    chmod +x /opt/trading-bot/start.sh
    /opt/trading-bot/start.sh restart

    echo ""
    echo "Deployment complete!"
    echo "Check logs: journalctl -u trading-bot -f"
EOF

echo ""
echo "=============================================="
echo "Deployment Complete"
echo "=============================================="
echo ""
echo "SSH: ssh -i ~/.ssh/${KEY_NAME}.pem ec2-user@${INSTANCE_IP}"
echo "Logs: ssh ... 'journalctl -u trading-bot -f'"
echo ""
