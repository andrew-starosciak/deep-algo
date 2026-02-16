//! CLI command for CLOB first-move timing strategy.
//!
//! This command runs the CLOB timing system, connecting the
//! `ClobTimingRunner` (signal detection) to `DirectionalExecutor` (order execution).
//!
//! # Example
//!
//! ```bash
//! # Paper trading with BTC and ETH for 4 hours
//! algo-trade clob-timing --mode paper --duration 4h
//!
//! # Live trading with $10 fixed bets
//! algo-trade clob-timing --mode live --coins btc,eth --bet-size 10
//! ```

use algo_trade_polymarket::arbitrage::clob_timing_runner::{
    ClobTimingConfig, ClobTimingRunner,
};
use algo_trade_polymarket::arbitrage::directional_executor::{
    DirectionalExecutor, DirectionalExecutorConfig,
};
use algo_trade_polymarket::arbitrage::{
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PolymarketExecutor,
};
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
use super::directional_auto::ExecutionMode;

/// Arguments for the clob-timing command.
#[derive(Args, Debug)]
pub struct ClobTimingArgs {
    /// Execution mode: paper (default) or live.
    #[arg(long, default_value = "paper")]
    pub mode: String,

    /// Duration to run (e.g., "30m", "1h", "4h", "12h").
    #[arg(short, long, default_value = "4h")]
    pub duration: String,

    /// Coins to monitor (comma-separated, e.g., "btc,eth").
    #[arg(long, default_value = "btc,eth")]
    pub coins: String,

    /// Fixed bet size in USDC (overrides Kelly if set).
    #[arg(long)]
    pub bet_size: Option<f64>,

    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    #[arg(long, default_value = "0.25")]
    pub kelly_fraction: f64,

    /// Seconds into window to start checking (default: 150 = 2.5 min).
    #[arg(long, default_value = "150")]
    pub observation_delay: u64,

    /// Seconds into window to stop checking (default: 300 = 5 min).
    #[arg(long, default_value = "300")]
    pub observation_end: u64,

    /// Minimum CLOB displacement from 0.50 (default: 0.15).
    #[arg(long, default_value = "0.15")]
    pub min_displacement: f64,

    /// Maximum entry price (default: 0.85).
    #[arg(long, default_value = "0.85")]
    pub max_entry_price: f64,

    /// Initial paper balance in USDC (paper mode only).
    #[arg(long, default_value = "1000")]
    pub paper_balance: f64,

    /// Maximum position per window in USDC. Default: 10.
    #[arg(long, default_value = "10")]
    pub max_position: f64,

    /// Maximum trades per 15-minute window. Default: 1.
    #[arg(long, default_value = "1")]
    pub max_trades_per_window: u32,

    /// Minimum edge required to trade (0.0 to 1.0). Default: 0.05 (5%).
    #[arg(long, default_value = "0.05")]
    pub min_edge: f64,

    /// Persist trades and sessions to database.
    #[arg(long)]
    pub persist: bool,

    /// Show verbose output (logs instead of dashboard).
    #[arg(short, long)]
    pub verbose: bool,

    /// Session ID for grouping trades (auto-generated if not set).
    #[arg(long)]
    pub session_id: Option<String>,

    /// UTC hours to skip signal generation (comma-separated, e.g., "4,9,21,22,23").
    #[arg(long, default_value = "4,9,21,22,23")]
    pub exclude_hours: String,
}

impl ClobTimingArgs {
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

    /// Parses the excluded hours string into a Vec<u8>.
    pub fn parsed_exclude_hours(&self) -> Vec<u8> {
        self.exclude_hours
            .split(',')
            .filter_map(|s| s.trim().parse::<u8>().ok())
            .filter(|&h| h < 24)
            .collect()
    }
}

/// Runs the clob-timing command.
pub async fn run(args: ClobTimingArgs) -> Result<()> {
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

    let excluded_hours = args.parsed_exclude_hours();

    info!(
        mode = ?mode,
        coins = ?coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
        duration = ?duration,
        observation_delay = args.observation_delay,
        observation_end = args.observation_end,
        min_displacement = args.min_displacement,
        excluded_hours = ?excluded_hours,
        "Starting CLOB timing strategy"
    );

    // Build runner config
    let runner_config = ClobTimingConfig {
        coins: coins.clone(),
        observation_start_secs: args.observation_delay,
        observation_end_secs: args.observation_end,
        min_displacement: Decimal::from_str(&format!("{:.2}", args.min_displacement))
            .unwrap_or(dec!(0.15)),
        max_entry_price: Decimal::from_str(&format!("{:.2}", args.max_entry_price))
            .unwrap_or(dec!(0.65)),
        poll_interval_secs: 5,
        gamma_rate_limit: 30,
        signal_buffer_size: 100,
        excluded_hours_utc: excluded_hours,
    };

    // Build executor config
    let executor_config = DirectionalExecutorConfig {
        kelly_fraction: args.kelly_fraction,
        fixed_bet_size: args.fixed_bet_size(),
        min_bet_size: dec!(5),
        max_bet_size: dec!(10),
        min_edge: args.min_edge,
        max_position_per_window: Decimal::from_str(&format!("{:.2}", args.max_position))
            .unwrap_or(dec!(10)),
        max_trades_per_window: args.max_trades_per_window,
        observe_mode: false,
        fee_rate: dec!(0.02),
        stats_interval_secs: 5,
        settlement_interval_secs: 30,
        buy_slippage: dec!(0.05),
        max_retries: 1,
    };

    // Create runner
    let (runner, signal_rx) = ClobTimingRunner::new(runner_config);
    let runner_stop = runner.stop_handle();
    let runner_stats = runner.stats();

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
                runner_stop,
                runner_stats,
                signal_rx,
                duration,
                db_pool,
                session_id,
                mode_str,
            )
            .await
        }
        ExecutionMode::Live => {
            let live_config = LiveExecutorConfig::mainnet();
            let mut live_executor = LiveExecutor::new(live_config).await?;
            live_executor.authenticate().await?;
            run_with_executor(
                live_executor,
                executor_config,
                runner,
                runner_stop,
                runner_stats,
                signal_rx,
                duration,
                db_pool,
                session_id,
                mode_str,
            )
            .await
        }
    }
}

/// Runs the system with a specific executor type.
async fn run_with_executor<E: PolymarketExecutor + 'static>(
    executor: E,
    config: DirectionalExecutorConfig,
    runner: ClobTimingRunner,
    runner_stop: Arc<AtomicBool>,
    runner_stats: Arc<tokio::sync::RwLock<algo_trade_polymarket::arbitrage::clob_timing_runner::ClobTimingRunnerStats>>,
    signal_rx: tokio::sync::mpsc::Receiver<algo_trade_polymarket::arbitrage::directional_detector::DirectionalSignal>,
    duration: Duration,
    db_pool: Option<PgPool>,
    session_id: Option<String>,
    mode: String,
) -> Result<()> {
    let mut dir_executor = match db_pool {
        Some(pool) => DirectionalExecutor::with_persistence(executor, config, pool, session_id, mode),
        None => DirectionalExecutor::new(executor, config),
    };
    dir_executor.set_dashboard_title("CLOB First-Move Timing Strategy");
    dir_executor.set_clob_timing_stats(runner_stats);
    let executor_stop = dir_executor.stop_handle();

    // Spawn runner
    let runner_handle = tokio::spawn(async move {
        if let Err(e) = runner.run().await {
            error!("CLOB timing runner error: {}", e);
        }
    });

    // Duration timer
    let stop_clone = executor_stop.clone();
    let runner_stop_clone = runner_stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        info!("Duration elapsed, stopping...");
        stop_clone.store(true, Ordering::SeqCst);
        runner_stop_clone.store(true, Ordering::SeqCst);
    });

    // Ctrl+C handler
    let stop_ctrlc = executor_stop.clone();
    let runner_stop_ctrlc = runner_stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Ctrl+C received, shutting down...");
            stop_ctrlc.store(true, Ordering::SeqCst);
            runner_stop_ctrlc.store(true, Ordering::SeqCst);
        }
    });

    // Run executor (blocks until done)
    dir_executor.run(signal_rx).await?;

    // Cleanup
    runner_stop.store(true, Ordering::SeqCst);
    runner_handle.abort();

    Ok(())
}
