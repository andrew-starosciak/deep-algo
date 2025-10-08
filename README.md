# Hyperliquid Algorithmic Trading System

A production-grade algorithmic trading system for Hyperliquid exchange, built in Rust with full modularity and backtest-live parity.

## Features

- **Event-Driven Architecture**: Identical code runs in backtesting and live trading
- **Pluggable Strategies**: Implement `Strategy` trait for custom algorithms
- **Multi-Tier Storage**: Arrow (hot), TimescaleDB (warm), Parquet (cold)
- **Web API**: Axum-based REST + WebSocket for real-time control
- **Bot Orchestration**: Actor-pattern multi-bot coordination with Tokio
- **Hot-Reload Config**: Update parameters without restart

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- PostgreSQL with TimescaleDB extension
- Hyperliquid API access

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
- **exchange-hyperliquid**: Hyperliquid REST/WebSocket integration
- **data**: TimescaleDB, Arrow, Parquet storage
- **strategy**: Strategy implementations (MA crossover, RSI, etc.)
- **execution**: Order management and execution
- **backtest**: Historical simulation with performance metrics
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
