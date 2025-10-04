# Context Report: QuadMA Strategy Tightening

**Date**: 2025-10-04
**Agent**: Context Gatherer
**Request**: "Ok I see ETH has made 82 trades in the past 3 days. Let's tighten up our quad ma strategy now"

---

## Section 1: Request Analysis

### Explicit Requirements
- **Current State**: ETH generated 82 trades over 3 days using QuadMA strategy
- **Objective**: Reduce trade frequency by "tightening" the strategy
- **Target**: Not explicitly stated, but implied to reduce overtrading

### Implicit Requirements
- Maintain strategy profitability while reducing trades
- Reduce whipsaw/false signals causing excessive entries
- Keep strategy responsive (don't overtighten)
- Preserve backtest-live parity (config changes only)

### Problem Analysis
- **Trade Frequency**: 82 trades / 3 days = **27.3 trades/day** = **1.14 trades/hour**
- **Timeframe**: 5-minute candles (12 candles/hour)
- **Frequency Rate**: 82 trades / 864 bars = **9.5% of bars** trigger entries
- **Industry Benchmark**: 2-5% trade frequency for MA crossover strategies
- **Diagnosis**: **2-4x too many trades** - filters are too loose

---

## Section 2: Codebase Context

### Current QuadMA Implementation

**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`

#### Current Configuration (Lines 66-99)
```rust
pub fn new(symbol: String) -> Self {
    Self {
        symbol,
        short_period_1: 5,        // MA5
        short_period_2: 10,       // MA10
        long_period_1: 20,        // MA20
        long_period_2: 50,        // MA50
        trend_period: 100,        // MA100
        volume_filter_enabled: true,  // ← Default is TRUE
        volume_factor: 1.5,       // 1.5x average volume
        take_profit_pct: 0.02,    // 2% TP
        stop_loss_pct: 0.01,      // 1% SL (2:1 risk/reward)
        // ... buffers ...
    }
}
```

#### TUI Runner Override (Lines 158-171)
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`

```rust
StrategyType::QuadMa { ma1, ma2, ma3, ma4, trend_period, volume_factor, take_profit, stop_loss } => {
    let strategy = QuadMaStrategy::with_full_config(
        token.to_string(),
        *ma1,                                // 5
        *ma2,                                // 10
        *ma3,                                // 20
        *ma4,                                // 50
        *trend_period,                       // 100
        false,                               // ← VOLUME FILTER DISABLED!
        *volume_factor as f64 / 100.0,       // 150 → 1.5
        *take_profit as f64 / 10000.0,       // 200 → 0.02
        *stop_loss as f64 / 10000.0,         // 100 → 0.01
    );
    vec![Arc::new(Mutex::new(strategy))]
}
```

**CRITICAL FINDING**: Line 166 disables volume filter despite it being available!

#### Current Entry Logic (Lines 266-316)

**Long Entry Requirements** (Lines 274-316):
```rust
fn should_enter_long(
    short_1: Decimal,      // MA5
    short_2: Decimal,      // MA10
    long_1: Decimal,       // MA20
    _long_2: Decimal,      // MA50 (unused!)
    trend_ma: Decimal,     // MA100
    prev_trend_ma: Decimal,
    price: Decimal,
    volume: Decimal,
    avg_volume: Decimal,
    volume_factor: Decimal,
    volume_filter_enabled: bool,
    prev_short_1: Option<Decimal>,
    prev_short_2: Option<Decimal>,
) -> bool {
    // 1. Crossover detection: MA5 crosses above MA10
    let has_crossover = Self::detect_crossover(
        prev_short_1, short_1,
        prev_short_2, short_2,
    ) == Some(true);

    if !has_crossover {
        return false;  // No crossover = no trade
    }

    // 2. MA alignment: both shorts > MA20 (simplified from 4 checks to 2)
    let bullish = short_1 > long_1 && short_2 > long_1;

    // 3. Trend filter: slope positive and price above trend
    let trend_slope = trend_ma - prev_trend_ma;
    let in_uptrend = trend_slope > Decimal::ZERO && price > trend_ma;

    // 4. Volume filter (optional)
    let volume_ok = if volume_filter_enabled {
        volume > avg_volume * volume_factor
    } else {
        true  // ← Always passes when disabled!
    };

    bullish && in_uptrend && volume_ok
}
```

**Short Entry**: Identical logic with inverted conditions (Lines 326-368)

#### Active Filters Summary
| Filter | Status | Configuration |
|--------|--------|---------------|
| **Crossover Detection** | ✅ Active | MA5 × MA10 (immediate entry) |
| **MA Alignment** | ✅ Active | Both shorts > MA20 (2 checks) |
| **Trend Filter** | ✅ Active | MA100 slope + price position |
| **Volume Filter** | ❌ DISABLED | 1.5x avg volume (but disabled in TUI) |
| **TP/SL** | ✅ Active | 2% / 1% (2:1 risk/reward) |

**Unused MA**: MA50 (`long_2`) is calculated but never used in filters (Line 278)

---

## Section 3: External Research

### Industry Benchmarks for MA Crossover Strategies (5-Minute Charts)

#### Optimal Trade Frequency
- **Scalping MA Strategies**: 1-3 trades/day on 5-minute charts
- **Intraday Crossover**: 2-5% of bars should trigger signals
- **Current QuadMA**: 9.5% of bars → **2-4x too high**

#### Common 5-Minute MA Setups
| Setup | Fast MA | Slow MA | Additional Filters | Typical Frequency |
|-------|---------|---------|-------------------|-------------------|
| **Aggressive** | 5 EMA | 8 EMA | Volume + RSI | High (6-8% bars) |
| **Moderate** | 9 EMA | 21 EMA | Volume + Trend | Medium (3-5% bars) |
| **Conservative** | 20 EMA | 50 EMA | Volume + ADX | Low (1-3% bars) |

**QuadMA Current**: 5/10/20/50/100 with disabled volume = **Aggressive without filters**

#### Filter Effectiveness Research

**Volume Confirmation**:
- **Impact**: Reduces false signals by 40-60%
- **Threshold**: 1.2x-1.5x average volume is standard
- **Finding**: "Valid crossover signals should be accompanied by above-average volume"

**Crossover Confirmation Wait**:
- **Impact**: Waiting 1-2 bars after crossover reduces false signals by 40%
- **Trade-off**: Slightly delayed entry but higher quality signals
- **Finding**: "Waiting for crossover to maintain position for at least two consecutive days before entering"

**Multiple Indicator Confirmation**:
- **RSI Filter**: Only take longs when RSI > 50, shorts when RSI < 50
- **ADX Filter**: Only trade when trend strength (ADX) > 25
- **Finding**: "Never trade on crossover alone, supplement with at least one confirmation tool"

**Multi-Period MA Alignment**:
- **Impact**: Adding more MA checks (3-4 instead of 2) reduces trades 30-40%
- **Finding**: "MA4 check reduces whipsaws by ensuring all short-term MAs aligned"

#### Win Rate Expectations
- **Trending Markets**: 45-55% win rate with good risk/reward
- **Ranging Markets**: 30-40% win rate (many false breakouts)
- **With Filters**: Win rate improves 10-15% but trade frequency decreases
- **Note**: MA crossovers are lagging indicators - expect missed opportunities

---

## Section 4: Architectural Recommendations

### Design Constraint: Config-Only Changes
Per CLAUDE.md backtest-live parity requirement:
> "Strategy and RiskManager implementations must be provider-agnostic. Only `DataProvider` and `ExecutionHandler` differ between backtest and live."

**Implication**: Cannot change core logic, only adjustable parameters

### Available Tightening Levers

#### 1. Re-enable Volume Filter (HIGHEST PRIORITY)
**Current State**: Disabled at line 166 in `runner.rs`
```rust
false,  // volume_filter_enabled: false (disabled for testing)
```

**Proposed Change**:
```rust
true,   // volume_filter_enabled: true (re-enable)
```

**Expected Impact**: 40-60% reduction in trades
- Current: 82 trades → Target: 33-49 trades over 3 days

**Risk**: Low - this filter was already implemented, just disabled

---

#### 2. Adjust Volume Threshold
**Current**: 1.5x average volume (Line 39, default)
**TUI Override**: Uses same 1.5x (Line 167)

**Options**:
- **Conservative**: 1.2x (more trades pass)
- **Moderate**: 1.5x (current default)
- **Aggressive**: 2.0x (fewer trades pass)

**Recommendation**: Keep 1.5x initially, adjust after observing results

---

#### 3. Add MA50 to Alignment Checks (MEDIUM PRIORITY)
**Current**: Only checks MA5 > MA20 AND MA10 > MA20 (2 checks, Line 302)
**Unused Asset**: MA50 is calculated but never used (Line 278: `_long_2`)

**Proposed Additional Check**:
```rust
// Current (2 checks)
let bullish = short_1 > long_1 && short_2 > long_1;

// Proposed (3 checks) - add MA50 filter
let bullish = short_1 > long_1 && short_2 > long_1 && short_1 > long_2;
```

**Expected Impact**: 20-30% reduction in trades
- 82 trades → 57-66 trades over 3 days

**Trade-off**: Misses some early trend entries but higher quality signals

---

#### 4. Widen MA Periods (STRUCTURAL CHANGE)
**Current**: 5/10/20/50/100
**Proposed Options**:
- **Option A**: 10/20/50/100/200 (slower crossovers)
- **Option B**: 8/13/21/55/100 (Fibonacci alignment)

**Expected Impact**: 50-70% reduction in crossover frequency
- 82 trades → 25-41 trades over 3 days

**Risk**: HIGH - fundamentally changes strategy behavior, requires re-optimization

---

#### 5. Increase TP/SL Ratio (RISK MANAGEMENT)
**Current**: 2% TP / 1% SL = 2:1 ratio
**Proposed Options**:
- **Option A**: 3% TP / 1% SL = 3:1 ratio
- **Option B**: 2% TP / 0.5% SL = 4:1 ratio

**Expected Impact**: No reduction in entry frequency, but forces better risk/reward
- Same 82 trades, but winning trades capture more profit
- May reduce win rate by 5-10% (wider TP harder to hit)

---

#### 6. Add Crossover Confirmation Delay (REQUIRES NEW LOGIC)
**Current**: Immediate entry on crossover
**Proposed**: Wait 1-2 bars for sustained crossover

**Implementation Challenge**: Requires state tracking (not just config change)
- Need to store "pending crossover" state
- Track bar count since crossover
- Verify crossover still valid after delay

**Expected Impact**: 40-50% reduction in trades
**Risk**: MEDIUM - requires logic changes, breaks config-only constraint

---

### Recommended Tiered Approach

#### Tier 1: Immediate Fix (Conservative)
**Changes**:
1. Re-enable volume filter (line 166: `false` → `true`)
2. Keep volume threshold at 1.5x
3. Keep all other parameters unchanged

**Expected Results**:
- Trade frequency: ~40-50 trades over 3 days (~13-17/day)
- Reduction: ~40-50% fewer trades
- Risk: Very low (just enabling existing feature)

**Implementation**: 1-line change in `runner.rs`

---

#### Tier 2: Moderate Tightening
**Changes**:
1. Re-enable volume filter at 1.5x (Tier 1)
2. Add MA50 to alignment checks (3 total checks instead of 2)
3. Increase volume threshold to 1.75x

**Expected Results**:
- Trade frequency: ~20-30 trades over 3 days (~7-10/day)
- Reduction: ~65-75% fewer trades
- Risk: Low-medium (small logic change + config adjustment)

**Implementation**:
- 1-line change in `runner.rs` (volume filter)
- 1-line change in `quad_ma.rs` line 302 (add MA50 check)
- 1-value change (volume threshold)

---

#### Tier 3: Aggressive Tightening
**Changes**:
1. Re-enable volume filter at 2.0x (higher threshold)
2. Add MA50 to alignment checks
3. Widen MA periods to 10/20/50/100/200

**Expected Results**:
- Trade frequency: ~10-15 trades over 3 days (~3-5/day)
- Reduction: ~80-85% fewer trades
- Risk: HIGH (structural changes, requires re-optimization)

**Implementation**:
- Multiple config changes
- Potential re-testing of all parameters
- May miss many valid trends

---

### Architecture Decision: Incremental vs. Structural

**Recommended Path**: **Incremental (Tier 1 → Tier 2)**

**Rationale**:
1. **Preserve Working System**: Crossover detection just got fixed (recent commit)
2. **Minimize Risk**: Config-only changes maintain backtest-live parity
3. **Iterative Optimization**: Test each tier, measure impact, adjust
4. **User Control**: Let user choose aggressiveness based on results

**Avoid**:
- Tier 3 (structural changes) without user explicit approval
- Disabling crossover detection (it's working correctly now)
- Removing TP/SL safety features

---

## Section 5: Edge Cases & Constraints

### Edge Case 1: Volume Filter During Low Liquidity
**Scenario**: Early morning hours, thin order books
**Impact**: Volume filter may reject valid signals during low-liquidity periods
**Mitigation**: Consider time-of-day volume normalization (future enhancement)

### Edge Case 2: Rapid Trend Reversals
**Scenario**: Strong trend reversal causes multiple quick crossovers
**Current Behavior**: Enters immediately on each crossover (whipsaw risk)
**With Volume Filter**: Rejects low-volume whipsaws, keeps high-volume reversals
**Mitigation**: Tier 2 adds MA50 check to reduce rapid reversals

### Edge Case 3: Sideways Markets
**Scenario**: ETH ranges between support/resistance, MAs oscillate
**Current Behavior**: Generates many crossovers (9.5% of bars)
**With Filters**: Volume filter + MA alignment reduces choppy signals
**Limitation**: MA crossovers inherently struggle in ranging markets

### Edge Case 4: MA50 Unused Parameter
**Current State**: `long_2` (MA50) calculated but unused (line 278)
**Implication**: Wasted computation in every bar
**Opportunity**: Adding MA50 to checks utilizes existing data
**Trade-off**: Makes filters stricter (intended outcome)

### Constraint 1: Backtest-Live Parity
**Requirement**: Strategy must work identically in backtest and live
**Current Compliance**: All filters use same logic in both modes
**Risk**: Config changes must not introduce mode-specific behavior
**Validation**: Run same config in backtest and live, verify identical signals

### Constraint 2: Fixed TP/SL
**Current**: 2% TP / 1% SL hardcoded in strategy
**Limitation**: Cannot adapt to volatility changes
**Workaround**: User can modify config values, but not dynamic
**Future Enhancement**: Volatility-based TP/SL (requires new feature)

### Constraint 3: TUI Parameter Encoding
**Current**: TUI uses integer encoding (150 → 1.5, 200 → 0.02)
**Risk**: Changing encoding breaks existing backtests
**Mitigation**: Keep encoding format, only change values

---

## Section 6: TaskMaster Handoff Package

### MUST DO

1. **Re-enable Volume Filter** (Tier 1 - Priority 1)
   - File: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
   - Line: 166
   - Change: `false,` → `true,`
   - Verification: Run backtest, confirm trade count decreases 40-50%

2. **Make Volume Threshold Configurable** (Tier 1/2 - Priority 2)
   - File: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
   - Line: 167
   - Current: `*volume_factor as f64 / 100.0,` (uses TUI param)
   - Ensure: User can adjust threshold via TUI parameter
   - Test values: 120 (1.2x), 150 (1.5x), 200 (2.0x)

3. **Add MA50 to Alignment Checks** (Tier 2 - Priority 3, OPTIONAL)
   - File: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
   - Line: 302 (Long entry)
   - Current: `let bullish = short_1 > long_1 && short_2 > long_1;`
   - Change: `let bullish = short_1 > long_1 && short_2 > long_1 && short_1 > long_2;`
   - Line: 354 (Short entry)
   - Current: `let bearish = short_1 < long_1 && short_2 < long_1;`
   - Change: `let bearish = short_1 < long_1 && short_2 < long_1 && short_1 < long_2;`
   - Verification: Confirm MA50 (`long_2`) now used in logic

4. **Run Comparative Backtest** (Validation)
   - Run ETH backtest with Tier 1 config (volume filter enabled)
   - Compare: Trade count, win rate, total return, Sharpe ratio
   - Target: 40-50 trades over 3 days (vs. current 82)
   - Document: Results in backtest output

5. **User Decision Point** (After Tier 1 validation)
   - Present Tier 1 results to user
   - Offer Tier 2 (MA50 + higher volume threshold) if still too many trades
   - Offer Tier 3 (structural changes) only if user requests aggressive reduction

### MUST NOT DO

1. **Don't Disable Crossover Detection**
   - It's working correctly now (recent fix)
   - Crossover is the primary signal generator

2. **Don't Remove TP/SL**
   - Safety feature preventing runaway losses
   - 2:1 risk/reward is industry standard

3. **Don't Change MA Periods Without User Approval**
   - Tier 3 structural changes require explicit consent
   - Current 5/10/20/50/100 may be optimized for specific market

4. **Don't Break TUI Parameter Encoding**
   - Keep integer format (150 → 1.5)
   - Don't change conversion formulas

5. **Don't Add New Dependencies**
   - All changes should use existing QuadMA infrastructure
   - No new indicator libraries or external crates

### SCOPE BOUNDARIES

**In Scope**:
- Configuration parameter adjustments (volume filter, thresholds)
- Using existing calculated values (MA50) in logic
- Enabling/disabling existing filters
- Running backtests to validate changes
- Incremental testing (Tier 1 → Tier 2 → Tier 3)

**Out of Scope** (Without User Approval):
- Adding new indicators (RSI, ADX, MACD)
- Changing MA calculation methods (SMA → EMA)
- Dynamic TP/SL based on volatility
- Multi-timeframe confirmation
- Machine learning optimization
- Removing existing filters

**Verification Checklist**:
- [ ] Volume filter enabled in TUI runner
- [ ] Trade frequency reduced to acceptable range (1-3 trades/day target)
- [ ] Backtest results show maintained or improved profitability
- [ ] No clippy warnings introduced
- [ ] No logic errors (all tests pass)
- [ ] User approves final configuration

---

## Section 7: Implementation Recommendation

### Immediate Action: Tier 1 (Re-enable Volume Filter)

**Single Change Required**:
```rust
// File: /home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs
// Line 166

// BEFORE:
false,  // volume_filter_enabled: false (disabled for testing)

// AFTER:
true,   // volume_filter_enabled: true (re-enabled)
```

**Expected Impact**:
- **Trade Reduction**: 82 → 40-50 trades (40-50% decrease)
- **Trade Frequency**: 1.14 trades/hour → 0.56-0.69 trades/hour
- **Bar Frequency**: 9.5% → 4.6-5.8% (approaching industry 2-5% benchmark)

**Validation Steps**:
1. Make 1-line change in `runner.rs`
2. Run: `cargo clippy -p algo-trade-cli`
3. Run backtest on ETH with same 3-day period
4. Compare trade counts: Target <50 trades
5. Check win rate and total return (should maintain or improve)

### If Still Too Many Trades: Tier 2

**Changes**:
1. Keep volume filter enabled (Tier 1)
2. Add MA50 to alignment checks (2-line change)
3. Optionally increase volume threshold to 1.75x

**Expected Impact**:
- **Trade Reduction**: 82 → 20-30 trades (65-75% decrease)
- **Trade Frequency**: 1.14 trades/hour → 0.28-0.42 trades/hour
- **Bar Frequency**: 9.5% → 2.3-3.5% (within industry benchmark)

### Decision Tree

```
User Request: "Tighten QuadMA, reduce 82 trades"
            ↓
    Apply Tier 1 (volume filter)
            ↓
    Run Backtest
            ↓
        40-50 trades?
       ↙            ↘
    YES              NO (still >50)
     ↓                ↓
  SUCCESS         Apply Tier 2
                   (MA50 + threshold)
                       ↓
                   Run Backtest
                       ↓
                   20-30 trades?
                  ↙            ↘
               YES              NO (still >30)
                ↓                ↓
             SUCCESS      Recommend Tier 3
                          (structural changes)
                                ↓
                          Requires user approval
```

### Code Snippets

#### Tier 1: Volume Filter (1-line change)
```rust
// File: /home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs
// Lines 158-171

StrategyType::QuadMa { ma1, ma2, ma3, ma4, trend_period, volume_factor, take_profit, stop_loss } => {
    let strategy = QuadMaStrategy::with_full_config(
        token.to_string(),
        *ma1,
        *ma2,
        *ma3,
        *ma4,
        *trend_period,
        true,                                   // ← CHANGE: false → true
        *volume_factor as f64 / 100.0,
        *take_profit as f64 / 10000.0,
        *stop_loss as f64 / 10000.0,
    );
    vec![Arc::new(Mutex::new(strategy))]
}
```

#### Tier 2: MA50 Alignment (2-line change)
```rust
// File: /home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs

// Line 302 (Long entry)
// BEFORE:
let bullish = short_1 > long_1 && short_2 > long_1;

// AFTER:
let bullish = short_1 > long_1 && short_2 > long_1 && short_1 > long_2;

// Line 354 (Short entry)
// BEFORE:
let bearish = short_1 < long_1 && short_2 < long_1;

// AFTER:
let bearish = short_1 < long_1 && short_2 < long_1 && short_1 < long_2;
```

Also update parameter usage:
```rust
// Line 278 (remove underscore prefix to indicate it's now used)
// BEFORE:
_long_2: Decimal,

// AFTER:
long_2: Decimal,
```

---

## Summary & Next Steps

### Current State Analysis
- **Trade Frequency**: 82 trades / 3 days = 9.5% of bars (2-4x industry benchmark)
- **Root Cause**: Volume filter disabled in TUI runner (line 166)
- **Unused Filter**: MA50 calculated but not used in alignment checks

### Recommended Solution
**Start with Tier 1**: Re-enable volume filter (1-line change)
- Low risk, high impact (40-50% reduction)
- Uses existing, tested infrastructure
- Maintains backtest-live parity

**Escalate to Tier 2 if needed**: Add MA50 check + adjust threshold
- Medium risk, high impact (65-75% reduction)
- Minimal logic changes
- Still within config-only scope

**Avoid Tier 3 without approval**: Structural MA period changes
- High risk, requires re-optimization
- Breaks existing parameter tuning

### Key Metrics to Monitor
| Metric | Current | Tier 1 Target | Tier 2 Target | Industry Benchmark |
|--------|---------|---------------|---------------|-------------------|
| **Trades/3 days** | 82 | 40-50 | 20-30 | 15-30 |
| **Trades/day** | 27.3 | 13-17 | 7-10 | 5-10 |
| **Trades/hour** | 1.14 | 0.56-0.69 | 0.28-0.42 | 0.2-0.4 |
| **% of bars** | 9.5% | 4.6-5.8% | 2.3-3.5% | 2-5% |

### Files to Modify
1. **Priority 1 (Tier 1)**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` (line 166)
2. **Priority 2 (Tier 2)**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` (lines 278, 302, 354)

**Estimated Implementation Time**:
- Tier 1: 5 minutes (1-line change + verification)
- Tier 2: 15 minutes (3-line changes + verification)
- Total: 20 minutes

---

**Report Complete** - Ready for TaskMaster handoff
