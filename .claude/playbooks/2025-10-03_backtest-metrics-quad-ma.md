# Playbook: Backtest Metrics Reporting & Quad Moving Average Strategy

**Date**: 2025-10-03
**Agent**: TaskMaster
**Status**: Ready for Execution
**Context Report**: `.claude/context/2025-10-03_backtest-metrics-quad-ma.md`

---

## User Request

"I ran my backtest following the approach above. We have csv data showing ohlvc information and we ran it with our ma_crossover strategy, it ran the test and says Backtest Completed, but doesn't show any output in the console. I think that there was perhaps no trades made using the ma_crossover strategy. Id like to create a new strategy following the Quad Moving Average Strategy. I expect that if no trades were made to tell me no trades were made. There is a backtest library in python i've used and the report it generates contains data such as start,end duration, exposure time, equity final, equity peak, return %, buy and hold return %, return %, sharpe ratio etc. Does our backtest crate support this format as well? Let's review this"

---

## Scope Boundaries

### MUST DO

1. ✅ Extend `PerformanceMetrics` struct in `/home/a/Work/algo-trade/crates/core/src/engine.rs:8-16` with new fields
2. ✅ Add metric tracking fields to `TradingSystem` struct in `/home/a/Work/algo-trade/crates/core/src/engine.rs:18-33`
3. ✅ Modify `TradingSystem::run()` in `/home/a/Work/algo-trade/crates/core/src/engine.rs:94-116` to track metrics
4. ✅ Update `calculate_metrics()` in `/home/a/Work/algo-trade/crates/core/src/engine.rs:134-188` with new calculations
5. ✅ Extend `MetricsFormatter::format()` in `/home/a/Work/algo-trade/crates/core/src/metrics_formatter.rs:7-60` with new metrics
6. ✅ Create `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` with Quad MA strategy implementation
7. ✅ Export `QuadMaStrategy` in `/home/a/Work/algo-trade/crates/strategy/src/lib.rs:1`
8. ✅ Modify `run_backtest()` in `/home/a/Work/algo-trade/crates/cli/src/main.rs:103-142` to display metrics
9. ✅ Add `extract_symbol_from_csv()` helper in `/home/a/Work/algo-trade/crates/cli/src/main.rs:143`

### MUST NOT DO

1. ❌ DO NOT use `f64` for financial values (prices, returns, PnL) - use `Decimal` only
2. ❌ DO NOT change `Strategy` trait signature (public API, breaks live trading)
3. ❌ DO NOT modify CSV parsing logic (format is established)
4. ❌ DO NOT remove existing PerformanceMetrics fields (breaking change)
5. ❌ DO NOT use lookahead in Quad MA (must use only past data via VecDeque)
6. ❌ DO NOT create separate metrics calculator (use TradingSystem fields directly)
7. ❌ DO NOT modify `PositionTracker` (working correctly, not needed for this task)
8. ❌ DO NOT change how market events flow (preserve event-driven architecture)

---

## Atomic Tasks

### Task 1: Extend PerformanceMetrics Struct

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 1-16 (imports and struct definition)
**Complexity**: LOW
**Dependencies**: None

**Action**:
1. Add `use chrono::{DateTime, Utc};` to imports (line 4)
2. Replace `PerformanceMetrics` struct (lines 8-16) with extended version containing:
   - Time metrics: `start_time`, `end_time`, `duration`
   - Capital metrics: `initial_capital`, `final_capital`, `equity_peak`
   - Return metrics: `total_return`, `buy_hold_return`
   - Risk metrics: `sharpe_ratio`, `max_drawdown`
   - Trade metrics: `num_trades`, `win_rate`, `exposure_time_pct`

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 15

---

### Task 2: Add Tracking Fields to TradingSystem

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 18-33 (TradingSystem struct definition)
**Complexity**: LOW
**Dependencies**: Task 1

**Action**:
Add the following fields to `TradingSystem` struct (after line 32):
- `start_time: Option<DateTime<Utc>>`
- `end_time: Option<DateTime<Utc>>`
- `first_price: Option<Decimal>`
- `last_price: Option<Decimal>`
- `equity_peak: Decimal`
- `total_bars: usize`
- `bars_in_position: usize`

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 8

---

### Task 3: Initialize New Fields in Constructors

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 40-80 (`new()` and `with_capital()` methods)
**Complexity**: LOW
**Dependencies**: Task 2

**Action**:
1. In `new()` method (around line 45-59), add initialization:
   - `start_time: None`
   - `end_time: None`
   - `first_price: None`
   - `last_price: None`
   - `equity_peak: initial_capital`
   - `total_bars: 0`
   - `bars_in_position: 0`

2. Apply identical changes to `with_capital()` method (around line 68-80)

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 14

---

### Task 4: Track Metrics in run() Method

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 94-116 (run() method event loop)
**Complexity**: MEDIUM
**Dependencies**: Tasks 1-3

**Action**:
Add metric tracking inside the event loop (after line 95, before strategy processing):
1. Extract timestamp and close price from MarketEvent
2. Track first/last timestamps and prices (if None, set them)
3. Increment `total_bars` counter
4. If position exists, increment `bars_in_position`
5. Update `equity_peak` if current equity exceeds it

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 25

---

### Task 5: Update calculate_metrics() Method

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 134-188 (calculate_metrics() function)
**Complexity**: MEDIUM
**Dependencies**: Tasks 1-4

**Action**:
Add new metric calculations in `calculate_metrics()`:
1. Calculate `buy_hold_return` using first_price and last_price
2. Calculate `exposure_time_pct` from bars_in_position / total_bars
3. Calculate `duration` from end_time - start_time
4. Return updated `PerformanceMetrics` struct with all new fields

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 40

---

### Task 6: Update MetricsFormatter

**File**: `/home/a/Work/algo-trade/crates/core/src/metrics_formatter.rs`
**Location**: Lines 7-60 (format() method)
**Complexity**: LOW
**Dependencies**: Task 1

**Action**:
Replace entire `format()` method with extended version that displays:
1. **Time Period** section: Start, End, Duration, Exposure Time
2. **Portfolio Performance** section: Initial Capital, Final Capital, Equity Peak, Total Return, Buy & Hold Return, Sharpe Ratio, Max Drawdown
3. **Trade Statistics** section: Total Trades, Win Rate
4. Special warning when `num_trades == 0`

**Verification**: `cargo check -p algo-trade-core`

**Estimated LOC**: 35

---

### Task 7: Create Quad MA Strategy

**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` (NEW FILE)
**Location**: N/A (new file)
**Complexity**: MEDIUM
**Dependencies**: None

**Action**:
Create complete Quad MA strategy implementation:
1. Define `QuadMaStrategy` struct with:
   - `symbol: String`
   - 4 period parameters (p1, p2, p3, p4)
   - 4 price buffers (VecDeque for each period)
   - `last_alignment: Option<SignalDirection>`
2. Implement constructor `new(symbol, p1, p2, p3, p4)`
3. Implement helper methods:
   - `calculate_ma()` - simple moving average
   - `check_bullish_alignment()` - MA1 > MA2 > MA3 > MA4
   - `check_bearish_alignment()` - MA1 < MA2 < MA3 < MA4
4. Implement `Strategy` trait:
   - `on_market_event()` - filters by symbol, updates buffers, checks alignment, emits signal on crossover
   - `name()` - returns "Quad MA"

**Verification**: `cargo check -p algo-trade-strategy`

**Estimated LOC**: 120

---

### Task 8: Export Quad MA Strategy

**File**: `/home/a/Work/algo-trade/crates/strategy/src/lib.rs`
**Location**: Lines 1-6 (module declarations and exports)
**Complexity**: LOW
**Dependencies**: Task 7

**Action**:
1. Add `pub mod quad_ma;` after `pub mod ma_crossover;`
2. Add `pub use quad_ma::QuadMaStrategy;` after `pub use ma_crossover::MaCrossoverStrategy;`

**Verification**: `cargo build -p algo-trade-strategy`

**Estimated LOC**: 2

---

### Task 9: Update CLI run_backtest() - Display Metrics & Strategy Factory

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Lines 103-142 (run_backtest() function)
**Complexity**: MEDIUM
**Dependencies**: Tasks 1-8

**Action**:
1. Add `extract_symbol_from_csv()` helper function that reads first CSV row and returns symbol (column index 1)
2. Replace `run_backtest()` function with updated version that:
   - Calls `extract_symbol_from_csv()` to get symbol from CSV
   - Implements strategy factory pattern matching on `strategy` arg:
     - "ma_crossover" → MaCrossoverStrategy::new(symbol, 10, 30)
     - "quad_ma" → QuadMaStrategy::new(symbol, 5, 8, 13, 21)
     - Unknown strategy → bail with available options
   - Captures `metrics` from `system.run().await?`
   - Formats metrics with `MetricsFormatter::format(&metrics)`
   - Prints formatted report to console

**Verification**: `cargo build -p algo-trade-cli`

**Estimated LOC**: 50

---

## Task Dependencies Graph

```
Task 1 (PerformanceMetrics) ──┬─→ Task 2 (TradingSystem fields)
                               ├─→ Task 6 (MetricsFormatter)
                               └─→ Task 9 (CLI display)

Task 2 ──→ Task 3 (Constructors) ──→ Task 4 (run() tracking) ──→ Task 5 (calculate_metrics)

Task 7 (Quad MA) ──→ Task 8 (export) ──→ Task 9 (CLI factory)

Tasks 1-8 ──→ Task 9 (CLI integration)
```

**Execution Order**:
1. Task 1 (PerformanceMetrics struct)
2. Task 2 (TradingSystem fields)
3. Task 3 (Constructor initialization)
4. Task 4 (run() method tracking)
5. Task 5 (calculate_metrics updates)
6. Task 6 (MetricsFormatter updates)
7. Task 7 (Quad MA strategy)
8. Task 8 (Export Quad MA)
9. Task 9 (CLI updates)

---

## Verification Checklist

### Per-Task Verification

- [ ] Task 1: `cargo check -p algo-trade-core` succeeds
- [ ] Task 2: `cargo check -p algo-trade-core` succeeds
- [ ] Task 3: `cargo check -p algo-trade-core` succeeds
- [ ] Task 4: `cargo check -p algo-trade-core` succeeds
- [ ] Task 5: `cargo check -p algo-trade-core` succeeds
- [ ] Task 6: `cargo check -p algo-trade-core` succeeds
- [ ] Task 7: `cargo check -p algo-trade-strategy` succeeds
- [ ] Task 8: `cargo build -p algo-trade-strategy` succeeds
- [ ] Task 9: `cargo build -p algo-trade-cli` succeeds

### Integration Verification

```bash
# Test MA Crossover with ETH data (should work now with symbol extraction)
cargo run -p algo-trade-cli -- backtest --data data/eth_5m.csv --strategy ma_crossover

# Test Quad MA with ETH data
cargo run -p algo-trade-cli -- backtest --data data/eth_5m.csv --strategy quad_ma

# Verify metrics displayed include:
# - Start/End dates (2025-10-02 to 2025-10-03)
# - Duration (1 day 0 hours)
# - Exposure Time (%)
# - Buy & Hold Return (manual verify: (last_eth_price - first_eth_price) / first_eth_price)
# - All other existing metrics
```

### Manual Calculation Verification

```bash
# From CSV data:
# First ETH price: 4361.6 (2025-10-02T00:00:00)
# Last ETH price: 4471.8 (2025-10-03T00:00:00)
# Expected Buy & Hold: (4471.8 - 4361.6) / 4361.6 = 0.0252 = 2.52%

# Run backtest and verify Buy & Hold Return ≈ 2.52%
```

### Karen Quality Gates (MANDATORY)

- [ ] Phase 0: `cargo build --workspace` succeeds
- [ ] Phase 1: Zero clippy warnings (`cargo clippy --workspace -- -D warnings`)
- [ ] Phase 2: Zero rust-analyzer diagnostics
- [ ] Phase 3: Cross-file validation (imports resolve correctly)
- [ ] Phase 4: Per-file verification (each modified file compiles individually)
- [ ] Phase 5: Report generation (capture terminal outputs)
- [ ] Phase 6: Final verification (`cargo test --workspace` passes)

---

## Complexity Summary

| Task | LOC | Time | Risk | Reason |
|------|-----|------|------|--------|
| 1 | 15 | 10m | LOW | Struct extension with new fields |
| 2 | 8 | 5m | LOW | Field additions to struct |
| 3 | 14 | 10m | LOW | Constructor initialization |
| 4 | 25 | 20m | MED | Event loop metric tracking |
| 5 | 40 | 25m | MED | New calculations (buy & hold, exposure) |
| 6 | 35 | 15m | LOW | Display formatting |
| 7 | 120 | 40m | MED | New strategy implementation |
| 8 | 2 | 2m | LOW | Module export |
| 9 | 50 | 30m | MED | CLI factory and display |

**Total**: ~309 LOC, ~2.5 hours, MEDIUM complexity

---

## Success Criteria

- [ ] CLI backtest command displays formatted metrics report to console
- [ ] Report explicitly states "No trades made" when num_trades == 0
- [ ] Quad MA strategy implemented and selectable via CLI `--strategy quad_ma`
- [ ] All Python backtest library metrics present: start, end, duration, exposure time, equity final/peak, return %, buy & hold return %, sharpe ratio, max drawdown
- [ ] MA Crossover strategy symbol configurable (not hardcoded to BTC)
- [ ] Backtest runs successfully on ETH data with both strategies
- [ ] Metrics accurate: buy & hold return matches manual calculation
- [ ] Exposure time correctly reflects time in position

---

## Notes

**Context Report Reference**: All architectural decisions, edge cases, and constraints documented in `.claude/context/2025-10-03_backtest-metrics-quad-ma.md`

**Critical Pattern Enforcement**:
- ALL financial values use `rust_decimal::Decimal` (never f64)
- Percentages (Sharpe, win_rate, exposure_time_pct) use f64 (non-financial)
- VecDeque pattern ensures no lookahead bias in strategies
- Event-driven architecture preserved for backtest-live parity

**Symbol Mismatch Fix**:
- CSV has ETH data, MA Crossover was hardcoded to BTC
- Extract symbol from CSV first row (column index 1)
- Pass extracted symbol to strategy constructor

**Quad MA Parameters**:
- Default Fibonacci periods: 5, 8, 13, 21
- Bullish alignment: MA(5) > MA(8) > MA(13) > MA(21)
- Signal emitted only on alignment change (crossover event)
- Warmup period: 21 bars (longest MA period)

---

**TaskMaster Completion**: ✅ Playbook Ready
**Next Step**: Execute tasks 1-9 in dependency order, then invoke Karen for quality review
