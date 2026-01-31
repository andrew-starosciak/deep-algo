# Polymarket BTC 15-Minute Binary Options Playbook

## Overview

Trade Bitcoin 15-minute binary options on Polymarket using liquidation cascade signals from Binance/Hyperliquid futures markets.

```
Liquidation Data → LiquidationCascadeSignal → Paper Trade Bot → Entry Strategy → Execute
```

## Current Status

| Component | Status | Location |
|-----------|--------|----------|
| Liquidation collection | WORKING | `collect-signals --sources liquidations` |
| Funding rate collection | WORKING | `collect-signals --sources funding` |
| LiquidationCascadeSignal | WORKING | `crates/signals/src/generator/liquidation_cascade.rs` |
| Polymarket odds collection | WORKING | `collect-polymarket` |
| Paper trading framework | WORKING | `polymarket-paper-trade` |
| Entry strategies | WORKING | `--entry-strategy edge_threshold` |
| Signal → Paper Trade wiring | WORKING | `--signal-mode cascade` with real signals |
| Startup script | WORKING | `scripts/start-polymarket-bot.sh` |

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    start-polymarket-bot.sh                               │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐  │
│   │ collect-signals  │    │ collect-polymarket│    │ polymarket-paper │  │
│   │ (background)     │    │ (background)      │    │ -trade (foreground│  │
│   └────────┬─────────┘    └────────┬─────────┘    └────────┬─────────┘  │
│            │                       │                        │            │
│            ▼                       ▼                        ▼            │
│   ┌──────────────────────────────────────────────────────────────────┐  │
│   │                         TimescaleDB                               │  │
│   │   liquidations │ funding_rates │ polymarket_odds │ paper_trades   │  │
│   └──────────────────────────────────────────────────────────────────┘  │
│                                     │                                    │
│                                     ▼                                    │
│                        ┌────────────────────────┐                       │
│                        │ LiquidationCascadeSignal│                       │
│                        │ - Query liquidations    │                       │
│                        │ - Build aggregate       │                       │
│                        │ - Compute signal        │                       │
│                        └───────────┬────────────┘                       │
│                                    ▼                                     │
│                        ┌────────────────────────┐                       │
│                        │   Decision Engine      │                       │
│                        │ - Kelly sizing         │                       │
│                        │ - Edge threshold       │                       │
│                        │ - Entry strategy       │                       │
│                        └───────────┬────────────┘                       │
│                                    ▼                                     │
│                        ┌────────────────────────┐                       │
│                        │   Paper Trade Logger   │                       │
│                        │ - Signal fired/not     │                       │
│                        │ - Trade placed/skipped │                       │
│                        │ - Entry timing         │                       │
│                        └────────────────────────┘                       │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### One-Command Startup

```bash
# Start everything with defaults
./scripts/start-polymarket-bot.sh

# Or with custom configuration
./scripts/start-polymarket-bot.sh \
  --duration 7d \
  --signal-mode cascade \
  --min-signal-strength 0.6 \
  --min-edge 0.03 \
  --entry-strategy edge_threshold \
  --entry-threshold 0.05 \
  --bankroll 10000

# Test with simulated signals first
./scripts/start-polymarket-bot.sh --simulated --duration 1h
```

The script starts:
1. **Data collector** - Liquidations + funding rates (background)
2. **Polymarket collector** - BTC market odds (background)
3. **Paper trading bot** - Real signals with entry timing (foreground)

Logs are written to `logs/` directory.

---

## Phase 1: Data Collection (Ready Now)

### Manual Start (if not using script)

```bash
# Terminal 1: Collect liquidations, funding rates, and order book from Binance/Hyperliquid
cargo run -p algo-trade-cli -- collect-signals \
  --duration 7d \
  --sources liquidations,funding,orderbook
```

### Start Collecting Polymarket Odds

```bash
# Terminal 2: Collect BTC-related Polymarket odds
cargo run -p algo-trade-cli -- collect-polymarket \
  --duration 7d \
  --market-pattern "Bitcoin|BTC" \
  --min-liquidity 5000 \
  --poll-interval-secs 15
```

### Verify Data is Flowing

```bash
# Check liquidations table
psql $DATABASE_URL -c "
SELECT COUNT(*), MIN(timestamp), MAX(timestamp)
FROM liquidations
WHERE timestamp > NOW() - INTERVAL '1 hour';
"

# Check funding rates
psql $DATABASE_URL -c "
SELECT COUNT(*), MIN(timestamp), MAX(timestamp)
FROM funding_rates
WHERE timestamp > NOW() - INTERVAL '1 hour';
"

# Check Polymarket odds
psql $DATABASE_URL -c "
SELECT COUNT(*), MIN(timestamp), MAX(timestamp)
FROM polymarket_odds
WHERE timestamp > NOW() - INTERVAL '1 hour';
"
```

---

## Phase 2: Signal Verification (Ready Now)

### Backtest the Liquidation Cascade Signal

```bash
# Run signal validation to see if liquidation_cascade has predictive power
cargo run -p algo-trade-cli -- validate-signals \
  --start 2026-01-01 \
  --end 2026-01-31 \
  --signal liquidation_cascade
```

### Check Signal Statistics

```bash
# Query liquidation aggregates
psql $DATABASE_URL -c "
SELECT
  date_trunc('hour', timestamp) as hour,
  SUM(CASE WHEN side = 'long' THEN usd_value ELSE 0 END) as long_liquidations,
  SUM(CASE WHEN side = 'short' THEN usd_value ELSE 0 END) as short_liquidations,
  COUNT(*) as count
FROM liquidations
WHERE timestamp > NOW() - INTERVAL '24 hours'
GROUP BY hour
ORDER BY hour DESC
LIMIT 24;
"
```

---

## Phase 3: Paper Trading (READY)

### Run with Real Signals

```bash
# Full paper trading with real liquidation cascade signals
cargo run -p algo-trade-cli -- polymarket-paper-trade \
  --duration 24h \
  --signal-mode cascade \
  --min-signal-strength 0.6 \
  --kelly-fraction 0.25 \
  --min-edge 0.03 \
  --min-volume-usd 100000 \
  --imbalance-threshold 0.6 \
  --liquidation-window-mins 5 \
  --entry-strategy edge_threshold \
  --entry-threshold 0.05 \
  --entry-fallback-mins 2
```

### Test with Simulated Signals

```bash
# Use simulated signals for testing without real data
cargo run -p algo-trade-cli -- polymarket-paper-trade \
  --duration 1h \
  --use-simulated-signals
```

### What the Bot Does

1. **Queries recent liquidations** from database (last 5 minutes)
2. **Aggregates into LiquidationAggregate** (long/short volumes)
3. **Builds SignalContext** with the aggregate
4. **Calls LiquidationCascadeSignal.compute()** to get real signal
5. **Evaluates trade decision** based on signal strength and edge
6. **Applies entry strategy** to optimize entry timing
7. **Logs everything** - signals, decisions, executions

---

## Signal Logic: LiquidationCascadeSignal

### How It Works

```
Liquidation Imbalance → Direction Signal
───────────────────────────────────────
More LONGS liquidated  → Price falling → Signal: DOWN (follow momentum)
More SHORTS liquidated → Price rising  → Signal: UP   (follow momentum)

Exhaustion Mode (reversal):
Long spike then decline → Signal: UP   (reversal expected)
Short spike then decline → Signal: DOWN (reversal expected)
```

### Signal Modes

| Mode | Strategy | When to Use |
|------|----------|-------------|
| `Cascade` | Follow momentum | High-conviction directional moves |
| `Exhaustion` | Reversal after spike | After large liquidation waves |
| `Combined` | Weight both | Default balanced approach |

### Configuration

```rust
let signal = LiquidationCascadeSignal::default()
    .with_mode(LiquidationSignalMode::Cascade)
    .with_cascade_config(CascadeConfig {
        min_volume_usd: dec!(100_000),    // $100k minimum
        imbalance_threshold: 0.6,         // 60% imbalance required
    })
    .with_min_volume(dec!(50_000));       // $50k minimum to generate signal
```

---

## Entry Timing Strategy

### For 15-Minute Windows

```bash
# Wait for 5% edge before entering, fallback at 13 minutes
--entry-strategy edge_threshold \
--entry-threshold 0.05 \
--entry-fallback-mins 2 \
--window-minutes 15
```

### Entry Decision Flow

```
1. Signal fires (liquidation_cascade strength > 0.6)
2. Check Polymarket odds for BTC 15-min market
3. Calculate edge: estimated_prob - market_price
4. If edge >= 5%: ENTER NOW
5. Else: Monitor odds every 10 seconds
6. If edge >= 5% before minute 13: ENTER
7. Else at minute 13: ENTER at current price (fallback)
```

---

## Risk Management

### Kelly Criterion Settings

| Parameter | Recommended | Description |
|-----------|-------------|-------------|
| `--kelly-fraction` | 0.25 | Quarter Kelly (conservative) |
| `--max-bet-fraction` | 0.05 | Max 5% of bankroll per bet |
| `--min-edge` | 0.03 | Minimum 3% expected edge |
| `--bankroll` | 10000 | Starting paper bankroll |

### Position Sizing Formula

```
Full Kelly: f* = (p(b+1) - 1) / b
Where: b = (1 - price) / price

Example:
- Estimated prob: 60%
- Market price: $0.52
- Odds (b): (1 - 0.52) / 0.52 = 0.923
- Full Kelly: (0.60 * 1.923 - 1) / 0.923 = 0.166 (16.6%)
- Quarter Kelly: 0.166 * 0.25 = 4.15% of bankroll
```

---

## Monitoring

### Check Paper Trading Stats

```bash
# Query paper trades for current session
psql $DATABASE_URL -c "
SELECT
  session_id,
  COUNT(*) as trades,
  SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
  ROUND(AVG(CASE WHEN outcome IS NOT NULL THEN
    CASE WHEN outcome = 'win' THEN 1.0 ELSE 0.0 END
  END) * 100, 1) as win_rate_pct,
  SUM(pnl) as total_pnl
FROM paper_trades
WHERE timestamp > NOW() - INTERVAL '24 hours'
GROUP BY session_id
ORDER BY MIN(timestamp) DESC;
"
```

### Check Entry Strategy Performance

```bash
psql $DATABASE_URL -c "
SELECT
  entry_strategy,
  COUNT(*) as trades,
  AVG(entry_offset_secs) as avg_entry_offset_secs,
  SUM(CASE WHEN used_fallback THEN 1 ELSE 0 END) as fallback_count,
  AVG(edge_at_entry) as avg_edge_at_entry
FROM paper_trades
WHERE timestamp > NOW() - INTERVAL '7 days'
GROUP BY entry_strategy;
"
```

---

## Next Steps

### Now: Run Paper Trading

```bash
# Start the full workflow
./scripts/start-polymarket-bot.sh --duration 7d

# Watch the logs
tail -f logs/paper-trade.log
```

### Then: Analyze Results

```bash
# Check paper trade performance
psql $DATABASE_URL -c "
SELECT
  COUNT(*) as trades,
  SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
  ROUND(AVG(CASE WHEN outcome IS NOT NULL THEN
    CASE WHEN outcome = 'win' THEN 1.0 ELSE 0.0 END
  END) * 100, 1) as win_rate_pct,
  SUM(pnl) as total_pnl
FROM paper_trades
WHERE timestamp > NOW() - INTERVAL '7 days';
"
```

### Production Criteria

Before live trading:
- [ ] 200+ paper trades completed
- [ ] Win rate > 52% with statistical significance (p < 0.05)
- [ ] Positive expected value after fees
- [ ] Entry strategy improves outcomes vs immediate entry

### Parameter Tuning

Based on paper trading results, tune:
- `--min-signal-strength` (higher = fewer but stronger signals)
- `--min-volume-usd` (higher = only major liquidation events)
- `--imbalance-threshold` (higher = stronger directional conviction)
- `--entry-threshold` (higher = wait for better entry prices)
