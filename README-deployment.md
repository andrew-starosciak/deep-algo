# IB Options Trading System — EC2 Deployment Guide

This guide walks through deploying the OpenClaw-orchestrated LLM-driven options trading system to AWS EC2.

## 📚 Documentation Quick Links

- **[IB Deployment Update (Feb 2025)](docs/IB-DEPLOYMENT-UPDATE.md)** - What we fixed and next steps
- **[IB Gateway Troubleshooting](docs/IB-GATEWAY-TROUBLESHOOTING.md)** - Comprehensive debugging guide
- **This README** - Initial setup and deployment commands

## Prerequisites

1. **AWS CLI** installed and configured:
   ```bash
   aws configure
   # Enter your AWS access key ID, secret access key, and default region
   ```

2. **Interactive Brokers Account** with 2FA enrolled:
   - Paper trading account (or live if you prefer)
   - 2FA enabled (mandatory as of Feb 2025)
   - IBKR Mobile app installed (for push notifications)
   - API access enabled: Account Management → Settings → API → "ActiveX and Socket Clients"
   - **Important:** You'll need to approve 2FA on your phone ~once per week

3. **Discord Bot** (recommended for interactive approval buttons):
   - Go to https://discord.com/developers/applications
   - Create New Application → Bot → Copy Token
   - Enable "Message Content Intent" and "Server Members Intent"
   - Invite to your server with appropriate permissions
   - Add to `.env`:
     ```bash
     DISCORD_BOT_TOKEN=your_bot_token
     DISCORD_CHANNEL_ID=  # Auto-detected from first message
     ```

4. **Discord Webhook** (alternative to bot, less features):
   - Go to your Discord server → Server Settings → Integrations → Webhooks
   - Create a new webhook, copy the URL
   - Add to `.env`:
     ```bash
     DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/YOUR_WEBHOOK_URL
     ```

3. **Telegram Bot** (alternative to Discord):
   - Message [@BotFather](https://t.me/botfather) on Telegram
   - Create a new bot with `/newbot`
   - Get your chat ID from [@userinfobot](https://t.me/userinfobot)
   - Add to `.env`:
     ```bash
     TELEGRAM_BOT_TOKEN=your_bot_token
     TELEGRAM_CHAT_ID=your_chat_id
     ```

4. **Environment Variables** configured in `.env`:
   ```bash
   # News APIs (for research pipeline)
   FINNHUB_API_KEY=your_key
   ALPHAVANTAGE_API_KEY=your_key
   NEWSAPI_KEY=your_key

   # Notifications (at least one)
   DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
   # OR
   TELEGRAM_BOT_TOKEN=...
   TELEGRAM_CHAT_ID=...
   ```

## Quick Start

### 1. Deploy EC2 Instance

This provisions an EC2 instance, installs PostgreSQL, Python environment, and uploads your code:

```bash
cd /home/a/Work/gambling/engine
./scripts/deploy-ib-options.sh deploy
```

What this does:
- ✅ Launches Ubuntu 22.04 t3.small instance in us-east-1
- ✅ Installs PostgreSQL 14 + Python 3.11
- ✅ Creates `algo_trade` database with `algo` user
- ✅ Creates Python virtualenv and installs dependencies
- ✅ Uploads your `.env` file (with remote DATABASE_URL override)
- ✅ Runs database migrations
- ✅ Creates log directory at `~/logs`

**Output:**
```
✅ IB Options Trading System deployed to 54.123.45.67

Next steps:
  ./scripts/deploy-ib-options.sh setup-cron      # Install scheduled workflows
  ./scripts/deploy-ib-options.sh start-manager   # Start position manager
  ./scripts/deploy-ib-options.sh ssh             # SSH into instance
```

### 2. Setup Cron Jobs

Install automated workflows (pre-market research, weekend deep dive):

```bash
./scripts/deploy-ib-options.sh setup-cron
```

**Cron Schedule:**
- **8:00 AM ET daily** (Mon-Fri): Pre-market research scan
- **10:00 AM ET Saturday**: Weekend deep dive (full research for watchlist)
- **2:00 AM daily**: Log rotation (delete logs >7 days old)

### 3. Start Position Manager

This daemon continuously monitors open positions and executes approved trades:

```bash
./scripts/deploy-ib-options.sh start-manager
```

The position manager:
- Polls database every 30 seconds for approved recommendations
- Executes trades via IB API (paper trading by default)
- Monitors P&L and enforces stop-losses (50% hard stop)
- Sends updates to Discord/Telegram

### 4. (Optional) Start Workflow Scheduler

For more advanced scheduling beyond cron:

```bash
./scripts/deploy-ib-options.sh start-scheduler
```

This runs the OpenClaw scheduler daemon for complex workflow orchestration.

## Common Operations

### SSH into Instance

```bash
./scripts/deploy-ib-options.sh ssh
```

### View Logs

```bash
# Position manager logs
./scripts/deploy-ib-options.sh logs manager

# Workflow scheduler logs
./scripts/deploy-ib-options.sh logs scheduler

# Pre-market research logs
./scripts/deploy-ib-options.sh logs premarket

# Weekend deep dive logs
./scripts/deploy-ib-options.sh logs weekly
```

### Check Status

```bash
./scripts/deploy-ib-options.sh status
```

Shows:
- EC2 instance IP and ID
- Position manager service status (running/stopped)
- Workflow scheduler service status
- Active cron jobs
- Recent log files

### Update Python Code

After making local changes to the Python code:

```bash
./scripts/deploy-ib-options.sh sync
```

This uploads the latest `python/` directory to EC2. Then restart services:

```bash
./scripts/deploy-ib-options.sh stop-all
./scripts/deploy-ib-options.sh start-manager
```

### Stop All Services

```bash
./scripts/deploy-ib-options.sh stop-all
```

Stops:
- Position manager daemon
- Workflow scheduler daemon

(Cron jobs continue running unless you `crontab -r` via SSH)

### Teardown Everything

**⚠️ WARNING: This deletes the EC2 instance, security group, and SSH key!**

```bash
./scripts/deploy-ib-options.sh teardown
```

You'll be prompted for confirmation. This is destructive and irreversible.

## Discord Bot Commands

Once deployed and Discord webhook is configured, your OpenClaw bot will send notifications to your Discord channel.

**Notification Types:**
- 🎯 **Trade Recommendations** — New thesis scored 7.0+, awaiting approval
- ⚠️ **Workflow Escalations** — LLM needs human input to continue
- 📊 **Weekly Battle Plans** — Saturday deep dive summary
- 📥/📤 **Position Updates** — Entry/exit confirmations with P&L

**Manual Commands** (run via CLI, not Discord slash commands yet):
```bash
# SSH into EC2 first
./scripts/deploy-ib-options.sh ssh

# Then run commands
cd ~/python
source ~/venv/bin/activate

# Run research for a ticker
python -m openclaw research NVDA

# Run full trade thesis workflow
python -m openclaw run trade-thesis --ticker AAPL --db-url $DATABASE_URL

# Approve a pending recommendation
python -m openclaw approve 123 --db-url $DATABASE_URL

# Check workflow status
python -m openclaw status --db-url $DATABASE_URL
```

## Architecture on EC2

```
EC2 Instance (t3.small, Ubuntu 22.04)
├── PostgreSQL (localhost:5432)
│   └── algo_trade database
├── Python 3.11 virtualenv
│   ├── openclaw CLI
│   ├── Research pipeline (news, technicals, flow)
│   └── LLM agents (researcher, analyst, risk checker)
├── Systemd Services
│   ├── ib-options-manager.service (position manager)
│   └── ib-options-scheduler.service (workflow scheduler)
├── Cron Jobs
│   ├── Pre-market scan (8 AM ET daily)
│   └── Weekend deep dive (Saturday 10 AM ET)
└── Logs
    ├── ~/logs/manager.log
    ├── ~/logs/scheduler.log
    ├── ~/logs/premarket.log
    └── ~/logs/weekly.log
```

## Costs

**t3.small on-demand** (us-east-1):
- Instance: ~$0.0208/hour = ~$15/month (if running 24/7)
- Storage: 20 GB GP3 = ~$1.60/month
- Data transfer: Minimal for this use case

**Cost optimization:**
- Use spot instances: `./scripts/deploy-ib-options.sh deploy --spot` (~70% cheaper, can be reclaimed)
- Stop instance overnight: Stop at 5 PM ET, start at 7 AM ET (saves ~60%)
- Use t3.micro for testing (~$7/month)

## Troubleshooting

### "Permission denied" when running script
```bash
chmod +x ./scripts/deploy-ib-options.sh
```

### "No module named 'openclaw'"
Python dependencies not installed. Re-run:
```bash
./scripts/deploy-ib-options.sh ssh
source ~/venv/bin/activate
cd ~/python
pip install -e .
```

### Position manager keeps crashing
Check logs:
```bash
./scripts/deploy-ib-options.sh logs manager
```

Common causes:
- Database connection failed (check DATABASE_URL in `~/.env`)
- IB API not reachable (ensure IB Gateway/TWS is running)
- Missing API keys in `.env`

### Discord notifications not sending
1. Verify webhook URL is correct in `~/.env`
2. Check service logs: `./scripts/deploy-ib-options.sh logs manager`
3. Test manually:
   ```bash
   ./scripts/deploy-ib-options.sh ssh
   source ~/venv/bin/activate
   python -c "
   import asyncio
   from openclaw.discord_notify import DiscordNotifier
   n = DiscordNotifier()
   asyncio.run(n.send('Test message'))
   "
   ```

### Cron jobs not running
1. SSH into instance: `./scripts/deploy-ib-options.sh ssh`
2. Check crontab: `crontab -l`
3. Check cron logs: `grep CRON /var/log/syslog`
4. Verify timezone (cron uses UTC, adjust times accordingly)

## Next Steps

After deployment:

1. **Paper trade for 2-4 weeks** — Let the system run in paper mode, collect data on thesis quality
2. **Review recommendations daily** — Check Discord for new trade ideas, approve/reject manually
3. **Analyze results** — Are high-scored theses actually winning? Which signals are most predictive?
4. **Adjust scoring** — Refine the thesis evaluation framework based on real outcomes
5. **Go live** — Only after paper results show consistent positive EV (>52% win rate, p < 0.05)

## Support

- **Logs**: Always check logs first (`./scripts/deploy-ib-options.sh logs [service]`)
- **SSH access**: `./scripts/deploy-ib-options.sh ssh` for debugging
- **Database**: Connect via `psql -h localhost -U algo -d algo_trade` (password: `algo_trade_local`)

## Security Notes

- `.env` file contains sensitive keys — never commit to git
- SSH key stored at `./scripts/.aws-latency-key.pem` — keep secure
- Security group allows SSH from 0.0.0.0/0 — consider restricting to your IP
- PostgreSQL only accessible from localhost (not exposed to internet)
- Webhook URLs are sensitive — rotate if leaked
