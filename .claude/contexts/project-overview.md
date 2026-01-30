# Project Context: Statistical Trading Engine

## Current State

**Phase:** Pre-implementation (specs complete, architecture designed)

**Focus:** Bitcoin 15-minute Polymarket binary options trading

## What We're Building

A statistical trading engine that:
1. Collects real-time market data (order books, funding rates, liquidations, news)
2. Generates validated trading signals with statistical significance
3. Combines signals into probability estimates
4. Executes trades on Polymarket with Kelly criterion sizing
5. Tracks performance with rigorous backtesting

## Key Specifications

Located in `specs/`:
- `OVERVIEW.md` - Architecture overview and crate analysis
- `ROADMAP.md` - 6-phase implementation plan
- `bitcoin-15min/` - Bitcoin 15-min strategy details
- `data-points/` - Reference Python scripts for data collection

## Signal Sources

| Signal | Data Source | Priority |
|--------|-------------|----------|
| Order Book Imbalance | Binance, Coinbase | P0 |
| Funding Rate Reversal | Coinglass, Binance | P0 |
| Liquidation Cascade | Binance | P0 |
| News/Sentiment | CryptoPanic | P1 |

## Existing Infrastructure

The codebase has mature infrastructure for:
- Event-driven trading (reusable traits)
- TimescaleDB storage (batch inserts, hypertables)
- WebSocket connections (Hyperliquid, adaptable)
- Backtesting framework (needs binary metrics)
- Bot orchestration (actor pattern)

## What Needs Building

1. **Data collectors** - Order book, funding, liquidations, news
2. **Signal generators** - SignalGenerator trait implementations
3. **Statistical validation** - Wilson CI, binomial tests, walk-forward
4. **Polymarket integration** - New exchange crate
5. **Binary backtesting** - Outcome simulation, fee model
6. **Risk management** - Kelly criterion, EV thresholds

## Go/No-Go Criteria

| Gate | Criteria | Phase |
|------|----------|-------|
| M1 | Data flowing >24h | End Phase 1 |
| M2 | Signal p < 0.10 | End Phase 2 |
| M3 | Backtest >53% win rate | End Phase 3 |
| M4 | Paper trading positive EV | End Phase 5 |
| M5 | Live profitable | End Phase 6 |

## Quick Reference

```bash
# Development
cargo check
cargo clippy -- -D warnings
cargo test

# Running (planned)
cargo run -p algo-trade-cli -- collect-signals --duration 24h
cargo run -p algo-trade-cli -- validate-signals --start 2025-01-01
cargo run -p algo-trade-cli -- binary-backtest --signal composite
cargo run -p algo-trade-cli -- binary-bot --mode paper
```
