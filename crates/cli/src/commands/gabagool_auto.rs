//! CLI command for automated Gabagool trading.
//!
//! This command runs the full automated trading system, connecting the
//! `GabagoolRunner` (signal detection) to a `PolymarketExecutor` (order execution).
//!
//! # Features
//!
//! - Paper trading mode (default) for safe testing
//! - Live trading mode with real funds
//! - Configurable bet sizing (Kelly criterion or fixed)
//! - Periodic stats display and export
//! - Graceful shutdown handling
//!
//! # Example
//!
//! ```bash
//! # Paper trading for 1 hour
//! algo-trade gabagool-auto --yes-token <id> --no-token <id> --duration 1h
//!
//! # Live trading with fixed $50 bets
//! algo-trade gabagool-auto --mode live --yes-token <id> --no-token <id> \
//!     --bet-size 50 --duration 4h
//! ```

use algo_trade_polymarket::arbitrage::{
    AutoExecutor, AutoExecutorConfig, GabagoolConfig, GabagoolRunner, GabagoolRunnerConfig,
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PolymarketExecutor,
};
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::Duration;
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

/// Arguments for the gabagool-auto command.
#[derive(Args, Debug)]
pub struct GabagoolAutoArgs {
    /// Execution mode: paper (default) or live.
    #[arg(long, default_value = "paper")]
    pub mode: String,

    /// Duration to run (e.g., "1h", "4h", "24h").
    #[arg(short, long, default_value = "1h")]
    pub duration: String,

    /// YES token ID (required).
    #[arg(long)]
    pub yes_token: String,

    /// NO token ID (required).
    #[arg(long)]
    pub no_token: String,

    /// Fixed bet size in USDC (overrides Kelly if set).
    #[arg(long)]
    pub bet_size: Option<f64>,

    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    #[arg(long, default_value = "0.25")]
    pub kelly_fraction: f64,

    /// Minimum edge required to execute (0.0 to 1.0). Default: 0.02 (2%).
    #[arg(long, default_value = "0.02")]
    pub min_edge: f64,

    /// Path to export trade history (JSONL format).
    #[arg(long)]
    pub history_path: Option<PathBuf>,

    /// Initial paper balance in USDC (paper mode only).
    #[arg(long, default_value = "1000")]
    pub paper_balance: f64,

    /// Stats update interval in seconds.
    #[arg(long, default_value = "60")]
    pub stats_interval_secs: u64,

    /// Maximum price to consider "cheap" for entry.
    #[arg(long, default_value = "0.42")]
    pub cheap_threshold: f64,

    /// Maximum pair cost for hedge.
    #[arg(long, default_value = "0.97")]
    pub pair_cost_threshold: f64,

    /// Time before window close to scratch (seconds).
    #[arg(long, default_value = "180")]
    pub scratch_time_secs: i64,

    /// Use aggressive config.
    #[arg(long)]
    pub aggressive: bool,

    /// Use conservative config.
    #[arg(long)]
    pub conservative: bool,

    /// Show verbose output.
    #[arg(short, long)]
    pub verbose: bool,
}

impl GabagoolAutoArgs {
    /// Parses the execution mode from the mode string.
    pub fn execution_mode(&self) -> Result<ExecutionMode> {
        ExecutionMode::from_str(&self.mode).map_err(|e| anyhow::anyhow!(e))
    }

    /// Parses the duration string.
    pub fn parsed_duration(&self) -> Result<Duration> {
        parse_duration(&self.duration)
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
        // Check token IDs
        if self.yes_token.is_empty() {
            anyhow::bail!("--yes-token is required");
        }
        if self.no_token.is_empty() {
            anyhow::bail!("--no-token is required");
        }

        // Check Kelly fraction
        if !(0.0..=1.0).contains(&self.kelly_fraction) {
            anyhow::bail!("--kelly-fraction must be between 0.0 and 1.0");
        }

        // Check min edge
        if !(0.0..=1.0).contains(&self.min_edge) {
            anyhow::bail!("--min-edge must be between 0.0 and 1.0");
        }

        // Check thresholds
        if !(0.0..=1.0).contains(&self.cheap_threshold) {
            anyhow::bail!("--cheap-threshold must be between 0.0 and 1.0");
        }
        if !(0.0..=1.0).contains(&self.pair_cost_threshold) {
            anyhow::bail!("--pair-cost-threshold must be between 0.0 and 1.0");
        }

        // Check conflicting options
        if self.aggressive && self.conservative {
            anyhow::bail!("Cannot use both --aggressive and --conservative");
        }

        // Parse duration to check validity
        let _ = self.parsed_duration()?;

        // Parse mode to check validity
        let _ = self.execution_mode()?;

        Ok(())
    }
}

/// Runs the gabagool automated trading command.
pub async fn run(args: GabagoolAutoArgs) -> Result<()> {
    // Validate arguments
    args.validate()?;

    let mode = args.execution_mode()?;
    let duration = args.parsed_duration()?;

    info!("=== Gabagool Automated Trading ===");
    info!(
        "Mode: {:?} | Duration: {:?} | Kelly: {} | Min Edge: {}%",
        mode,
        duration,
        args.kelly_fraction,
        args.min_edge * 100.0
    );

    // Build gabagool detector config
    let detector_config = if args.aggressive {
        info!("Using AGGRESSIVE config");
        GabagoolConfig::aggressive()
    } else if args.conservative {
        info!("Using CONSERVATIVE config");
        GabagoolConfig::conservative()
    } else {
        GabagoolConfig {
            cheap_threshold: Decimal::try_from(args.cheap_threshold)?,
            pair_cost_threshold: Decimal::try_from(args.pair_cost_threshold)?,
            scratch_time_secs: args.scratch_time_secs,
            ..GabagoolConfig::default()
        }
    };

    // Build runner config
    let runner_config = GabagoolRunnerConfig {
        yes_token_id: args.yes_token.clone(),
        no_token_id: args.no_token.clone(),
        detector_config,
        ..Default::default()
    };

    // Build auto executor config
    let mut auto_config = AutoExecutorConfig::default()
        .with_yes_token(&args.yes_token)
        .with_no_token(&args.no_token)
        .with_kelly_fraction(args.kelly_fraction);

    auto_config.min_edge = args.min_edge;

    if let Some(fixed) = args.fixed_bet_size() {
        auto_config = auto_config.with_fixed_bet(fixed);
        info!("Using fixed bet size: ${}", fixed);
    }

    if let Some(ref path) = args.history_path {
        auto_config = auto_config.with_history_path(path);
        info!("Trade history will be exported to: {}", path.display());
    }

    // Create runner and get signal receiver
    let (runner, signal_rx) = GabagoolRunner::new(runner_config);

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
                signal_rx,
                auto_config,
                duration,
                args.stats_interval_secs,
                args.verbose,
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
                signal_rx,
                auto_config,
                duration,
                args.stats_interval_secs,
                args.verbose,
            )
            .await
        }
    }
}

/// Runs the automated trading loop with the given executor.
async fn run_auto_trading<E: PolymarketExecutor + Send + 'static>(
    runner: GabagoolRunner,
    executor: E,
    signal_rx: tokio::sync::mpsc::Receiver<algo_trade_polymarket::arbitrage::GabagoolSignal>,
    config: AutoExecutorConfig,
    duration: Duration,
    stats_interval_secs: u64,
    verbose: bool,
) -> Result<()> {
    let runner_stop = runner.stop_handle();
    let runner_stats = runner.stats();

    // Create auto executor
    let mut auto_executor = AutoExecutor::new(executor, config);
    let auto_stop = auto_executor.stop_handle();
    let auto_stats = auto_executor.stats();
    let auto_history = auto_executor.history();

    // Spawn runner
    let runner_handle = tokio::spawn(async move {
        if let Err(e) = runner.run().await {
            error!("Runner error: {}", e);
        }
    });

    // Spawn auto executor
    let executor_handle = tokio::spawn(async move {
        if let Err(e) = auto_executor.run(signal_rx).await {
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
    print_summary(&runner_stats, &auto_stats, &auto_history).await;

    Ok(())
}

/// Prints current statistics.
async fn print_stats(
    runner_stats: &std::sync::Arc<
        tokio::sync::RwLock<algo_trade_polymarket::arbitrage::GabagoolRunnerStats>,
    >,
    auto_stats: &std::sync::Arc<
        tokio::sync::RwLock<algo_trade_polymarket::arbitrage::AutoExecutorStats>,
    >,
    verbose: bool,
) {
    let rs = runner_stats.read().await;
    let as_ = auto_stats.read().await;

    if verbose {
        info!(
            "Runner: {} checks, {} windows, {} signals | Executor: {} signals, {} orders, ${} volume",
            rs.checks_performed,
            rs.windows_processed,
            rs.total_signals(),
            as_.signals_received,
            as_.orders_attempted,
            as_.total_volume
        );
    } else {
        info!(
            "Signals: {} | Orders: {} filled / {} attempted | Volume: ${} | P&L: ${}",
            as_.signals_received,
            as_.orders_filled,
            as_.orders_attempted,
            as_.total_volume,
            as_.realized_pnl
        );
    }
}

/// Prints the final session summary.
async fn print_summary(
    runner_stats: &std::sync::Arc<
        tokio::sync::RwLock<algo_trade_polymarket::arbitrage::GabagoolRunnerStats>,
    >,
    auto_stats: &std::sync::Arc<
        tokio::sync::RwLock<algo_trade_polymarket::arbitrage::AutoExecutorStats>,
    >,
    history: &std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::VecDeque<algo_trade_polymarket::arbitrage::TradeRecord>,
        >,
    >,
) {
    let rs = runner_stats.read().await;
    let as_ = auto_stats.read().await;
    let hist = history.read().await;

    println!();
    println!("=== Session Summary ===");
    println!();
    println!("Signal Detection:");
    println!("  Checks performed: {}", rs.checks_performed);
    println!("  Windows processed: {}", rs.windows_processed);
    println!(
        "  Signals generated: {} (Entry: {}, Hedge: {}, Scratch: {})",
        rs.total_signals(),
        rs.entry_signals,
        rs.hedge_signals,
        rs.scratch_signals
    );
    println!();
    println!("Order Execution:");
    println!("  Signals received: {}", as_.signals_received);
    println!("  Signals skipped: {}", as_.signals_skipped);
    println!("  Orders attempted: {}", as_.orders_attempted);
    println!(
        "  Orders filled: {} ({} partial, {} failed)",
        as_.orders_filled, as_.orders_partial, as_.orders_failed
    );
    println!();
    println!("Trade Breakdown:");
    println!("  Entry trades: {}", as_.entry_trades);
    println!("  Hedge trades: {}", as_.hedge_trades);
    println!("  Scratch trades: {}", as_.scratch_trades);
    println!();
    println!("Financial:");
    println!("  Total volume: ${}", as_.total_volume);
    println!("  Current position: ${}", as_.current_position_value);
    println!("  Realized P&L: ${}", as_.realized_pnl);
    println!();
    println!("Trade History: {} records", hist.len());

    if !hist.is_empty() {
        println!();
        println!("=== Recent Trades ===");
        for record in hist.iter().rev().take(10) {
            println!(
                "  {} {:?} {:?} @ ${} | Filled: {} | Status: {:?}",
                record.execution_timestamp.format("%H:%M:%S"),
                record.signal_type,
                record.direction,
                record.price,
                record.filled_size,
                record.status
            );
        }
    }

    // Win rate analysis if we have completed trades
    if as_.orders_filled > 0 {
        let fill_rate = as_.orders_filled as f64 / as_.orders_attempted.max(1) as f64;
        println!();
        println!("=== Performance Metrics ===");
        println!("  Fill rate: {:.1}%", fill_rate * 100.0);
        if as_.total_volume > rust_decimal::Decimal::ZERO {
            let roi = as_.realized_pnl / as_.total_volume;
            println!(
                "  ROI on volume: {:.2}%",
                roi * rust_decimal_macros::dec!(100)
            );
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Argument Parsing Tests
    // =========================================================================

    #[test]
    fn test_execution_mode_paper() {
        let mode = ExecutionMode::from_str("paper").unwrap();
        assert_eq!(mode, ExecutionMode::Paper);
    }

    #[test]
    fn test_execution_mode_live() {
        let mode = ExecutionMode::from_str("live").unwrap();
        assert_eq!(mode, ExecutionMode::Live);
    }

    #[test]
    fn test_execution_mode_case_insensitive() {
        assert_eq!(
            ExecutionMode::from_str("PAPER").unwrap(),
            ExecutionMode::Paper
        );
        assert_eq!(
            ExecutionMode::from_str("Live").unwrap(),
            ExecutionMode::Live
        );
    }

    #[test]
    fn test_execution_mode_invalid() {
        let result = ExecutionMode::from_str("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_args_validate_missing_yes_token() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "".to_string(),
            no_token: "no-token-123".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("yes-token"));
    }

    #[test]
    fn test_args_validate_missing_no_token() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no-token"));
    }

    #[test]
    fn test_args_validate_invalid_kelly_fraction() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: None,
            kelly_fraction: 1.5, // Invalid: > 1.0
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("kelly-fraction"));
    }

    #[test]
    fn test_args_validate_invalid_min_edge() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: -0.05, // Invalid: negative
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("min-edge"));
    }

    #[test]
    fn test_args_validate_conflicting_configs() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: true,
            conservative: true, // Conflict!
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("aggressive") || err_msg.contains("conservative"));
    }

    #[test]
    fn test_args_validate_invalid_duration() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "invalid".to_string(), // Invalid
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_args_validate_invalid_mode() {
        let args = GabagoolAutoArgs {
            mode: "invalid_mode".to_string(), // Invalid
            duration: "1h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_args_validate_success() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "4h".to_string(),
            yes_token: "yes-token-123".to_string(),
            no_token: "no-token-456".to_string(),
            bet_size: Some(50.0),
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let result = args.validate();
        assert!(result.is_ok());
    }

    // =========================================================================
    // Helper Method Tests
    // =========================================================================

    #[test]
    fn test_fixed_bet_size_some() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes".to_string(),
            no_token: "no".to_string(),
            bet_size: Some(50.5),
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let bet = args.fixed_bet_size();
        assert!(bet.is_some());
        assert_eq!(bet.unwrap(), Decimal::from_str("50.50").unwrap());
    }

    #[test]
    fn test_fixed_bet_size_none() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes".to_string(),
            no_token: "no".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        assert!(args.fixed_bet_size().is_none());
    }

    #[test]
    fn test_paper_balance_decimal() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "1h".to_string(),
            yes_token: "yes".to_string(),
            no_token: "no".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 2500.50,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let balance = args.paper_balance_decimal();
        assert_eq!(balance, Decimal::from_str("2500.50").unwrap());
    }

    #[test]
    fn test_parsed_duration() {
        let args = GabagoolAutoArgs {
            mode: "paper".to_string(),
            duration: "2h".to_string(),
            yes_token: "yes".to_string(),
            no_token: "no".to_string(),
            bet_size: None,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            history_path: None,
            paper_balance: 1000.0,
            stats_interval_secs: 60,
            cheap_threshold: 0.42,
            pair_cost_threshold: 0.97,
            scratch_time_secs: 180,
            aggressive: false,
            conservative: false,
            verbose: false,
        };

        let duration = args.parsed_duration().unwrap();
        assert_eq!(duration, Duration::from_secs(2 * 3600));
    }

    #[test]
    fn test_execution_mode_default() {
        let mode = ExecutionMode::default();
        assert_eq!(mode, ExecutionMode::Paper);
    }
}
