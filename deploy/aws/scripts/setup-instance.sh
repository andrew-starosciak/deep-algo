#!/bin/bash
# One-time instance setup for low-latency trading
set -e

echo "=============================================="
echo "Setting up trading instance"
echo "=============================================="

# Update system
echo "[1/6] Updating system packages..."
sudo yum update -y
sudo yum install -y htop iotop

# TCP tuning for low latency
echo "[2/6] Configuring TCP for low latency..."
sudo tee /etc/sysctl.d/99-trading-tuning.conf > /dev/null << 'SYSCTL'
# TCP Low-Latency Tuning for Trading

# Disable TCP slow start after idle
net.ipv4.tcp_slow_start_after_idle = 0

# Enable TCP Fast Open
net.ipv4.tcp_fastopen = 3

# Reduce TCP FIN timeout
net.ipv4.tcp_fin_timeout = 15

# Enable TCP window scaling
net.ipv4.tcp_window_scaling = 1

# Increase TCP buffer sizes
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216

# Reduce latency with TCP_NODELAY (app-level, but ensure kernel allows it)
net.ipv4.tcp_low_latency = 1

# Enable timestamps for better RTT estimation
net.ipv4.tcp_timestamps = 1

# Enable selective ACKs
net.ipv4.tcp_sack = 1

# Reduce keepalive time
net.ipv4.tcp_keepalive_time = 60
net.ipv4.tcp_keepalive_intvl = 10
net.ipv4.tcp_keepalive_probes = 6

# Increase connection tracking table
net.netfilter.nf_conntrack_max = 131072
SYSCTL

sudo sysctl --system

# Create systemd service
echo "[3/6] Creating systemd service..."
sudo tee /etc/systemd/system/trading-bot.service > /dev/null << 'SERVICE'
[Unit]
Description=Polymarket Trading Bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ec2-user
Group=ec2-user
WorkingDirectory=/opt/trading-bot
ExecStart=/opt/trading-bot/algo-trade-cli gabagool-auto --mode paper --duration 24h
Restart=always
RestartSec=10

# Environment
Environment=RUST_LOG=info,algo_trade_polymarket=debug
Environment=RUST_BACKTRACE=1

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

# Security
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/opt/trading-bot

[Install]
WantedBy=multi-user.target
SERVICE

sudo systemctl daemon-reload
sudo systemctl enable trading-bot

# Set up log rotation
echo "[4/6] Configuring log rotation..."
sudo tee /etc/logrotate.d/trading-bot > /dev/null << 'LOGROTATE'
/opt/trading-bot/logs/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 640 ec2-user ec2-user
}
LOGROTATE

# Create directory structure
echo "[5/6] Creating directory structure..."
mkdir -p /opt/trading-bot/logs
mkdir -p /opt/trading-bot/data
mkdir -p /opt/trading-bot/positions

# Set up CloudWatch agent (optional)
echo "[6/6] Setting up monitoring..."
# CloudWatch agent can be configured separately if needed

echo ""
echo "=============================================="
echo "Setup Complete"
echo "=============================================="
echo ""
echo "TCP tuning applied for low-latency trading"
echo "Systemd service created: trading-bot"
echo ""
echo "Start bot: sudo systemctl start trading-bot"
echo "View logs: journalctl -u trading-bot -f"
echo ""
