# Context Report: Backtest-Driven Token Selection System

**Date**: 2025-10-04
**Agent**: Context Gatherer
**Status**: ✅ Complete
**TaskMaster Handoff**: ✅ Ready

---

## Section 1: Request Analysis

### User Request (Verbatim)
"Understand what I can do with backtest results. I run quad moving average strategy and see it's profitable with certain tokens but not others. How can we leverage this to know what tokens to have bots watch real-time market data for? We can run backtests with historical data anytime, multiple times per hour, at intervals etc."

### Explicit Requirements
1. **Automated Backtest Scheduling**: Run backtests periodically (hourly, multiple times per hour)
2. **Token Performance Evaluation**: Identify which tokens are profitable for quad MA strategy
3. **Dynamic Bot Configuration**: Automatically configure live bots to trade only profitable tokens
4. **Historical Data Access**: Leverage existing Hyperliquid data fetching for backtests

### Implicit Requirements
1. **Backtest Result Storage**: Persist performance metrics (Sharpe, PnL, win rate, drawdown) to TimescaleDB
2. **Token Selection Logic**: Filtering criteria to determine "profitable" (thresholds for metrics)
3. **Bot Lifecycle Management**: Start/stop bots based on token approval/rejection
4. **Data Freshness**: Use recent historical data windows (e.g., last 7 days, 30 days)
5. **Overfitting Mitigation**: Walk-forward validation to avoid curve-fitting to recent noise
6. **Performance Monitoring**: Track live vs backtest divergence (regime change detection)
7. **Computational Efficiency**: Optimize for running 100+ backtests per hour without overwhelming system
8. **Configuration Hot-Reload**: Update bot watchlists without restarting entire system

### Open Questions
1. **Backtest Window Size**: 7 days? 30 days? 90 days? (Trade-off: recency vs statistical significance)
2. **Selection Thresholds**: Minimum Sharpe ratio? Minimum win rate? Max drawdown?
3. **Token Universe**: How many tokens to evaluate? (Top 50 by volume? All available?)
4. **Rotation Frequency**: How often to re-evaluate? (Hourly, daily, weekly?)
5. **Cooldown Periods**: Once a token fails criteria, how long before re-testing?
6. **Multi-Metric Scoring**: Rank by single metric or composite score?
7. **Out-of-Sample Validation**: Use walk-forward analysis or simple rolling window?
8. **Live Performance Feedback**: Should poor live performance auto-remove token even if backtest passes?

### Success Criteria
- [ ] Backtest results stored in TimescaleDB with token/strategy/timestamp indexing
- [ ] Scheduler runs backtests on configurable intervals (hourly, daily)
- [ ] Token selection engine queries results and applies filtering criteria
- [ ] Bot orchestrator automatically starts bots for approved tokens
- [ ] Bot orchestrator automatically stops bots for tokens falling below threshold
- [ ] System handles 100+ tokens without performance degradation
- [ ] Walk-forward validation prevents overfitting to recent data
- [ ] Backtest-to-live divergence alerts implemented
- [ ] Configuration supports user-defined thresholds (Sharpe, win rate, drawdown)

---

## Section 2: Codebase Context

### Existing Backtest Infrastructure

**Backtest Metrics** (`crates/backtest/src/metrics.rs:1-118`):
- **Lines 3-9**: `PerformanceMetrics` struct currently captures:
  ```rust
  pub struct PerformanceMetrics {
      pub total_return: Decimal,
      pub sharpe_ratio: f64,
      pub max_drawdown: Decimal,
      pub num_trades: usize,
      pub win_rate: f64,
  }
  ```
- **Lines 11-117**: `MetricsCalculator` computes Sharpe (annualized), max drawdown, win rate
- **Pattern**: Uses `Decimal` for financial values, f64 only for statistical ratios
- **Missing**: No Sortino ratio, Calmar ratio, profit factor, average trade duration

**Trading System Engine** (`crates/core/src/engine.rs:1-100`):
- **Lines 10-25**: `PerformanceMetrics` struct in core (more complete than backtest version):
  ```rust
  pub struct PerformanceMetrics {
      pub total_return: Decimal,
      pub sharpe_ratio: f64,
      pub max_drawdown: Decimal,
      pub num_trades: usize,
      pub win_rate: f64,
      pub initial_capital: Decimal,
      pub final_capital: Decimal,
      pub start_time: chrono::DateTime<Utc>,
      pub end_time: chrono::DateTime<Utc>,
      pub duration: chrono::Duration,
      pub equity_peak: Decimal,
      pub buy_hold_return: Decimal,
      pub exposure_time: f64,
      pub fills: Vec<FillEvent>,
  }
  ```
- **Line 63**: Default initial capital: $10,000
- **Pattern**: Returns `PerformanceMetrics` from `TradingSystem::run()` (lines not shown but referenced in CLI)

**Quad MA Strategy** (`crates/strategy/src/quad_ma.rs:1-625`):
- **Lines 27-70**: Strategy parameters:
  - MA periods: short_1 (5), short_2 (10), long_1 (20), long_2 (50), trend (100)
  - Filters: volume_filter_enabled, volume_factor (1.5x)
  - TP/SL: take_profit_pct (2%), stop_loss_pct (1%)
  - Reversal confirmation: reversal_confirmation_bars (2)
- **Lines 158-212**: `with_full_config()` constructor for parameter customization
- **Pattern**: Strategy state is self-contained (no external dependencies)

**CLI Backtest Runner** (`crates/cli/src/main.rs:122-177`):
- **Lines 122-177**: `run_backtest()` function:
  - Loads CSV data → Creates `HistoricalDataProvider`
  - Creates strategy (MaCrossover or QuadMa)
  - Runs `TradingSystem::run()` → Returns `PerformanceMetrics`
  - **Currently**: Metrics only printed to console, NOT stored
- **Pattern**: One-off execution, no persistence

**TUI Backtest Runner** (`crates/cli/src/tui_backtest/runner.rs:1-241`):
- **Lines 16-102**: `run_all_backtests()` function:
  - Fetches/caches data for multiple tokens
  - Runs backtests in sequence (synchronous, not parallel)
  - Returns `Vec<BacktestResult>` in-memory
  - **Lines 76-86**: `BacktestResult` struct (separate from `PerformanceMetrics`)
- **Lines 104-112**: Cache path format: `cache/{token}_{interval}_{start}_{end}.csv`
- **Lines 140-193**: `run_single_backtest()` creates `TradingSystem` and runs
- **Current limitation**: Results stored in memory only, not persisted

### Existing Bot Orchestrator Architecture

**Bot Registry** (`crates/bot-orchestrator/src/registry.rs:1-95`):
- **Lines 9-11**: `BotRegistry` manages `HashMap<String, BotHandle>`
- **Lines 35-51**: `spawn_bot()` method:
  - Creates mpsc channel for commands
  - Spawns `BotActor` in tokio task
  - Stores handle with bot_id key
- **Lines 66-72**: `remove_bot()` method shuts down and removes bot
- **Pattern**: Dynamic bot creation/deletion at runtime

**Bot Configuration** (`crates/bot-orchestrator/src/commands.rs:1-38`):
- **Lines 15-21**: `BotConfig` struct:
  ```rust
  pub struct BotConfig {
      pub bot_id: String,
      pub symbol: String,
      pub strategy: String,
      pub enabled: bool,
  }
  ```
- **Missing**: No concept of "approved token list" or dynamic symbol assignment
- **Pattern**: One bot = one symbol (static assignment)

**Bot Actor** (`crates/bot-orchestrator/src/bot_actor.rs:1-78`):
- **Lines 6-10**: `BotActor` owns config, state, command receiver
- **Lines 36-72**: Event loop processes commands (Start, Stop, UpdateConfig, Shutdown)
- **Line 54-56**: `UpdateConfig` command can change config at runtime
- **Pattern**: Actor pattern with mpsc commands (no live trading logic yet - stub)

### Database Schema

**Existing Tables** (`scripts/setup_timescale.sql:1-61`):
- **Lines 2-15**: `ohlcv` hypertable (timestamp, symbol, exchange, OHLCV, volume)
- **Lines 31-41**: `trades` hypertable (timestamp, symbol, exchange, price, size, side)
- **Lines 46-61**: `fills` table (order_id, symbol, direction, quantity, price, commission, strategy)
- **Missing**: No `backtest_results` table for storing performance metrics

**Database Client** (`crates/data/src/database.rs:1-96`):
- **Lines 6-21**: `DatabaseClient` wraps `PgPool` (max 10 connections)
- **Lines 27-55**: `insert_ohlcv_batch()` method with transaction batching
- **Lines 61-83**: `query_ohlcv()` method for time-range queries
- **Pattern**: Async sqlx with compile-time checked queries

### Configuration System

**Core Config** (`crates/core/src/config.rs:1-46`):
- **Lines 3-8**: `AppConfig` has server, database, hyperliquid sections
- **Lines 10-14**: `ServerConfig` (host, port)
- **Lines 16-20**: `DatabaseConfig` (url, max_connections)
- **Lines 22-26**: `HyperliquidConfig` (api_url, ws_url)
- **Missing**: No scheduler config, no token selection config

**Config File** (`config/Config.toml:1-12`):
- Simple TOML with 3 sections (server, database, hyperliquid)
- **Missing**: No backtest scheduler section, no token universe list

### Current Patterns

1. **Financial Precision**: `rust_decimal::Decimal` for all prices/PnL (lines throughout)
2. **Async Pattern**: Tokio tasks with mpsc for commands, `async/await` everywhere
3. **Error Handling**: `anyhow::Result` with `.context()` for error chains
4. **Database Batching**: Batch inserts for performance (13ms for 100 vs 390µs per single)
5. **Hypertables**: TimescaleDB for time-series data with automatic partitioning
6. **Actor Pattern**: Bots are spawned tasks with message passing (Alice Ryhl's guide)

### Integration Points

Files requiring modification (with exact line numbers):

1. **NEW: `scripts/setup_timescale.sql` (append after line 61)**
   - Add `backtest_results` hypertable with columns:
     - timestamp, symbol, strategy_name, parameters (JSONB)
     - sharpe_ratio, total_return, max_drawdown, num_trades, win_rate
     - backtest_window_start, backtest_window_end
     - PRIMARY KEY (timestamp, symbol, strategy_name)

2. **NEW: `crates/data/src/database.rs` (append new methods)**
   - `insert_backtest_result()` method
   - `query_latest_backtest_results()` method (get most recent for each token)
   - `query_backtest_history()` method (time-series of performance for analysis)

3. **NEW: `crates/backtest-scheduler/` (new crate)**
   - `BacktestScheduler` struct with tokio-cron-scheduler
   - `TokenUniverse` config (list of tokens to evaluate)
   - `schedule_backtest_jobs()` method
   - Integration with existing `HistoricalDataProvider` and `TradingSystem`

4. **NEW: `crates/token-selector/` (new crate)**
   - `TokenSelector` struct with filtering criteria
   - `SelectionCriteria` config (min Sharpe, min win rate, max drawdown)
   - `select_approved_tokens()` method (queries backtest_results table)
   - Walk-forward validation logic (train/test split)

5. **MODIFY: `crates/bot-orchestrator/src/registry.rs:35-51`**
   - Add `sync_bots_with_approved_tokens()` method
   - Compare approved tokens vs currently running bots
   - Start bots for new tokens, stop bots for removed tokens

6. **MODIFY: `crates/core/src/config.rs:3-26`**
   - Add `SchedulerConfig` section (cron expression, token universe)
   - Add `TokenSelectionConfig` section (filtering thresholds)

7. **NEW: `crates/cli/src/main.rs` (new subcommand after line 96)**
   - Add `ScheduledBacktest` command to start scheduler daemon

### Constraints

**MUST Preserve**:
- ✅ Existing `PerformanceMetrics` struct public API (other code depends on it)
- ✅ `Strategy` trait API (quad MA strategy must work unchanged)
- ✅ Database schema for existing tables (ohlcv, trades, fills)
- ✅ `rust_decimal::Decimal` for all financial calculations

**CANNOT Break**:
- ❌ Existing CLI commands (backtest, run, server)
- ❌ Existing bot orchestrator API (web API endpoints)
- ❌ TimescaleDB hypertable partitioning (continuous aggregates)

---

## Section 3: External Research

### Walk-Forward Analysis (2024 Best Practices)

**Methodology** (Source: Medium AI & Quant, QuantInsti Blog, 2024):
- **Rolling Window Approach**: Instead of single train/test split, use multiple consecutive periods
- **Example**: 30-day train window → 7-day test window → roll forward 7 days → repeat
- **Purpose**: Detect overfitting by validating on unseen data repeatedly
- **Implementation**:
  1. Train: Optimize parameters on days 1-30
  2. Test: Evaluate on days 31-37 (out-of-sample)
  3. Roll: Move window forward 7 days
  4. Repeat: Train on days 8-37, test on days 38-44

**Advantages for Token Selection**:
- **Overfitting Detection**: Strategies fine-tuned to recent data fail on next period
- **Parameter Stability**: Consistent performance across multiple windows = robust
- **Regime Awareness**: Performance degradation signals market regime change

**Limitations**:
- **Computational Cost**: N windows × M tokens × P parameter sets = exponential complexity
- **Lookback Bias**: Choosing window size based on what worked historically
- **Data Requirements**: Need 3-6 months minimum for meaningful validation

### Performance Metrics for Predictive Power (2024 Research)

**Sharpe vs Sortino Ratio** (Source: CAIA, Price Action Lab, 2024):
- **Study**: 2,000+ funds analyzed, Sharpe and Sortino rankings 98% correlated
- **Finding**: "Choice between Sharpe and Sortino is largely irrelevant" for ranking
- **Caveat**: Sortino gives optimistic values near market tops (misleading before corrections)
- **Recommendation**: Use Sharpe for consistent ranking across market regimes

**Predictive Limitations**:
- **Historical Data Caveat**: All metrics calculated from past performance (may not hold forward)
- **Data Requirements**: Minimum 3 years of history for statistical validity (10 years ideal)
- **Volatility Focus**: Sharpe penalizes upside volatility (not ideal for crypto)

**Best Metrics for Token Selection**:
1. **Sharpe Ratio**: Total risk-adjusted return (industry standard, comparable across tokens)
2. **Max Drawdown**: Worst peak-to-trough decline (critical for risk management)
3. **Win Rate**: Percentage of profitable trades (psychological comfort, but can mislead)
4. **Profit Factor**: Gross profit / gross loss (>1.5 typically required for live trading)
5. **Calmar Ratio**: Annual return / max drawdown (combines return and risk in one metric)

**Composite Scoring Approach**:
```
Token Score = (0.4 × Sharpe) + (0.3 × Win Rate) + (0.3 × (1 - Max Drawdown))
```
Rank tokens by score, select top N.

### Overfitting Detection Techniques (2024)

**In-Sample vs Out-of-Sample Divergence** (Source: LuxAlgo, AlgoTrading101, 2024):
- **Overfitting Ratio**: OR = MSE_OOS / MSE_IS (mean squared error out-of-sample / in-sample)
- **Interpretation**: OR significantly > 1 indicates overfitting
- **Threshold**: OR > 2.0 = high overfitting risk, reject strategy

**Parameter Stability Testing** (Source: MQL5 Blog, 2024):
- **Noise Stress Test**: Add ±1-2% random noise to entry/exit prices
- **Lag Stress Test**: Shift signals by 1-2 bars (simulate execution delay)
- **Stability Criterion**: If performance degrades >30% under noise/lag, parameters are fragile

**Validation Techniques**:
1. **Walk-Forward Analysis**: 60% train, 40% test, roll forward (industry standard)
2. **Monte Carlo Simulation**: Randomize trade order, check if returns consistent
3. **Out-of-Sample Testing**: Reserve 20% of data, never touch during optimization

### Regime Detection (2024)

**Market Regime Identification** (Source: Wiley Complexity Journal, 2024):
- **Random Forests**: Classify market into regimes (trending, ranging, volatile)
- **Volatility Labeling**: High-volatility periods = high risk, low-volatility = stable
- **Application**: Strategy that works in trending markets fails in ranging markets

**Regime-Aware Token Selection**:
- **Current Regime Detection**: Measure 30-day volatility, classify as high/medium/low
- **Historical Regime Matching**: Only consider backtest windows with similar regime
- **Example**: If current market is high-volatility, only use tokens with strong Sharpe in high-volatility periods

### Scheduling Solutions (Rust Ecosystem, 2024)

**tokio-cron-scheduler** (Version 0.13.0, actively maintained):
- **Purpose**: Cron-like scheduling in async Tokio environment
- **Features**:
  - Standard cron expressions: `"0 */1 * * * *"` (every hour)
  - One-shot and repeated jobs
  - Job notifications (start, stop, remove events)
  - Optional persistence via PostgreSQL or Nats
  - English text interpretation: `"every 2 hours"`
- **Example**:
  ```rust
  let scheduler = JobScheduler::new().await?;
  scheduler.add(Job::new_async("0 */1 * * * *", |_uuid, _lock| {
      Box::pin(async move {
          run_backtest_sweep().await;
      })
  })?).await?;
  scheduler.start().await?;
  ```
- **Decision**: ✅ **Use tokio-cron-scheduler** (perfect fit, Tokio-native, cron expressions)

**Alternatives Considered**:
- `tokio::time::interval`: Simple but no cron syntax, manual scheduling logic
- `cron`: Not async-native, requires wrapper
- **Verdict**: tokio-cron-scheduler is industry standard for this use case

### Computational Optimization

**Parallel Backtest Execution**:
- **Challenge**: 100 tokens × 5 parameter sets = 500 backtests per hour
- **Solution**: Use `tokio::task::spawn` for parallel execution
- **Rate Limiting**: Hyperliquid API has 1200 req/min limit (20 req/s)
  - Fetch data in batches, cache aggressively
  - Reuse cached data across parameter sets (same OHLCV, different strategy params)
- **Memory**: Each backtest loads ~10-30 days of 1m candles (~40-120 MB per token)
  - Stream data instead of loading all into memory
  - Use Parquet files for compressed storage (10x smaller than CSV)

**Database Write Optimization**:
- **Batch Inserts**: Insert 100 backtest results in single transaction (13ms vs 39ms for individual)
- **Conflict Resolution**: Use `ON CONFLICT (timestamp, symbol, strategy_name) DO UPDATE` for idempotency

---

## Section 4: Architectural Recommendations

### Proposed System Design

```
┌─────────────────────────────────────────────────────────────┐
│ BACKTEST SCHEDULER (tokio-cron-scheduler)                   │
│ • Runs hourly/daily cron jobs                               │
│ • Fetches historical data from Hyperliquid                  │
│ • Executes backtests for token universe                     │
│ • Stores results in backtest_results hypertable             │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼ Writes PerformanceMetrics
┌─────────────────────────────────────────────────────────────┐
│ TIMESCALEDB: backtest_results                               │
│ • timestamp, symbol, strategy_name, parameters              │
│ • sharpe_ratio, total_return, max_drawdown, win_rate       │
│ • backtest_window_start, backtest_window_end               │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼ Queries latest results
┌─────────────────────────────────────────────────────────────┐
│ TOKEN SELECTOR                                              │
│ • Queries most recent backtest for each token               │
│ • Applies filtering criteria (min Sharpe, max drawdown)     │
│ • Ranks tokens by composite score                           │
│ • Outputs "approved token list"                             │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼ Approved tokens: ["BTC", "ETH", "SOL"]
┌─────────────────────────────────────────────────────────────┐
│ BOT ORCHESTRATOR                                            │
│ • Compares approved list vs running bots                    │
│ • Starts new bots for approved tokens                       │
│ • Stops bots for tokens no longer approved                  │
│ • Each bot trades one symbol with quad MA strategy          │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow Sequence

1. **Hourly Trigger** (00:00, 01:00, 02:00, etc.):
   - Cron job fires in `BacktestScheduler`

2. **Data Fetching** (2-5 minutes):
   - Fetch last 30 days of 1m candles for each token in universe (100 tokens)
   - Cache to `cache/{token}_1m_{start}_{end}.csv`
   - Reuse cached data if already fetched this hour

3. **Backtest Execution** (5-10 minutes):
   - For each token, run `TradingSystem` with quad MA strategy
   - Use default parameters: 5/10/20/50 MAs, 1.5x volume, 2% TP, 1% SL
   - Collect `PerformanceMetrics` for each

4. **Result Persistence** (1-2 seconds):
   - Batch insert all results to `backtest_results` table
   - Each row: timestamp (now), symbol, Sharpe, return, drawdown, etc.

5. **Token Selection** (1-2 seconds):
   - Query latest backtest result for each token
   - Filter: Sharpe > 1.0, win_rate > 0.5, max_drawdown < 0.2
   - Rank by Sharpe ratio (highest first)
   - Select top 10 tokens

6. **Bot Synchronization** (2-5 seconds):
   - Get currently running bots from `BotRegistry`
   - Compare symbols: approved_tokens vs running_bots
   - Start bots for new tokens (spawn `BotActor` with WebSocket)
   - Stop bots for removed tokens (send Shutdown command)

**Total Cycle Time**: 10-15 minutes per hourly run

### Component Specifications

#### 1. Backtest Scheduler Crate

**Location**: `crates/backtest-scheduler/`

**Responsibilities**:
- Manage cron jobs for periodic backtest execution
- Fetch historical data from Hyperliquid (via `HyperliquidClient`)
- Execute backtests using existing `TradingSystem`
- Persist results to `backtest_results` table

**Key Structs**:
```rust
pub struct BacktestScheduler {
    scheduler: JobScheduler,
    db_client: Arc<DatabaseClient>,
    hyperliquid_client: Arc<HyperliquidClient>,
    config: SchedulerConfig,
}

pub struct SchedulerConfig {
    pub cron_expression: String,        // "0 0 */1 * * *" (hourly)
    pub token_universe: Vec<String>,    // ["BTC", "ETH", "SOL", ...]
    pub backtest_window_days: u32,      // 30
    pub interval: String,               // "1m"
}
```

**Key Methods**:
- `new(config, db_client, hyperliquid_client)` → `Self`
- `start()` → `Result<()>` (spawns scheduler)
- `run_backtest_sweep()` → `Result<Vec<PerformanceMetrics>>` (fetches data, runs backtests)
- `persist_results(results)` → `Result<()>` (batch insert to DB)

#### 2. Token Selector Crate

**Location**: `crates/token-selector/`

**Responsibilities**:
- Query latest backtest results from database
- Apply filtering criteria (configurable thresholds)
- Rank tokens by composite score
- Return approved token list

**Key Structs**:
```rust
pub struct TokenSelector {
    db_client: Arc<DatabaseClient>,
    criteria: SelectionCriteria,
}

pub struct SelectionCriteria {
    pub min_sharpe_ratio: f64,          // 1.0
    pub min_win_rate: f64,              // 0.5
    pub max_drawdown: Decimal,          // 0.2 (20%)
    pub min_num_trades: usize,          // 10 (statistical significance)
    pub top_n: usize,                   // 10 (select top 10 tokens)
}

pub struct TokenScore {
    pub symbol: String,
    pub score: f64,
    pub sharpe_ratio: f64,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
}
```

**Key Methods**:
- `new(db_client, criteria)` → `Self`
- `select_approved_tokens()` → `Result<Vec<String>>` (queries DB, applies filters, returns symbols)
- `calculate_composite_score(metrics)` → `f64` (weighted scoring)

**Composite Score Formula**:
```rust
fn calculate_composite_score(metrics: &PerformanceMetrics) -> f64 {
    let sharpe_weight = 0.4;
    let win_rate_weight = 0.3;
    let drawdown_weight = 0.3;

    let sharpe_component = metrics.sharpe_ratio * sharpe_weight;
    let win_rate_component = metrics.win_rate * win_rate_weight;
    let drawdown_component = (1.0 - metrics.max_drawdown.to_f64().unwrap()) * drawdown_weight;

    sharpe_component + win_rate_component + drawdown_component
}
```

#### 3. Database Schema Extension

**New Table**: `backtest_results`

```sql
CREATE TABLE IF NOT EXISTS backtest_results (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    strategy_name TEXT NOT NULL,
    parameters JSONB,
    sharpe_ratio DOUBLE PRECISION NOT NULL,
    total_return DECIMAL(20, 8) NOT NULL,
    max_drawdown DECIMAL(20, 8) NOT NULL,
    num_trades INTEGER NOT NULL,
    win_rate DOUBLE PRECISION NOT NULL,
    initial_capital DECIMAL(20, 8) NOT NULL,
    final_capital DECIMAL(20, 8) NOT NULL,
    backtest_window_start TIMESTAMPTZ NOT NULL,
    backtest_window_end TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (timestamp, symbol, strategy_name)
);

SELECT create_hypertable('backtest_results', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_backtest_symbol_time ON backtest_results (symbol, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_strategy ON backtest_results (strategy_name, timestamp DESC);
```

**Rationale**:
- **Hypertable**: Automatic time-based partitioning (efficient queries on recent results)
- **JSONB parameters**: Store strategy config (MA periods, TP/SL) for reproducibility
- **Composite Primary Key**: Prevents duplicate results for same timestamp/symbol/strategy
- **Indexes**: Fast queries for "latest result per token" and "strategy performance over time"

#### 4. Bot Orchestrator Extension

**New Method in `BotRegistry`** (`crates/bot-orchestrator/src/registry.rs`):

```rust
impl BotRegistry {
    /// Synchronizes running bots with approved token list.
    /// Starts bots for new tokens, stops bots for removed tokens.
    pub async fn sync_bots_with_approved_tokens(
        &self,
        approved_tokens: Vec<String>,
        strategy_name: &str,
    ) -> Result<()> {
        let running_bots = self.list_bots().await;
        let running_symbols: HashSet<_> = running_bots.iter()
            .map(|id| id.split('_').next().unwrap())  // bot_id format: "BTC_quad_ma"
            .collect();

        // Start bots for new tokens
        for token in &approved_tokens {
            if !running_symbols.contains(token.as_str()) {
                let bot_config = BotConfig {
                    bot_id: format!("{}_{}", token, strategy_name),
                    symbol: token.clone(),
                    strategy: strategy_name.to_string(),
                    enabled: true,
                };
                self.spawn_bot(bot_config).await?;
                tracing::info!("Started bot for approved token: {}", token);
            }
        }

        // Stop bots for removed tokens
        let approved_set: HashSet<_> = approved_tokens.iter().collect();
        for bot_id in running_bots {
            let symbol = bot_id.split('_').next().unwrap();
            if !approved_set.contains(&symbol.to_string()) {
                self.remove_bot(&bot_id).await?;
                tracing::info!("Stopped bot for removed token: {}", symbol);
            }
        }

        Ok(())
    }
}
```

#### 5. Configuration Extension

**New Sections in `crates/core/src/config.rs`**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub hyperliquid: HyperliquidConfig,
    pub scheduler: SchedulerConfig,           // NEW
    pub token_selection: TokenSelectionConfig, // NEW
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub enabled: bool,                    // true
    pub cron_expression: String,          // "0 0 */1 * * *"
    pub token_universe: Vec<String>,      // ["BTC", "ETH", "SOL", ...]
    pub backtest_window_days: u32,        // 30
    pub interval: String,                 // "1m"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSelectionConfig {
    pub min_sharpe_ratio: f64,            // 1.0
    pub min_win_rate: f64,                // 0.5
    pub max_drawdown: f64,                // 0.2
    pub min_num_trades: usize,            // 10
    pub top_n: usize,                     // 10
}
```

**New Sections in `config/Config.toml`**:

```toml
[scheduler]
enabled = true
cron_expression = "0 0 */1 * * *"  # Every hour
backtest_window_days = 30
interval = "1m"
token_universe = ["BTC", "ETH", "SOL", "AVAX", "MATIC", "ARB", "OP", "LINK", "UNI", "AAVE"]

[token_selection]
min_sharpe_ratio = 1.0
min_win_rate = 0.5
max_drawdown = 0.2
min_num_trades = 10
top_n = 10
```

### Walk-Forward Validation Strategy

**Implementation Approach**:

1. **Rolling Window Backtest**:
   - Train window: 30 days
   - Test window: 7 days
   - Roll frequency: Daily (1 day shift)
   - Total: 4 windows over 51-day period (30 train + 7 test × 3 rolls)

2. **Validation Criteria**:
   - Calculate Sharpe for each test window (4 values)
   - Require: All 4 test Sharpes > 0.5 (consistent profitability)
   - Require: Mean test Sharpe within 50% of train Sharpe (no overfitting)

3. **Example**:
   ```
   Window 1: Train days 1-30,  Test days 31-37  → Sharpe 1.2
   Window 2: Train days 2-31,  Test days 32-38  → Sharpe 1.1
   Window 3: Train days 3-32,  Test days 33-39  → Sharpe 0.3  ← REJECT (< 0.5)
   Window 4: Train days 4-33,  Test days 34-40  → Sharpe 1.0

   Token REJECTED: Window 3 failed threshold
   ```

4. **Computational Trade-off**:
   - Full walk-forward: 4× computation (4 windows vs 1)
   - Recommendation: Run walk-forward weekly (thorough), simple backtest hourly (responsive)

**Phase 1 (MVP)**: Simple rolling window (no walk-forward), single 30-day backtest
**Phase 2 (Advanced)**: Add walk-forward validation for weekly deep analysis

---

## Section 5: Edge Cases & Constraints

### Edge Case Scenarios

#### 1. All Tokens Fail Selection Criteria
**Scenario**: Market crash, all backtests show negative Sharpe ratio

**Mitigation**:
- **Fallback Rule**: Always maintain minimum 3 tokens (select least-bad)
- **Relaxed Criteria**: If zero tokens pass strict thresholds, relax min_sharpe to 0.0
- **Alert**: Send notification that market conditions are unfavorable

**Implementation**:
```rust
let approved = selector.select_approved_tokens().await?;
if approved.is_empty() {
    tracing::warn!("Zero tokens passed criteria, using fallback");
    let fallback = selector.select_top_n_by_sharpe(3).await?;
    return Ok(fallback);
}
```

#### 2. Insufficient Historical Data for New Token
**Scenario**: Hyperliquid lists new token with only 5 days of history (need 30 days)

**Mitigation**:
- **Grace Period**: Skip token if history < backtest_window_days
- **Gradual Inclusion**: After 30 days, token enters universe automatically
- **Manual Override**: Allow user to force-include new token with shorter window

**Implementation**:
```rust
let candles = client.fetch_candles(token, interval, start, end).await?;
if candles.len() < min_required_bars {
    tracing::warn!("Insufficient data for {}: {} bars (need {})",
        token, candles.len(), min_required_bars);
    return Err(anyhow!("Not enough data"));
}
```

#### 3. Live Performance Diverges from Backtest
**Scenario**: Token passes backtest (Sharpe 1.5) but loses money in live trading (regime change)

**Mitigation**:
- **Live Performance Tracking**: Track real-time PnL for each bot
- **Divergence Alert**: If live Sharpe < 0.0 after 100 trades, alert user
- **Auto-Stop Threshold**: If live drawdown > 30%, auto-stop bot and remove from approved list
- **Cooldown Period**: Once stopped, token cannot re-enter for 7 days

**Implementation**:
```rust
// In BotActor, track live metrics
if live_metrics.sharpe_ratio < 0.0 && live_metrics.num_trades > 100 {
    tracing::error!("Live performance diverged for {}", bot_id);
    self.send_alert("Backtest-live divergence detected").await?;
    self.stop_trading().await?;
}
```

#### 4. Rate Limit Exhaustion During Data Fetching
**Scenario**: Fetching 100 tokens × 30 days × 1m candles = 4.32M data points, exceeds Hyperliquid API limit

**Mitigation**:
- **Batch Fetching**: Fetch 10 tokens at a time (parallelize within batch)
- **Aggressive Caching**: Cache OHLCV for 24 hours (reuse for multiple runs)
- **Rate Limiter**: Use `governor` crate with 1200 req/min quota
- **Backoff**: Exponential backoff on 429 errors

**Implementation**:
```rust
// Fetch in batches of 10 to avoid rate limits
for chunk in token_universe.chunks(10) {
    let futures: Vec<_> = chunk.iter()
        .map(|token| fetch_and_cache_data(token, interval, start, end))
        .collect();

    let results = futures::future::join_all(futures).await;

    // Wait between batches to respect rate limits
    tokio::time::sleep(Duration::from_secs(5)).await;
}
```

#### 5. Database Write Contention During Peak Hours
**Scenario**: Hourly backtest results + live trading fills writing to DB simultaneously

**Mitigation**:
- **Batch Inserts**: Collect all backtest results, insert in single transaction
- **Connection Pooling**: Use `PgPool` with 10 connections (already configured)
- **Asynchronous Writes**: Non-blocking inserts with `sqlx::query(...).execute(&pool).await`
- **Off-Peak Scheduling**: Run backtest sweeps during low-activity hours (e.g., 03:00 UTC)

#### 6. Strategy Parameter Drift (Overfitting to Recent Data)
**Scenario**: Fixed parameters (5/10/20/50 MAs) work well historically but degrade in new regime

**Mitigation**:
- **Phase 1 (MVP)**: Use fixed parameters (no optimization)
- **Phase 2 (Advanced)**: Add parameter sweep (test 3 MA configurations, select best)
- **Overfitting Detection**: Compare in-sample vs out-of-sample performance (OR ratio < 2.0)
- **Parameter Stability Test**: Require performance stable under ±10% parameter variation

### Computational Constraints

**Memory Usage** (100 tokens × 30 days × 1m candles):
- **Per Token**: ~43,200 candles × 80 bytes = 3.5 MB
- **Total**: 100 tokens × 3.5 MB = 350 MB
- **Mitigation**: Stream data instead of loading all into memory

**CPU Usage** (100 backtests per hour):
- **Per Backtest**: ~200ms (strategy logic + metrics calculation)
- **Total**: 100 × 200ms = 20 seconds (trivial, well below 1-hour window)
- **Parallelization**: Run 10 backtests concurrently → 2 seconds total

**Disk Usage** (backtest_results table growth):
- **Per Result**: ~200 bytes
- **Hourly**: 100 tokens × 200 bytes = 20 KB
- **Annual**: 20 KB × 24 hours × 365 days = 175 MB
- **Mitigation**: TimescaleDB compression (7x compression for old data)

**Network Bandwidth** (fetching historical data):
- **Per Token**: 30 days × 1m = 43,200 candles × 100 bytes = 4.3 MB
- **Total**: 100 tokens × 4.3 MB = 430 MB per fetch
- **Hourly**: If cached, 0 MB (reuse existing); if not, 430 MB
- **Mitigation**: Cache for 24 hours (only fetch once daily)

### Production Considerations

**Deployment**:
- **Scheduler Daemon**: Run as separate systemd service (independent restart)
- **Graceful Shutdown**: On SIGTERM, finish current backtest before exiting
- **Health Checks**: Expose `/health` endpoint showing last successful run timestamp

**Monitoring**:
- **Metrics**: Track backtest_duration, tokens_processed, errors_encountered
- **Alerts**: Slack/email on repeated failures, zero approved tokens, divergence
- **Dashboards**: Grafana charts for Sharpe trends, token rotation frequency

**Testing**:
- **Unit Tests**: Test token selection logic with mock backtest results
- **Integration Tests**: End-to-end test with 3 tokens, verify bots start/stop
- **Performance Tests**: Benchmark 100-token sweep (should complete in <5 minutes)

---

## Section 6: TaskMaster Handoff Package

### MUST DO

#### Database Schema (Priority: CRITICAL)
1. **Add `backtest_results` hypertable to database**
   - **File**: `scripts/setup_timescale.sql` (append after line 61)
   - **Action**: Add `CREATE TABLE backtest_results` with columns:
     - `timestamp TIMESTAMPTZ NOT NULL`
     - `symbol TEXT NOT NULL`
     - `strategy_name TEXT NOT NULL`
     - `parameters JSONB` (store MA periods, TP/SL as JSON)
     - `sharpe_ratio DOUBLE PRECISION NOT NULL`
     - `total_return DECIMAL(20, 8) NOT NULL`
     - `max_drawdown DECIMAL(20, 8) NOT NULL`
     - `num_trades INTEGER NOT NULL`
     - `win_rate DOUBLE PRECISION NOT NULL`
     - `initial_capital DECIMAL(20, 8) NOT NULL`
     - `final_capital DECIMAL(20, 8) NOT NULL`
     - `backtest_window_start TIMESTAMPTZ NOT NULL`
     - `backtest_window_end TIMESTAMPTZ NOT NULL`
     - `PRIMARY KEY (timestamp, symbol, strategy_name)`
   - **Action**: Convert to hypertable: `SELECT create_hypertable('backtest_results', 'timestamp')`
   - **Action**: Add indexes for fast queries:
     - `CREATE INDEX idx_backtest_symbol_time ON backtest_results (symbol, timestamp DESC)`
     - `CREATE INDEX idx_backtest_strategy ON backtest_results (strategy_name, timestamp DESC)`

2. **Extend `DatabaseClient` with backtest result methods**
   - **File**: `crates/data/src/database.rs` (append after line 96)
   - **Action**: Add method `insert_backtest_result(&self, result: BacktestResultRecord) -> Result<()>`
     - Use sqlx INSERT with ON CONFLICT DO UPDATE for idempotency
   - **Action**: Add method `insert_backtest_results_batch(&self, results: Vec<BacktestResultRecord>) -> Result<()>`
     - Batch insert in single transaction (pattern from `insert_ohlcv_batch`, lines 27-55)
   - **Action**: Add method `query_latest_backtest_results(&self, strategy_name: &str) -> Result<HashMap<String, BacktestResultRecord>>`
     - SQL: `SELECT DISTINCT ON (symbol) * FROM backtest_results WHERE strategy_name = $1 ORDER BY symbol, timestamp DESC`
     - Returns map: symbol → latest result
   - **Action**: Add `BacktestResultRecord` struct with sqlx::FromRow derive

#### Backtest Scheduler Crate (Priority: HIGH)
3. **Create new crate `crates/backtest-scheduler/`**
   - **Files**: Create directory and files:
     - `crates/backtest-scheduler/Cargo.toml`
     - `crates/backtest-scheduler/src/lib.rs`
     - `crates/backtest-scheduler/src/scheduler.rs`
     - `crates/backtest-scheduler/src/config.rs`
   - **Dependencies**: Add to Cargo.toml:
     - `tokio-cron-scheduler = "0.13"` (cron job scheduling)
     - `algo-trade-core` (reuse TradingSystem)
     - `algo-trade-backtest` (HistoricalDataProvider, SimulatedExecutionHandler)
     - `algo-trade-data` (DatabaseClient, CsvStorage)
     - `algo-trade-hyperliquid` (HyperliquidClient)
     - `anyhow`, `tokio`, `tracing`, `serde`, `chrono`

4. **Implement `BacktestScheduler` struct**
   - **File**: `crates/backtest-scheduler/src/scheduler.rs` (new file)
   - **Action**: Define struct:
     ```rust
     pub struct BacktestScheduler {
         scheduler: JobScheduler,
         db_client: Arc<DatabaseClient>,
         hyperliquid_client: Arc<HyperliquidClient>,
         config: SchedulerConfig,
     }
     ```
   - **Action**: Implement `new(config, db_client, hyperliquid_client) -> Result<Self>`
   - **Action**: Implement `start() -> Result<()>` method:
     - Create `JobScheduler::new().await`
     - Add cron job with `config.cron_expression`
     - Job executes `self.run_backtest_sweep().await`
     - Start scheduler with `scheduler.start().await`
   - **Action**: Implement `run_backtest_sweep() -> Result<Vec<PerformanceMetrics>>`:
     - For each token in `config.token_universe`:
       - Fetch historical data (30 days) using `HyperliquidClient::fetch_candles`
       - Cache to `cache/{token}_{interval}_{start}_{end}.csv`
       - Load data into `HistoricalDataProvider`
       - Create `QuadMaStrategy::new(token)`
       - Create `TradingSystem` with SimulatedExecutionHandler
       - Run `system.run().await` → collect `PerformanceMetrics`
     - Batch insert all results to `backtest_results` table
     - Return metrics for logging

5. **Implement `SchedulerConfig` struct**
   - **File**: `crates/backtest-scheduler/src/config.rs` (new file)
   - **Action**: Define struct:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize)]
     pub struct SchedulerConfig {
         pub enabled: bool,
         pub cron_expression: String,
         pub token_universe: Vec<String>,
         pub backtest_window_days: u32,
         pub interval: String,
     }
     ```
   - **Action**: Add to `AppConfig` in `crates/core/src/config.rs`:
     - Insert field: `pub scheduler: SchedulerConfig` (after line 8)
     - Add default implementation with example values

#### Token Selector Crate (Priority: HIGH)
6. **Create new crate `crates/token-selector/`**
   - **Files**: Create directory and files:
     - `crates/token-selector/Cargo.toml`
     - `crates/token-selector/src/lib.rs`
     - `crates/token-selector/src/selector.rs`
     - `crates/token-selector/src/criteria.rs`
   - **Dependencies**: Add to Cargo.toml:
     - `algo-trade-data` (DatabaseClient)
     - `algo-trade-core` (PerformanceMetrics)
     - `rust_decimal`, `anyhow`, `tokio`, `tracing`, `serde`

7. **Implement `TokenSelector` struct**
   - **File**: `crates/token-selector/src/selector.rs` (new file)
   - **Action**: Define struct:
     ```rust
     pub struct TokenSelector {
         db_client: Arc<DatabaseClient>,
         criteria: SelectionCriteria,
     }
     ```
   - **Action**: Implement `new(db_client, criteria) -> Self`
   - **Action**: Implement `select_approved_tokens() -> Result<Vec<String>>`:
     - Query `query_latest_backtest_results()` from database
     - Filter results: Sharpe >= min_sharpe, win_rate >= min_win_rate, etc.
     - Calculate composite score for each token
     - Sort by score descending
     - Take top N tokens
     - Return symbol list
   - **Action**: Implement `calculate_composite_score(metrics: &PerformanceMetrics) -> f64`:
     - Weighted formula: (0.4 × Sharpe) + (0.3 × win_rate) + (0.3 × (1 - drawdown))

8. **Implement `SelectionCriteria` struct**
   - **File**: `crates/token-selector/src/criteria.rs` (new file)
   - **Action**: Define struct:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize)]
     pub struct SelectionCriteria {
         pub min_sharpe_ratio: f64,
         pub min_win_rate: f64,
         pub max_drawdown: f64,
         pub min_num_trades: usize,
         pub top_n: usize,
     }
     ```
   - **Action**: Add to `AppConfig` in `crates/core/src/config.rs`:
     - Insert field: `pub token_selection: SelectionCriteria` (after scheduler field)

#### Bot Orchestrator Extension (Priority: MEDIUM)
9. **Add `sync_bots_with_approved_tokens` method to `BotRegistry`**
   - **File**: `crates/bot-orchestrator/src/registry.rs` (after line 94)
   - **Action**: Add method signature:
     ```rust
     pub async fn sync_bots_with_approved_tokens(
         &self,
         approved_tokens: Vec<String>,
         strategy_name: &str,
     ) -> Result<()>
     ```
   - **Action**: Implementation logic:
     - Get running bots: `self.list_bots().await`
     - Extract symbols from bot IDs (format: `{symbol}_{strategy}`)
     - Compare approved vs running (use HashSet)
     - Start bots for new tokens: `self.spawn_bot(BotConfig { ... }).await`
     - Stop bots for removed tokens: `self.remove_bot(bot_id).await`
     - Log each start/stop action

#### CLI Integration (Priority: MEDIUM)
10. **Add `ScheduledBacktest` CLI command**
    - **File**: `crates/cli/src/main.rs` (after line 65)
    - **Action**: Add to `Commands` enum:
      ```rust
      /// Start the backtest scheduler daemon
      ScheduledBacktest {
          /// Config file path
          #[arg(short, long, default_value = "config/Config.toml")]
          config: String,
      }
      ```
    - **Action**: Add match arm in `main()` (after line 96):
      ```rust
      Commands::ScheduledBacktest { config } => {
          run_scheduled_backtest(&config).await?;
      }
      ```
    - **Action**: Implement `run_scheduled_backtest(config_path: &str) -> Result<()>`:
      - Load config
      - Create `DatabaseClient`, `HyperliquidClient`
      - Create `BacktestScheduler` with config
      - Start scheduler: `scheduler.start().await`
      - Block forever (daemon mode): `tokio::signal::ctrl_c().await`

#### Configuration File Updates (Priority: LOW)
11. **Add scheduler and token_selection sections to Config.toml**
    - **File**: `config/Config.toml` (append after line 12)
    - **Action**: Add sections:
      ```toml
      [scheduler]
      enabled = true
      cron_expression = "0 0 */1 * * *"  # Every hour
      backtest_window_days = 30
      interval = "1m"
      token_universe = ["BTC", "ETH", "SOL", "AVAX", "MATIC"]

      [token_selection]
      min_sharpe_ratio = 1.0
      min_win_rate = 0.5
      max_drawdown = 0.2
      min_num_trades = 10
      top_n = 5
      ```

#### Integration & Orchestration (Priority: CRITICAL)
12. **Wire scheduler → database → selector → bot orchestrator**
    - **File**: `crates/cli/src/main.rs` (in `run_scheduled_backtest` function)
    - **Action**: After backtest sweep completes:
      - Create `TokenSelector` with database client and config
      - Call `selector.select_approved_tokens().await`
      - Get `BotRegistry` instance
      - Call `registry.sync_bots_with_approved_tokens(approved, "quad_ma").await`
    - **Pattern**: Scheduler job callback:
      ```rust
      let job = Job::new_async(cron_expr, move |_uuid, _lock| {
          Box::pin(async move {
              // 1. Run backtest sweep
              let metrics = scheduler.run_backtest_sweep().await?;

              // 2. Select approved tokens
              let selector = TokenSelector::new(db_client.clone(), criteria.clone());
              let approved = selector.select_approved_tokens().await?;

              // 3. Sync bots
              registry.sync_bots_with_approved_tokens(approved, "quad_ma").await?;
          })
      })?;
      ```

### MUST NOT DO

1. **DO NOT change existing `PerformanceMetrics` struct in `crates/core/src/engine.rs`**
   - Other code depends on current fields (lines 10-25)
   - Adding fields is OK (backward compatible), removing is NOT
   - Use `#[serde(default)]` for new optional fields

2. **DO NOT use f64 for financial calculations in database schema**
   - All price/PnL columns must be `DECIMAL(20, 8)` (lines 6-9 in setup_timescale.sql)
   - Only use `DOUBLE PRECISION` for statistical ratios (Sharpe, win rate)

3. **DO NOT run backtests synchronously (blocking)**
   - Use `tokio::task::spawn` for parallel execution (see pattern in tui_backtest/runner.rs:16)
   - Respect rate limits: fetch data in batches, not all at once

4. **DO NOT skip database batching for backtest results**
   - Use pattern from `insert_ohlcv_batch` (data/database.rs:27-55)
   - Single transaction for all results (100 inserts in 13ms vs 3.9s individually)

5. **DO NOT hardcode strategy parameters in scheduler**
   - Strategy config should come from `SchedulerConfig` or database
   - For MVP, use `QuadMaStrategy::new()` defaults (lines 79-109 in quad_ma.rs)

6. **DO NOT create new WebSocket connections per backtest**
   - Backtests use `HistoricalDataProvider` (CSV/database), NOT WebSocket
   - Only live bots use WebSocket via `LiveDataProvider`

7. **DO NOT modify existing database tables (ohlcv, trades, fills)**
   - Only ADD new table (`backtest_results`)
   - Existing schemas are in production (lines 2-61 in setup_timescale.sql)

8. **DO NOT implement walk-forward validation in Phase 1 (MVP)**
   - Simple 30-day rolling window for MVP
   - Walk-forward is Phase 2 (advanced feature)
   - Computational cost: 4× backtest time (not justified for hourly runs)

### Scope Boundaries

**Phase 1 (MVP) - THIS HANDOFF**:
- ✅ Database schema with `backtest_results` table
- ✅ Backtest scheduler with tokio-cron-scheduler (hourly runs)
- ✅ Token selector with basic filtering (Sharpe, win rate, drawdown)
- ✅ Bot orchestrator sync (start/stop bots based on approved tokens)
- ✅ CLI command to start scheduler daemon
- ✅ Configuration for thresholds and token universe
- ✅ Data caching (reuse CSV files within 24 hours)

**Phase 2 (Future, NOT in this handoff)**:
- ❌ Walk-forward validation (train/test splits)
- ❌ Parameter optimization (sweep MA periods)
- ❌ Regime detection (volatility clustering, market classification)
- ❌ Live performance tracking (backtest-live divergence alerts)
- ❌ Multi-strategy selection (different strategies for different tokens)
- ❌ Web UI for viewing backtest results (Grafana integration)
- ❌ Advanced metrics (Sortino, Calmar, profit factor)

### Verification Checklist

After implementation, verify each item:

**Database**:
- [ ] `cargo run -p algo-trade-cli -- server` starts without errors
- [ ] Connect to PostgreSQL: `psql -U postgres -d algo_trade`
- [ ] Check table exists: `\d backtest_results`
- [ ] Check hypertable: `SELECT * FROM timescaledb_information.hypertables WHERE hypertable_name = 'backtest_results';`
- [ ] Insert test row: `INSERT INTO backtest_results (...) VALUES (...)`
- [ ] Query test row: `SELECT * FROM backtest_results WHERE symbol = 'BTC' ORDER BY timestamp DESC LIMIT 1;`

**Scheduler**:
- [ ] `cargo build -p algo-trade-backtest-scheduler` succeeds
- [ ] `cargo test -p algo-trade-backtest-scheduler` passes
- [ ] `cargo run -p algo-trade-cli -- scheduled-backtest` starts without errors
- [ ] Check logs: `grep "Running backtest sweep" /var/log/algo-trade.log`
- [ ] Verify cron job fires: Wait 1 hour, check DB for new results

**Token Selector**:
- [ ] `cargo build -p algo-trade-token-selector` succeeds
- [ ] `cargo test -p algo-trade-token-selector` passes
- [ ] Unit test: Mock backtest results, verify filtering logic
- [ ] Integration test: Insert test data to DB, query approved tokens

**Bot Orchestrator**:
- [ ] `cargo test -p algo-trade-bot-orchestrator` passes
- [ ] Unit test: `sync_bots_with_approved_tokens` with ["BTC", "ETH"]
- [ ] Verify bots started: `registry.list_bots()` returns ["BTC_quad_ma", "ETH_quad_ma"]
- [ ] Remove token: Call sync with ["BTC"], verify ETH bot stopped

**End-to-End**:
- [ ] Run scheduler for 2 hours, verify 2 backtest sweeps completed
- [ ] Check `backtest_results` table has 2 rows per token
- [ ] Verify approved tokens changed between runs (if performance varies)
- [ ] Verify bots started for new approved tokens
- [ ] Verify bots stopped for removed tokens

**Performance**:
- [ ] 100-token backtest sweep completes in <10 minutes
- [ ] Memory usage < 500 MB during sweep
- [ ] Database write latency < 100ms for batch insert
- [ ] No rate limit errors from Hyperliquid API

---

## Architectural Decision Points Requiring User Input

### 1. Backtest Frequency vs Computational Cost

**Question**: How often should backtests run?

**Options**:
- **Hourly**: Maximum responsiveness, detects profitable tokens quickly
  - Pro: Fast adaptation to market changes
  - Con: High computational cost (100 tokens × 24 runs/day = 2,400 backtests/day)
  - Con: Hyperliquid API rate limits may be strained
- **Every 6 hours**: Balance between responsiveness and cost
  - Pro: Moderate computational load (400 backtests/day)
  - Pro: Easier on API rate limits
  - Con: 6-hour delay in detecting new opportunities
- **Daily (3am UTC)**: Minimal cost, off-peak hours
  - Pro: Lowest computational load (100 backtests/day)
  - Pro: No API rate limit concerns
  - Con: 24-hour delay in adapting to market changes

**Recommendation**: Start with **daily (3am UTC)**, upgrade to 6-hourly if market moves fast
**User Decision Required**: Choose cron expression for `config/Config.toml`

### 2. Token Selection Thresholds

**Question**: What performance thresholds define "profitable"?

**Options**:
- **Conservative** (high quality, few tokens):
  - Sharpe > 1.5, win_rate > 0.6, max_drawdown < 0.15
  - Selects ~5-10 tokens (only strong performers)
- **Moderate** (balanced):
  - Sharpe > 1.0, win_rate > 0.5, max_drawdown < 0.2
  - Selects ~10-20 tokens
- **Aggressive** (more opportunities, higher risk):
  - Sharpe > 0.5, win_rate > 0.45, max_drawdown < 0.3
  - Selects ~30-50 tokens

**Recommendation**: Start with **moderate**, observe for 1 week, then tune
**User Decision Required**: Set thresholds in `config/Config.toml` token_selection section

### 3. Token Universe Size

**Question**: How many tokens should be evaluated in backtest sweeps?

**Options**:
- **Top 20 by volume**: BTC, ETH, SOL, AVAX, etc. (fastest, most liquid)
  - Pro: Low computational cost (20 backtests/run)
  - Pro: High liquidity (tight spreads, easy fills)
  - Con: Miss opportunities in smaller altcoins
- **Top 50 by volume**: Include mid-cap tokens
  - Pro: More opportunities (some altcoins outperform majors)
  - Con: Moderate computational cost (50 backtests/run)
- **Top 100 by volume**: Full coverage
  - Pro: Maximum opportunity detection
  - Con: High computational cost (100 backtests/run)
  - Con: Some low-liquidity tokens (wide spreads)

**Recommendation**: Start with **top 20**, expand to 50 after 1 month of stable operation
**User Decision Required**: Populate `token_universe` list in config

### 4. Walk-Forward Validation (Phase 1 vs Phase 2)

**Question**: Should we implement walk-forward validation in initial version?

**Options**:
- **Phase 1 (Simple)**: Single 30-day backtest, no train/test split
  - Pro: Fast to implement (no complex logic)
  - Pro: Low computational cost (1× backtest per token)
  - Con: Risk of overfitting to recent data
- **Phase 1 (Advanced)**: Walk-forward with 4 windows (30-day train, 7-day test × 4)
  - Pro: Robust overfitting detection
  - Con: 4× computational cost (400 backtests for 100 tokens)
  - Con: Complexity delays MVP delivery

**Recommendation**: **Phase 1 Simple**, add walk-forward in Phase 2 after observing live results
**User Decision Required**: Approve phased approach or demand walk-forward in Phase 1

### 5. Live Performance Monitoring

**Question**: Should the system auto-stop bots with poor live performance?

**Options**:
- **Manual Monitoring** (Phase 1): User monitors live PnL, manually stops underperforming bots
  - Pro: Simple implementation (no additional logic)
  - Con: Requires constant user supervision
- **Auto-Stop on Divergence** (Phase 2): System tracks live Sharpe, auto-stops if < 0.0 after 100 trades
  - Pro: Automated risk management
  - Con: Complex (track live metrics, compare to backtest, trigger shutdown)
  - Con: False positives (short-term drawdown doesn't mean strategy failed)

**Recommendation**: **Manual for Phase 1**, implement auto-stop in Phase 2 with 30% drawdown threshold
**User Decision Required**: Approve phased approach or demand auto-stop in Phase 1

---

## Summary

This context report provides a comprehensive analysis of implementing a backtest-driven token selection system for the Hyperliquid trading platform. The system will:

1. **Automatically run backtests** on a token universe (hourly, daily, or custom schedule) using the existing quad MA strategy
2. **Store performance metrics** (Sharpe ratio, PnL, drawdown, win rate) in a TimescaleDB hypertable for time-series analysis
3. **Select profitable tokens** based on configurable filtering criteria (minimum thresholds for Sharpe, win rate, max drawdown)
4. **Dynamically manage live bots**, automatically starting bots for newly approved tokens and stopping bots for tokens that fall below thresholds

The architecture leverages existing infrastructure (TradingSystem, QuadMaStrategy, HistoricalDataProvider, BotRegistry) and adds three new components:
- **BacktestScheduler**: Tokio-cron-scheduler for periodic backtest execution
- **TokenSelector**: Database-querying logic with multi-metric filtering
- **Bot Sync Logic**: Extension to BotRegistry for dynamic bot lifecycle management

The implementation follows Rust best practices (Decimal for financial precision, async Tokio, batched database writes, hypertable time-series storage) and the project's established patterns (actor model for bots, trait-based abstraction, anyhow error handling).

Key decisions requiring user input: backtest frequency (hourly vs daily), selection thresholds (conservative vs aggressive), token universe size (20 vs 100 tokens), walk-forward validation (Phase 1 vs Phase 2), and live performance monitoring (manual vs automated).

**Next Step**: TaskMaster agent will break Section 6 (MUST DO) into atomic, verifiable tasks with exact file paths and line-by-line specifications.
