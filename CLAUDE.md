# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**Statistical trading engine** in Rust with rigorous signal validation, backtesting, and risk management. Event-driven architecture enables backtest-live parity across any market type.

**Current Focus:** Bitcoin 15-minute Polymarket binary options trading using order book imbalance, funding rates, liquidation cascades, and news signals.

**Core Philosophy:** Every trading signal must be statistically validated before deployment. No signal goes live without p < 0.05 significance testing.

## Architecture

### Core Design Principles

1. **Statistical Rigor First**: All signals require hypothesis testing, confidence intervals, and walk-forward validation
2. **Event-Driven Architecture**: Components process discrete events sequentially - identical code runs in backtest and live
3. **Trait Abstraction**: `DataProvider`, `ExecutionHandler`, `SignalGenerator` traits enable market-agnostic strategies
4. **Actor Pattern**: Bots use Tokio channels following Alice Ryhl's actor guide

### Workspace Structure

```
crates/
├── core/                 # Event types, traits, TradingSystem engine
├── signals/              # Signal generators (order book, funding, liquidations, news)
├── exchange-hyperliquid/ # Hyperliquid REST/WebSocket (existing)
├── exchange-polymarket/  # Polymarket CLOB integration (new)
├── data/                 # TimescaleDB, Arrow, Parquet
├── strategy/             # Strategy trait implementations
├── backtest/             # Historical simulation, binary metrics
├── bot-orchestrator/     # Multi-bot coordination
├── web-api/              # Axum REST + WebSocket
└── cli/                  # Command-line interface
```

### Event Flow

```
MarketEvent → SignalGenerator::compute() → SignalValue
SignalValue → CompositeSignal → P(outcome) estimate
P(outcome) → RiskManager (Kelly, EV threshold) → BetDecision
BetDecision → ExecutionHandler → FillEvent/SettlementEvent
```

### Key Dependencies

- **tokio**: Async runtime (all async code uses Tokio)
- **axum**: Web framework for API
- **sqlx**: PostgreSQL/TimescaleDB (async, compile-time checked queries)
- **polars**: DataFrame processing for signal analysis
- **rust_decimal**: Financial precision (NEVER use f64 for money)
- **governor**: Rate limiting
- **statrs**: Statistical distributions and hypothesis testing

## Development Commands

### Building

```bash
cargo check                           # Check all crates
cargo build --release                 # Build release
cargo build -p algo-trade-signals     # Build specific crate
```

### Testing

```bash
cargo test                            # All tests
cargo test -p algo-trade-backtest     # Specific crate
cargo test --test integration_test    # Integration tests
```

### Running

```bash
# Data collection
cargo run -p algo-trade-cli -- collect-signals --duration 24h

# Signal validation
cargo run -p algo-trade-cli -- validate-signals --start 2025-01-01 --end 2025-01-28

# Binary backtest
cargo run -p algo-trade-cli -- binary-backtest --start 2024-01-01 --end 2025-01-01 --signal composite

# Paper trading
cargo run -p algo-trade-cli -- binary-bot --mode paper --duration 2w

# Live trading
cargo run -p algo-trade-cli -- binary-bot --mode live

# With debug logging
RUST_LOG=debug cargo run -p algo-trade-cli -- <command>
```

### Linting

```bash
cargo clippy -- -D warnings          # Clippy with warnings as errors
cargo fmt                             # Format code
```

## Critical Patterns

### 1. Financial Precision

**ALWAYS use `rust_decimal::Decimal` for prices, quantities, stakes, P&L**. Never use `f64` for financial calculations.

```rust
use rust_decimal::Decimal;
let stake: Decimal = "100.00".parse()?;
let price: Decimal = dec!(0.45);  // Use dec! macro
```

### 2. Signal Generator Trait

All signals implement the `SignalGenerator` trait for composability:

```rust
pub trait SignalGenerator: Send + Sync {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue>;
    fn name(&self) -> &str;
    fn weight(&self) -> f64 { 1.0 }
}

pub struct SignalValue {
    pub direction: Direction,    // Up, Down, Neutral
    pub strength: f64,           // 0.0 to 1.0
    pub confidence: f64,         // Statistical confidence
}
```

### 3. Statistical Validation Required

Every signal must pass validation before use:

```rust
// Required metrics for signal approval
pub struct SignalValidation {
    pub correlation: f64,           // With outcome
    pub p_value: f64,               // Must be < 0.05
    pub information_coefficient: f64,
    pub conditional_probability: f64, // P(Up | signal > threshold)
}
```

### 4. Binary Outcome Metrics

Use Wilson score CI and binomial tests for win rate confidence:

```rust
pub struct BinaryBacktestMetrics {
    pub total_bets: u32,
    pub wins: u32,
    pub win_rate: f64,
    pub wilson_ci_lower: f64,    // 95% CI
    pub wilson_ci_upper: f64,
    pub binomial_p_value: f64,   // H0: p = 0.50
    pub ev_per_bet: Decimal,
    pub kelly_fraction: f64,
}
```

### 5. Kelly Criterion for Sizing

Position sizing uses fractional Kelly with uncertainty adjustment:

```rust
// Kelly for binary bets: f* = (p(b+1) - 1) / b
// Where b = (1 - price) / price (net odds)
// Use 1/4 to 1/2 Kelly in practice
```

### 6. Rate Limiting

Use `governor` crate with per-exchange quotas:
- Binance: 1200 req/min
- Polymarket: Check API docs
- Apply exponential backoff on rate limit errors

## Current Implementation Focus

### Phase 1: Data Infrastructure (Active)
- Order book snapshots (1/sec, 20 levels)
- Funding rates (real-time + historical)
- Liquidation events (>$3K threshold)
- Polymarket odds polling
- News/sentiment collection

### Phase 2: Signal Development
- Order book imbalance signal
- Funding rate reversal signal
- Liquidation cascade signal
- News urgency signal
- Composite signal aggregation

### Phase 3: Backtesting Framework
- Binary outcome simulation
- Wilson CI, binomial significance
- Walk-forward optimization
- Conditional edge analysis

### Phase 4: Polymarket Integration
- CLOB order placement
- Settlement tracking
- Paper trading mode

### Phase 5: Risk Management
- Kelly criterion sizing
- Drawdown limits
- EV thresholds

### Phase 6: Production
- 15-minute window timing
- Live execution
- Monitoring dashboard

## Database Schema

### Key Tables (TimescaleDB Hypertables)

```sql
-- Order book snapshots
orderbook_snapshots (timestamp, symbol, exchange, bid_levels, ask_levels, imbalance)

-- Funding rates
funding_rates (timestamp, symbol, exchange, funding_rate, rate_percentile, rate_zscore)

-- Liquidations
liquidations (timestamp, symbol, exchange, side, quantity, price, usd_value)

-- Polymarket odds
polymarket_odds (timestamp, market_id, up_price, down_price, volume)

-- Binary trades
binary_trades (timestamp, market_id, outcome, shares, price, stake, signals_snapshot, pnl)
```

## Statistical Formulas Reference

### Wilson Score Confidence Interval
```
CI = (p̂ + z²/2n ± z√(p̂(1-p̂)/n + z²/4n²)) / (1 + z²/n)
where z = 1.96 for 95% CI
```

### Kelly Criterion (Binary)
```
f* = (p(b+1) - 1) / b
where p = win probability, b = net odds
```

### Required Sample Size (5% edge detection)
```
n ≈ 784 bets for 80% power at α = 0.05
```

### Expected Value per Bet
```
EV = p * (1 - c) - (1-p) * c - fee
where c = cost per share
```

## Go/No-Go Criteria

**Phase 2 Gate:** At least 1 signal with p < 0.10 predictive power
**Phase 3 Gate:** Composite signal >53% backtest win rate, p < 0.05
**Phase 6 Gate:** 200+ paper bets with positive EV, win rate >52%

## Troubleshooting

### Rate limit errors
Check `governor` quota configuration and add exponential backoff.

### Database connection errors
Verify TimescaleDB: `CREATE EXTENSION IF NOT EXISTS timescaledb;`

### Signal validation fails
Ensure sufficient historical data (>1000 data points for statistical power).

### Backtest vs live divergence
Check for look-ahead bias in signal computation. All logic must be point-in-time.

## References

- **Polymarket API**: https://docs.polymarket.com/
- **Binance Futures API**: https://binance-docs.github.io/apidocs/
- **Alice Ryhl's Actor Guide**: https://ryhl.io/blog/actors-with-tokio/
- **TimescaleDB**: https://docs.timescale.com
- **Wilson Score**: https://en.wikipedia.org/wiki/Binomial_proportion_confidence_interval
