//! CLI command for automated single-leg directional trading.
//!
//! This command runs the directional trading system, connecting the
//! `DirectionalRunner` (signal detection) to `DirectionalExecutor` (order execution).
//!
//! # Example
//!
//! ```bash
//! # Paper trading with all coins for 1 hour
//! algo-trade directional-auto --mode paper --duration 1h
//!
//! # Live trading with $10 fixed bets on BTC and ETH
//! algo-trade directional-auto --mode live --coins btc,eth --bet-size 10
//! ```

use algo_trade_polymarket::arbitrage::data_service::{DataService, DataServiceConfig};
use algo_trade_polymarket::arbitrage::directional_detector::DirectionalConfig;
use algo_trade_polymarket::arbitrage::directional_executor::{
    DirectionalExecutor, DirectionalExecutorConfig,
};
use algo_trade_polymarket::arbitrage::directional_runner::{
    DirectionalRunner, DirectionalRunnerConfig,
};
use algo_trade_polymarket::arbitrage::{
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PolymarketExecutor,
};
use algo_trade_polymarket::arbitrage::reference_tracker::ReferenceTrackerConfig;
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

use super::collect_signals::parse_duration;

/// Trading execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    #[default]
    Paper,
    Live,
}

impl FromStr for ExecutionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "paper" => Ok(ExecutionMode::Paper),
            "live" => Ok(ExecutionMode::Live),
            _ => Err(format!(
                "Invalid mode '{}'. Valid options: paper, live",
                s
            )),
        }
    }
}

/// Arguments for the directional-auto command.
#[derive(Args, Debug)]
pub struct DirectionalAutoArgs {
    /// Execution mode: paper (default) or live.
    #[arg(long, default_value = "paper")]
    pub mode: String,

    /// Duration to run (e.g., "30m", "1h", "4h", "12h").
    #[arg(short, long, default_value = "1h")]
    pub duration: String,

    /// Coins to monitor (comma-separated, e.g., "btc,eth,sol,xrp").
    #[arg(long, default_value = "btc,eth,sol,xrp")]
    pub coins: String,

    /// Fixed bet size in USDC (overrides Kelly if set).
    #[arg(long)]
    pub bet_size: Option<f64>,

    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    #[arg(long, default_value = "0.25")]
    pub kelly_fraction: f64,

    /// Minimum edge required to trade (0.0 to 1.0). Default: 0.03 (3%).
    #[arg(long, default_value = "0.03")]
    pub min_edge: f64,

    /// Minimum spot-vs-reference delta to consider (e.g., 0.0005 = 0.05%).
    #[arg(long, default_value = "0.0005")]
    pub min_delta: f64,

    /// Maximum entry price (e.g., 0.55). Default: 0.55.
    #[arg(long, default_value = "0.55")]
    pub max_entry_price: f64,

    /// Entry window start: minutes before window close to START trading.
    #[arg(long, default_value = "10")]
    pub entry_start_mins: i64,

    /// Entry window end: minutes before window close to STOP trading.
    #[arg(long, default_value = "2")]
    pub entry_end_mins: i64,

    /// Maximum position per window in USDC. Default: 200.
    #[arg(long, default_value = "200")]
    pub max_position: f64,

    /// Maximum trades per 15-minute window. Default: 1.
    #[arg(long, default_value = "1")]
    pub max_trades_per_window: u32,

    /// Initial paper balance in USDC (paper mode only).
    #[arg(long, default_value = "1000")]
    pub paper_balance: f64,

    /// Stats/dashboard update interval in seconds.
    #[arg(long, default_value = "5")]
    pub stats_interval_secs: u64,

    /// Detection check interval in milliseconds.
    #[arg(long, default_value = "200")]
    pub check_interval_ms: u64,

    /// Show verbose output (logs instead of dashboard).
    #[arg(short, long)]
    pub verbose: bool,

    /// Persist trades and sessions to database.
    #[arg(long)]
    pub persist: bool,

    /// Session ID for grouping trades (auto-generated if not set).
    #[arg(long)]
    pub session_id: Option<String>,

    /// Enable Binance signal aggregation (order book, funding, liquidations, etc.).
    #[arg(long)]
    pub signals: bool,

    /// Persist raw Binance data (order book, funding, liquidations) to database.
    /// Requires --persist (DATABASE_URL).
    #[arg(long)]
    pub raw_persist: bool,
}

impl DirectionalAutoArgs {
    /// Parses the execution mode.
    pub fn execution_mode(&self) -> Result<ExecutionMode> {
        ExecutionMode::from_str(&self.mode).map_err(|e| anyhow::anyhow!(e))
    }

    /// Parses the duration string.
    pub fn parsed_duration(&self) -> Result<Duration> {
        parse_duration(&self.duration)
    }

    /// Parses the coins list.
    pub fn parsed_coins(&self) -> Vec<Coin> {
        self.coins
            .split(',')
            .filter_map(|s| Coin::from_slug(s.trim()))
            .collect()
    }

    /// Returns the fixed bet size as Decimal if set.
    pub fn fixed_bet_size(&self) -> Option<Decimal> {
        self.bet_size
            .map(|v| Decimal::from_str(&format!("{:.2}", v)).unwrap_or_default())
    }
}

/// Runs the directional-auto command.
pub async fn run(args: DirectionalAutoArgs) -> Result<()> {
    let mode = args.execution_mode()?;
    let duration = args.parsed_duration()?;
    let coins = args.parsed_coins();

    if coins.is_empty() {
        anyhow::bail!("No valid coins specified. Use --coins btc,eth,sol,xrp");
    }

    // Connect to database if persistence is enabled
    let db_pool = if args.persist {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required for --persist"))?;
        let pool = PgPool::connect(&database_url).await?;
        info!("Database connected for trade persistence");
        Some(pool)
    } else {
        None
    };

    let session_id = args.session_id.clone();
    let mode_str = args.mode.clone();

    if args.raw_persist && db_pool.is_none() {
        info!("--raw-persist requires --persist (DATABASE_URL); raw persistence disabled");
    }

    info!(
        mode = ?mode,
        coins = ?coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
        duration = ?duration,
        persist = args.persist,
        raw_persist = args.raw_persist,
        "Starting directional auto trading"
    );

    // Build detector config
    let detector_config = DirectionalConfig {
        min_delta_pct: args.min_delta,
        max_delta_pct: 0.03,
        max_entry_price: Decimal::from_str(&format!("{:.2}", args.max_entry_price))
            .unwrap_or(dec!(0.55)),
        min_edge: args.min_edge,
        entry_window_start_secs: args.entry_start_mins * 60,
        entry_window_end_secs: args.entry_end_mins * 60,
        signal_cooldown_ms: 30_000,
    };

    // Build runner config
    let runner_config = DirectionalRunnerConfig {
        coins: coins.clone(),
        detector_config,
        reference_config: ReferenceTrackerConfig::default(),
        check_interval_ms: args.check_interval_ms,
        signal_buffer_size: 100,
        gamma_rate_limit: 30,
        enable_signals: args.signals,
    };

    // Build executor config
    let executor_config = DirectionalExecutorConfig {
        kelly_fraction: args.kelly_fraction,
        fixed_bet_size: args.fixed_bet_size(),
        min_bet_size: dec!(5),
        max_bet_size: dec!(100),
        min_edge: args.min_edge,
        max_position_per_window: Decimal::from_str(&format!("{:.2}", args.max_position))
            .unwrap_or(dec!(200)),
        max_trades_per_window: args.max_trades_per_window,
        observe_mode: false,
        fee_rate: dec!(0.02),
        stats_interval_secs: args.stats_interval_secs,
        settlement_interval_secs: 30,
        buy_slippage: dec!(0.05),
        max_retries: 1,
        max_entry_price: Decimal::from_str(&format!("{:.2}", args.max_entry_price))
            .unwrap_or(dec!(0.85)),
    };

    // Create DataService when --signals or --raw-persist is enabled
    let data_stop = Arc::new(AtomicBool::new(false));
    let data_handle = if args.signals || args.raw_persist {
        let data_config = DataServiceConfig {
            coins: coins.clone(),
            enable_signals: args.signals,
            signal_compute_interval: Duration::from_secs(5),
            enable_signal_persistence: args.signals && db_pool.is_some(),
            signal_flush_interval: Duration::from_secs(15),
            enable_raw_persistence: args.raw_persist,
        };
        let data_service = DataService::new(data_config, db_pool.clone(), Arc::clone(&data_stop));
        let handle = data_service.handle();

        tokio::spawn(async move {
            data_service.run().await;
        });
        info!("DataService started for shared data collection");
        Some(handle)
    } else {
        None
    };

    // Create runner (with or without DataService)
    let (runner, signal_rx) = match data_handle {
        Some(handle) => DirectionalRunner::with_data_service(runner_config, handle),
        None => DirectionalRunner::new(runner_config),
    };
    let runner_stats = runner.stats();
    let runner_stop = runner.stop_handle();

    match mode {
        ExecutionMode::Paper => {
            let paper_config = PaperExecutorConfig {
                initial_balance: Decimal::from_str(&format!("{:.2}", args.paper_balance))
                    .unwrap_or(dec!(1000)),
                ..Default::default()
            };
            let paper_executor = PaperExecutor::new(paper_config);
            run_with_executor(
                paper_executor,
                executor_config,
                runner,
                runner_stats,
                runner_stop,
                signal_rx,
                duration,
                db_pool,
                session_id,
                mode_str,
                data_stop,
            )
            .await
        }
        ExecutionMode::Live => {
            let live_config = LiveExecutorConfig::mainnet();
            let live_executor = LiveExecutor::new(live_config).await?;
            run_with_executor(
                live_executor,
                executor_config,
                runner,
                runner_stats,
                runner_stop,
                signal_rx,
                duration,
                db_pool,
                session_id,
                mode_str,
                data_stop,
            )
            .await
        }
    }
}

/// Runs the system with a specific executor type.
async fn run_with_executor<E: PolymarketExecutor + 'static>(
    executor: E,
    config: DirectionalExecutorConfig,
    runner: DirectionalRunner,
    runner_stats: Arc<tokio::sync::RwLock<algo_trade_polymarket::arbitrage::directional_runner::DirectionalRunnerStats>>,
    runner_stop: Arc<AtomicBool>,
    signal_rx: tokio::sync::mpsc::Receiver<algo_trade_polymarket::arbitrage::directional_detector::DirectionalSignal>,
    duration: Duration,
    db_pool: Option<PgPool>,
    session_id: Option<String>,
    mode: String,
    data_stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut dir_executor = match db_pool {
        Some(pool) => DirectionalExecutor::with_persistence(executor, config, pool, session_id, mode),
        None => DirectionalExecutor::new(executor, config),
    };
    dir_executor.set_runner_stats(runner_stats);
    let executor_stop = dir_executor.stop_handle();

    // Spawn runner
    let runner_handle = tokio::spawn(async move {
        if let Err(e) = runner.run().await {
            error!("Runner error: {}", e);
        }
    });

    // Duration timer
    let stop_clone = executor_stop.clone();
    let runner_stop_clone = runner_stop.clone();
    let data_stop_clone = data_stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        info!("Duration elapsed, stopping...");
        stop_clone.store(true, Ordering::SeqCst);
        runner_stop_clone.store(true, Ordering::SeqCst);
        data_stop_clone.store(true, Ordering::SeqCst);
    });

    // Ctrl+C handler
    let stop_ctrlc = executor_stop.clone();
    let runner_stop_ctrlc = runner_stop.clone();
    let data_stop_ctrlc = data_stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Ctrl+C received, shutting down...");
            stop_ctrlc.store(true, Ordering::SeqCst);
            runner_stop_ctrlc.store(true, Ordering::SeqCst);
            data_stop_ctrlc.store(true, Ordering::SeqCst);
        }
    });

    // Run executor (blocks until done)
    dir_executor.run(signal_rx).await?;

    // Cleanup
    runner_stop.store(true, Ordering::SeqCst);
    data_stop.store(true, Ordering::SeqCst);
    runner_handle.abort();

    Ok(())
}
