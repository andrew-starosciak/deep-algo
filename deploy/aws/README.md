# AWS Deployment for Low-Latency Trading

## Overview

Deploy the gabagool arbitrage bot to AWS for ~50-150ms latency reduction
by co-locating near Polymarket's infrastructure.

## Prerequisites

- AWS CLI configured (`aws configure`)
- SSH key pair for EC2 access
- Rust cross-compilation toolchain (for building Linux binaries)

## Quick Start

```bash
# 1. Find Polymarket API latency by region
./scripts/measure-latency.sh

# 2. Deploy to optimal region (likely us-east-1)
./scripts/deploy.sh us-east-1

# 3. SSH and start the bot
ssh -i ~/.ssh/trading-bot.pem ec2-user@<instance-ip>
cd /opt/trading-bot
./start.sh
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        AWS us-east-1                         │
│  ┌─────────────────┐      ┌─────────────────────────────┐  │
│  │   EC2 c6i.large │      │   Polymarket CLOB API       │  │
│  │   Trading Bot   │◄────►│   (same region = ~10-20ms)  │  │
│  └─────────────────┘      └─────────────────────────────┘  │
│           │                                                  │
│           ▼                                                  │
│  ┌─────────────────┐      ┌─────────────────────────────┐  │
│  │   CloudWatch    │      │   Binance WebSocket         │  │
│  │   Logs/Metrics  │      │   (spot price feed)         │  │
│  └─────────────────┘      └─────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Instance Selection

| Instance | vCPU | RAM | Network | Cost/month | Use Case |
|----------|------|-----|---------|------------|----------|
| t3.small | 2 | 2GB | Up to 5 Gbps | ~$15 | Testing |
| c6i.large | 2 | 4GB | Up to 12.5 Gbps | ~$60 | Production |
| c6i.xlarge | 4 | 8GB | Up to 12.5 Gbps | ~$120 | High volume |

**Recommendation**: Start with `c6i.large` for production.

## Network Optimization

The deployment script configures:

1. **TCP tuning** - Low-latency kernel parameters
2. **Enhanced networking** - ENA driver for better throughput
3. **Connection keep-alive** - Persistent connections to APIs

## Security

- Private key stored in AWS Secrets Manager (not on disk)
- Security group allows only outbound HTTPS
- No inbound access except SSH from your IP

## Monitoring

- CloudWatch metrics for latency tracking
- Alerts on execution failures
- Daily P&L reports via SNS

## Cost Estimate

| Resource | Monthly Cost |
|----------|--------------|
| EC2 c6i.large | $60 |
| EBS 20GB | $2 |
| CloudWatch | $5 |
| Data transfer | ~$10 |
| **Total** | **~$77/month** |

## Files

- `terraform/` - Infrastructure as code
- `scripts/` - Deployment and monitoring scripts
- `config/` - Environment-specific configuration
