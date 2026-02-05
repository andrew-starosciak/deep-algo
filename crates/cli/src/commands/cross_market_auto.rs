//! CLI command for automated cross-market arbitrage trading.
//!
//! This command runs the full automated trading system, connecting the
//! `CrossMarketRunner` (signal detection) to `CrossMarketAutoExecutor` (order execution).
//!
//! # Features
//!
//! - Paper trading mode (default) for safe testing
//! - Live trading mode with real funds
//! - Configurable bet sizing (Kelly criterion or fixed)
//! - Pair filtering (BTC/ETH recommended for highest correlation)
//! - Periodic stats display
//! - Graceful shutdown handling
//!
//! # Example
//!
//! ```bash
//! # Paper trading BTC/ETH for 1 hour
//! algo-trade cross-market-auto --duration 1h
//!
//! # Live trading with fixed $25 bets per leg
//! algo-trade cross-market-auto --mode live --bet-size 25 --duration 4h
//! ```

use algo_trade_polymarket::arbitrage::{
    CrossMarketAutoExecutor, CrossMarketAutoExecutorConfig, CrossMarketAutoExecutorStats,
    CrossMarketCombination, CrossMarketRunner, CrossMarketRunnerConfig, CrossMarketRunnerStats,
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PolymarketExecutor,
};
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::collect_signals::parse_duration;

/// Trading execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    /// Paper trading (simulated, no real funds).
    #[default]
    Paper,
    /// Live trading (real funds on Polymarket).
    Live,
}

impl FromStr for ExecutionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "paper" => Ok(ExecutionMode::Paper),
            "live" => Ok(ExecutionMode::Live),
            _ => Err(format!("Invalid mode '{}'. Valid options: paper, live", s)),
        }
    }
}

/// Arguments for the cross-market-auto command.
#[derive(Args, Debug)]
pub struct CrossMarketAutoArgs {
    /// Execution mode: paper (default) or live.
    #[arg(long, default_value = "paper")]
    pub mode: String,

    /// Duration to run (e.g., "1h", "4h", "24h").
    #[arg(short, long, default_value = "1h")]
    pub duration: String,

    /// Filter to specific coin pair (e.g., "btc,eth" for BTC/ETH only).
    /// If not specified, trades all pairs.
    #[arg(long, default_value = "btc,eth")]
    pub pair: String,

    /// Filter to specific combination.
    /// Options: coin1down_coin2up (default), coin1up_coin2down, all.
    #[arg(long, default_value = "coin1down_coin2up")]
    pub combination: String,

    /// Fixed bet size in USDC per leg (overrides Kelly if set).
    #[arg(long)]
    pub bet_size: Option<f64>,

    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    #[arg(long, default_value = "0.25")]
    pub kelly_fraction: f64,

    /// Minimum spread required to execute. Default: 0.03 ($0.03).
    #[arg(long, default_value = "0.03")]
    pub min_spread: f64,

    /// Minimum win probability required. Default: 0.85 (85%).
    #[arg(long, default_value = "0.85")]
    pub min_win_prob: f64,

    /// Maximum position per window in USDC. Default: 200.
    #[arg(long, default_value = "200")]
    pub max_position: f64,

    /// Initial paper balance in USDC (paper mode only).
    #[arg(long, default_value = "1000")]
    pub paper_balance: f64,

    /// Stats update interval in seconds.
    #[arg(long, default_value = "30")]
    pub stats_interval_secs: u64,

    /// Scan interval in milliseconds.
    #[arg(long, default_value = "1000")]
    pub scan_interval_ms: u64,

    /// Show verbose output.
    #[arg(short, long)]
    pub verbose: bool,

    /// Persist executed trades to database (requires DATABASE_URL env var).
    #[arg(long)]
    pub persist: bool,

    /// Session ID for grouping trades (auto-generated if not provided).
    #[arg(long)]
    pub session_id: Option<String>,
}

impl CrossMarketAutoArgs {
    /// Parses the execution mode from the mode string.
    pub fn execution_mode(&self) -> Result<ExecutionMode> {
        ExecutionMode::from_str(&self.mode).map_err(|e| anyhow::anyhow!(e))
    }

    /// Parses the duration string.
    pub fn parsed_duration(&self) -> Result<Duration> {
        parse_duration(&self.duration)
    }

    /// Parses the pair filter.
    pub fn parse_pair(&self) -> Option<(Coin, Coin)> {
        let parts: Vec<&str> = self.pair.split(',').collect();
        if parts.len() != 2 {
            return None;
        }

        let coin1 = parse_coin(parts[0])?;
        let coin2 = parse_coin(parts[1])?;
        Some((coin1, coin2))
    }

    /// Parses the combination filter.
    pub fn parse_combination(&self) -> Option<CrossMarketCombination> {
        match self.combination.to_lowercase().as_str() {
            "coin1down_coin2up" | "c1d_c2u" => Some(CrossMarketCombination::Coin1DownCoin2Up),
            "coin1up_coin2down" | "c1u_c2d" => Some(CrossMarketCombination::Coin1UpCoin2Down),
            "all" => None,
            _ => None,
        }
    }

    /// Returns the fixed bet size as Decimal if set.
    pub fn fixed_bet_size(&self) -> Option<Decimal> {
        self.bet_size
            .map(|v| Decimal::from_str(&format!("{:.2}", v)).unwrap_or_default())
    }

    /// Returns the paper balance as Decimal.
    pub fn paper_balance_decimal(&self) -> Decimal {
        Decimal::from_str(&format!("{:.2}", self.paper_balance)).unwrap_or_default()
    }

    /// Validates the arguments.
    pub fn validate(&self) -> Result<()> {
        // Check Kelly fraction
        if !(0.0..=1.0).contains(&self.kelly_fraction) {
            anyhow::bail!("--kelly-fraction must be between 0.0 and 1.0");
        }

        // Check min spread
        if !(0.0..=1.0).contains(&self.min_spread) {
            anyhow::bail!("--min-spread must be between 0.0 and 1.0");
        }

        // Check min win prob
        if !(0.0..=1.0).contains(&self.min_win_prob) {
            anyhow::bail!("--min-win-prob must be between 0.0 and 1.0");
        }

        // Parse duration to check validity
        let _ = self.parsed_duration()?;

        // Parse mode to check validity
        let _ = self.execution_mode()?;

        Ok(())
    }
}

/// Parses a coin string to Coin enum.
fn parse_coin(s: &str) -> Option<Coin> {
    match s.trim().to_lowercase().as_str() {
        "btc" | "bitcoin" => Some(Coin::Btc),
        "eth" | "ethereum" => Some(Coin::Eth),
        "sol" | "solana" => Some(Coin::Sol),
        "xrp" | "ripple" => Some(Coin::Xrp),
        _ => None,
    }
}

/// Runs the cross-market automated trading command.
pub async fn run(args: CrossMarketAutoArgs) -> Result<()> {
    // Validate arguments
    args.validate()?;

    let mode = args.execution_mode()?;
    let duration = args.parsed_duration()?;
    let pair = args.parse_pair();
    let combination = args.parse_combination();

    info!("=== Cross-Market Automated Trading ===");
    info!(
        "Mode: {:?} | Duration: {:?} | Kelly: {} | Min Spread: ${:.2}",
        mode, duration, args.kelly_fraction, args.min_spread
    );

    if let Some((c1, c2)) = &pair {
        info!("Pair filter: {}/{}", c1.slug_prefix().to_uppercase(), c2.slug_prefix().to_uppercase());
    } else {
        info!("Pair filter: ALL PAIRS");
    }

    if let Some(combo) = &combination {
        info!("Combination filter: {:?}", combo);
    } else {
        info!("Combination filter: ALL COMBINATIONS");
    }

    // Connect to database if persistence is enabled
    let db_pool = if args.persist {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required for --persist"))?;
        info!("Connecting to database for trade persistence...");
        let pool = PgPool::connect(&database_url).await?;
        info!("Database connected");
        Some(pool)
    } else {
        None
    };

    let session_id = args.session_id.clone();
    if args.persist {
        info!("Session ID: {}", session_id.as_deref().unwrap_or("auto-generated"));
    }

    // Build runner config with depth tracking enabled for order book analysis
    let runner_config = CrossMarketRunnerConfig {
        scan_interval_ms: args.scan_interval_ms,
        track_depth: true, // Enable WebSocket depth tracking for execution analysis
        ..Default::default()
    };

    // Build auto executor config
    let mut auto_config = CrossMarketAutoExecutorConfig {
        filter_pair: pair,
        filter_combination: combination,
        kelly_fraction: args.kelly_fraction,
        min_spread: Decimal::from_str(&format!("{:.4}", args.min_spread))?,
        min_win_probability: args.min_win_prob,
        max_position_per_window: Decimal::from_str(&format!("{:.2}", args.max_position))?,
        ..Default::default()
    };

    if let Some(fixed) = args.fixed_bet_size() {
        auto_config.fixed_bet_size = Some(fixed);
        info!("Using fixed bet size: ${} per leg", fixed);
    }

    // Create runner and get opportunity receiver
    let (runner, opp_rx) = CrossMarketRunner::new(runner_config);

    // Run based on mode
    match mode {
        ExecutionMode::Paper => {
            let paper_config = PaperExecutorConfig {
                initial_balance: args.paper_balance_decimal(),
                fill_rate: 0.95, // Higher fill rate for paper trading
                ..Default::default()
            };
            let executor = PaperExecutor::new(paper_config);
            info!("Paper trading with ${} balance", args.paper_balance);

            run_auto_trading(
                runner,
                executor,
                opp_rx,
                auto_config,
                duration,
                args.stats_interval_secs,
                args.verbose,
                db_pool,
                session_id,
            )
            .await
        }
        ExecutionMode::Live => {
            warn!("LIVE TRADING MODE - Real funds will be used!");
            info!("Creating live executor from environment...");

            let live_config = LiveExecutorConfig::mainnet();
            let executor = LiveExecutor::new(live_config).await?;

            let balance = executor.get_balance().await?;
            info!("Live trading with ${} available balance", balance);

            run_auto_trading(
                runner,
                executor,
                opp_rx,
                auto_config,
                duration,
                args.stats_interval_secs,
                args.verbose,
                db_pool,
                session_id,
            )
            .await
        }
    }
}

/// Runs the automated trading loop with the given executor.
async fn run_auto_trading<E: PolymarketExecutor + Send + 'static>(
    runner: CrossMarketRunner,
    executor: E,
    opp_rx: tokio::sync::mpsc::Receiver<algo_trade_polymarket::arbitrage::CrossMarketOpportunity>,
    config: CrossMarketAutoExecutorConfig,
    duration: Duration,
    stats_interval_secs: u64,
    verbose: bool,
    db_pool: Option<PgPool>,
    session_id: Option<String>,
) -> Result<()> {
    let runner_stop = runner.stop_handle();
    let runner_stats = runner.stats();

    // Create auto executor (with or without persistence)
    let mut auto_executor = match db_pool {
        Some(pool) => {
            info!("Trade persistence ENABLED");
            CrossMarketAutoExecutor::with_persistence(executor, config, pool, session_id)
        }
        None => {
            info!("Trade persistence DISABLED (use --persist to enable)");
            CrossMarketAutoExecutor::new(executor, config)
        }
    };
    let auto_stop = auto_executor.stop_handle();
    let auto_stats = auto_executor.stats();

    // Spawn runner
    let runner_handle = tokio::spawn(async move {
        if let Err(e) = runner.run().await {
            error!("Runner error: {}", e);
        }
    });

    // Spawn auto executor
    let executor_handle = tokio::spawn(async move {
        if let Err(e) = auto_executor.run(opp_rx).await {
            error!("AutoExecutor error: {}", e);
        }
    });

    // Set up Ctrl+C handler
    let stop_runner = runner_stop.clone();
    let stop_executor = auto_stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received Ctrl+C, stopping...");
            stop_runner.store(true, Ordering::SeqCst);
            stop_executor.store(true, Ordering::SeqCst);
        }
    });

    // Main monitoring loop
    let deadline = tokio::time::Instant::now() + duration;
    let stats_interval = Duration::from_secs(stats_interval_secs);
    let mut last_stats = tokio::time::Instant::now();

    info!("");
    info!("Automated trading started...");
    info!("   Press Ctrl+C to stop early");
    info!("");

    loop {
        // Check if we should stop
        if runner_stop.load(Ordering::SeqCst) || auto_stop.load(Ordering::SeqCst) {
            break;
        }

        // Check deadline
        if tokio::time::Instant::now() >= deadline {
            info!("Duration elapsed");
            break;
        }

        // Print periodic stats
        if last_stats.elapsed() >= stats_interval {
            print_stats(&runner_stats, &auto_stats, verbose).await;
            last_stats = tokio::time::Instant::now();
        }

        // Small sleep to avoid busy waiting
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Stop both components
    runner_stop.store(true, Ordering::SeqCst);
    auto_stop.store(true, Ordering::SeqCst);

    // Wait for tasks to finish
    let _ = tokio::time::timeout(Duration::from_secs(5), runner_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), executor_handle).await;

    // Print final summary
    print_summary(&runner_stats, &auto_stats).await;

    Ok(())
}

/// Prints current statistics.
async fn print_stats(
    runner_stats: &Arc<RwLock<CrossMarketRunnerStats>>,
    auto_stats: &Arc<RwLock<CrossMarketAutoExecutorStats>>,
    verbose: bool,
) {
    let runner = runner_stats.read().await;
    let auto = auto_stats.read().await;

    if verbose {
        info!(
            "Scans: {} | Opps: {} | Filled: {} | Pending: {} | W/L: {}/{} | P&L: ${}",
            runner.scans_performed,
            auto.opportunities_received,
            auto.both_filled,
            auto.pending_settlement,
            auto.settled_wins,
            auto.settled_losses,
            auto.realized_pnl
        );
    } else {
        info!(
            "Trades: {} | Pending: {} | Wins: {} | Losses: {} | P&L: ${}",
            auto.both_filled,
            auto.pending_settlement,
            auto.settled_wins,
            auto.settled_losses,
            auto.realized_pnl
        );
    }
}

/// Prints final summary.
async fn print_summary(
    runner_stats: &Arc<RwLock<CrossMarketRunnerStats>>,
    auto_stats: &Arc<RwLock<CrossMarketAutoExecutorStats>>,
) {
    let runner = runner_stats.read().await;
    let auto = auto_stats.read().await;

    info!("");
    info!("=== Final Summary ===");
    info!("Scanner:");
    info!("  Scans performed: {}", runner.scans_performed);
    info!("  Opportunities detected: {}", runner.opportunities_detected);
    if let Some(best) = runner.best_spread {
        info!("  Best spread seen: ${}", best);
    }

    info!("");
    info!("Executor:");
    info!("  Opportunities received: {}", auto.opportunities_received);
    info!("  Opportunities skipped: {}", auto.opportunities_skipped);
    info!("  Executions attempted: {}", auto.executions_attempted);
    info!("  Both legs filled: {}", auto.both_filled);
    info!("  Partial fills: {}", auto.partial_fills);
    info!("  Both rejected: {}", auto.both_rejected);
    info!("  Total volume: ${}", auto.total_volume);

    if auto.executions_attempted > 0 {
        let fill_rate =
            auto.both_filled as f64 / auto.executions_attempted as f64 * 100.0;
        info!("  Fill rate: {:.1}%", fill_rate);
    }

    info!("");
    info!("Settlement (Paper):");
    info!("  Pending settlement: {}", auto.pending_settlement);
    info!("  Settled wins: {}", auto.settled_wins);
    info!("  Settled losses: {}", auto.settled_losses);
    info!("  Double wins: {}", auto.double_wins);
    info!("  Realized P&L: ${}", auto.realized_pnl);

    if auto.settled_wins + auto.settled_losses > 0 {
        let win_rate = auto.settled_wins as f64 / (auto.settled_wins + auto.settled_losses) as f64 * 100.0;
        info!("  Win rate: {:.1}%", win_rate);
    }

    info!("");
}
