# Statistical Trading Engine

A production-grade algorithmic trading system built in Rust with full modularity and backtest-live parity. Supports Hyperliquid perpetuals and Polymarket binary options.

## Features

- **Event-Driven Architecture**: Identical code runs in backtesting and live trading
- **Pluggable Strategies**: Implement `Strategy` trait for custom algorithms
- **Multi-Signal Composites**: Combine multiple indicators with voting consensus
- **Polymarket Paper Trading**: Bitcoin 15-minute binary options with real-time signals
- **Statistical Validation**: Wilson CI, binomial tests, walk-forward optimization
- **Multi-Tier Storage**: Arrow (hot), TimescaleDB (warm), Parquet (cold)
- **Web API**: Axum-based REST + WebSocket for real-time control
- **Bot Orchestration**: Actor-pattern multi-bot coordination with Tokio
- **Hot-Reload Config**: Update parameters without restart

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- PostgreSQL with TimescaleDB extension
- Hyperliquid API access (for perpetuals trading)
- Polygon RPC endpoint (optional, for Chainlink settlement - uses public endpoint by default)

### Installation

```bash
# Clone repository
git clone https://github.com/yourusername/algo-trade
cd algo-trade

# Build
cargo build --release

# Install Just task runner (optional but recommended)
cargo install just

# Setup database
psql -f scripts/setup_timescale.sql
```

### Configuration

Copy example config:
```bash
cp config/Config.example.toml config/Config.toml
# Edit config/Config.toml with your settings
```

## Usage

This project uses [Just](https://github.com/casey/just) as a task runner to simplify common commands. All commands can be run with `just <command>` or the full `cargo` command.

### Quick Reference

```bash
# Show all available commands
just --list

# Start the persistent trading daemon
just daemon

# Start the TUI for bot management
just tui

# Run a backtest
just backtest BTC ma_crossover

# Fetch historical data
just fetch BTC 1h

# Run tests
just test

# Run development workflow (check + test + lint)
just dev
```

### Daemon Mode (Persistent Bots)

The trading system can run as a persistent daemon, allowing bots to continue running even when the TUI is closed:

```bash
# Start daemon (uses config/Config.toml)
just daemon

# Or with custom database location
BOT_DATABASE_URL=sqlite:///path/to/bots.db just daemon
```

The daemon:
- Persists bot configurations to SQLite
- Auto-restores enabled bots on startup (in Stopped state)
- Handles SIGTERM/SIGINT for graceful shutdown
- Runs web API on configured host:port

### TUI Mode (Visual Management)

Use the TUI to manage bots visually:

```bash
just tui
```

The TUI provides:
- Real-time bot status (color-coded by state)
- Performance metrics (equity, return %, Sharpe ratio)
- Open positions and trade history
- Bot control (start/stop/pause)

### Fetch Historical Data

Fetch OHLCV candle data from Hyperliquid for backtesting:

```bash
# Using Just (recommended)
just fetch BTC 1h

# Or full cargo command
cargo run -p algo-trade-cli -- fetch-data \
  --symbol BTC \
  --interval 1h \
  --start 2025-01-01T00:00:00Z \
  --end 2025-02-01T00:00:00Z \
  --output data/btc_jan2025.csv
```

**Supported intervals**: `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `8h`, `12h`, `1d`, `3d`, `1w`, `1M`

**Note**: Hyperliquid limits responses to 5000 candles per request. Larger time ranges are automatically paginated.

### Run Backtest

```bash
# Using Just
just backtest BTC ma_crossover

# Or full cargo command
cargo run -p algo-trade-cli -- backtest \
  --data data/btc_jan2025.csv \
  --strategy ma_crossover
```

### Polymarket Paper Trading

Paper trade Bitcoin 15-minute binary options on Polymarket using real-time signals.

#### Quick Start

```bash
# Start paper trading with default settings (liquidation cascade signal)
./scripts/start-polymarket-bot.sh --duration 24h
```

This starts:
1. Data collectors (liquidations, funding rates)
2. Polymarket odds collector
3. Paper trading bot with edge-threshold entry strategy

#### Multi-Signal Composite Mode

Based on research findings, use multiple signals with voting for higher conviction trades:

```bash
# Require 2+ signals to agree before trading
./scripts/start-polymarket-bot.sh --duration 24h \
    --composite \
    --min-signals-agree 2 \
    --enable-orderbook \
    --enable-funding \
    --enable-liq-ratio
```

**Available Signals:**
| Signal | Flag | Description |
|--------|------|-------------|
| Liquidation Cascade | (always on) | Detects momentum from large liquidation events |
| Order Book Imbalance | `--enable-orderbook` | Bid/ask wall detection for support/resistance |
| Funding Rate Percentile | `--enable-funding` | Compares to 30-day history (top/bottom 20%) |
| Liquidation Ratio | `--enable-liq-ratio` | 24h long vs short liquidation ratio |

#### Configuration Options

```bash
./scripts/start-polymarket-bot.sh --help

# Key options:
--duration <time>         # Trading duration (default: 24h)
--signal-mode <mode>      # cascade|exhaustion|combined (default: cascade)
--min-signal-strength <n> # Minimum signal strength 0.0-1.0 (default: 0.6)
--min-edge <n>            # Minimum edge threshold (default: 0.03)
--max-price <n>           # Max price for decent odds (default: 0.55)
--kelly-fraction <n>      # Kelly fraction for sizing (default: 0.25)
--entry-strategy <s>      # immediate|fixed_time|edge_threshold (default: edge_threshold)
--max-signal-age-mins <n> # Max signal age before rejection (default: 4, 0 to disable)
--bankroll <n>            # Starting bankroll (default: 10000)
--simulated               # Use simulated signals for testing
```

#### Signal Age Constraint

Liquidation cascades cause rapid price moves that may retrace before the 15-minute window settles. The `--max-signal-age-mins` flag prevents entering trades on stale signals where the initial momentum move has likely already played out.

```
Window: 14:00 - 14:15
[0-2 min]  CASCADE detected - trade with full confidence
[2-4 min]  TRANSITION - still tradeable (within default 4-min threshold)
[4+ min]   STALE - signal rejected, initial move likely played out
```

Adjust based on your risk tolerance:
- `--max-signal-age-mins 3` - More aggressive, fewer but fresher trades
- `--max-signal-age-mins 5` - More permissive, captures more signals
- `--max-signal-age-mins 0` - Disabled (not recommended for cascade mode)

#### How It Works

1. **Signal Generation**: Monitors liquidation data, funding rates, and order books
2. **Composite Voting**: When `--composite` enabled, requires N signals to agree on direction
3. **Entry Strategy**: Waits for favorable edge before entering (edge_threshold mode)
4. **Position Sizing**: Uses Kelly criterion with configurable fraction
5. **Settlement**: Tracks outcomes via Chainlink BTC/USD price feed on Polygon

#### Example Output

```
Paper trading config: signal=composite, mode=real (Cascade) from BTCUSDT/binance
Composite mode: require 2 signals to agree (orderbook=true, funding=true, liq_ratio=true)
Entry strategy: edge_threshold (poll every 10s)

Composite signal fired: direction=Up, strength=0.72, signals_agreed=2
Entry strategy triggered - executing trade: edge=0.058
Paper trade stored: market_id=btc-15min, entry_offset_mins=7
```

### Cross-Market Correlation Arbitrage

Scan for arbitrage opportunities across BTC, ETH, SOL, and XRP 15-minute binary options on Polymarket. The strategy exploits the ~85% correlation between crypto assets - when buying cheap sides on two different coins totaling < $1.00, you profit if at least one wins.

#### Strategy Overview

```
Buy: ETH UP @ $0.05 + BTC DOWN @ $0.91 = $0.96 total

Outcomes (with 85% correlation):
- Both DOWN (44%): BTC DOWN wins → $1.00 payout → +$0.04 profit
- Both UP (44%):   ETH UP wins   → $1.00 payout → +$0.04 profit
- ETH↑ BTC↓ (4%):  Both win!    → $2.00 payout → +$1.04 profit
- ETH↓ BTC↑ (8%):  Both lose    → $0.00 payout → -$0.96 loss

Expected win rate: ~92% (loss scenario is rare due to correlation)
```

#### Data Collection

Start the scanner to collect opportunities with order book depth tracking:

```bash
# Quick start (12 hours, with depth tracking)
./scripts/run_cross_market.sh scan

# Custom duration (1 hour)
DURATION_MINS=60 ./scripts/run_cross_market.sh scan

# Run scanner + settlement handler together
./scripts/run_cross_market.sh both

# More restrictive thresholds
MAX_COST=0.80 MIN_SPREAD=0.20 ./scripts/run_cross_market.sh scan
```

The scanner captures:
- All coin pair combinations (BTC/ETH, BTC/SOL, BTC/XRP, ETH/SOL, ETH/XRP, SOL/XRP)
- All direction combinations (Coin1Up+Coin2Down, Coin1Down+Coin2Up, BothUp, BothDown)
- Order book depth at detection time (bid/ask depth, spread in bps)
- Settlement outcomes (tracked by settlement handler)

#### Analyzing Collected Data

After collecting data, run the backtest to analyze performance:

```bash
./scripts/run_cross_market.sh backtest
```

This produces:
- **Trade-level stats**: Win rate by combination type, average P&L
- **Window-level stats**: Best entry per 15-min window (deduplicated)
- **Kelly analysis**: Optimal bet sizing based on observed win rates
- **Bankroll requirements**: Ruin probability at various bankroll levels
- **Depth analysis**: Liquidity available at detection time

#### SQL Queries for Analysis

```sql
-- Win rate by combination type
SELECT
    combination,
    COUNT(*) as trades,
    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
    ROUND(COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::decimal / COUNT(*) * 100, 1) as win_rate
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY combination
ORDER BY win_rate DESC;

-- Best performing pairs
SELECT
    coin1 || '/' || coin2 as pair,
    combination,
    COUNT(*) as trades,
    ROUND(AVG(actual_pnl), 4) as avg_pnl,
    SUM(actual_pnl) as total_pnl
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY coin1, coin2, combination
ORDER BY avg_pnl DESC;

-- Depth analysis (fill probability)
SELECT
    CASE
        WHEN LEAST(leg1_bid_depth, leg2_bid_depth) < 1000 THEN '<1K shares'
        WHEN LEAST(leg1_bid_depth, leg2_bid_depth) < 5000 THEN '1-5K shares'
        ELSE '5K+ shares'
    END as depth_bucket,
    COUNT(*) as opportunities,
    ROUND(AVG(actual_pnl), 4) as avg_pnl
FROM cross_market_opportunities
WHERE status = 'settled' AND leg1_bid_depth IS NOT NULL
GROUP BY 1
ORDER BY 1;

-- Entry timing analysis (minute within 15-min window)
SELECT
    EXTRACT(MINUTE FROM timestamp)::int % 15 as minute_in_window,
    COUNT(*) as entries,
    ROUND(AVG(spread), 4) as avg_spread,
    ROUND(AVG(actual_pnl), 4) as avg_pnl
FROM cross_market_opportunities
WHERE status = 'settled'
GROUP BY 1
ORDER BY 1;
```

#### Key Findings from Backtesting

Based on historical analysis:

| Metric | Value |
|--------|-------|
| Best combination | Coin1UpCoin2Down (89% win rate) |
| Optimal entry | Cost < $0.80 (spread > 20%) |
| Observed correlation | 64% (lower than assumed 85%) |
| Recommended Kelly | 1/4 Kelly (~19% of bankroll) |
| Best pairs | SOL/XRP, BTC/SOL, BTC/XRP |

#### Live Trading (Coming Soon)

```bash
# Paper trade with optimal settings
cargo run -p algo-trade-cli -- cross-market-scan --optimal --track-depth

# Settings used by --optimal:
# - Only Coin1UpCoin2Down combinations
# - Max cost $0.80 (spread > 20%)
# - 64% observed correlation
```

### Web API Only

Start just the web API server without TUI:

```bash
# Using Just (default: 0.0.0.0:8080)
just server

# Custom host/port
just server 127.0.0.1 9000

# Or full cargo command
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080
```

Then access:
- REST API: `http://localhost:8080/api/bots`
- WebSocket: `ws://localhost:8080/ws`

## Docker Deployment

### Quick Start

The entire trading system can be deployed using Docker Compose for a self-contained, production-ready setup.

#### Prerequisites

- Docker Engine 20.10+ with BuildKit enabled
- Docker Compose 2.0+
- 4GB RAM minimum (8GB recommended for TimescaleDB)
- 10GB disk space for Docker images and volumes

#### Initial Setup

1. **Create secrets directory and set database password**:
   ```bash
   mkdir -p secrets
   echo "your_secure_password_here" > secrets/db_password.txt
   chmod 600 secrets/db_password.txt
   ```

2. **Create environment file**:
   ```bash
   cp .env.example .env
   # Edit .env and set DB_PASSWORD to match secrets/db_password.txt
   nano .env
   ```

3. **Build Docker images**:
   ```bash
   DOCKER_BUILDKIT=1 docker compose build
   ```
   First build: ~5-10 minutes. Subsequent builds: <1 minute (with cache).

4. **Start services**:
   ```bash
   docker compose up -d
   ```

5. **Verify services are running**:
   ```bash
   docker compose ps
   docker compose logs -f app
   ```

#### Access Points

- **Web API**: http://localhost:8080
- **TUI (Web Terminal)**: http://localhost:7681
- **TimescaleDB**: postgresql://localhost:5432/algo_trade (development only)

### Managing Services

#### Daily Operations

```bash
# Start all services
docker compose up -d

# Stop all services (graceful shutdown)
docker compose stop

# Restart services
docker compose restart app

# View logs (all services)
docker compose logs -f

# View logs (app only)
docker compose logs -f app

# Check service status
docker compose ps

# Shell access to app container
docker exec -it algo-trade-app /bin/bash

# Access TUI via terminal (alternative to web)
docker exec -it algo-trade-app algo-trade live-bot-tui
```

#### Updating the Application

```bash
# Pull latest code
git pull

# Rebuild and restart
docker compose down
DOCKER_BUILDKIT=1 docker compose build
docker compose up -d
```

#### Complete Teardown

```bash
# Stop and remove containers (keeps volumes/data)
docker compose down

# Stop and remove everything including volumes (DELETES ALL DATA)
docker compose down -v
```

### Data Backup and Restore

#### Backup TimescaleDB

```bash
# Create backups directory
mkdir -p backups

# Backup TimescaleDB volume
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/timescale-$(date +%Y%m%d-%H%M%S).tar.gz /data
```

#### Backup SQLite (Bot Configurations)

```bash
# Backup SQLite volume (bots.db)
docker run --rm \
  -v algo-trade_sqlite-data:/data \
  -v $(pwd)/backups:/backup \
  alpine tar czf /backup/sqlite-$(date +%Y%m%d-%H%M%S).tar.gz /data
```

#### Restore TimescaleDB

```bash
# Stop services first
docker compose down

# Restore TimescaleDB volume
docker run --rm \
  -v algo-trade_timescale-data:/data \
  -v $(pwd)/backups:/backup \
  alpine sh -c "rm -rf /data/* && tar xzf /backup/timescale-YYYYMMDD-HHMMSS.tar.gz -C /"

# Restart services
docker compose up -d
```

#### Restore SQLite

```bash
# Stop services first
docker compose down

# Restore SQLite volume
docker run --rm \
  -v algo-trade_sqlite-data:/data \
  -v $(pwd)/backups:/backup \
  alpine sh -c "rm -rf /data/* && tar xzf /backup/sqlite-YYYYMMDD-HHMMSS.tar.gz -C /"

# Restart services
docker compose up -d
```

### Troubleshooting

#### Services won't start

**Check logs**:
```bash
docker compose logs timescaledb
docker compose logs app
```

**Common issues**:
- Port conflicts (8080, 7681, 5432 already in use) - Change ports in .env
- Memory limits - TimescaleDB needs 2GB minimum
- Database password mismatch - Verify secrets/db_password.txt matches .env

#### TUI not accessible at port 7681

**Verify ttyd is running**:
```bash
docker exec -it algo-trade-app ps aux | grep ttyd
```

**Check entrypoint logs**:
```bash
docker compose logs app | grep ttyd
```

#### Database initialization fails

**Remove TimescaleDB volume and recreate**:
```bash
docker compose down
docker volume rm algo-trade_timescale-data
docker compose up -d
```

#### Build fails with cache issues

**Clear BuildKit cache**:
```bash
docker builder prune -af
DOCKER_BUILDKIT=1 docker compose build --no-cache
```

## Architecture

### Workspace Crates

- **core**: Event types, traits, trading engine
- **signals**: Signal generators (order book, funding, liquidations, news)
- **exchange-hyperliquid**: Hyperliquid REST/WebSocket integration
- **exchange-polymarket**: Polymarket CLOB integration
- **data**: TimescaleDB, Arrow, Parquet storage
- **strategy**: Strategy implementations (MA crossover, RSI, etc.)
- **execution**: Order management and execution
- **backtest**: Historical simulation with binary metrics
- **bot-orchestrator**: Multi-bot actor-pattern coordination
- **web-api**: Axum REST + WebSocket API
- **cli**: Command-line interface

### Event Flow

```
MarketEvent → Strategy → SignalEvent → RiskManager → OrderEvent → ExecutionHandler → FillEvent
```

### Backtest-Live Parity

Same `TradingSystem` works with different providers:

**Backtest**:
```rust
TradingSystem::new(
    HistoricalDataProvider::from_csv("data.csv")?,
    SimulatedExecutionHandler::new(0.001, 5.0),
    strategies,
    risk_manager,
)
```

**Live**:
```rust
TradingSystem::new(
    LiveDataProvider::new(ws_url, symbols).await?,
    LiveExecutionHandler::new(client),
    strategies, // SAME strategies!
    risk_manager, // SAME risk manager!
)
```

## Development

See [CLAUDE.md](CLAUDE.md) for detailed development guide.

### Common Development Tasks

```bash
# Check all crates for compilation errors (fast)
just check

# Run all tests
just test

# Run tests for a specific crate
just test-crate backtest

# Run clippy linter (strict mode)
just lint

# Format code
just fmt

# Full development workflow (check + test + lint)
just dev

# Full CI workflow (fmt-check + check + test + lint + build)
just ci

# Build release binary
just build

# Clean build artifacts
just clean

# Show project information
just info

# Run with debug logging
just debug daemon
# Or: RUST_LOG=debug just daemon

# Watch mode (auto-recompile on changes, requires cargo-watch)
cargo install cargo-watch
just watch
```

### Using Just

All commands are defined in the `justfile` at the project root. To see all available commands with descriptions:

```bash
just --list
# or
just -l
```

You can also run any command without Just by using the full `cargo` commands shown in each recipe.

## License

MIT
