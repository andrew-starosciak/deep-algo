# Playbook: Paper Trading Mode for Live Bot Testing

**Date**: 2025-10-06
**Agent**: TaskMaster
**Source**: Context Gatherer Report `/home/a/Work/algo-trade/.claude/context/2025-10-06_paper-trading-mode.md`

---

## User Request

> "Create a paper trading mode which connects to live OHLCV in real-time. It will take positions but not use real money but paper money to ensure our bots are working and setup correctly."

---

## Scope Boundaries

### MUST DO

1. [ ] Create `PaperTradingExecutionHandler` in `exchange-hyperliquid` crate (NEW FILE: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/paper_execution.rs`)
2. [ ] Add `ExecutionMode` enum to `bot-orchestrator` crate (FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`, after line 17)
3. [ ] Update `BotConfig` struct with paper trading fields (FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`, lines 19-48)
4. [ ] Create `ExecutionHandlerWrapper` enum (NEW FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/execution_wrapper.rs`)
5. [ ] Update `BotActor` system field type (FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`, line 16)
6. [ ] Update `BotActor::initialize_system()` with conditional execution handler (FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`, lines 47-122)
7. [ ] Export `PaperTradingExecutionHandler` from `exchange-hyperliquid` crate (FILE: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`)
8. [ ] Add module declaration for `execution_wrapper` (FILE: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`)
9. [ ] Update `exchange-hyperliquid` Cargo.toml dependencies (FILE: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`)
10. [ ] Add paper mode example to config (FILE: `/home/a/Work/algo-trade/config/Config.example.toml`)

### MUST NOT DO

- **DO NOT modify `LiveExecutionHandler`**: Keep live handler pristine (no dry_run flags, no paper logic)
- **DO NOT modify `SimulatedExecutionHandler`**: Reuse as-is from backtest crate (no changes)
- **DO NOT modify `LiveDataProvider`**: Paper mode uses live data provider verbatim
- **DO NOT modify TUI display logic**: Events are already mode-agnostic (TUI works as-is)
- **DO NOT add persistent storage**: Paper portfolio resets on bot restart (in-memory only)
- **DO NOT implement partial fills**: Instant complete fills (same as backtest)
- **DO NOT implement Level 2 order book**: Simple slippage model (no market depth simulation)
- **DO NOT add paper-specific metrics**: Reuse existing `TradingSystem` metrics (equity, Sharpe, etc.)
- **DO NOT create separate CLI commands**: Use existing `cargo run -p algo-trade-cli -- run` with paper config
- **DO NOT modify core traits**: `ExecutionHandler` and `DataProvider` traits stay unchanged

---

## Context Summary

Paper trading mode enables risk-free validation of bot strategies using live market data with simulated execution. The implementation reuses existing `SimulatedExecutionHandler` logic (proven slippage/commission modeling) wrapped in a new `PaperTradingExecutionHandler`. An `ExecutionMode` enum in `BotConfig` allows explicit selection between live and paper modes. The `BotActor::initialize_system()` method chooses the appropriate execution handler at runtime via an enum-based wrapper (avoiding trait object complexity). This maintains backtest-live parity while ensuring zero risk of accidental real money trading.

---

## Atomic Tasks

### Task 1: Create PaperTradingExecutionHandler

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/paper_execution.rs` (CREATE NEW FILE)
**Location**: New file
**Action**: Create struct wrapping `SimulatedExecutionHandler` from backtest crate

**Code**:
```rust
use algo_trade_backtest::execution::SimulatedExecutionHandler;
use algo_trade_core::events::{FillEvent, OrderEvent};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;

/// Paper trading execution handler for live bot testing with simulated fills.
///
/// Uses real-time market data but executes orders with virtual money, enabling
/// strategy validation without capital risk. Wraps `SimulatedExecutionHandler`
/// from the backtest crate for proven slippage and commission modeling.
///
/// # Configuration
///
/// - `slippage_bps`: Basis points of slippage (default: 10 bps = 0.1%)
/// - `commission_rate`: Commission as decimal (default: 0.00025 = 0.025% taker fee)
///
/// # Safety
///
/// Paper mode makes ZERO API calls to exchange. All fills are simulated in-memory.
#[derive(Debug)]
pub struct PaperTradingExecutionHandler {
    inner: SimulatedExecutionHandler,
}

impl PaperTradingExecutionHandler {
    /// Creates a new paper trading execution handler.
    ///
    /// # Arguments
    ///
    /// * `slippage_bps` - Slippage in basis points (10 bps = 0.1%)
    /// * `commission_rate` - Commission rate as decimal (0.00025 = 0.025%)
    ///
    /// # Examples
    ///
    /// ```
    /// let handler = PaperTradingExecutionHandler::new(10.0, 0.00025);
    /// ```
    pub fn new(slippage_bps: f64, commission_rate: f64) -> Self {
        Self {
            inner: SimulatedExecutionHandler::new(commission_rate, slippage_bps),
        }
    }
}

#[async_trait]
impl ExecutionHandler for PaperTradingExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // Delegate to simulated execution handler (zero API calls)
        self.inner.execute_order(order).await
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-hyperliquid
```

**Acceptance**:
- File created at exact path
- Struct wraps `SimulatedExecutionHandler` from `algo_trade_backtest` crate
- `ExecutionHandler` trait implemented via delegation
- Constructor accepts `slippage_bps` and `commission_rate` parameters
- No API calls in code (safety guarantee)
- Rustdoc comments present with `# Safety` and `# Examples` sections

**Estimated LOC**: 50 lines

---

### Task 2: Add ExecutionMode Enum

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
**Location**: After line 17 (before `BotCommand` enum definition)
**Action**: Create enum for execution mode selection (Live vs Paper)

**Code**:
```rust
/// Execution mode for bot trading.
///
/// # Modes
///
/// - `Live`: Execute real orders via exchange API (requires wallet credentials)
/// - `Paper`: Simulate fills with virtual money (zero API calls, zero capital risk)
///
/// # Safety
///
/// Default is `Live` to ensure explicit opt-in for paper mode. Existing configs
/// without `execution_mode` field will use live trading (backward compatible).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    /// Real exchange execution (requires authentication)
    Live,
    /// Simulated execution (paper trading, no API calls)
    Paper,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        Self::Live // Safe default: explicit opt-in required for paper mode
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```

**Acceptance**:
- Enum added after line 17 in `commands.rs`
- Two variants: `Live` and `Paper`
- `Default` trait implementation returns `Live` (safe default)
- `#[serde(rename_all = "lowercase")]` for TOML/JSON config parsing
- Rustdoc comments explain safety reasoning
- No other files modified

**Estimated LOC**: 12 lines

---

### Task 3: Update BotConfig Struct with Paper Trading Fields

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
**Location**: Lines 19-48 (add fields after line 29) and helper functions after line 64
**Action**: Add `execution_mode`, `paper_slippage_bps`, `paper_commission_rate` fields to `BotConfig`

**Code**:
```rust
// Add these fields after line 29 (after `strategy_config: Option<String>` field):

    /// Execution mode (live vs paper trading)
    #[serde(default)]
    pub execution_mode: ExecutionMode,

    /// Paper trading slippage in basis points (default: 10 bps = 0.1%)
    #[serde(default = "default_paper_slippage")]
    pub paper_slippage_bps: f64,

    /// Paper trading commission rate (default: 0.00025 = 0.025% Hyperliquid taker fee)
    #[serde(default = "default_paper_commission")]
    pub paper_commission_rate: f64,

// Add these helper functions after line 64 (after existing default functions):

const fn default_paper_slippage() -> f64 {
    10.0 // 10 basis points = 0.1%
}

const fn default_paper_commission() -> f64 {
    0.00025 // 0.025% (Hyperliquid taker fee)
}
```

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
cargo test -p algo-trade-bot-orchestrator --lib
```

**Acceptance**:
- Three new fields added to `BotConfig` struct
- `execution_mode` uses `#[serde(default)]` (backward compatible)
- `paper_slippage_bps` and `paper_commission_rate` use default functions
- Default slippage is 10 bps (conservative vs backtest)
- Default commission is 0.025% (Hyperliquid taker fee)
- Compilation succeeds with zero warnings

**Estimated LOC**: 15 lines

---

### Task 4: Create ExecutionHandlerWrapper Enum

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/execution_wrapper.rs` (CREATE NEW FILE)
**Location**: New file
**Action**: Create enum wrapper for runtime dispatch between live and paper execution handlers

**Code**:
```rust
use algo_trade_core::events::{FillEvent, OrderEvent};
use algo_trade_core::traits::ExecutionHandler;
use algo_trade_hyperliquid::execution::LiveExecutionHandler;
use algo_trade_hyperliquid::paper_execution::PaperTradingExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;

/// Wrapper enum for execution handlers supporting runtime mode selection.
///
/// Enables switching between live and paper trading at runtime without
/// recompilation. Uses enum dispatch (zero-cost abstraction) instead of
/// trait objects (avoids heap allocation and vtable overhead).
///
/// # Variants
///
/// - `Live`: Real exchange API execution
/// - `Paper`: Simulated execution with virtual money
#[derive(Debug)]
pub enum ExecutionHandlerWrapper {
    /// Live execution handler (real API calls)
    Live(LiveExecutionHandler),
    /// Paper execution handler (simulated fills)
    Paper(PaperTradingExecutionHandler),
}

#[async_trait]
impl ExecutionHandler for ExecutionHandlerWrapper {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        match self {
            Self::Live(handler) => handler.execute_order(order).await,
            Self::Paper(handler) => handler.execute_order(order).await,
        }
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```

**Acceptance**:
- File created at exact path
- Enum has `Live` and `Paper` variants with correct handler types
- `ExecutionHandler` trait implemented with match-based delegation
- No trait objects (no `Box<dyn>`)
- Rustdoc comments explain design rationale (enum vs trait object)
- Compilation succeeds

**Estimated LOC**: 40 lines

---

### Task 5: Update BotActor System Field Type

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Line 16 (system field in `BotActor` struct)
**Action**: Change generic type parameter from `LiveExecutionHandler` to `ExecutionHandlerWrapper`

**Code**:
```rust
// OLD (line 16):
system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,

// NEW (line 16):
system: Option<TradingSystem<LiveDataProvider, ExecutionHandlerWrapper>>,
```

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```

**Acceptance**:
- Line 16 updated exactly as specified
- Type parameter changed from `LiveExecutionHandler` to `ExecutionHandlerWrapper`
- No other lines modified in this task
- Compilation succeeds (may have errors until Task 6 completes)

**Estimated LOC**: 1 line changed

---

### Task 6: Update BotActor::initialize_system() Method

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 47-122 (`initialize_system()` method)
**Action**: Add conditional execution handler creation based on `execution_mode` config

**Code**:
```rust
// Add after line 69 (after warmup() completes), REPLACE lines 72-88:

        // Create execution handler based on mode
        let execution_handler = match self.config.execution_mode {
            ExecutionMode::Live => {
                // Live mode: create authenticated client
                tracing::info!("Execution mode: Live (real exchange API)");

                let client = if let Some(wallet_cfg) = &self.config.wallet {
                    tracing::info!("Authenticating with wallet");
                    HyperliquidClient::with_wallet(
                        &self.config.api_url,
                        &wallet_cfg.api_wallet_address,
                        &wallet_cfg.api_wallet_private_key,
                    )
                    .await?
                } else {
                    tracing::warn!("No wallet configured - using unauthenticated client (will fail on order submission)");
                    HyperliquidClient::new(&self.config.api_url).await?
                };

                ExecutionHandlerWrapper::Live(LiveExecutionHandler::new(client))
            }
            ExecutionMode::Paper => {
                // Paper mode: simulated execution (zero API calls)
                tracing::info!(
                    "Execution mode: Paper (simulated fills, slippage: {} bps, commission: {:.4}%)",
                    self.config.paper_slippage_bps,
                    self.config.paper_commission_rate * 100.0
                );

                if self.config.wallet.is_some() {
                    tracing::warn!("Paper mode enabled but wallet provided - wallet will be IGNORED (zero API calls)");
                }

                ExecutionHandlerWrapper::Paper(PaperTradingExecutionHandler::new(
                    self.config.paper_slippage_bps,
                    self.config.paper_commission_rate,
                ))
            }
        };

// Continue with existing code (line 90 onwards) - change `LiveExecutionHandler::new(client)` to `execution_handler`:
// Replace line 87-88 with the execution_handler variable from above
```

**Full Modified Section (lines 47-122 replacement)**:
```rust
    async fn initialize_system(&mut self) -> Result<()> {
        tracing::info!("Initializing trading system for bot {}", self.config.bot_id);

        // Create data provider
        let mut data_provider = LiveDataProvider::new(
            &self.config.ws_url,
            &self.config.symbol,
            &self.config.interval,
        )
        .await?;

        // Warmup period
        tracing::info!("Fetching warmup data ({} periods)", self.config.warmup_periods);
        let warmup_events = data_provider
            .warmup(&self.config.symbol, &self.config.interval, self.config.warmup_periods)
            .await?;

        // Create execution handler based on mode
        let execution_handler = match self.config.execution_mode {
            ExecutionMode::Live => {
                // Live mode: create authenticated client
                tracing::info!("Execution mode: Live (real exchange API)");

                let client = if let Some(wallet_cfg) = &self.config.wallet {
                    tracing::info!("Authenticating with wallet");
                    HyperliquidClient::with_wallet(
                        &self.config.api_url,
                        &wallet_cfg.api_wallet_address,
                        &wallet_cfg.api_wallet_private_key,
                    )
                    .await?
                } else {
                    tracing::warn!("No wallet configured - using unauthenticated client (will fail on order submission)");
                    HyperliquidClient::new(&self.config.api_url).await?
                };

                ExecutionHandlerWrapper::Live(LiveExecutionHandler::new(client))
            }
            ExecutionMode::Paper => {
                // Paper mode: simulated execution (zero API calls)
                tracing::info!(
                    "Execution mode: Paper (simulated fills, slippage: {} bps, commission: {:.4}%)",
                    self.config.paper_slippage_bps,
                    self.config.paper_commission_rate * 100.0
                );

                if self.config.wallet.is_some() {
                    tracing::warn!("Paper mode enabled but wallet provided - wallet will be IGNORED (zero API calls)");
                }

                ExecutionHandlerWrapper::Paper(PaperTradingExecutionHandler::new(
                    self.config.paper_slippage_bps,
                    self.config.paper_commission_rate,
                ))
            }
        };

        // Create strategy
        let mut strategy = create_strategy(&self.config.strategy, self.config.strategy_config.as_deref())?;

        // Feed warmup events
        tracing::info!("Processing warmup events");
        for event in warmup_events {
            strategy.on_market_event(&event).await?;
        }

        // Create risk manager
        let risk_manager = Arc::new(SimpleRiskManager::new(
            self.config.initial_capital,
            self.config.risk_per_trade_pct,
            self.config.max_position_pct,
        ));

        // Construct trading system
        tracing::info!("Creating trading system with initial capital: {}", self.config.initial_capital);
        let system = TradingSystem::with_capital(
            data_provider,
            execution_handler,
            vec![Arc::new(Mutex::new(strategy))],
            risk_manager,
            self.config.initial_capital,
        );

        self.system = Some(system);
        tracing::info!("Trading system initialized successfully");

        Ok(())
    }
```

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
cargo clippy -p algo-trade-bot-orchestrator -- -D warnings
```

**Acceptance**:
- Method creates `ExecutionHandlerWrapper::Live` when `execution_mode == Live`
- Method creates `ExecutionHandlerWrapper::Paper` when `execution_mode == Paper`
- Paper mode skips `HyperliquidClient` creation (no authentication)
- Paper mode logs warning if wallet provided
- Tracing logs indicate execution mode and parameters
- No other methods modified
- Compilation succeeds with zero warnings

**Estimated LOC**: 35 lines added/modified

---

### Task 7: Export PaperTradingExecutionHandler from exchange-hyperliquid Crate

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`
**Location**: After existing module declarations (check for existing pattern)
**Action**: Add module and public export for `PaperTradingExecutionHandler`

**Code**:
```rust
// Add after existing module declarations:

mod paper_execution;
pub use paper_execution::PaperTradingExecutionHandler;
```

**Verification**:
```bash
cargo check -p algo-trade-hyperliquid
```

**Acceptance**:
- Module `paper_execution` declared
- `PaperTradingExecutionHandler` exported publicly
- Compilation succeeds
- No other modules modified

**Estimated LOC**: 2 lines

---

### Task 8: Add Module Declaration for execution_wrapper in bot-orchestrator

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`
**Location**: After existing module declarations
**Action**: Add internal module declaration for `execution_wrapper`

**Code**:
```rust
// Add after existing module declarations:

mod execution_wrapper;
```

**Note**: This module is internal to `bot-orchestrator` (not publicly exported). Only `BotActor` uses `ExecutionHandlerWrapper`.

**Verification**:
```bash
cargo check -p algo-trade-bot-orchestrator
```

**Acceptance**:
- Module `execution_wrapper` declared
- NOT in `pub use` exports (internal only)
- Compilation succeeds

**Estimated LOC**: 1 line

---

### Task 9: Update exchange-hyperliquid Cargo.toml Dependencies

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
**Location**: `[dependencies]` section
**Action**: Add `algo_trade_backtest` dependency (required for `SimulatedExecutionHandler` import)

**Code**:
```toml
# Add to [dependencies] section:
algo_trade_backtest = { path = "../backtest" }
```

**Verification**:
```bash
cargo check -p algo-trade-hyperliquid
```

**Acceptance**:
- Dependency added to `[dependencies]` section
- Uses path-based workspace dependency
- Compilation succeeds
- No version conflicts

**Estimated LOC**: 1 line

---

### Task 10: Add Paper Mode Example to Config

**File**: `/home/a/Work/algo-trade/config/Config.example.toml`
**Location**: End of file (or create new bot config section if none exists)
**Action**: Add commented example of paper trading bot configuration

**Code**:
```toml
# Example: Paper Trading Bot Configuration
#
# Paper mode connects to live market data but executes orders with virtual money.
# Use this to validate bot strategies before deploying with real capital.
#
# [[bots]]
# bot_id = "paper-ma-crossover"
# symbol = "BTC"
# strategy = "ma_crossover"
# enabled = true
# interval = "1m"
# execution_mode = "paper"              # Use paper trading (simulated fills)
# paper_slippage_bps = 10.0             # 10 basis points = 0.1% slippage
# paper_commission_rate = 0.00025       # 0.025% commission (Hyperliquid taker fee)
# initial_capital = 10000.0             # Start with $10,000 virtual money
# ws_url = "wss://api.hyperliquid.xyz/ws"
# api_url = "https://api.hyperliquid.xyz"
# warmup_periods = 50
# strategy_config = '{"fast_period": 10, "slow_period": 30}'
# risk_per_trade_pct = 2.0
# max_position_pct = 10.0
# leverage = 1
# margin_mode = "cross"
# # Note: No wallet required for paper mode (simulated execution)
```

**Verification**:
```bash
# Verify TOML syntax
cargo run -p algo-trade-cli -- --help
```

**Acceptance**:
- Example added as commented TOML
- Includes all required fields for paper bot
- Explains `execution_mode = "paper"` parameter
- Documents that wallet is NOT required for paper mode
- Syntax is valid TOML (no parse errors)

**Estimated LOC**: 10 lines

---

## Task Dependencies

Task dependency graph (must complete in order respecting dependencies):

```
Task 2 (ExecutionMode enum) ──────────┐
                                       │
Task 1 (PaperTradingExecutionHandler) ├──> Task 4 (ExecutionHandlerWrapper)
                                       │          │
Task 9 (Cargo.toml dependency) ───────┘          │
                                                  │
Task 3 (BotConfig fields) ───────────────────────┤
                                                  │
Task 5 (BotActor system field) ──────────────────┤
                                                  │
                                                  ├──> Task 6 (BotActor::initialize_system)
Task 7 (export PaperTradingExecutionHandler) ────┤
                                                  │
Task 8 (execution_wrapper module) ───────────────┘

Task 10 (Config example) ───> Last (documentation)
```

**Execution Order**:
1. **Phase 1** (Parallel): Task 1, Task 2, Task 9 (independent foundation tasks)
2. **Phase 2** (Parallel): Task 3, Task 7, Task 8 (depends on Phase 1)
3. **Phase 3**: Task 4 (depends on Task 1, 2, 7)
4. **Phase 4**: Task 5 (depends on Task 4)
5. **Phase 5**: Task 6 (depends on Task 3, 4, 5)
6. **Phase 6**: Task 10 (documentation, depends on all)

---

## Verification Checklist

### Phase 1: Compilation Checks

- [ ] `cargo check` succeeds with zero errors
- [ ] `cargo check -p algo-trade-hyperliquid` succeeds
- [ ] `cargo check -p algo-trade-bot-orchestrator` succeeds
- [ ] `cargo build --release` succeeds

### Phase 2: Clippy Quality Checks

- [ ] `cargo clippy -- -D warnings` passes (default lints)
- [ ] `cargo clippy -- -D clippy::pedantic` passes (pedantic lints)
- [ ] `cargo clippy -- -D clippy::nursery` passes (nursery lints)

### Phase 3: Functional Verification

- [ ] Create test bot config with `execution_mode = "paper"`
- [ ] Start bot: `cargo run -p algo-trade-cli -- run --config <paper_bot.json>`
- [ ] Verify log: "Execution mode: Paper (simulated fills, slippage: 10 bps, commission: 0.0250%)"
- [ ] Verify log: "Connected to live WebSocket feed" (data provider)
- [ ] Bot receives live market data (verify candle events in logs)
- [ ] Strategy generates signal (verify signal event in logs)
- [ ] Order execution produces fill (verify "Paper fill executed" in logs)
- [ ] Verify ZERO Hyperliquid API POST requests (search logs for "POST /exchange" - should NOT appear)

### Phase 4: TUI Monitoring Verification

- [ ] TUI displays bot with "Paper" execution mode indicator
- [ ] Live market data updates shown in TUI (price, volume)
- [ ] Paper position displayed after fill (quantity, avg price)
- [ ] Equity updates in real-time (starts at initial_capital)
- [ ] Realized PnL calculated on position close
- [ ] Metrics update: Sharpe ratio, max drawdown, win rate

### Phase 5: Configuration Safety Checks

- [ ] Create config with `execution_mode = "paper"` + wallet credentials
- [ ] Start bot
- [ ] Verify log warning: "Paper mode enabled but wallet provided - wallet will be IGNORED"
- [ ] Verify zero authentication errors (wallet not loaded)
- [ ] Verify zero API calls (paper mode isolation)

### Phase 6: Regression Testing

- [ ] Start bot with `execution_mode = "live"` (default)
- [ ] Verify "Execution mode: Live (real exchange API)" in logs
- [ ] Verify authentication succeeds (if wallet provided)
- [ ] Verify live trading flow unchanged (backward compatibility)

### Phase 7: Code Quality (Pre-Karen)

- [ ] All new public items have rustdoc comments
- [ ] `ExecutionMode` enum documented with usage examples
- [ ] `PaperTradingExecutionHandler` documents slippage and commission parameters
- [ ] `ExecutionHandlerWrapper` explains design rationale (enum vs trait object)
- [ ] No unused imports (`cargo clippy` check)
- [ ] No dead code (`cargo clippy` check)

### Phase 8: Git Diff Verification

- [ ] Only files in scope modified (7 files total: 5 modified + 2 new)
- [ ] Total LOC changed ≈ 170 lines (±20% tolerance)
- [ ] No unexpected files modified
- [ ] No new dependencies beyond `algo_trade_backtest`

---

## Karen Quality Gate (MANDATORY)

After completing ALL tasks, invoke Karen agent for comprehensive quality review:

```bash
Task(
  subagent_type: "general-purpose",
  description: "Karen code quality review for paper trading mode",
  prompt: "Act as Karen agent from .claude/agents/karen.md. Review packages algo-trade-hyperliquid and algo-trade-bot-orchestrator following ALL 6 phases. Include actual terminal outputs for: Phase 0 (Compilation), Phase 1 (Clippy all levels), Phase 2 (rust-analyzer), Phase 3 (Cross-file validation), Phase 4 (Per-file verification), Phase 5 (Report generation), Phase 6 (Final verification)."
)
```

### Karen Success Criteria (Zero Tolerance)

- [ ] **Phase 0**: Compilation check passes (`cargo build --package algo-trade-hyperliquid --lib` and `cargo build --package algo-trade-bot-orchestrator --lib`)
- [ ] **Phase 1**: Clippy (default + pedantic + nursery) - ZERO warnings
- [ ] **Phase 2**: rust-analyzer diagnostics - ZERO issues
- [ ] **Phase 3**: Cross-file validation - All references valid (ExecutionHandlerWrapper, PaperTradingExecutionHandler)
- [ ] **Phase 4**: Per-file verification - Each new/modified file passes individually
- [ ] **Phase 5**: Report includes actual terminal outputs for all phases
- [ ] **Phase 6**: Final verification passes (release build + tests compile)

### If Karen Fails

1. **STOP** - Do not proceed or mark playbook complete
2. **Document** - Record all Karen findings from report
3. **Fix Atomically** - Address each issue as atomic task following TaskMaster rules
4. **Re-verify** - Run Karen again after ALL fixes applied
5. **Iterate** - Repeat fix→verify cycle until Karen passes with zero issues

**Critical Rule**: This playbook is ONLY complete after Karen review passes with zero issues.

---

## Rollback Plan

If verification fails at any phase:

1. **Immediate Rollback**:
   ```bash
   git checkout -- crates/exchange-hyperliquid/src/paper_execution.rs
   git checkout -- crates/exchange-hyperliquid/src/lib.rs
   git checkout -- crates/exchange-hyperliquid/Cargo.toml
   git checkout -- crates/bot-orchestrator/src/commands.rs
   git checkout -- crates/bot-orchestrator/src/bot_actor.rs
   git checkout -- crates/bot-orchestrator/src/execution_wrapper.rs
   git checkout -- crates/bot-orchestrator/src/lib.rs
   git checkout -- config/Config.example.toml
   ```

2. **Identify Failure Root Cause**:
   - Review error messages from failed verification step
   - Check task completion status (which task failed?)
   - Review Karen report findings (if applicable)

3. **Re-run TaskMaster**:
   - Generate revised playbook with failure context
   - Address root cause in updated task specifications
   - Do NOT attempt fixes without playbook update

4. **Prevention**:
   - Run `cargo check` after each task completion
   - Run `cargo clippy` before Karen review
   - Test paper mode activation manually before final verification

---

## Summary

**Total Tasks**: 10 atomic tasks
**Total Estimated LOC**: ~170 lines
**Total Estimated Time**: 2-3 hours

**First 3 Tasks**:
1. Create `PaperTradingExecutionHandler` (50 LOC, wraps `SimulatedExecutionHandler`)
2. Add `ExecutionMode` enum (12 LOC, type-safe mode selection)
3. Update `BotConfig` with paper fields (15 LOC, config parameters)

**Critical Path**: Task 1→4→6 (execution handler creation → wrapper → bot integration)

**Risk Mitigation**:
- Enum wrapper ensures physical separation of handlers (can't mix up live/paper)
- Skip wallet loading in paper mode (zero authentication risk)
- Reuse proven `SimulatedExecutionHandler` (no new slippage/commission logic)
- Conservative defaults (10 bps slippage vs 2-5 bps in backtest)

**Validation Strategy**:
- Compile checks after each task
- Functional testing with live WebSocket + simulated fills
- Karen zero-tolerance quality gate (mandatory before completion)
- Log verification (search for "POST /exchange" - must NOT appear)

---

**Playbook Status**: Ready for Execution
**Next Step**: User approval → Execute Task 1
**Karen Review Required**: YES (mandatory after all tasks complete)
