# Playbook: QuadMA Production Fix - Volume Filter + MA Timing Bug

## User Request
> "I have seen trades work in the past so I trust that all that display logic still works, lets focus on the production fix"

## Context
User has verified display logic works correctly (trades appear in TUI when generated). The production issue is:
- **Current behavior**: QuadMA generates 0 trades on ETH data (188 bars)
- **Expected behavior**: 1-5 trades (based on strategy parameters and market conditions)
- **Root causes identified**:
  1. Volume filter enabled by default, may be too restrictive
  2. Previous MA timing bug: Values stored AFTER calculation but BEFORE entry checks, causing crossover detection to compare bar N with bar N (always false)

## Scope Boundaries

### MUST DO
- [ ] Disable volume filter by default in TUI runner (Option 2)
- [ ] Fix previous MA storage timing in QuadMA strategy (Option 4)
- [ ] Ensure zero compiler warnings (Karen standards)
- [ ] Run TUI integration test to verify trades generated

### MUST NOT DO
- Do not remove crossover detection logic (industry-standard pattern)
- Do not modify TP/SL safety features (2% TP, 1% SL)
- Do not change MA periods or default parameters
- Do not add new features or refactoring
- Do not modify display logic (already verified working)
- Do not add documentation files

## Atomic Tasks

### Task 1: Disable Volume Filter in TUI Runner
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
**Location**: Line 166
**Action**: Change volume filter default from enabled to disabled
  - Change `true,                                    // volume_filter_enabled: true (default)`
  - To `false,                                   // volume_filter_enabled: false (default for testing)`

**Verification**:
```bash
cargo check -p algo-trade-cli
```

**Acceptance**:
- Line 166 contains `false,` as parameter value
- Comment updated to reflect new default
- No other lines modified in this file
- No new functions or imports added

**Estimated Lines Changed**: 1

**Rationale**: Volume filter at 1.5x average may be too restrictive for backtesting on limited historical data. Disabling by default allows MA crossover signals to generate trades without volume confirmation requirement.

---

### Task 2: Fix Previous MA Storage Timing (Part 1 - Save Old Values)
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Before line 461 (immediately after line 459: `self.prev_trend_ma = Some(trend_ma);`)
**Action**: Save OLD previous MA values to local variables BEFORE updating struct fields

Add these 4 lines between line 459 and 461:
```rust
        // Save OLD previous values before updating (for crossover detection on THIS bar)
        let old_prev_short_1 = self.prev_short_1;
        let old_prev_short_2 = self.prev_short_2;
```

**Verification**:
```bash
cargo check -p algo-trade-strategy
```

**Acceptance**:
- Two new local variables defined: `old_prev_short_1` and `old_prev_short_2`
- Variables capture state BEFORE struct field updates
- Comment explains purpose (crossover detection on current bar)
- No other logic modified

**Estimated Lines Changed**: 3 (including comment)

**Rationale**: Current bug stores bar N's MAs as "previous" values, then immediately uses them for crossover detection, causing comparison of bar N vs bar N. We need to save bar N-1's values (stored in `self.prev_short_1/2` from last iteration) before overwriting.

---

### Task 3: Fix Previous MA Storage Timing (Part 2 - Update Entry Logic Calls)
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Lines 507-508 and 517-518 (inside `should_enter_long` and `should_enter_short` function calls)
**Action**: Pass saved OLD previous values instead of updated struct fields

Change lines 507-508 from:
```rust
            self.prev_short_1,
            self.prev_short_2,
```

To:
```rust
            old_prev_short_1,
            old_prev_short_2,
```

Change lines 517-518 from:
```rust
            self.prev_short_1,
            self.prev_short_2,
```

To:
```rust
            old_prev_short_1,
            old_prev_short_2,
```

**Verification**:
```bash
cargo check -p algo-trade-strategy
```

**Acceptance**:
- Both function calls now use local variables (`old_prev_short_1`, `old_prev_short_2`)
- Crossover detection now compares bar N with bar N-1 (correct logic)
- No other parameters modified
- No new functions added

**Estimated Lines Changed**: 4

**Rationale**: This completes the fix. Now `detect_crossover()` receives:
- `prev_short_1` = bar N-1's MA5 (from `old_prev_short_1`)
- `short_1` = bar N's MA5 (newly calculated)
- Correctly detects MA5 crossing MA10 between consecutive bars

---

### Task 4: Run Karen Quality Review
**Package**: `algo-trade-strategy` and `algo-trade-cli`
**Action**: Invoke Karen agent for comprehensive quality review following ALL 6 phases

**Command**:
```bash
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review packages algo-trade-strategy and algo-trade-cli following ALL 6 phases. Include actual terminal outputs for each phase."
)
```

**Karen Success Criteria (Zero Tolerance)**:
- [ ] Phase 0: Compilation check passes (`cargo build --package algo-trade-strategy --lib`)
- [ ] Phase 1: Clippy (default + pedantic + nursery) - ZERO warnings
- [ ] Phase 2: rust-analyzer diagnostics - ZERO issues
- [ ] Phase 3: Cross-file validation - No broken references
- [ ] Phase 4: Per-file verification - Each file passes individually
- [ ] Phase 5: Report includes actual terminal outputs
- [ ] Phase 6: Final verification passes (release build + tests compile)

**If Karen Fails**:
1. STOP - Do not proceed to next task
2. Document all findings from Karen's report
3. Fix each issue as atomic task
4. Re-run Karen after ALL fixes
5. Iterate until Karen passes with zero issues

**Estimated Time**: 5 minutes

---

### Task 5: Full Workspace Build Verification
**Command**: Release build to verify all crates compile with optimizations

**Verification**:
```bash
cargo build --release
```

**Acceptance**:
- Build completes successfully
- Zero warnings across all workspace crates
- Binary artifacts created in `target/release/`
- No new dependencies added to Cargo.toml files

**Estimated Time**: 2 minutes

**Rationale**: Ensures production-ready code compiles cleanly with all optimizations enabled.

---

### Task 6: TUI Integration Test (Manual Verification)
**Package**: `algo-trade-cli`
**Action**: Run TUI backtest with QuadMA strategy on ETH data to verify trades generated

**Manual Steps**:
1. Launch TUI: `cargo run -p algo-trade-cli -- tui-backtest`
2. Select strategy: `QuadMA` (use arrow keys + Enter)
3. Select symbol: `ETH` (use arrow keys + Enter)
4. Select date range: `2025-10-02` to `2025-10-02` (or latest available)
5. Press `r` to run backtest
6. Press `h` to view trade history
7. Verify: **Trade count > 0** (expected: 1-5 trades in 188 bars)
8. Press `q` to quit

**Expected Output**:
- Trade history shows at least 1 entry/exit pair
- Trades have valid timestamps, prices, PnL
- No runtime errors or panics
- Volume filter status shows "Disabled" in strategy params

**Acceptance**:
- Trade history is NOT empty (current bug: 0 trades)
- All trades have crossover signal justification
- Entry/exit logic respects TP/SL (2% / 1%)
- No crashes or unexpected behavior

**Estimated Time**: 3 minutes

**Rationale**: This is the ultimate production validation. If TUI shows trades, the fix is successful. User has already verified display logic works, so any trades shown are genuine strategy signals.

---

## Verification Checklist

After ALL tasks completed:
- [ ] `cargo build --release` succeeds
- [ ] `cargo test -p algo-trade-strategy` passes
- [ ] `cargo clippy -p algo-trade-strategy -- -D warnings` passes
- [ ] `cargo clippy -p algo-trade-cli -- -D warnings` passes
- [ ] No new files created
- [ ] No new functions/structs added
- [ ] Git diff shows ONLY changes in:
  - `crates/cli/src/tui_backtest/runner.rs` (1 line)
  - `crates/strategy/src/quad_ma.rs` (7 lines)
- [ ] Total lines changed: 8 lines
- [ ] Karen review passes with zero issues
- [ ] TUI backtest generates trades (> 0 trade history entries)

## Technical Analysis

### Bug Explanation

**Current Code Flow (Buggy)**:
```
Bar N arrives
├─ Calculate MAs: short_1 = MA5(bar N), short_2 = MA10(bar N)
├─ Store immediately: self.prev_short_1 = Some(short_1)  ← Updates to bar N's value
├─ Store immediately: self.prev_short_2 = Some(short_2)  ← Updates to bar N's value
├─ Call should_enter_long(
│     short_1,              ← bar N's MA5
│     short_2,              ← bar N's MA10
│     self.prev_short_1,   ← bar N's MA5 (just updated!) ❌
│     self.prev_short_2,   ← bar N's MA10 (just updated!) ❌
│  )
└─ Crossover detection compares bar N vs bar N → Always false
```

**Fixed Code Flow**:
```
Bar N arrives
├─ Save OLD state: old_prev_short_1 = self.prev_short_1  ← Captures bar N-1's MA5
├─ Save OLD state: old_prev_short_2 = self.prev_short_2  ← Captures bar N-1's MA10
├─ Calculate MAs: short_1 = MA5(bar N), short_2 = MA10(bar N)
├─ Store current: self.prev_short_1 = Some(short_1)  ← Updates for NEXT iteration
├─ Store current: self.prev_short_2 = Some(short_2)  ← Updates for NEXT iteration
├─ Call should_enter_long(
│     short_1,              ← bar N's MA5
│     short_2,              ← bar N's MA10
│     old_prev_short_1,    ← bar N-1's MA5 ✅
│     old_prev_short_2,    ← bar N-1's MA10 ✅
│  )
└─ Crossover detection compares bar N vs bar N-1 → Correct!
```

### Crossover Detection Logic
The `detect_crossover` function returns:
- `Some(true)`: Bullish crossover (prev_MA5 <= prev_MA10 AND current_MA5 > current_MA10)
- `Some(false)`: Bearish crossover (prev_MA5 >= prev_MA10 AND current_MA5 < current_MA10)
- `None`: No crossover (MAs maintain same relationship)

With the bug, both "previous" and "current" values are identical, so crossover never detected.

## Expected Outcome

**Before Fix**:
- TUI backtest on ETH: 0 trades
- Crossover detection always returns `None` (no MA relationship change)
- Volume filter may also block trades, but irrelevant since crossover fails first

**After Fix**:
- TUI backtest on ETH: 1-5 trades (depending on MA crossovers in data)
- Crossover detection correctly identifies MA5/MA10 crosses
- Volume filter disabled by default for testing
- Trade signals follow industry-standard crossover + alignment logic

## Rollback Plan

If verification fails:
1. Revert changes: `git checkout -- crates/cli/src/tui_backtest/runner.rs crates/strategy/src/quad_ma.rs`
2. Review Karen report findings if available
3. Re-run TaskMaster with failure context
4. Generate revised playbook if different approach needed
5. Do NOT attempt manual fixes without playbook update

## Success Criteria

Playbook succeeds when:
- ✅ All 6 tasks completed without rollback
- ✅ Karen review passes with zero issues
- ✅ TUI integration test shows trades generated (trade history not empty)
- ✅ Git diff shows exactly 8 lines changed across 2 files
- ✅ No unexpected files modified
- ✅ Crossover detection logic remains unchanged (industry-standard preserved)
- ✅ TP/SL safety features intact (2% / 1%)

## File Inventory

**Files Modified** (2 total):
1. `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` - Line 166 (1 change)
2. `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` - Lines 461-463, 507-508, 517-518 (7 changes)

**Files NOT Modified** (everything else):
- Display logic (TUI event handling, trade history rendering)
- Database queries (TimescaleDB OHLCV retrieval)
- Strategy selection menu
- Any other crates or modules

**Total Impact**: 8 lines changed, 2 files modified, 0 files created
