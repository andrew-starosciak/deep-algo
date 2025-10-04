# Playbook: QuadMA Strategy Tightening (82 → 20-40 trades)

## User Request
> User wants to tighten QuadMA strategy (currently 82 trades in 3 days, target ~20-40 trades).

## Scope Boundaries

### MUST DO
- **Tier 1**: Re-enable volume filter (immediate ~40-50% reduction)
- **Tier 2**: Stricter MA alignment (add MA50 checks if Tier 1 insufficient)
- **Tier 3**: Make volume threshold configurable in TUI (optional enhancement)
- Test each tier incrementally before escalating
- Provide decision checkpoints based on trade count

### MUST NOT DO
- ❌ Do not disable crossover detection (it's working correctly now)
- ❌ Do not remove TP/SL (2% / 1%)
- ❌ Do not change MA periods (5/10/20/50 - structural change requiring re-optimization)
- ❌ Do not modify alignment logic to use `_long_2` parameter without removing underscore prefix
- ❌ Do not create new strategy files or modules
- ❌ Do not add documentation files
- ❌ Do not add logging or metrics

## Atomic Tasks

### TIER 1: Volume Filter Re-enablement (Immediate - Target: 40-50 trades)

#### Task 1.1: Enable Volume Filter
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
**Location**: Line 166 (QuadMA strategy instantiation)
**Action**: Change volume filter from disabled to enabled
  - Change `false,` (line 166)
  - To `true,` (line 166)

**Verification**:
```bash
cargo check --package algo-trade-cli
```

**Acceptance**:
- Line 166 reads: `true,                                   // volume_filter_enabled: true`
- No other lines modified
- Code compiles without errors

**Estimated Lines Changed**: 1

#### Task 1.2: Update Comment (Volume Filter Enabled)
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
**Location**: Line 166 (comment)
**Action**: Update comment to reflect enabled state
  - Change `// volume_filter_enabled: false (disabled for testing)`
  - To `// volume_filter_enabled: true (filters low-volume signals)`

**Verification**:
```bash
cargo check --package algo-trade-cli
```

**Acceptance**:
- Comment accurately describes enabled state
- No functional changes

**Estimated Lines Changed**: 1

#### Checkpoint 1: Tier 1 Verification
**Action**: Run TUI backtest with Tier 1 changes
**Command**:
```bash
cargo run --package algo-trade-cli --bin algo-trade-cli
# In TUI: Configure backtest → Run → Count total trades
```

**Decision Point**:
- **If trades = 40-50**: SUCCESS - Stop here, Tier 1 achieved goal
- **If trades = 50-70**: BORDERLINE - Discuss with user, may proceed to Tier 2
- **If trades > 70**: INSUFFICIENT - Proceed to Tier 2

**Expected Outcome**: 40-50 trades (40-50% reduction from 82)

---

### TIER 2: Stricter MA Alignment (Conditional - Target: 20-30 trades)

**ONLY execute if Checkpoint 1 shows insufficient reduction (>50 trades)**

#### Task 2.1: Add MA50 Check to Long Entry Alignment
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Line 302 (function `should_enter_long`)
**Action**: Extend bullish alignment check to include MA50 (long_2)
  - Change `let bullish = short_1 > long_1 && short_2 > long_1;`
  - To `let bullish = short_1 > long_1 && short_2 > long_1 && short_1 > long_2 && short_2 > long_2;`

**Rationale**: Requires both MA5 and MA10 to be above MA50 (stronger bullish structure)

**Verification**:
```bash
cargo check --package algo-trade-strategy
```

**Acceptance**:
- Alignment check has 4 conditions (was 2)
- Both shorts must be above both longs (MA3 and MA50)
- No other logic changed

**Estimated Lines Changed**: 1

#### Task 2.2: Add MA50 Check to Short Entry Alignment
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Line 355 (function `should_enter_short`)
**Action**: Extend bearish alignment check to include MA50 (long_2)
  - Find line: `let bearish = short_1 < long_1 && short_2 < long_1;`
  - Change to: `let bearish = short_1 < long_1 && short_2 < long_1 && short_1 < long_2 && short_2 < long_2;`

**Rationale**: Requires both MA5 and MA10 to be below MA50 (stronger bearish structure)

**Verification**:
```bash
cargo check --package algo-trade-strategy
```

**Acceptance**:
- Alignment check has 4 conditions (was 2)
- Both shorts must be below both longs (MA3 and MA50)
- Symmetry with long entry logic

**Estimated Lines Changed**: 1

#### Task 2.3: Remove Unused Parameter Prefix (Long Entry)
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Line 278 (function signature `should_enter_long`)
**Action**: Remove underscore prefix from `_long_2` parameter (now used)
  - Change `_long_2: Decimal,`
  - To `long_2: Decimal,`

**Verification**:
```bash
cargo check --package algo-trade-strategy
```

**Acceptance**:
- No Clippy warning about unused parameter
- Parameter name is `long_2` (no underscore)

**Estimated Lines Changed**: 1

#### Task 2.4: Remove Unused Parameter Prefix (Short Entry)
**File**: `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs`
**Location**: Line 330 (function signature `should_enter_short`)
**Action**: Remove underscore prefix from `_long_2` parameter (now used)
  - Change `_long_2: Decimal,`
  - To `long_2: Decimal,`

**Verification**:
```bash
cargo check --package algo-trade-strategy
```

**Acceptance**:
- No Clippy warning about unused parameter
- Parameter name is `long_2` (no underscore)

**Estimated Lines Changed**: 1

#### Checkpoint 2: Tier 2 Verification
**Action**: Run TUI backtest with Tier 2 changes
**Command**:
```bash
cargo run --package algo-trade-cli --bin algo-trade-cli
# In TUI: Configure backtest → Run → Count total trades
```

**Decision Point**:
- **If trades = 20-30**: SUCCESS - Stop here, Tier 2 achieved goal
- **If trades = 30-40**: ACCEPTABLE - Within target range, discuss with user
- **If trades > 40**: BORDERLINE - Proceed to Tier 3 or discuss alternative filters

**Expected Outcome**: 20-30 trades (50% reduction from Tier 1's 40-50 trades)

---

### TIER 3: Configurable Volume Threshold (Optional Enhancement)

**ONLY execute if requested by user for fine-tuning capability**

#### Task 3.1: Add Volume Threshold Option to TUI Config
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 93-96 (StrategyConfig default, QuadMa variant)
**Action**: Document current volume_factor value (no code change, preparation task)
  - Current value: `volume_factor: 150` (1.5x average volume)
  - Options for user adjustment: 120 (1.2x), 150 (1.5x), 175 (1.75x), 200 (2.0x)
  - Note: This task is informational only - actual TUI editing requires UI changes

**Verification**:
```bash
# No verification needed (informational task)
```

**Acceptance**:
- Document current value and options
- User can manually edit value in TUI number input

**Estimated Lines Changed**: 0 (documentation only)

#### Task 3.2: Document Volume Threshold Tuning Guidance
**File**: Create user guide (if requested)
**Location**: N/A
**Action**: Document volume threshold tuning:
  - 120 (1.2x): More trades, includes moderate volume signals
  - 150 (1.5x): Balanced (current default)
  - 175 (1.75x): Fewer trades, requires strong volume confirmation
  - 200 (2.0x): Minimal trades, only exceptional volume spikes

**Note**: This task only executes if user explicitly requests documentation

**Estimated Lines Changed**: N/A

---

## Verification Checklist

### After Tier 1 Completion:
- [ ] `cargo check --package algo-trade-cli` succeeds
- [ ] Line 166 in runner.rs shows `true` for volume_filter_enabled
- [ ] Comment updated to reflect enabled state
- [ ] TUI backtest runs without errors
- [ ] Trade count recorded (expected: 40-50 trades)
- [ ] Git diff shows exactly 2 lines changed in runner.rs

### After Tier 2 Completion (if executed):
- [ ] `cargo check --package algo-trade-strategy` succeeds
- [ ] `cargo clippy --package algo-trade-strategy -- -D warnings` passes (no unused parameter warnings)
- [ ] Long entry alignment has 4 conditions (line 302)
- [ ] Short entry alignment has 4 conditions (line ~355)
- [ ] Both `long_2` parameters have underscore removed
- [ ] TUI backtest runs without errors
- [ ] Trade count recorded (expected: 20-30 trades)
- [ ] Git diff shows exactly 4 lines changed in quad_ma.rs

### After Tier 3 Completion (if executed):
- [ ] Volume threshold options documented
- [ ] User can manually adjust volume_factor in TUI
- [ ] No code changes (documentation only)

### Karen Quality Gate (MANDATORY)

After completing any tier, invoke Karen agent for quality review:

```bash
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review - QuadMA Tightening Tier [1/2/3]",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review packages algo-trade-cli and algo-trade-strategy following ALL 6 phases. Include actual terminal outputs for: Phase 0 (Compilation), Phase 1 (Clippy all levels), Phase 2 (rust-analyzer), Phase 3 (Cross-file), Phase 4 (Per-file), Phase 5 (Report), Phase 6 (Final verification)."
)
```

**Karen Success Criteria (Zero Tolerance):**
- [ ] Phase 0: Compilation check passes
- [ ] Phase 1: Clippy (default + pedantic + nursery) - ZERO warnings
- [ ] Phase 2: rust-analyzer diagnostics - ZERO issues
- [ ] Phase 3: Cross-file validation - No broken references
- [ ] Phase 4: Per-file verification - Each modified file passes
- [ ] Phase 5: Report includes actual terminal outputs
- [ ] Phase 6: Final verification passes (release build + tests compile)

**If Karen Fails:**
1. STOP - Do not proceed to next tier
2. Fix each issue as atomic task
3. Re-run Karen after fixes
4. Iterate until zero issues

---

## Rollback Plan

### Tier 1 Rollback (Volume Filter)
If Tier 1 produces too few trades (<20):
```bash
git checkout -- crates/cli/src/tui_backtest/runner.rs
# Revert to volume_filter_enabled: false
```

### Tier 2 Rollback (Strict Alignment)
If Tier 2 produces too few trades (<10):
```bash
git checkout -- crates/strategy/src/quad_ma.rs
# Revert to 2-condition alignment checks
```

### Full Rollback
To revert all changes:
```bash
git checkout -- crates/cli/src/tui_backtest/runner.rs
git checkout -- crates/strategy/src/quad_ma.rs
```

---

## Expected Outcomes

### Tier 1 Impact
- **Before**: 82 trades (3 days of data)
- **After**: 40-50 trades
- **Reduction**: 40-50% (volume filter removes low-conviction signals)
- **Mechanism**: Filters out crossovers occurring during low-volume periods

### Tier 2 Impact
- **Before**: 40-50 trades (post-Tier 1)
- **After**: 20-30 trades
- **Reduction**: 50% additional (stricter alignment)
- **Mechanism**: Requires full trend alignment (both shorts above/below both longs)

### Combined Impact (Tier 1 + Tier 2)
- **Before**: 82 trades
- **After**: 20-30 trades
- **Total Reduction**: 63-76%
- **Quality**: Higher-conviction signals only (full alignment + volume confirmation)

---

## Decision Tree

```
Start: 82 trades
    ↓
Execute Tier 1 (enable volume filter)
    ↓
Run Checkpoint 1
    ↓
Trades = 40-50? → YES → DONE (goal achieved)
    ↓ NO (>50 trades)
Execute Tier 2 (stricter MA alignment)
    ↓
Run Checkpoint 2
    ↓
Trades = 20-30? → YES → DONE (goal achieved)
    ↓ NO (discuss with user)
Optional: Tier 3 (volume threshold tuning)
    ↓
User adjusts volume_factor manually in TUI
    ↓
Iterate until desired trade frequency
```

---

## File Change Summary

### Tier 1 Files:
- `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` (2 lines changed)

### Tier 2 Files:
- `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` (4 lines changed)

### Tier 3 Files:
- No code changes (documentation only)

### Total Changes:
- **Maximum**: 6 lines across 2 files
- **Minimal scope**: Configuration and alignment logic only
- **No new functions**: Only parameter changes to existing logic
- **No new files**: Only edits to existing implementation

---

## Success Criteria

A tier is successful when:
- ✅ Code compiles without errors
- ✅ Clippy passes with zero warnings
- ✅ Backtest runs without panics
- ✅ Trade count is within expected range for tier
- ✅ Git diff matches estimated line count exactly
- ✅ Karen review passes with zero issues
- ✅ No unintended behavioral changes (TP/SL, crossover detection unchanged)

Playbook is complete when:
- ✅ Trade count is within target range (20-40 trades)
- ✅ User approves trade frequency
- ✅ All executed tiers pass verification
- ✅ Karen quality gate passed
- ✅ Changes are minimal and reversible

---

## Notes

### Design Rationale
- **Incremental tightening**: Test each tier before escalating (prevents over-tightening)
- **Reversible changes**: Each tier can be rolled back independently
- **No structural changes**: MA periods remain unchanged (avoid re-optimization)
- **Preserves existing logic**: Crossover detection and TP/SL unchanged

### User Control Points
- **After Tier 1**: User decides if sufficient or proceed to Tier 2
- **After Tier 2**: User decides if acceptable or requires Tier 3
- **Tier 3**: User manually adjusts volume_factor in TUI for fine-tuning

### Testing Strategy
- Run TUI backtest after each tier
- Compare trade count to expected range
- Analyze trade quality (win rate, avg profit) if needed
- Iterate with user feedback

---

## Playbook Metadata

**Created**: 2025-10-04
**Feature**: QuadMA Strategy Tightening (Trade Frequency Reduction)
**Tiers**: 3 (incremental)
**Total Tasks**: 8 atomic tasks + 2 checkpoints
**Estimated Total LOC**: 6 lines (Tier 1 + Tier 2)
**Affected Packages**: `algo-trade-cli`, `algo-trade-strategy`
**Risk Level**: Low (configuration changes only, fully reversible)
**User Approval Required**: Yes (after each tier checkpoint)
