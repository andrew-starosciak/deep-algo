# Playbook: Backtest-Driven Token Selection System

**Date**: 2025-10-04
**Agent**: TaskMaster
**Status**: Ready for Execution
**Context Report**: `.claude/context/2025-10-04_backtest-driven-token-selection.md`

---

## User Request

"Understand what I can do with backtest results. I run quad moving average strategy and see it's profitable with certain tokens but not others. How can we leverage this to know what tokens to have bots watch real-time market data for? We can run backtests with historical data anytime, multiple times per hour, at intervals etc."

---

## Configuration Decisions

Based on user input, the following configuration defaults will be used:

- **Backtest Frequency**: Daily at 3am UTC (`"0 0 3 * * *"`)
- **Token Selection Thresholds**: Moderate (Sharpe > 1.0, Win Rate > 50%, Max Drawdown < 20%)
- **Token Universe**: Top 20 tokens by volume (BTC, ETH, SOL, AVAX, ARB, OP, MATIC, ATOM, NEAR, FTM, INJ, SEI, SUI, APT, TIA, DOGE, SHIB, WLD, PEPE, BONK)
- **Backtest Window**: 30 days (simple rolling window, no walk-forward in Phase 1)
- **Live Monitoring**: Manual (no auto-stop bots in Phase 1)

---

## Scope Boundaries

### MUST DO (from Context Report Section 6)

#### Database Schema (Priority: CRITICAL)
1. ✅ Add `backtest_results` hypertable to database
2. ✅ Extend `DatabaseClient` with backtest result methods

#### Backtest Scheduler Crate (Priority: HIGH)
3. ✅ Create new crate `crates/backtest-scheduler/`
4. ✅ Implement `BacktestScheduler` struct
5. ✅ Implement `SchedulerConfig` struct

#### Token Selector Crate (Priority: HIGH)
6. ✅ Create new crate `crates/token-selector/`
7. ✅ Implement `TokenSelector` struct
8. ✅ Implement `SelectionCriteria` struct

#### Bot Orchestrator Extension (Priority: MEDIUM)
9. ✅ Add `sync_bots_with_approved_tokens` method to `BotRegistry`

#### CLI Integration (Priority: MEDIUM)
10. ✅ Add `ScheduledBacktest` CLI command
11. ✅ Add `TokenSelection` CLI command (manual trigger)

#### Configuration File Updates (Priority: LOW)
12. ✅ Add scheduler and token_selection sections to Config.toml

#### Integration & Orchestration (Priority: CRITICAL)
13. ✅ Wire scheduler → database → selector → bot orchestrator

### MUST NOT DO

1. ❌ DO NOT change existing `PerformanceMetrics` struct in `crates/core/src/engine.rs` (remove fields)
2. ❌ DO NOT use f64 for financial calculations in database schema
3. ❌ DO NOT run backtests synchronously (blocking)
4. ❌ DO NOT skip database batching for backtest results
5. ❌ DO NOT hardcode strategy parameters in scheduler
6. ❌ DO NOT create new WebSocket connections per backtest
7. ❌ DO NOT modify existing database tables (ohlcv, trades, fills)
8. ❌ DO NOT implement walk-forward validation in Phase 1 (MVP)

---

## Atomic Tasks

### Phase 0: Database Schema

#### Task 0.1: Create backtest_results hypertable
**File**: `/home/andrew/Projects/deep-algo/scripts/setup_timescale.sql`
**Location**: Append after line 61
**Action**: Add SQL to create `backtest_results` table with hypertable and indexes
**Schema**:
```sql
-- Create backtest_results hypertable
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

-- Convert to hypertable (partitioned by time)
SELECT create_hypertable('backtest_results', 'timestamp', if_not_exists => TRUE);

-- Create indexes for common queries
CREATE INDEX IF NOT EXISTS idx_backtest_results_symbol_time ON backtest_results (symbol, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_results_strategy_time ON backtest_results (strategy_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_results_sharpe ON backtest_results (sharpe_ratio DESC);

-- Enable compression for old data
ALTER TABLE backtest_results SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol, strategy_name'
);

-- Compress data older than 7 days
SELECT add_compression_policy('backtest_results', INTERVAL '7 days');
```
**Verification**: `psql -U postgres -d algo_trade -f scripts/setup_timescale.sql && psql -U postgres -d algo_trade -c "\d backtest_results"`
**Estimated LOC**: 35

---

### Phase 1: Extend Database Client for Backtest Results

#### Task 1.1: Add serde derives to PerformanceMetrics
**File**: `/home/andrew/Projects/deep-algo/crates/backtest/src/metrics.rs`
**Location**: Line 3 (PerformanceMetrics struct)
**Action**: Add `#[derive(Serialize, Deserialize)]` to enable JSON serialization
**Change**:
```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
}
```
**Verification**: `cargo check -p algo-trade-backtest`
**Estimated LOC**: 3

#### Task 1.2: Add BacktestResultRecord struct to database module
**File**: `/home/andrew/Projects/deep-algo/crates/data/src/database.rs`
**Location**: Append after line 95 (after OhlcvRecord)
**Action**: Add `BacktestResultRecord` struct with sqlx::FromRow derive
**Code**:
```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BacktestResultRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub strategy_name: String,
    pub parameters: Option<serde_json::Value>,
    pub sharpe_ratio: f64,
    pub total_return: Decimal,
    pub max_drawdown: Decimal,
    pub num_trades: i32,
    pub win_rate: f64,
    pub initial_capital: Decimal,
    pub final_capital: Decimal,
    pub backtest_window_start: DateTime<Utc>,
    pub backtest_window_end: DateTime<Utc>,
}
```
**Verification**: `cargo check -p algo-trade-data`
**Estimated LOC**: 17

#### Task 1.3: Add insert_backtest_result method
**File**: `/home/andrew/Projects/deep-algo/crates/data/src/database.rs`
**Location**: Append after line 82 (after query_ohlcv method, inside impl DatabaseClient)
**Action**: Add single insert method with ON CONFLICT for idempotency
**Code**:
```rust
/// Inserts a single backtest result into the database.
///
/// # Errors
/// Returns an error if the database insert fails.
pub async fn insert_backtest_result(&self, result: BacktestResultRecord) -> Result<()> {
    sqlx::query(
        r"
        INSERT INTO backtest_results (
            timestamp, symbol, strategy_name, parameters,
            sharpe_ratio, total_return, max_drawdown, num_trades, win_rate,
            initial_capital, final_capital, backtest_window_start, backtest_window_end
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (timestamp, symbol, strategy_name) DO UPDATE SET
            parameters = EXCLUDED.parameters,
            sharpe_ratio = EXCLUDED.sharpe_ratio,
            total_return = EXCLUDED.total_return,
            max_drawdown = EXCLUDED.max_drawdown,
            num_trades = EXCLUDED.num_trades,
            win_rate = EXCLUDED.win_rate,
            initial_capital = EXCLUDED.initial_capital,
            final_capital = EXCLUDED.final_capital,
            backtest_window_start = EXCLUDED.backtest_window_start,
            backtest_window_end = EXCLUDED.backtest_window_end
        ",
    )
    .bind(result.timestamp)
    .bind(&result.symbol)
    .bind(&result.strategy_name)
    .bind(result.parameters)
    .bind(result.sharpe_ratio)
    .bind(result.total_return)
    .bind(result.max_drawdown)
    .bind(result.num_trades)
    .bind(result.win_rate)
    .bind(result.initial_capital)
    .bind(result.final_capital)
    .bind(result.backtest_window_start)
    .bind(result.backtest_window_end)
    .execute(&self.pool)
    .await?;

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-data`
**Estimated LOC**: 44

#### Task 1.4: Add insert_backtest_results_batch method
**File**: `/home/andrew/Projects/deep-algo/crates/data/src/database.rs`
**Location**: Append after insert_backtest_result method
**Action**: Add batch insert in single transaction (pattern from insert_ohlcv_batch)
**Code**:
```rust
/// Inserts a batch of backtest results into the database.
///
/// # Errors
/// Returns an error if the database transaction fails or any record insertion fails.
pub async fn insert_backtest_results_batch(&self, results: Vec<BacktestResultRecord>) -> Result<()> {
    let mut tx = self.pool.begin().await?;

    for result in results {
        sqlx::query(
            r"
            INSERT INTO backtest_results (
                timestamp, symbol, strategy_name, parameters,
                sharpe_ratio, total_return, max_drawdown, num_trades, win_rate,
                initial_capital, final_capital, backtest_window_start, backtest_window_end
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (timestamp, symbol, strategy_name) DO UPDATE SET
                parameters = EXCLUDED.parameters,
                sharpe_ratio = EXCLUDED.sharpe_ratio,
                total_return = EXCLUDED.total_return,
                max_drawdown = EXCLUDED.max_drawdown,
                num_trades = EXCLUDED.num_trades,
                win_rate = EXCLUDED.win_rate,
                initial_capital = EXCLUDED.initial_capital,
                final_capital = EXCLUDED.final_capital,
                backtest_window_start = EXCLUDED.backtest_window_start,
                backtest_window_end = EXCLUDED.backtest_window_end
            ",
        )
        .bind(result.timestamp)
        .bind(&result.symbol)
        .bind(&result.strategy_name)
        .bind(result.parameters)
        .bind(result.sharpe_ratio)
        .bind(result.total_return)
        .bind(result.max_drawdown)
        .bind(result.num_trades)
        .bind(result.win_rate)
        .bind(result.initial_capital)
        .bind(result.final_capital)
        .bind(result.backtest_window_start)
        .bind(result.backtest_window_end)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-data`
**Estimated LOC**: 46

#### Task 1.5: Add query_latest_backtest_results method
**File**: `/home/andrew/Projects/deep-algo/crates/data/src/database.rs`
**Location**: Append after insert_backtest_results_batch method
**Action**: Query most recent backtest result for each token
**Code**:
```rust
/// Queries the most recent backtest result for each symbol.
///
/// # Errors
/// Returns an error if the database query fails.
pub async fn query_latest_backtest_results(
    &self,
    strategy_name: &str,
) -> Result<std::collections::HashMap<String, BacktestResultRecord>> {
    use std::collections::HashMap;

    let records = sqlx::query_as::<_, BacktestResultRecord>(
        r"
        SELECT DISTINCT ON (symbol)
            timestamp, symbol, strategy_name, parameters,
            sharpe_ratio, total_return, max_drawdown, num_trades, win_rate,
            initial_capital, final_capital, backtest_window_start, backtest_window_end
        FROM backtest_results
        WHERE strategy_name = $1
        ORDER BY symbol, timestamp DESC
        ",
    )
    .bind(strategy_name)
    .fetch_all(&self.pool)
    .await?;

    let mut map = HashMap::new();
    for record in records {
        map.insert(record.symbol.clone(), record);
    }

    Ok(map)
}
```
**Verification**: `cargo check -p algo-trade-data`
**Estimated LOC**: 30

---

### Phase 2: Backtest Scheduler Crate

#### Task 2.1: Create backtest-scheduler crate structure
**File**: NEW `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/Cargo.toml`
**Action**: Create new crate directory and Cargo.toml
**Code**:
```toml
[package]
name = "algo-trade-backtest-scheduler"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio-cron-scheduler = "0.13"
algo-trade-core = { path = "../core" }
algo-trade-backtest = { path = "../backtest" }
algo-trade-data = { path = "../data" }
algo-trade-hyperliquid = { path = "../exchange-hyperliquid" }
algo-trade-strategy = { path = "../strategy" }

tokio = { version = "1", features = ["full"] }
anyhow = "1"
tracing = "0.1"
serde = { version = "1", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }
rust_decimal = "1"
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 20

#### Task 2.2: Create backtest-scheduler lib.rs
**File**: NEW `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/lib.rs`
**Action**: Create library entry point
**Code**:
```rust
pub mod config;
pub mod scheduler;

pub use config::SchedulerConfig;
pub use scheduler::BacktestScheduler;
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 5

#### Task 2.3: Create SchedulerConfig struct
**File**: NEW `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/config.rs`
**Action**: Define configuration struct for scheduler
**Code**:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub enabled: bool,
    pub cron_expression: String,
    pub token_universe: Vec<String>,
    pub backtest_window_days: u32,
    pub interval: String,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cron_expression: "0 0 3 * * *".to_string(), // Daily at 3am UTC
            token_universe: vec![
                "BTC".to_string(),
                "ETH".to_string(),
                "SOL".to_string(),
                "AVAX".to_string(),
                "ARB".to_string(),
                "OP".to_string(),
                "MATIC".to_string(),
                "ATOM".to_string(),
                "NEAR".to_string(),
                "FTM".to_string(),
                "INJ".to_string(),
                "SEI".to_string(),
                "SUI".to_string(),
                "APT".to_string(),
                "TIA".to_string(),
                "DOGE".to_string(),
                "SHIB".to_string(),
                "WLD".to_string(),
                "PEPE".to_string(),
                "BONK".to_string(),
            ],
            backtest_window_days: 30,
            interval: "1m".to_string(),
        }
    }
}
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 47

#### Task 2.4: Create BacktestScheduler struct (part 1: struct definition and new)
**File**: NEW `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Action**: Define BacktestScheduler struct with constructor
**Code**:
```rust
use crate::config::SchedulerConfig;
use algo_trade_data::DatabaseClient;
use algo_trade_hyperliquid::HyperliquidClient;
use anyhow::Result;
use std::sync::Arc;
use tokio_cron_scheduler::JobScheduler;

pub struct BacktestScheduler {
    scheduler: JobScheduler,
    db_client: Arc<DatabaseClient>,
    hyperliquid_client: Arc<HyperliquidClient>,
    config: SchedulerConfig,
}

impl BacktestScheduler {
    /// Creates a new backtest scheduler.
    ///
    /// # Errors
    /// Returns an error if the scheduler cannot be created.
    pub async fn new(
        config: SchedulerConfig,
        db_client: Arc<DatabaseClient>,
        hyperliquid_client: Arc<HyperliquidClient>,
    ) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;

        Ok(Self {
            scheduler,
            db_client,
            hyperliquid_client,
            config,
        })
    }
}
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 35

#### Task 2.5: Add start method to BacktestScheduler
**File**: `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Location**: Append inside impl BacktestScheduler block
**Action**: Implement start method that registers cron job
**Code**:
```rust
/// Starts the backtest scheduler.
///
/// # Errors
/// Returns an error if the scheduler cannot be started.
pub async fn start(&self) -> Result<()> {
    use tokio_cron_scheduler::Job;

    let db_client = self.db_client.clone();
    let hyperliquid_client = self.hyperliquid_client.clone();
    let config = self.config.clone();

    let job = Job::new_async(config.cron_expression.as_str(), move |_uuid, _lock| {
        let db_client = db_client.clone();
        let hyperliquid_client = hyperliquid_client.clone();
        let config = config.clone();

        Box::pin(async move {
            tracing::info!("Running scheduled backtest sweep");
            if let Err(e) = run_backtest_sweep(db_client, hyperliquid_client, config).await {
                tracing::error!("Backtest sweep failed: {}", e);
            }
        })
    })?;

    self.scheduler.add(job).await?;
    self.scheduler.start().await?;

    tracing::info!("Backtest scheduler started with cron: {}", self.config.cron_expression);

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 30

#### Task 2.6: Implement run_backtest_sweep function (part 1: signature and data fetching)
**File**: `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Location**: Append after impl BacktestScheduler block (module-level function)
**Action**: Implement backtest sweep logic - fetching data
**Code**:
```rust
async fn run_backtest_sweep(
    db_client: Arc<DatabaseClient>,
    hyperliquid_client: Arc<HyperliquidClient>,
    config: SchedulerConfig,
) -> Result<()> {
    use algo_trade_data::{BacktestResultRecord, CsvStorage};
    use chrono::{Duration, Utc};

    let end = Utc::now();
    let start = end - Duration::days(i64::from(config.backtest_window_days));

    tracing::info!(
        "Fetching historical data for {} tokens ({} to {})",
        config.token_universe.len(),
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d")
    );

    let mut results = Vec::new();

    for symbol in &config.token_universe {
        // Fetch and cache data
        let cache_path = format!(
            "cache/{}_{}_{}_{}.csv",
            symbol,
            config.interval,
            start.format("%Y%m%d"),
            end.format("%Y%m%d")
        );

        // Check if cache exists and is recent (< 24 hours old)
        let use_cache = std::path::Path::new(&cache_path).exists();

        let records = if use_cache {
            tracing::info!("Using cached data for {}: {}", symbol, cache_path);
            // Will load from cache in next step
            vec![]
        } else {
            tracing::info!("Fetching data for {} from Hyperliquid", symbol);
            match hyperliquid_client
                .fetch_candles(symbol, &config.interval, start, end)
                .await
            {
                Ok(candles) => {
                    if candles.is_empty() {
                        tracing::warn!("No candles returned for {}, skipping", symbol);
                        continue;
                    }

                    // Create cache directory
                    std::fs::create_dir_all("cache")?;

                    // Write to cache
                    CsvStorage::write_ohlcv(&cache_path, &candles)?;
                    tracing::info!("Cached {} candles for {}", candles.len(), symbol);
                    candles
                }
                Err(e) => {
                    tracing::error!("Failed to fetch data for {}: {}", symbol, e);
                    continue;
                }
            }
        };
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 62

#### Task 2.7: Implement run_backtest_sweep function (part 2: backtest execution)
**File**: `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Location**: Continue in run_backtest_sweep function
**Action**: Run backtest and collect metrics
**Code**:
```rust
        // Run backtest
        match run_single_backtest(symbol, &cache_path, start, end).await {
            Ok(metrics) => {
                let result = BacktestResultRecord {
                    timestamp: Utc::now(),
                    symbol: symbol.clone(),
                    strategy_name: "quad_ma".to_string(),
                    parameters: None, // MVP: use default parameters
                    sharpe_ratio: metrics.sharpe_ratio,
                    total_return: metrics.total_return,
                    max_drawdown: metrics.max_drawdown,
                    num_trades: metrics.num_trades as i32,
                    win_rate: metrics.win_rate,
                    initial_capital: rust_decimal::Decimal::new(10000, 0), // Default $10k
                    final_capital: rust_decimal::Decimal::new(10000, 0)
                        + (rust_decimal::Decimal::new(10000, 0) * metrics.total_return),
                    backtest_window_start: start,
                    backtest_window_end: end,
                };

                results.push(result);

                tracing::info!(
                    "Backtest complete for {}: Sharpe={:.2}, Return={:.2}%, Trades={}",
                    symbol,
                    metrics.sharpe_ratio,
                    metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0) * 100.0,
                    metrics.num_trades
                );
            }
            Err(e) => {
                tracing::error!("Backtest failed for {}: {}", symbol, e);
            }
        }
    }

    // Batch insert results
    if !results.is_empty() {
        tracing::info!("Inserting {} backtest results to database", results.len());
        db_client.insert_backtest_results_batch(results).await?;
    }

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 46

#### Task 2.8: Implement run_single_backtest helper function
**File**: `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Location**: Append after run_backtest_sweep function
**Action**: Run single backtest using existing TradingSystem
**Code**:
```rust
async fn run_single_backtest(
    symbol: &str,
    data_path: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> Result<algo_trade_backtest::PerformanceMetrics> {
    use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
    use algo_trade_core::TradingSystem;
    use algo_trade_strategy::{QuadMaStrategy, SimpleRiskManager};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Load historical data
    let data_provider = HistoricalDataProvider::from_csv(data_path)?;

    // Create simulated execution handler
    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0); // 0.1% commission, 5 bps slippage

    // Create quad MA strategy with default parameters
    let strategy = QuadMaStrategy::new(symbol.to_string());
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> =
        vec![Arc::new(Mutex::new(strategy))];

    // Create risk manager
    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(0.05, 0.20));

    // Create and run trading system
    let mut system = TradingSystem::new(
        data_provider,
        execution_handler,
        strategies,
        risk_manager,
    );

    let metrics = system.run().await?;

    Ok(metrics)
}
```
**Verification**: `cargo check -p algo-trade-backtest-scheduler`
**Estimated LOC**: 38

---

### Phase 3: Token Selector Crate

#### Task 3.1: Create token-selector crate structure
**File**: NEW `/home/andrew/Projects/deep-algo/crates/token-selector/Cargo.toml`
**Action**: Create new crate directory and Cargo.toml
**Code**:
```toml
[package]
name = "algo-trade-token-selector"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-data = { path = "../data" }

rust_decimal = "1"
anyhow = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
serde = { version = "1", features = ["derive"] }
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 15

#### Task 3.2: Create token-selector lib.rs
**File**: NEW `/home/andrew/Projects/deep-algo/crates/token-selector/src/lib.rs`
**Action**: Create library entry point
**Code**:
```rust
pub mod criteria;
pub mod selector;

pub use criteria::SelectionCriteria;
pub use selector::TokenSelector;
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 5

#### Task 3.3: Create SelectionCriteria struct
**File**: NEW `/home/andrew/Projects/deep-algo/crates/token-selector/src/criteria.rs`
**Action**: Define selection criteria configuration
**Code**:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionCriteria {
    pub min_sharpe_ratio: f64,
    pub min_win_rate: f64,
    pub max_drawdown: f64,
    pub min_num_trades: usize,
    pub top_n: usize,
}

impl Default for SelectionCriteria {
    fn default() -> Self {
        Self {
            min_sharpe_ratio: 1.0,
            min_win_rate: 0.5,
            max_drawdown: 0.2,
            min_num_trades: 10,
            top_n: 10,
        }
    }
}
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 25

#### Task 3.4: Create TokenSelector struct
**File**: NEW `/home/andrew/Projects/deep-algo/crates/token-selector/src/selector.rs`
**Action**: Define TokenSelector with constructor
**Code**:
```rust
use crate::criteria::SelectionCriteria;
use algo_trade_data::{BacktestResultRecord, DatabaseClient};
use anyhow::Result;
use std::sync::Arc;

pub struct TokenSelector {
    db_client: Arc<DatabaseClient>,
    criteria: SelectionCriteria,
}

impl TokenSelector {
    /// Creates a new token selector.
    #[must_use]
    pub fn new(db_client: Arc<DatabaseClient>, criteria: SelectionCriteria) -> Self {
        Self {
            db_client,
            criteria,
        }
    }
}
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 20

#### Task 3.5: Implement select_approved_tokens method
**File**: `/home/andrew/Projects/deep-algo/crates/token-selector/src/selector.rs`
**Location**: Append inside impl TokenSelector block
**Action**: Query database and apply filtering criteria
**Code**:
```rust
/// Selects approved tokens based on latest backtest results.
///
/// # Errors
/// Returns an error if the database query fails.
pub async fn select_approved_tokens(&self) -> Result<Vec<String>> {
    // Query latest backtest results
    let results = self
        .db_client
        .query_latest_backtest_results("quad_ma")
        .await?;

    if results.is_empty() {
        tracing::warn!("No backtest results found in database");
        return Ok(Vec::new());
    }

    // Filter and score tokens
    let mut scored_tokens: Vec<_> = results
        .into_iter()
        .filter_map(|(symbol, record)| {
            // Apply filtering criteria
            if !self.passes_criteria(&record) {
                tracing::debug!(
                    "Token {} failed criteria: Sharpe={:.2}, WinRate={:.2}, Drawdown={:.2}, Trades={}",
                    symbol,
                    record.sharpe_ratio,
                    record.win_rate,
                    record.max_drawdown.to_string().parse::<f64>().unwrap_or(0.0),
                    record.num_trades
                );
                return None;
            }

            // Calculate composite score
            let score = self.calculate_composite_score(&record);

            Some((symbol, score, record))
        })
        .collect();

    if scored_tokens.is_empty() {
        tracing::warn!("No tokens passed selection criteria, using fallback");
        // Fallback: select top 3 by Sharpe even if they don't meet criteria
        let mut all_results: Vec<_> = self
            .db_client
            .query_latest_backtest_results("quad_ma")
            .await?
            .into_iter()
            .collect();

        all_results.sort_by(|a, b| {
            b.1.sharpe_ratio
                .partial_cmp(&a.1.sharpe_ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        return Ok(all_results
            .into_iter()
            .take(3)
            .map(|(symbol, _)| symbol)
            .collect());
    }

    // Sort by composite score (descending)
    scored_tokens.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Take top N
    let approved: Vec<String> = scored_tokens
        .into_iter()
        .take(self.criteria.top_n)
        .map(|(symbol, score, _)| {
            tracing::info!("Approved token: {} (score: {:.3})", symbol, score);
            symbol
        })
        .collect();

    tracing::info!("Selected {} approved tokens", approved.len());

    Ok(approved)
}
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 77

#### Task 3.6: Implement helper methods for TokenSelector
**File**: `/home/andrew/Projects/deep-algo/crates/token-selector/src/selector.rs`
**Location**: Append inside impl TokenSelector block
**Action**: Add passes_criteria and calculate_composite_score methods
**Code**:
```rust
fn passes_criteria(&self, record: &BacktestResultRecord) -> bool {
    let drawdown_f64 = record
        .max_drawdown
        .to_string()
        .parse::<f64>()
        .unwrap_or(1.0);

    record.sharpe_ratio >= self.criteria.min_sharpe_ratio
        && record.win_rate >= self.criteria.min_win_rate
        && drawdown_f64 <= self.criteria.max_drawdown
        && (record.num_trades as usize) >= self.criteria.min_num_trades
}

fn calculate_composite_score(&self, record: &BacktestResultRecord) -> f64 {
    let sharpe_weight = 0.4;
    let win_rate_weight = 0.3;
    let drawdown_weight = 0.3;

    let drawdown_f64 = record
        .max_drawdown
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);

    let sharpe_component = record.sharpe_ratio * sharpe_weight;
    let win_rate_component = record.win_rate * win_rate_weight;
    let drawdown_component = (1.0 - drawdown_f64) * drawdown_weight;

    sharpe_component + win_rate_component + drawdown_component
}
```
**Verification**: `cargo check -p algo-trade-token-selector`
**Estimated LOC**: 30

---

### Phase 4: Bot Orchestrator Extension

#### Task 4.1: Add sync_bots_with_approved_tokens method to BotRegistry
**File**: `/home/andrew/Projects/deep-algo/crates/bot-orchestrator/src/registry.rs`
**Location**: Append after shutdown_all method (after line 94)
**Action**: Add method to synchronize running bots with approved token list
**Code**:
```rust
/// Synchronizes running bots with approved token list.
/// Starts bots for new tokens, stops bots for removed tokens.
///
/// # Errors
/// Returns an error if bot spawning or removal fails.
pub async fn sync_bots_with_approved_tokens(
    &self,
    approved_tokens: Vec<String>,
    strategy_name: &str,
) -> Result<()> {
    use std::collections::HashSet;

    let running_bots = self.list_bots().await;

    // Extract symbols from bot IDs (format: "{symbol}_{strategy}")
    let running_symbols: HashSet<String> = running_bots
        .iter()
        .filter_map(|id| {
            let parts: Vec<&str> = id.split('_').collect();
            if parts.len() >= 2 {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
        .collect();

    // Start bots for new tokens
    for token in &approved_tokens {
        if !running_symbols.contains(token) {
            let bot_config = BotConfig {
                bot_id: format!("{}_{}", token, strategy_name),
                symbol: token.clone(),
                strategy: strategy_name.to_string(),
                enabled: true,
            };

            match self.spawn_bot(bot_config).await {
                Ok(_) => {
                    tracing::info!("✅ Started bot for approved token: {}", token);
                }
                Err(e) => {
                    tracing::error!("Failed to start bot for {}: {}", token, e);
                }
            }
        }
    }

    // Stop bots for removed tokens
    let approved_set: HashSet<String> = approved_tokens.into_iter().collect();
    for bot_id in running_bots {
        let parts: Vec<&str> = bot_id.split('_').collect();
        if let Some(symbol) = parts.first() {
            if !approved_set.contains(&symbol.to_string()) {
                match self.remove_bot(&bot_id).await {
                    Ok(_) => {
                        tracing::info!("🛑 Stopped bot for removed token: {}", symbol);
                    }
                    Err(e) => {
                        tracing::error!("Failed to stop bot {}: {}", bot_id, e);
                    }
                }
            }
        }
    }

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 66

---

### Phase 5: Configuration Extensions

#### Task 5.1: Add SchedulerConfig to core config
**File**: `/home/andrew/Projects/deep-algo/crates/core/src/config.rs`
**Location**: Insert after line 8 in AppConfig struct
**Action**: Add scheduler field to AppConfig
**Change**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub hyperliquid: HyperliquidConfig,
    pub scheduler: SchedulerConfig,
    pub token_selection: SelectionCriteria,
}
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 2

#### Task 5.2: Import and re-export scheduler types in core config
**File**: `/home/andrew/Projects/deep-algo/crates/core/src/config.rs`
**Location**: Add at top of file (after existing use statements)
**Action**: Import SchedulerConfig and SelectionCriteria
**Code**:
```rust
// Re-export from other crates for convenience
pub use algo_trade_backtest_scheduler::SchedulerConfig;
pub use algo_trade_token_selector::SelectionCriteria;
```
**Note**: This requires updating `crates/core/Cargo.toml` dependencies first
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 3

#### Task 5.3: Update core/Cargo.toml dependencies
**File**: `/home/andrew/Projects/deep-algo/crates/core/Cargo.toml`
**Location**: Add to [dependencies] section
**Action**: Add backtest-scheduler and token-selector as dependencies
**Code**:
```toml
algo-trade-backtest-scheduler = { path = "../backtest-scheduler" }
algo-trade-token-selector = { path = "../token-selector" }
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 2

#### Task 5.4: Update AppConfig default implementation
**File**: `/home/andrew/Projects/deep-algo/crates/core/src/config.rs`
**Location**: Modify Default impl for AppConfig (around line 28-44)
**Action**: Add scheduler and token_selection fields to default
**Change**:
```rust
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "postgresql://localhost/algo_trade".to_string(),
                max_connections: 10,
            },
            hyperliquid: HyperliquidConfig {
                api_url: "https://api.hyperliquid.xyz".to_string(),
                ws_url: "wss://api.hyperliquid.xyz/ws".to_string(),
            },
            scheduler: SchedulerConfig::default(),
            token_selection: SelectionCriteria::default(),
        }
    }
}
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 2

#### Task 5.5: Update Config.toml with new sections
**File**: `/home/andrew/Projects/deep-algo/config/Config.toml`
**Location**: Append after line 12
**Action**: Add [scheduler] and [token_selection] sections
**Code**:
```toml

[scheduler]
enabled = true
cron_expression = "0 0 3 * * *"  # Daily at 3am UTC
backtest_window_days = 30
interval = "1m"
token_universe = [
    "BTC", "ETH", "SOL", "AVAX", "ARB", "OP", "MATIC", "ATOM", "NEAR", "FTM",
    "INJ", "SEI", "SUI", "APT", "TIA", "DOGE", "SHIB", "WLD", "PEPE", "BONK"
]

[token_selection]
min_sharpe_ratio = 1.0
min_win_rate = 0.5
max_drawdown = 0.2
min_num_trades = 10
top_n = 10
```
**Verification**: `cargo run -p algo-trade-cli -- run --config config/Config.toml` (dry run test)
**Estimated LOC**: 18

---

### Phase 6: CLI Integration

#### Task 6.1: Add ScheduledBacktest command to CLI
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Insert after line 65 in Commands enum
**Action**: Add ScheduledBacktest variant
**Code**:
```rust
/// Start the backtest scheduler daemon
ScheduledBacktest {
    /// Config file path
    #[arg(short, long, default_value = "config/Config.toml")]
    config: String,
},
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 6

#### Task 6.2: Add TokenSelection command to CLI
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Insert after ScheduledBacktest in Commands enum
**Action**: Add TokenSelection variant for manual trigger
**Code**:
```rust
/// Manually run token selection and display results
TokenSelection {
    /// Config file path
    #[arg(short, long, default_value = "config/Config.toml")]
    config: String,
},
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 6

#### Task 6.3: Add match arms for new commands
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Insert after line 96 in match statement
**Action**: Handle ScheduledBacktest and TokenSelection commands
**Code**:
```rust
Commands::ScheduledBacktest { config } => {
    run_scheduled_backtest(&config).await?;
}
Commands::TokenSelection { config } => {
    run_token_selection(&config).await?;
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 6

#### Task 6.4: Implement run_scheduled_backtest function
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Append at end of file (after run_tui_backtest)
**Action**: Implement scheduler daemon
**Code**:
```rust
async fn run_scheduled_backtest(config_path: &str) -> anyhow::Result<()> {
    use algo_trade_backtest_scheduler::BacktestScheduler;
    use algo_trade_bot_orchestrator::BotRegistry;
    use algo_trade_data::DatabaseClient;
    use algo_trade_hyperliquid::HyperliquidClient;
    use algo_trade_token_selector::TokenSelector;
    use std::sync::Arc;

    tracing::info!("Starting backtest scheduler daemon with config: {}", config_path);

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    if !config.scheduler.enabled {
        anyhow::bail!("Scheduler is disabled in config. Set scheduler.enabled = true");
    }

    // Create database client
    let db_client = Arc::new(DatabaseClient::new(&config.database.url).await?);

    // Create Hyperliquid client
    let hyperliquid_client = Arc::new(HyperliquidClient::new(config.hyperliquid.api_url.clone()));

    // Create bot registry
    let registry = Arc::new(BotRegistry::new());

    // Create and start scheduler
    let scheduler = BacktestScheduler::new(
        config.scheduler.clone(),
        db_client.clone(),
        hyperliquid_client.clone(),
    )
    .await?;

    scheduler.start().await?;

    tracing::info!(
        "📅 Scheduler active with cron: {}",
        config.scheduler.cron_expression
    );
    tracing::info!("Token universe: {} tokens", config.scheduler.token_universe.len());
    tracing::info!("Selection criteria: Sharpe>{}, WinRate>{}, MaxDD<{}",
        config.token_selection.min_sharpe_ratio,
        config.token_selection.min_win_rate,
        config.token_selection.max_drawdown
    );

    // Wait for Ctrl+C
    tracing::info!("Press Ctrl+C to stop scheduler");
    tokio::signal::ctrl_c().await?;

    tracing::info!("Shutting down scheduler...");
    registry.shutdown_all().await?;

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 54

#### Task 6.5: Implement run_token_selection function
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Append after run_scheduled_backtest function
**Action**: Implement manual token selection trigger
**Code**:
```rust
async fn run_token_selection(config_path: &str) -> anyhow::Result<()> {
    use algo_trade_data::DatabaseClient;
    use algo_trade_token_selector::TokenSelector;
    use std::sync::Arc;

    tracing::info!("Running manual token selection with config: {}", config_path);

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    // Create database client
    let db_client = Arc::new(DatabaseClient::new(&config.database.url).await?);

    // Create token selector
    let selector = TokenSelector::new(db_client, config.token_selection.clone());

    // Run selection
    let approved = selector.select_approved_tokens().await?;

    // Display results
    println!("\n╔══════════════════════════════════════╗");
    println!("║   Token Selection Results           ║");
    println!("╚══════════════════════════════════════╝\n");

    if approved.is_empty() {
        println!("⚠️  No tokens passed selection criteria");
        println!("\nCriteria:");
        println!("  • Min Sharpe Ratio: {}", config.token_selection.min_sharpe_ratio);
        println!("  • Min Win Rate: {}", config.token_selection.min_win_rate);
        println!("  • Max Drawdown: {}", config.token_selection.max_drawdown);
        println!("  • Min Trades: {}", config.token_selection.min_num_trades);
    } else {
        println!("✅ Approved Tokens ({}): {}\n", approved.len(), approved.join(", "));

        println!("These tokens will be traded by live bots.");
        println!("\nTo apply changes, restart the trading system:");
        println!("  cargo run -p algo-trade-cli -- run --config {}", config_path);
    }

    Ok(())
}
```
**Verification**: `cargo check -p algo-trade-cli && cargo run -p algo-trade-cli -- token-selection --help`
**Estimated LOC**: 45

---

### Phase 7: Integration Wiring

#### Task 7.1: Add post-backtest token selection to scheduler
**File**: `/home/andrew/Projects/deep-algo/crates/backtest-scheduler/src/scheduler.rs`
**Location**: Modify run_backtest_sweep function - add after batch insert (near end)
**Action**: Trigger token selection after backtest sweep completes
**Code**: This task requires passing a callback or adding token selector dependency. For simplicity in MVP, this will be handled at CLI level in Task 7.2.
**Note**: Skip this task - integration happens in CLI layer
**Estimated LOC**: 0

#### Task 7.2: Wire scheduler → selector → orchestrator in CLI
**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Modify run_scheduled_backtest function - enhance to include token selection loop
**Action**: Add periodic token selection after scheduler runs
**Change**: Replace the scheduler start section with:
```rust
scheduler.start().await?;

tracing::info!(
    "📅 Scheduler active with cron: {}",
    config.scheduler.cron_expression
);
tracing::info!("Token universe: {} tokens", config.scheduler.token_universe.len());
tracing::info!("Selection criteria: Sharpe>{}, WinRate>{}, MaxDD<{}",
    config.token_selection.min_sharpe_ratio,
    config.token_selection.min_win_rate,
    config.token_selection.max_drawdown
);

// Spawn token selection loop (runs every hour after scheduler)
let db_client_clone = db_client.clone();
let registry_clone = registry.clone();
let selection_config = config.token_selection.clone();

tokio::spawn(async move {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3700)); // Run 100s after scheduler (at 3:01:40am)
    interval.tick().await; // Skip first tick

    loop {
        interval.tick().await;

        tracing::info!("Running token selection...");
        let selector = TokenSelector::new(db_client_clone.clone(), selection_config.clone());

        match selector.select_approved_tokens().await {
            Ok(approved) => {
                tracing::info!("Token selection complete: {} approved", approved.len());

                if let Err(e) = registry_clone
                    .sync_bots_with_approved_tokens(approved, "quad_ma")
                    .await
                {
                    tracing::error!("Failed to sync bots: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Token selection failed: {}", e);
            }
        }
    }
});

// Wait for Ctrl+C
tracing::info!("Press Ctrl+C to stop scheduler");
tokio::signal::ctrl_c().await?;
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 40

---

## Verification Checklist

Run after all tasks complete (in order):

### Database Verification
- [ ] `psql -U postgres -d algo_trade -f scripts/setup_timescale.sql`
- [ ] `psql -U postgres -d algo_trade -c "\d backtest_results"`
- [ ] Verify hypertable: `psql -U postgres -d algo_trade -c "SELECT * FROM timescaledb_information.hypertables WHERE hypertable_name = 'backtest_results';"`

### Build Verification
- [ ] `cargo build --workspace`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test -p algo-trade-data`
- [ ] `cargo test -p algo-trade-token-selector`
- [ ] `cargo test -p algo-trade-bot-orchestrator`

### Functional Verification
- [ ] `cargo run -p algo-trade-cli -- scheduled-backtest --help`
- [ ] `cargo run -p algo-trade-cli -- token-selection --help`
- [ ] Run manual token selection: `cargo run -p algo-trade-cli -- token-selection` (should show no results if DB empty)
- [ ] Start scheduler daemon: `cargo run -p algo-trade-cli -- scheduled-backtest` (Ctrl+C after 10 seconds)

### Integration Test (Optional - requires historical data)
- [ ] Fetch sample data for 3 tokens (BTC, ETH, SOL)
- [ ] Run scheduler for 1 cycle
- [ ] Verify backtest_results table has 3 rows
- [ ] Run token-selection command
- [ ] Verify approved tokens displayed

### Karen Review (MANDATORY)
- [ ] Run Karen agent review on all modified packages
- [ ] Ensure zero clippy warnings (default + pedantic + nursery)
- [ ] Ensure all public APIs documented
- [ ] Ensure all financial values use Decimal

---

## Dependencies Between Phases

```
Phase 0 (Database)
    ↓
Phase 1 (Database Client) ← DEPENDS ON Phase 0
    ↓
Phase 2 (Scheduler) ← DEPENDS ON Phase 1
    ↓
Phase 3 (Token Selector) ← DEPENDS ON Phase 1
    ↓
Phase 4 (Bot Orchestrator) ← DEPENDS ON Phase 3
    ↓
Phase 5 (Config) ← DEPENDS ON Phase 2, 3
    ↓
Phase 6 (CLI) ← DEPENDS ON Phase 2, 3, 4, 5
    ↓
Phase 7 (Integration) ← DEPENDS ON ALL
```

**Execution Order**: Phase 0 → Phase 1 → Phase 2 & 3 (parallel) → Phase 4 → Phase 5 → Phase 6 → Phase 7

---

## Task Summary

- **Total Tasks**: 31 atomic tasks
- **Estimated Total LOC**: ~750 lines
- **New Crates**: 2 (backtest-scheduler, token-selector)
- **Modified Crates**: 5 (core, data, backtest, bot-orchestrator, cli)
- **New Files**: 10
- **Modified Files**: 7

### Tasks Exceeding 50 LOC (may need breaking down):

1. **Task 2.6**: run_backtest_sweep part 1 (62 LOC) - Consider acceptable, focused on data fetching
2. **Task 3.5**: select_approved_tokens (77 LOC) - Consider acceptable, single logical function
3. **Task 4.1**: sync_bots_with_approved_tokens (66 LOC) - Consider acceptable, single logical function
4. **Task 6.4**: run_scheduled_backtest (54 LOC) - Consider acceptable, main entry point

**Verdict**: All tasks are cohesive single units of work. No further breakdown needed.

---

## Notes

- All financial values use `rust_decimal::Decimal` (prices, PnL, returns)
- Statistical ratios (Sharpe, win rate) use `f64` (acceptable per CLAUDE.md)
- Database uses `DECIMAL(20, 8)` for financial columns, `DOUBLE PRECISION` for ratios
- Batch inserts follow pattern from existing `insert_ohlcv_batch` (13ms for 100 records)
- Async/await with Tokio throughout
- Error handling with `anyhow::Result` and `.context()`
- Logging with `tracing` crate
- Configuration hot-reload ready (watch channels can be added in Phase 2)

---

## Post-Implementation: Phase 2 Features (Future)

After Phase 1 MVP is stable and Karen-approved, consider:

1. **Walk-Forward Validation**: 30-day train, 7-day test windows
2. **Parameter Optimization**: Sweep MA periods, select best per token
3. **Regime Detection**: High/low volatility classification
4. **Live Performance Tracking**: Backtest-live divergence alerts
5. **Auto-Stop Bots**: Stop on 30% drawdown or negative Sharpe after 100 trades
6. **Multi-Strategy Selection**: Different strategies for different tokens
7. **Web UI**: Grafana dashboard for backtest results visualization
8. **Advanced Metrics**: Sortino, Calmar, Profit Factor

---

**End of Playbook**
