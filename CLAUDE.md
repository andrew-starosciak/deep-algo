# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hyperliquid algorithmic trading system in Rust with modular architecture enabling backtest-live parity. Event-driven design ensures identical strategy code runs in backtesting and production.

## Architecture

### Core Design Pattern

**Event-Driven Architecture**: All components process discrete events sequentially, eliminating look-ahead bias and matching real-time trading exactly.

**Trait Abstraction**: `DataProvider` and `ExecutionHandler` traits enable swapping between backtest (historical data, simulated fills) and live (WebSocket data, real orders) without changing strategy code.

**Actor Pattern**: Bots use Tokio channels (mpsc for commands, watch for config updates, broadcast for status) following Alice Ryhl's DIY actor guide—no heavyweight frameworks.

### Workspace Structure

```
crates/
├── core/               # Event types, traits, TradingSystem engine
├── exchange-hyperliquid/ # REST/WebSocket, rate limiting, auth
├── data/               # TimescaleDB, Arrow, Parquet
├── strategy/           # Strategy trait impls (MA, RSI, etc.)
├── execution/          # Order management
├── backtest/           # Historical simulation, metrics
├── bot-orchestrator/   # Multi-bot coordination
├── web-api/            # Axum REST + WebSocket
└── cli/                # Command-line interface
```

### Event Flow

```
MarketEvent → Strategy::on_market_event() → SignalEvent
SignalEvent → RiskManager::evaluate_signal() → OrderEvent
OrderEvent → ExecutionHandler::execute_order() → FillEvent
```

### Key Dependencies

- **tokio**: Async runtime (all async code uses Tokio)
- **axum**: Web framework for API (preferred over actix-web for memory efficiency)
- **sqlx**: PostgreSQL/TimescaleDB client (async, compile-time checked queries)
- **polars**: DataFrame processing (10-100x faster than pandas)
- **arrow/parquet**: Columnar storage
- **figment**: Multi-source config (TOML + env + JSON)
- **hyperliquid-rust-sdk**: Official exchange SDK (maintain fork for production)

## Development Commands

### Building

```bash
# Check all crates
cargo check

# Build release
cargo build --release

# Build specific crate
cargo build -p algo-trade-core
```

### Testing

```bash
# All tests
cargo test

# Integration tests only
cargo test --test integration_test

# Specific crate
cargo test -p algo-trade-backtest
```

### Running

```bash
# Backtest
cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv --strategy ma_crossover

# Live trading
cargo run -p algo-trade-cli -- run --config config/Config.toml

# Web API only
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080

# With debug logging
RUST_LOG=debug cargo run -p algo-trade-cli -- run
```

### Linting

```bash
# Clippy (all warnings as errors)
cargo clippy -- -D warnings

# Clippy for specific crate
cargo clippy -p algo-trade-core -- -D warnings

# Format
cargo fmt
```

## Critical Patterns

### 1. Financial Precision

**ALWAYS use `rust_decimal::Decimal` for prices, quantities, PnL**. Never use `f64` for financial calculations—rounding errors compound over thousands of operations.

```rust
// CORRECT
use rust_decimal::Decimal;
let price: Decimal = "42750.50".parse()?;

// WRONG - will accumulate errors
let price: f64 = 42750.50;
```

### 2. Backtest-Live Parity

Strategy and RiskManager implementations must be provider-agnostic. Only `DataProvider` and `ExecutionHandler` differ between backtest and live.

```rust
// Strategy sees MarketEvent - doesn't know if backtest or live
async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
    // Same logic runs everywhere
}
```

### 3. Actor Pattern for Bots

Each bot is a spawned task owning `mpsc::Receiver<BotCommand>`. Handle is `Clone` with `mpsc::Sender` for multiple controllers.

```rust
// Spawn bot
let (tx, rx) = mpsc::channel(32);
let handle = BotHandle::new(tx);
tokio::spawn(async move { BotActor::new(config, rx).run().await });
```

### 4. Rate Limiting

Use `governor` crate with per-exchange quotas:
- Hyperliquid: 1200 weight/min (20 req/s)
- Binance: 1200 req/min
- Apply backoff on rate limit errors

### 5. Database Operations

**Batch writes for performance**: Single inserts ~390µs, batching 100 inserts ~13ms (3x speedup per record).

```rust
// Collect records, then batch insert
db.insert_ohlcv_batch(records).await?;
```

**Use hypertables**: TimescaleDB's `create_hypertable()` for time-series data, automatic partitioning.

### 6. Configuration Hot-Reload

Config updates flow via `tokio::sync::watch` channels. Bots subscribe and receive latest config without restart.

```rust
let (watcher, mut config_rx) = ConfigWatcher::new(config);
tokio::select! {
    _ = config_rx.changed() => {
        let new_config = config_rx.borrow().clone();
        // Apply new config
    }
}
```

## Adding New Features

### New Strategy

1. Implement `Strategy` trait in `crates/strategy/src/`
2. Add state (price buffers, indicators) as struct fields
3. Process `MarketEvent` in `on_market_event()`
4. Return `SignalEvent` on signal generation

```rust
pub struct MyStrategy { /* state */ }

#[async_trait]
impl Strategy for MyStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        // Update state, generate signal
    }
    fn name(&self) -> &str { "My Strategy" }
}
```

### New Exchange Integration

1. Create crate `crates/exchange-{name}/`
2. Implement `DataProvider` for WebSocket market data
3. Implement `ExecutionHandler` for order execution
4. Add rate limiting with `governor`
5. Handle authentication and reconnection

### New REST Endpoint

Add to `crates/web-api/src/handlers.rs`:

```rust
pub async fn my_handler(
    State(registry): State<Arc<BotRegistry>>,
    Json(req): Json<MyRequest>,
) -> Result<Json<MyResponse>, StatusCode> {
    // Implementation
}
```

Add route in `crates/web-api/src/server.rs`:

```rust
.route("/api/my-endpoint", post(handlers::my_handler))
```

## Database Schema

### OHLCV Table (Hypertable)

```sql
CREATE TABLE ohlcv (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    open DECIMAL(20, 8) NOT NULL,
    high DECIMAL(20, 8) NOT NULL,
    low DECIMAL(20, 8) NOT NULL,
    close DECIMAL(20, 8) NOT NULL,
    volume DECIMAL(20, 8) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);
```

- **DECIMAL(20, 8)**: Precise financial data (never FLOAT/DOUBLE)
- **Hypertable**: Automatic time-based partitioning
- **Compression**: Enabled for data >7 days old

## Troubleshooting

### "Task panicked" errors

Check Tokio runtime: all async code must run inside `#[tokio::main]` or spawned tasks.

### Rate limit errors from Hyperliquid

Check `governor` quota configuration. Hyperliquid allows 1200 weight/min, most requests cost 1 weight.

### Database connection errors

Verify TimescaleDB extension: `CREATE EXTENSION IF NOT EXISTS timescaledb;`

### WebSocket disconnects

Check auto-reconnect logic in `HyperliquidWebSocket::reconnect()`. Should have exponential backoff.

### Backtest vs Live divergence

Strategy implementation likely has look-ahead bias. Ensure all logic works event-by-event, not on future data.

## Playbook Reference

The complete implementation plan is available in `.claude/playbooks/2025-10-01_hyperliquid-trading-system.md`. This playbook contains:

- 10 phases of atomic implementation tasks
- Exact file paths and line-by-line code specifications
- Verification steps for each phase
- Architecture decisions based on research-validated patterns
- **Karen quality gates at every phase boundary (mandatory)**

## Quality Assurance with Karen Agent

This project follows **Anthropic's 3-step AI Orchestration Cycle**:
1. **Information Gathering** - Research and analysis
2. **Task Creation** - TaskMaster generates atomic playbooks
3. **Quality Assurance** - Karen enforces zero-tolerance quality

### Karen Integration

**After completing each phase, Karen agent MUST review the code before proceeding to the next phase.**

#### Karen Review Process:
```bash
# Invoke Karen after completing Phase N:
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review Phase N",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review package <package-name> following ALL 6 phases. Include actual terminal outputs."
)
```

#### Karen's 6 Review Phases:
1. **Phase 0: Compilation Check** - `cargo build --package <pkg> --lib` must succeed
2. **Phase 1: Clippy Analysis** - Zero warnings at ALL levels (default + pedantic + nursery)
3. **Phase 2: rust-analyzer** - Deep diagnostics with zero issues
4. **Phase 3: Cross-file Validation** - No broken references after API changes
5. **Phase 4: Per-file Verification** - Each file passes individually
6. **Phase 5: Report Generation** - Complete report with actual terminal outputs
7. **Phase 6: Final Verification** - Release build + tests compile

#### Zero Tolerance Standard:
- ✅ Zero rustc errors/warnings
- ✅ Zero clippy warnings (including pedantic/nursery)
- ✅ Zero unused imports or dead code
- ✅ All public APIs documented
- ✅ All unsafe blocks justified
- ✅ Consistent patterns throughout

#### Blocking Failures:
If Karen finds issues:
1. **STOP** - Do not proceed to next phase
2. **Fix Atomically** - Address each issue following TaskMaster methodology
3. **Re-verify** - Run Karen again after fixes
4. **Iterate** - Repeat until Karen passes with zero issues

**A phase is ONLY complete after Karen review passes.**

## References

- **Barter-rs**: Event-driven architecture patterns (https://github.com/barter-rs/barter-rs)
- **Hyperliquid Docs**: API reference (https://hyperliquid.gitbook.io)
- **Alice Ryhl's Actor Guide**: Tokio channel patterns (https://ryhl.io/blog/actors-with-tokio/)
- **TimescaleDB**: Time-series best practices (https://docs.timescale.com)
