# Context Report: Paper Trading Mode

**Date**: 2025-10-06
**Agent**: Context Gatherer
**Feature**: Paper Trading Mode with Live OHLCV Data

---

## Section 1: Request Analysis

### User Request (Verbatim)

> "Create a paper trading mode which connects to live OHLCV in real-time. It will take positions but not use real money but paper money to ensure our bots are working and setup correctly."

### Explicit Requirements

1. **Live Data Connection**: Connect to real-time OHLCV market data (not historical replay)
2. **Simulated Execution**: Execute trades with virtual money (no real exchange API calls)
3. **Position Tracking**: Track paper positions to validate bot logic
4. **Bot Validation**: Ensure bots work correctly before live deployment

### Implicit Requirements

1. **Safety**: Zero risk of accidental real money trading
2. **Production Validation**: Bot should run with identical code paths as live mode (backtest-live parity)
3. **Configuration**: Easy toggle between paper/live modes without code changes
4. **Realistic Simulation**: Paper fills should model real-world behavior (slippage, commission)
5. **Observability**: TUI monitor should display paper trading events (fills, positions, PnL)
6. **Initial Capital**: Configurable paper account starting balance
7. **WebSocket Connection**: Reuse existing live data infrastructure
8. **Isolated State**: Paper portfolio isolated from any live trading state

### Open Questions

1. **Slippage Model**: Should we use same slippage as backtest (`SimulatedExecutionHandler`), or more aggressive?
2. **Fill Timing**: Instant fills vs realistic delay simulation?
3. **Persistence**: Should paper portfolio state persist across restarts, or always start fresh?
4. **Multiple Symbols**: Can paper mode trade multiple symbols concurrently (like live)?
5. **WebSocket Failures**: If live feed drops, should paper trading halt or continue with last known price?

### Success Criteria

- [ ] User can set `execution_mode = "paper"` in bot config
- [ ] Bot connects to live Hyperliquid WebSocket for OHLCV data
- [ ] Orders execute with simulated fills (no Hyperliquid POST requests)
- [ ] TUI live bot monitor shows paper trading events (market updates, signals, fills, positions)
- [ ] Paper PnL calculated and displayed in real-time
- [ ] Zero risk of real money loss (all execution handler calls simulated)
- [ ] Backtest-live parity maintained (same strategy code, same risk manager)

### Research Scope Boundaries

**IN SCOPE**:
- New `PaperTradingExecutionHandler` in `exchange-hyperliquid` crate
- Add `execution_mode` enum to `BotConfig`
- Update `BotActor::initialize_system()` to choose execution handler
- Configuration schema changes
- TUI display enhancements (if needed)

**OUT OF SCOPE**:
- Persistent paper portfolio storage (start fresh on restart)
- Advanced fill simulation (Level 2 order book, partial fills)
- Cross-symbol paper portfolio (one bot = one paper account)
- Paper trading analytics/reporting (reuse existing metrics)
- CLI commands to manage paper accounts

---

## Section 2: Codebase Context

### 2.1 Current Trading Modes

The system currently supports **two modes**:

1. **Backtest Mode**: Historical data (`HistoricalDataProvider`) + Simulated execution (`SimulatedExecutionHandler`)
2. **Live Mode**: WebSocket data (`LiveDataProvider`) + Real API execution (`LiveExecutionHandler`)

**Paper mode** = Live data + Simulated execution (a NEW combination)

### 2.2 Execution Handlers

#### File: `/home/a/Work/algo-trade/crates/backtest/src/execution.rs`

**Lines 8-11**: `SimulatedExecutionHandler` struct
```rust
pub struct SimulatedExecutionHandler {
    commission_rate: Decimal,
    slippage_bps: Decimal,
}
```

**Lines 13-26**: Constructor with commission and slippage parameters
```rust
pub fn new(commission_rate: f64, slippage_bps: f64) -> Self
```

**Lines 28-34**: Slippage calculation logic
```rust
fn apply_slippage(&self, price: Decimal, direction: &OrderDirection) -> Decimal {
    let slippage = price * self.slippage_bps / Decimal::from(10000);
    match direction {
        OrderDirection::Buy => price + slippage,   // Buy higher
        OrderDirection::Sell => price - slippage,  // Sell lower
    }
}
```

**Lines 38-64**: `ExecutionHandler` trait implementation
- Market orders: apply slippage to current price
- Limit orders: assume instant fill at limit price
- Commission calculated as `fill_price * quantity * commission_rate`
- Returns `FillEvent` with UUID order ID

**Pattern**: This is the PERFECT base for paper trading - already has slippage/commission modeling

#### File: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`

**Lines 11-13**: `LiveExecutionHandler` struct
```rust
pub struct LiveExecutionHandler {
    client: HyperliquidClient,
}
```

**Lines 24-91**: `ExecutionHandler` trait implementation
- **Line 27-42**: Builds JSON order request for Hyperliquid API
- **Line 44**: `self.client.post_signed("/exchange", order_request).await?` ← REAL API CALL
- **Line 47-56**: Parses response status, checks for errors
- **Line 58-75**: Extracts fill data from API response

**Key Observation**: Paper mode MUST NOT call `client.post_signed()` - this is the real money risk

### 2.3 Data Providers

#### File: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/data_provider.rs`

**Lines 11-16**: `LiveDataProvider` struct
```rust
pub struct LiveDataProvider {
    ws: HyperliquidWebSocket,
    symbol: String,
    interval: String,
}
```

**Lines 18-39**: Constructor with WebSocket subscription
- Connects to Hyperliquid WebSocket
- Subscribes to candle feed for symbol/interval
- **Pattern**: Paper mode can reuse this EXACTLY (live data source)

**Lines 41-74**: `warmup()` method
- Fetches historical candles via REST API for strategy initialization
- **Pattern**: Paper mode needs this too (strategy warmup with historical data)

**Lines 94-124**: `DataProvider` trait implementation
- Polls WebSocket for next candle message
- Parses JSON to `MarketEvent::Bar`
- **Pattern**: Paper mode uses this verbatim (same data provider)

### 2.4 Bot Configuration

#### File: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`

**Lines 19-48**: `BotConfig` struct
```rust
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
    pub interval: String,
    pub ws_url: String,
    pub api_url: String,
    pub warmup_periods: usize,
    pub strategy_config: Option<String>,
    pub initial_capital: Decimal,
    pub risk_per_trade_pct: f64,
    pub max_position_pct: f64,
    pub leverage: u8,
    pub margin_mode: MarginMode,
    pub wallet: Option<WalletConfig>,  // ← Only needed for live mode
}
```

**Integration Point**: Add new field `execution_mode: ExecutionMode` here

**Lines 50-64**: Default values (initial_capital = 10000, risk = 5%, max_position = 20%)

**Lines 82-94**: `WalletConfig` struct
- **Line 90**: `api_wallet_private_key` - only needed for live trading
- **Pattern**: Paper mode should NOT load wallet (skip authentication)

### 2.5 Bot Actor System Initialization

#### File: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`

**Lines 12-22**: `BotActor` struct
```rust
pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
    system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,  // ← Hardcoded types
    event_tx: broadcast::Sender<BotEvent>,
    status_tx: watch::Sender<EnhancedBotStatus>,
    recent_events: VecDeque<BotEvent>,
}
```

**Critical Issue**: `TradingSystem` is generic over `<D, E>` (data provider, execution handler), but `BotActor` hardcodes `LiveDataProvider` and `LiveExecutionHandler`

**Lines 47-122**: `initialize_system()` method
- **Lines 52-57**: Creates `LiveDataProvider` with WebSocket connection
- **Lines 59-69**: Calls `warmup()` to fetch historical data
- **Lines 72-85**: Creates `HyperliquidClient` (authenticated if wallet provided)
- **Lines 87-88**: Creates `LiveExecutionHandler` with client
- **Lines 90-100**: Creates strategy and feeds warmup events
- **Lines 105-110**: Creates `SimpleRiskManager` with config parameters
- **Lines 113-118**: Constructs `TradingSystem::with_capital()`

**Integration Points**:
- **Line 16**: Change `system` field type to support multiple execution handlers (requires enum or dynamic dispatch)
- **Lines 87-88**: Conditional: create `LiveExecutionHandler` OR `PaperTradingExecutionHandler` based on config
- **Lines 72-85**: Skip client creation if paper mode (no authentication needed)

**Lines 124-164**: `trading_loop()` method
- **Line 129**: Calls `system.process_next_event().await`
- Returns `ProcessingCycleEvents` with fills, orders, signals
- **Pattern**: Paper mode reuses this loop EXACTLY (same event processing)

**Lines 166-204**: `emit_cycle_events()` method
- Broadcasts market updates, signals, orders, fills via channels
- **Pattern**: TUI monitor already listens to these events (no changes needed)

### 2.6 Trading System Engine

#### File: `/home/a/Work/algo-trade/crates/core/src/engine.rs`

**Lines 36-59**: `TradingSystem<D, E>` struct (generic over data provider and execution handler)
```rust
pub struct TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    data_provider: D,
    execution_handler: E,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
    position_tracker: PositionTracker,
    initial_capital: Decimal,
    // ... metrics fields ...
}
```

**Lines 95-122**: `with_capital()` constructor (used by BotActor)
- Accepts initial capital as parameter
- **Pattern**: Paper mode uses this constructor with paper capital (e.g., $10k virtual)

**Lines 319-414**: `process_next_event()` method (called by BotActor trading loop)
- **Line 320**: Polls data provider for next market event
- **Lines 352-379**: Generates signals, evaluates risk, executes orders
- **Line 369**: `self.execution_handler.execute_order(order.clone()).await?` ← Key line
- **Pattern**: This line calls EITHER `LiveExecutionHandler` (real API) OR `PaperTradingExecutionHandler` (simulated)

**Lines 434-529**: Public accessor methods for metrics
- Used by BotActor to populate `EnhancedBotStatus` for TUI display
- **Pattern**: Paper mode metrics shown in TUI (no changes needed)

### 2.7 Position Tracking

#### File: `/home/a/Work/algo-trade/crates/core/src/position.rs`

**Lines 5-10**: `Position` struct
```rust
pub struct Position {
    pub symbol: String,
    pub quantity: Decimal,
    pub avg_price: Decimal,
}
```

**Lines 23-33**: `PositionTracker` struct (in-memory HashMap)
- **Pattern**: Paper mode reuses this (isolated paper portfolio per bot)

**Lines 35-106**: `process_fill()` method
- Handles long/short opening, adding, closing
- Calculates realized PnL on position close
- **Pattern**: Paper mode uses this EXACTLY (same position logic)

**Lines 108-116**: Public accessors (`get_position()`, `all_positions()`)
- Used by TradingSystem and BotActor for metrics
- **Pattern**: Paper positions displayed in TUI via these methods

### 2.8 Core Traits

#### File: `/home/a/Work/algo-trade/crates/core/src/traits.rs`

**Lines 6-9**: `DataProvider` trait
```rust
pub trait DataProvider: Send + Sync {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>>;
}
```

**Lines 17-20**: `ExecutionHandler` trait
```rust
pub trait ExecutionHandler: Send + Sync {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent>;
}
```

**Pattern**: Paper mode implements `ExecutionHandler` trait with simulated fills

### 2.9 TUI Live Bot Monitor

#### File: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`

**Lines 1-19**: Imports and dependencies (Ratatui for TUI rendering)

**Lines 97-105**: `BotScreen` enum (BotList, BotMonitor, Configuration, etc.)

**Pattern**: TUI subscribes to `BotEvent` broadcast channel from BotActor
- Events are agnostic to execution mode (paper vs live)
- TUI displays fills, positions, PnL identically for paper and live
- **No TUI changes needed** (events are mode-agnostic)

### 2.10 Configuration File

#### File: `/home/a/Work/algo-trade/config/Config.toml`

```toml
[server]
host = "0.0.0.0"
port = 8080

[database]
url = "postgresql://localhost/algo_trade"
max_connections = 10

[hyperliquid]
api_url = "https://api.hyperliquid.xyz"
ws_url = "wss://api.hyperliquid.xyz/ws"
```

**Pattern**: Bot configs are loaded separately (per-bot TOML or JSON)
- **Integration Point**: Add `execution_mode = "paper"` field to bot config schema

---

## Section 3: External Research

### 3.1 Paper Trading Architecture Patterns

Based on research of QuantConnect, Backtrader, and industry best practices:

#### QuantConnect LEAN Architecture

**Key Findings**:
1. **Live Data + Virtual Brokerage**: Paper trading uses real-time data feeds but `PaperBrokerage.cs` for simulated fills
2. **Brokerage Model Pattern**: Uses `DefaultBrokerageModel` to simulate fills if no custom model set
3. **Backtest Parity**: Reuses `BacktestingBrokerage.cs` logic for fill simulation (same slippage/commission models)
4. **Data Resolution Matters**: Minute data gives precise timing; daily bars execute on next open
5. **Configuration Flag**: `isLive` flag lets algorithms differentiate backtest vs live data sources

**Architecture Diagram** (QuantConnect):
```
LiveDataFeed → Algorithm → Signal → PaperBrokerage (simulated fill) → Portfolio Update
```

**Source**: [QuantConnect Paper Trading Docs](https://www.quantconnect.com/docs/v2/cloud-platform/live-trading/brokerages/quantconnect-paper-trading)

#### Backtrader Cerebro Pattern

**Key Findings**:
1. **Cerebro Brain**: Central orchestrator coordinates data feeds, strategies, brokers
2. **Broker Abstraction**: Swap between `BacktestBroker` and `LiveBroker` without strategy changes
3. **Extensible Objects**: Components are Python objects with clear interfaces
4. **Performance Tracking**: Built-in metrics (PnL, Sharpe, drawdown) work across all modes

**Pattern**: Similar to our trait-based approach (DataProvider, ExecutionHandler)

### 3.2 Slippage and Fill Modeling Best Practices

Based on Quantitative Finance Stack Exchange and industry blogs:

#### Slippage Modeling

**Simple Model** (volume < 1% of market):
- Assume 1 basis point (0.01%) slippage
- Buy orders: fill at `price * (1 + slippage_bps / 10000)`
- Sell orders: fill at `price * (1 - slippage_bps / 10000)`

**Realistic Model** (for paper trading):
- Use higher slippage than backtest (e.g., 5-10 bps vs 2-5 bps)
- Model volatility impact: higher slippage during high volatility periods
- Partial fills for large orders (NOT implemented in this phase)

**Current Implementation**: `SimulatedExecutionHandler` uses 2-5 bps by default
**Recommendation**: Paper mode should use 5-10 bps (more conservative than backtest)

**Source**: [How to simulate slippage](https://quant.stackexchange.com/questions/1264/how-to-simulate-slippage)

#### Commission Modeling

**Hyperliquid Fees**:
- Maker fee: -0.01% (rebate)
- Taker fee: +0.025%
- Market orders: always taker (0.025%)
- Limit orders: maker if resting (rebate), taker if instant fill

**Current Implementation**: `SimulatedExecutionHandler` uses configurable `commission_rate`
**Recommendation**: Paper mode use 0.025% (taker fee assumption) for conservatism

#### Fill Timing

**Instant vs Delayed**:
- Backtest: Instant fills (deterministic)
- Paper trading: Instant fills OK (real-time validation focus, not microstructure simulation)
- Live production: Actual exchange latency (100-500ms)

**Recommendation**: Paper mode = instant fills (same as backtest, matches our architecture)

**Source**: [Paper Trading Insights](https://blog.traderspost.io/article/the-reliability-of-paper-trading-insights-and-best-practices)

### 3.3 Position Tracking and Portfolio Management

**Best Practices**:
1. **Isolated Paper Portfolio**: One paper portfolio per bot (no cross-contamination with live)
2. **In-Memory State**: Start fresh on restart (simple, no persistence overhead)
3. **Metrics Parity**: Reuse existing metrics (equity curve, Sharpe, drawdown) - already implemented
4. **Performance Review**: Weekly/monthly review of paper performance before live deployment

**Current Implementation**: `PositionTracker` already isolates positions per symbol in HashMap
**Recommendation**: Reuse existing `PositionTracker` (perfect fit for paper mode)

---

## Section 4: Analysis & Synthesis

### 4.1 Architectural Recommendation

**Proposed Architecture**: **PaperTradingExecutionHandler** Pattern

```
LiveDataProvider (WebSocket) → Strategy → Signal → RiskManager → Order
    ↓
PaperTradingExecutionHandler (simulated fill) → FillEvent → PositionTracker
```

**Key Design Decisions**:

1. **Reuse `SimulatedExecutionHandler` Logic**: Create `PaperTradingExecutionHandler` in `exchange-hyperliquid` crate that wraps/extends `SimulatedExecutionHandler` from `backtest` crate
   - **Rationale**: Proven slippage/commission modeling, no duplication
   - **Location**: `crates/exchange-hyperliquid/src/paper_execution.rs` (NEW FILE)

2. **Add `ExecutionMode` Enum to `BotConfig`**:
   ```rust
   pub enum ExecutionMode {
       Live,   // Use LiveExecutionHandler (real API)
       Paper,  // Use PaperTradingExecutionHandler (simulated)
   }
   ```
   - **Rationale**: Explicit, type-safe mode selection
   - **Location**: `crates/bot-orchestrator/src/commands.rs` (lines 19-48, add new field)

3. **Conditional Execution Handler in `BotActor`**:
   - **Challenge**: `BotActor` currently hardcodes `TradingSystem<LiveDataProvider, LiveExecutionHandler>`
   - **Solution**: Use **enum-based dynamic dispatch**:
     ```rust
     enum ExecutionHandlerWrapper {
         Live(LiveExecutionHandler),
         Paper(PaperTradingExecutionHandler),
     }

     impl ExecutionHandler for ExecutionHandlerWrapper {
         async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
             match self {
                 Self::Live(h) => h.execute_order(order).await,
                 Self::Paper(h) => h.execute_order(order).await,
             }
         }
     }
     ```
   - **Rationale**: Avoids complex trait object Boxing, zero-cost at runtime (enum dispatch)
   - **Location**: `crates/bot-orchestrator/src/bot_actor.rs` (lines 12-22, update system field type)

4. **Skip Authentication for Paper Mode**:
   - If `execution_mode = Paper`, skip `HyperliquidClient` creation with wallet
   - No API calls = no authentication needed
   - **Rationale**: Safety (can't accidentally send real orders), performance (no auth overhead)
   - **Location**: `crates/bot-orchestrator/src/bot_actor.rs` (lines 72-85, add conditional)

5. **Configuration Parameters**:
   - `execution_mode`: "live" or "paper" (default: "live" for safety)
   - `paper_slippage_bps`: Paper trading slippage (default: 10 bps = 0.1%)
   - `paper_commission_rate`: Commission rate (default: 0.00025 = 0.025% Hyperliquid taker fee)
   - **Location**: `crates/bot-orchestrator/src/commands.rs` (add to BotConfig)

6. **TUI Display**:
   - No changes needed (events are mode-agnostic)
   - OPTIONAL enhancement: Display mode indicator ("PAPER" badge) in bot status
   - **Location**: `crates/cli/src/tui_live_bot.rs` (optional cosmetic change)

### 4.2 Alternative Designs Considered (and rejected)

#### Alternative 1: Separate `PaperTradingSystem` Struct
- **Approach**: Create entire parallel `PaperTradingSystem` struct
- **Rejected**: Massive code duplication, breaks backtest-live parity principle

#### Alternative 2: Trait Objects (Box<dyn ExecutionHandler>)
- **Approach**: `system: Option<TradingSystem<LiveDataProvider, Box<dyn ExecutionHandler>>>`
- **Rejected**: Heap allocation overhead, trait object complexity, harder debugging

#### Alternative 3: Compile-Time Feature Flag
- **Approach**: `#[cfg(feature = "paper-trading")]` conditional compilation
- **Rejected**: Can't switch modes at runtime, user friction (rebuild required)

#### Alternative 4: Extend `LiveExecutionHandler` with `dry_run` Flag
- **Approach**: Add `dry_run: bool` to `LiveExecutionHandler`, skip API call if true
- **Rejected**: Pollutes live handler with paper logic, risk of bugs (forgot to check flag)

**Chosen Approach** (enum wrapper) balances:
- Type safety (enum exhaustiveness)
- Runtime flexibility (no recompilation)
- Zero duplication (reuse existing handlers)
- Safety (physically separate handlers = can't mix up)

### 4.3 Implementation Strategy

**Phase 1**: Create `PaperTradingExecutionHandler`
- File: `crates/exchange-hyperliquid/src/paper_execution.rs`
- Wraps `SimulatedExecutionHandler` from backtest crate
- Configurable slippage and commission

**Phase 2**: Add `ExecutionMode` enum and config fields
- File: `crates/bot-orchestrator/src/commands.rs`
- Add `ExecutionMode` enum
- Add `execution_mode`, `paper_slippage_bps`, `paper_commission_rate` fields to `BotConfig`

**Phase 3**: Create `ExecutionHandlerWrapper` enum
- File: `crates/bot-orchestrator/src/execution_wrapper.rs` (NEW FILE)
- Enum with Live and Paper variants
- Implement `ExecutionHandler` trait for wrapper

**Phase 4**: Update `BotActor` to use wrapper
- File: `crates/bot-orchestrator/src/bot_actor.rs`
- Update `system` field type to use `ExecutionHandlerWrapper`
- Modify `initialize_system()` to choose handler based on config
- Skip wallet loading if paper mode

**Phase 5**: Integration testing
- Start bot in paper mode
- Verify WebSocket connection
- Verify simulated fills
- Verify TUI display (positions, PnL)
- Verify zero API calls to Hyperliquid

---

## Section 5: Edge Cases & Constraints

### 5.1 WebSocket Connection Failures

**Scenario**: Live data feed disconnects mid-trading

**Current Behavior**: `LiveDataProvider` has auto-reconnect with exponential backoff

**Paper Mode Behavior**: Same as live (reconnect automatically)

**Edge Case**: What if reconnect fails permanently?
- **Constraint**: Bot should halt trading (can't trade without data)
- **Solution**: BotActor transitions to Error state, user notified via TUI

### 5.2 Market Data Gaps

**Scenario**: WebSocket reconnects, missing 5 minutes of candles

**Current Behavior**: `next_event()` returns next available candle (gap in timestamp)

**Paper Mode Risk**: Strategy state may be stale (e.g., MA calculations missing recent prices)

**Mitigation**: Same as live mode (acceptable, strategy handles gaps)

**Enhancement** (OUT OF SCOPE): Backfill missing candles from REST API on reconnect

### 5.3 Concurrent Bot Safety

**Scenario**: User runs multiple bots in paper mode with same symbol

**Current Behavior**: Each bot has isolated `PositionTracker` (separate paper portfolios)

**Safety Check**: Paper positions can't interfere (no shared state)

**Edge Case**: Two bots subscribe to same WebSocket symbol
- **Current Behavior**: Each bot creates separate WebSocket connection
- **Constraint**: Hyperliquid may throttle duplicate subscriptions (unlikely, no auth required)
- **Mitigation**: Acceptable (paper mode = validation, not production scale)

### 5.4 Memory Consumption

**Scenario**: Bot runs for days/weeks in paper mode, accumulates fills

**Current Behavior**: `TradingSystem` stores all fills in `Vec<FillEvent>` (unbounded growth)

**Constraint**: Long-running paper bot may consume excessive memory

**Mitigation Options**:
1. **OUT OF SCOPE**: Add `max_fills_history` limit (retain last N fills)
2. **ACCEPTED RISK**: User restarts bot periodically (validation workflow, not 24/7 production)

**Recommendation**: Accept risk for Phase 1, document in user guide

### 5.5 Configuration Errors

**Scenario**: User sets `execution_mode = "paper"` but provides wallet credentials

**Current Behavior**: Wallet loaded, but never used (paper handler doesn't call API)

**Risk**: User confusion (why did I provide wallet if paper mode?)

**Mitigation**: Log warning if paper mode + wallet provided:
```rust
if config.execution_mode == ExecutionMode::Paper && config.wallet.is_some() {
    tracing::warn!("Paper mode enabled but wallet provided - wallet will be ignored");
}
```

**Location**: `crates/bot-orchestrator/src/bot_actor.rs` (lines 72-85, add warning)

### 5.6 Slippage Overestimation

**Scenario**: Paper mode uses 10 bps slippage, but live market has 2 bps actual slippage

**Impact**: Paper performance appears worse than live reality

**Rationale**: **CONSERVATIVE BIAS IS GOOD**
- Better to underestimate paper profits than overestimate
- Prevents false confidence before live deployment

**Recommendation**: Document in user guide (paper slippage is intentionally pessimistic)

### 5.7 Commission Model Simplification

**Scenario**: Hyperliquid uses maker/taker fees, but paper mode uses flat 0.025% taker fee

**Impact**: Paper mode slightly overestimates costs for limit orders (if they'd get maker rebate)

**Rationale**: Simplicity + conservatism (taker fee is worst case)

**Enhancement** (OUT OF SCOPE): Advanced fill simulator with maker/taker detection

**Recommendation**: Accept simplification for Phase 1

### 5.8 Order Type Limitations

**Scenario**: User places limit order in paper mode

**Current Behavior**: `SimulatedExecutionHandler` assumes instant fill at limit price

**Real World**: Limit order may not fill, or fill later

**Constraint**: Paper mode can't perfectly simulate order book dynamics without Level 2 data

**Mitigation**: Document limitation (paper mode is for strategy validation, not microstructure testing)

**Recommendation**: Accept limitation, focus on strategy logic validation (not fill timing)

---

## Section 6: TaskMaster Handoff Package

### 6.1 MUST DO (Scope Definition)

#### Task Category 1: Core Implementation

1. **Create `PaperTradingExecutionHandler`**
   - **File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/paper_execution.rs` (NEW FILE)
   - **Specification**:
     - Import `SimulatedExecutionHandler` from `algo_trade_backtest`
     - Create struct wrapping `SimulatedExecutionHandler`
     - Implement `ExecutionHandler` trait (delegate to inner handler)
     - Constructor accepts slippage and commission parameters
   - **Dependencies**: `algo_trade_backtest` crate, `algo_trade_core` traits
   - **Estimated LOC**: ~50 lines

2. **Add `ExecutionMode` enum**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
   - **Location**: After line 17, before `BotCommand` enum
   - **Specification**:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
     pub enum ExecutionMode {
         Live,
         Paper,
     }

     impl Default for ExecutionMode {
         fn default() -> Self {
             Self::Live  // Safe default
         }
     }
     ```
   - **Estimated LOC**: ~12 lines

3. **Update `BotConfig` struct**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
   - **Location**: Lines 19-48, add new fields
   - **Specification**:
     ```rust
     // Add after line 29 (after strategy_config field):

     // Execution mode (live vs paper trading)
     #[serde(default)]
     pub execution_mode: ExecutionMode,

     // Paper trading parameters
     #[serde(default = "default_paper_slippage")]
     pub paper_slippage_bps: f64,

     #[serde(default = "default_paper_commission")]
     pub paper_commission_rate: f64,

     // Add after line 64:
     const fn default_paper_slippage() -> f64 {
         10.0  // 10 bps = 0.1%
     }

     const fn default_paper_commission() -> f64 {
         0.00025  // 0.025% (Hyperliquid taker fee)
     }
     ```
   - **Estimated LOC**: ~15 lines

4. **Create `ExecutionHandlerWrapper` enum**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/execution_wrapper.rs` (NEW FILE)
   - **Specification**:
     - Create enum with `Live(LiveExecutionHandler)` and `Paper(PaperTradingExecutionHandler)` variants
     - Implement `ExecutionHandler` trait with match-based delegation
     - Import from `algo_trade_hyperliquid` crate
   - **Estimated LOC**: ~40 lines

5. **Update `BotActor` system field type**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
   - **Location**: Line 16
   - **Change**:
     ```rust
     // OLD:
     system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,

     // NEW:
     system: Option<TradingSystem<LiveDataProvider, ExecutionHandlerWrapper>>,
     ```
   - **Estimated LOC**: 1 line change

6. **Update `BotActor::initialize_system()` method**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
   - **Location**: Lines 47-122
   - **Specification**:
     - After line 69 (warmup complete), add conditional execution handler creation
     - If `self.config.execution_mode == ExecutionMode::Live`: create `LiveExecutionHandler` (existing logic)
     - If `self.config.execution_mode == ExecutionMode::Paper`: create `PaperTradingExecutionHandler` (skip client creation)
     - Wrap chosen handler in `ExecutionHandlerWrapper` enum
     - Log warning if paper mode + wallet provided
   - **Estimated LOC**: ~30 lines (add conditional block)

#### Task Category 2: Export and Module Wiring

7. **Export `PaperTradingExecutionHandler` from `exchange-hyperliquid` crate**
   - **File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`
   - **Specification**: Add `mod paper_execution; pub use paper_execution::PaperTradingExecutionHandler;`
   - **Estimated LOC**: 2 lines

8. **Export `ExecutionHandlerWrapper` from `bot-orchestrator` crate**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`
   - **Specification**: Add `mod execution_wrapper;` (internal module, not public export)
   - **Estimated LOC**: 1 line

9. **Update `bot-orchestrator` Cargo.toml dependencies**
   - **File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/Cargo.toml`
   - **Specification**: Ensure `algo_trade_backtest` is in dependencies (for `SimulatedExecutionHandler` import in wrapper)
   - **Estimated LOC**: 1 line (if not already present)

#### Task Category 3: Configuration and Documentation

10. **Update example config with paper mode**
    - **File**: `/home/a/Work/algo-trade/config/Config.example.toml`
    - **Specification**: Add commented example bot config with `execution_mode = "paper"`
    - **Estimated LOC**: ~10 lines

### 6.2 MUST NOT DO (Explicit Exclusions)

1. **DO NOT modify `LiveExecutionHandler`**: Keep live handler pristine (no dry_run flags, no paper logic)
2. **DO NOT modify `SimulatedExecutionHandler`**: Reuse as-is from backtest crate (no changes)
3. **DO NOT modify `LiveDataProvider`**: Paper mode uses live data provider verbatim
4. **DO NOT modify TUI display logic**: Events are already mode-agnostic (TUI works as-is)
5. **DO NOT add persistent storage**: Paper portfolio resets on bot restart (in-memory only)
6. **DO NOT implement partial fills**: Instant complete fills (same as backtest)
7. **DO NOT implement Level 2 order book**: Simple slippage model (no market depth simulation)
8. **DO NOT add paper-specific metrics**: Reuse existing `TradingSystem` metrics (equity, Sharpe, etc.)
9. **DO NOT create separate CLI commands**: Use existing `cargo run -p algo-trade-cli -- run` with paper config
10. **DO NOT modify core traits**: `ExecutionHandler` and `DataProvider` traits stay unchanged

### 6.3 Integration Points (Exact File Locations)

| Component | File Path | Lines | Modification Type |
|-----------|-----------|-------|-------------------|
| BotConfig | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs` | 19-48 | Add fields |
| ExecutionMode enum | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs` | After 17 | New enum |
| BotActor system field | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs` | 16 | Change type |
| BotActor::initialize_system | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs` | 47-122 | Add conditional |
| PaperTradingExecutionHandler | `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/paper_execution.rs` | NEW FILE | Create struct |
| ExecutionHandlerWrapper | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/execution_wrapper.rs` | NEW FILE | Create enum |
| exchange-hyperliquid lib.rs | `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs` | N/A | Add export |
| bot-orchestrator lib.rs | `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs` | N/A | Add module |

### 6.4 Acceptance Criteria (Verification Steps)

#### Functional Tests

1. **Paper Mode Activation**
   - [ ] Create bot config with `execution_mode = "paper"`
   - [ ] Start bot with `cargo run -p algo-trade-cli -- run --config <paper_bot.json>`
   - [ ] Verify log message: "Execution mode: Paper (simulated fills)"
   - [ ] Verify log message: "Connected to live WebSocket feed"

2. **Simulated Fill Verification**
   - [ ] Bot receives live market data (WebSocket candles)
   - [ ] Strategy generates signal
   - [ ] Order execution produces fill event
   - [ ] Verify log: "Paper fill executed: {symbol} {direction} {quantity} @ {price}"
   - [ ] Verify NO Hyperliquid API POST request in logs (search for "POST /exchange")

3. **Position Tracking**
   - [ ] Paper bot takes long position
   - [ ] Verify TUI displays open position with quantity and avg price
   - [ ] Strategy generates exit signal
   - [ ] Verify position closes, realized PnL calculated
   - [ ] Verify TUI updates to show closed position

4. **Metrics Display**
   - [ ] TUI shows current equity (starts at initial_capital, e.g., $10,000)
   - [ ] After profitable trade: equity increases
   - [ ] After losing trade: equity decreases
   - [ ] Sharpe ratio, max drawdown, win rate update in real-time

5. **WebSocket Resilience**
   - [ ] Disconnect WebSocket mid-trading (simulate network failure)
   - [ ] Verify bot reconnects automatically
   - [ ] Verify trading resumes after reconnect

6. **Configuration Safety**
   - [ ] Set `execution_mode = "paper"` + provide wallet credentials
   - [ ] Verify log warning: "Paper mode enabled but wallet provided - wallet will be ignored"
   - [ ] Verify zero authentication errors (wallet not used)

#### Regression Tests

7. **Live Mode Unchanged**
   - [ ] Start bot with `execution_mode = "live"` (or default)
   - [ ] Verify `LiveExecutionHandler` used (authentication required)
   - [ ] Verify live trading flow works as before (no regressions)

8. **Backtest Mode Unchanged**
   - [ ] Run backtest: `cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv`
   - [ ] Verify `SimulatedExecutionHandler` used
   - [ ] Verify backtest metrics match pre-change baseline

#### Code Quality

9. **Compilation**
   - [ ] `cargo check` passes with zero warnings
   - [ ] `cargo clippy -- -D warnings` passes
   - [ ] `cargo build --release` succeeds

10. **Documentation**
    - [ ] All new public items have rustdoc comments
    - [ ] `ExecutionMode` enum documented with usage examples
    - [ ] `PaperTradingExecutionHandler` documents slippage and commission parameters

### 6.5 Dependencies and Constraints

**Crate Dependencies**:
- `bot-orchestrator` crate depends on `exchange-hyperliquid` (already exists)
- `exchange-hyperliquid` crate depends on `backtest` (NEW DEPENDENCY - add to Cargo.toml)
- `bot-orchestrator` may need `backtest` for `SimulatedExecutionHandler` import (check if transitive dependency suffices)

**Type System Constraints**:
- `TradingSystem<D, E>` is generic - changing `E` type requires updating `BotActor` field type
- Enum wrapper avoids trait object complexity while maintaining type safety

**Configuration Compatibility**:
- Default `execution_mode = "live"` ensures backward compatibility (existing configs work as-is)
- New fields have `#[serde(default)]` attribute (optional in config files)

**Runtime Safety**:
- Paper mode MUST NOT call `HyperliquidClient::post_signed()` (verify with code review + integration test)
- WebSocket connection is authenticated (read-only, safe for paper mode)

### 6.6 Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Accidental live trading in paper mode | CRITICAL | Enum wrapper ensures physical separation of handlers; integration test verifies zero API calls |
| Paper fills differ from live fills | MEDIUM | Document slippage assumptions; user validates on small live positions after paper validation |
| Memory leak from unbounded fills | LOW | Document restart recommendation; out-of-scope for Phase 1 |
| WebSocket throttling (multiple paper bots) | LOW | Acceptable for validation use case; not production deployment |
| Configuration confusion (mode mismatch) | LOW | Log warnings; clear documentation |

---

## Section 7: Report Generation

### 7.1 Summary

Paper trading mode enables risk-free validation of bot strategies using live market data with simulated execution. This feature bridges the gap between historical backtesting and live deployment, allowing users to verify bot logic, configuration, and strategy performance in real-time without capital risk.

**Key Implementation Approach**:
- Reuse existing `SimulatedExecutionHandler` (proven slippage/commission logic)
- Create lightweight `PaperTradingExecutionHandler` wrapper in `exchange-hyperliquid` crate
- Add `ExecutionMode` enum to `BotConfig` for explicit mode selection
- Use enum-based `ExecutionHandlerWrapper` for type-safe dispatch (avoid trait objects)
- Modify `BotActor::initialize_system()` to choose execution handler based on config
- Zero changes to TUI, data providers, or trading system core (backtest-live parity maintained)

**Scope**: 10 atomic tasks, ~170 LOC, 2 new files, minimal surface area changes

### 7.2 Research Citations

1. **QuantConnect Paper Trading**: https://www.quantconnect.com/docs/v2/cloud-platform/live-trading/brokerages/quantconnect-paper-trading
2. **Slippage Modeling**: https://quant.stackexchange.com/questions/1264/how-to-simulate-slippage
3. **Paper Trading Best Practices**: https://blog.traderspost.io/article/the-reliability-of-paper-trading-insights-and-best-practices
4. **Backtrader Architecture**: https://www.ml4trading.io/chapter/7 (Machine Learning for Trading)

### 7.3 Files for TaskMaster Review

**Existing Files to Modify**:
1. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs` (add ExecutionMode enum, update BotConfig)
2. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs` (update system type, modify initialize_system)
3. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs` (add module export)
4. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs` (add module declaration)
5. `/home/a/Work/algo-trade/config/Config.example.toml` (add paper mode example)

**New Files to Create**:
1. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/paper_execution.rs` (PaperTradingExecutionHandler)
2. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/execution_wrapper.rs` (ExecutionHandlerWrapper enum)

**Dependency Changes**:
1. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml` (add `algo_trade_backtest` dependency)

### 7.4 Next Steps for TaskMaster

TaskMaster should:
1. Read **Section 6: TaskMaster Handoff Package** (contains all atomic tasks)
2. Extract MUST DO / MUST NOT DO lists (scope boundaries defined)
3. Use exact file paths and line numbers from **Section 2: Codebase Context**
4. Generate atomic playbook with verification criteria per task
5. Ensure each task is < 50 LOC (already sized in handoff package)
6. Define task dependencies (e.g., Task 1 before Task 4, Task 2 before Task 3)

**Task Dependency Graph**:
```
Task 1 (PaperTradingExecutionHandler) ──┐
                                         ├──> Task 4 (ExecutionHandlerWrapper)
Task 2 (ExecutionMode enum) ────────────┘          │
Task 3 (BotConfig fields) ─────────────────────────┤
                                                    ├──> Task 6 (BotActor::initialize_system)
Task 5 (BotActor system field) ────────────────────┘
Task 7-9 (Exports/modules) ───> After all core tasks
Task 10 (Config example) ───> Last (documentation)
```

**Estimated Implementation Time**: 2-3 hours (10 atomic tasks, well-defined scope)

---

**Report Generated**: 2025-10-06
**Context Gatherer Agent**: COMPLETE
**Handoff to**: TaskMaster (ready for playbook generation)
