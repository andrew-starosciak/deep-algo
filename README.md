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

# Setup database
psql -f scripts/setup_timescale.sql
```

### Configuration

Copy example config:
```bash
cp config/Config.example.toml config/Config.toml
# Edit config/Config.toml with your settings
```

### Fetch Historical Data

Fetch OHLCV candle data from Hyperliquid for backtesting:

```bash
# Fetch 1 month of hourly BTC data
cargo run -p algo-trade-cli -- fetch-data \
  --symbol BTC \
  --interval 1h \
  --start 2025-01-01T00:00:00Z \
  --end 2025-02-01T00:00:00Z \
  --output data/btc_jan2025.csv

# Fetch 1 week of 5-minute ETH data
cargo run -p algo-trade-cli -- fetch-data \
  --symbol ETH \
  --interval 5m \
  --start 2025-01-15T00:00:00Z \
  --end 2025-01-22T00:00:00Z \
  --output data/eth_week.csv

# Fetch daily SOL data for 3 months
cargo run -p algo-trade-cli -- fetch-data \
  --symbol SOL \
  --interval 1d \
  --start 2024-10-01T00:00:00Z \
  --end 2025-01-01T00:00:00Z \
  --output data/sol_q4.csv
```

**Supported intervals**: `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `8h`, `12h`, `1d`, `3d`, `1w`, `1M`

**Note**: Hyperliquid limits responses to 5000 candles per request. Larger time ranges are automatically paginated.

### Run Backtest

```bash
cargo run -p algo-trade-cli -- backtest \
  --data data/btc_jan2025.csv \
  --strategy ma_crossover
```

### Run Live Trading

```bash
cargo run -p algo-trade-cli -- run --config config/Config.toml
```

### Start Web API

```bash
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080
```

Then access:
- REST API: `http://localhost:8080/api/bots`
- WebSocket: `ws://localhost:8080/ws`

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

### Common Commands

```bash
# Check all crates
cargo check

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run -p algo-trade-cli -- run

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

## License

MIT
