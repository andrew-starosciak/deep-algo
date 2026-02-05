//! CLI command for automated cross-market arbitrage trading.
//!
//! This command runs the full automated trading system, connecting the
//! `CrossMarketRunner` (signal detection) to `CrossMarketAutoExecutor` (order execution).

use algo_trade_polymarket::arbitrage::{
    CrossMarketAutoExecutor, CrossMarketAutoExecutorConfig, CrossMarketAutoExecutorStats,
    CrossMarketCombination, CrossMarketRunner, CrossMarketRunnerConfig, CrossMarketRunnerStats,
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PolymarketExecutor,
};
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use chrono::Local;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::io::{stdout, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::error;

use super::collect_signals::parse_duration;

/// Trading execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    #[default]
    Paper,
    Live,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Paper => write!(f, "PAPER"),
            ExecutionMode::Live => write!(f, "LIVE"),
        }
    }
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
    #[arg(long, default_value = "btc,eth")]
    pub pair: String,

    /// Filter to specific combination.
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
    #[arg(long, default_value = "1")]
    pub stats_interval_secs: u64,

    /// Scan interval in milliseconds.
    #[arg(long, default_value = "1000")]
    pub scan_interval_ms: u64,

    /// Show verbose output (logs instead of dashboard).
    #[arg(short, long)]
    pub verbose: bool,

    /// Persist executed trades to database.
    #[arg(long)]
    pub persist: bool,

    /// Session ID for grouping trades.
    #[arg(long)]
    pub session_id: Option<String>,
}

impl CrossMarketAutoArgs {
    pub fn execution_mode(&self) -> Result<ExecutionMode> {
        ExecutionMode::from_str(&self.mode).map_err(|e| anyhow::anyhow!(e))
    }

    pub fn parsed_duration(&self) -> Result<Duration> {
        parse_duration(&self.duration)
    }

    pub fn parse_pair(&self) -> Option<(Coin, Coin)> {
        let parts: Vec<&str> = self.pair.split(',').collect();
        if parts.len() != 2 {
            return None;
        }
        let coin1 = parse_coin(parts[0])?;
        let coin2 = parse_coin(parts[1])?;
        Some((coin1, coin2))
    }

    pub fn parse_combination(&self) -> Option<CrossMarketCombination> {
        match self.combination.to_lowercase().as_str() {
            "coin1down_coin2up" | "c1d_c2u" => Some(CrossMarketCombination::Coin1DownCoin2Up),
            "coin1up_coin2down" | "c1u_c2d" => Some(CrossMarketCombination::Coin1UpCoin2Down),
            "all" => None,
            _ => None,
        }
    }

    pub fn fixed_bet_size(&self) -> Option<Decimal> {
        self.bet_size
            .map(|v| Decimal::from_str(&format!("{:.2}", v)).unwrap_or_default())
    }

    pub fn paper_balance_decimal(&self) -> Decimal {
        Decimal::from_str(&format!("{:.2}", self.paper_balance)).unwrap_or_default()
    }

    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.kelly_fraction) {
            anyhow::bail!("--kelly-fraction must be between 0.0 and 1.0");
        }
        if !(0.0..=1.0).contains(&self.min_spread) {
            anyhow::bail!("--min-spread must be between 0.0 and 1.0");
        }
        if !(0.0..=1.0).contains(&self.min_win_prob) {
            anyhow::bail!("--min-win-prob must be between 0.0 and 1.0");
        }
        let _ = self.parsed_duration()?;
        let _ = self.execution_mode()?;
        Ok(())
    }
}

fn parse_coin(s: &str) -> Option<Coin> {
    match s.trim().to_lowercase().as_str() {
        "btc" | "bitcoin" => Some(Coin::Btc),
        "eth" | "ethereum" => Some(Coin::Eth),
        "sol" | "solana" => Some(Coin::Sol),
        "xrp" | "ripple" => Some(Coin::Xrp),
        _ => None,
    }
}

/// Dashboard configuration passed to display functions.
struct DashboardConfig {
    mode: ExecutionMode,
    pair: String,
    combination: String,
    bet_size: String,
    kelly_fraction: f64,
    min_spread: f64,
    balance: Decimal,
    duration: Duration,
    persist: bool,
}

/// Runs the cross-market automated trading command.
pub async fn run(args: CrossMarketAutoArgs) -> Result<()> {
    args.validate()?;

    let mode = args.execution_mode()?;
    let duration = args.parsed_duration()?;
    let pair = args.parse_pair();
    let combination = args.parse_combination();

    // Connect to database if persistence is enabled
    let db_pool = if args.persist {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required for --persist"))?;
        let pool = PgPool::connect(&database_url).await?;
        Some(pool)
    } else {
        None
    };

    let session_id = args.session_id.clone();

    // Build runner config
    let runner_config = CrossMarketRunnerConfig {
        scan_interval_ms: args.scan_interval_ms,
        track_depth: true,
        ..Default::default()
    };

    // Build auto executor config
    let mut auto_config = CrossMarketAutoExecutorConfig {
        filter_pair: pair.clone(),
        filter_combination: combination,
        kelly_fraction: args.kelly_fraction,
        min_spread: Decimal::from_str(&format!("{:.4}", args.min_spread))?,
        min_win_probability: args.min_win_prob,
        max_position_per_window: Decimal::from_str(&format!("{:.2}", args.max_position))?,
        ..Default::default()
    };

    if let Some(fixed) = args.fixed_bet_size() {
        auto_config.fixed_bet_size = Some(fixed);
    }

    // Create runner
    let (runner, opp_rx) = CrossMarketRunner::new(runner_config);

    // Dashboard config
    let pair_str = if let Some((c1, c2)) = &pair {
        format!("{}/{}", c1.slug_prefix().to_uppercase(), c2.slug_prefix().to_uppercase())
    } else {
        "ALL".to_string()
    };

    let combo_str = match combination {
        Some(CrossMarketCombination::Coin1DownCoin2Up) => "Coin1↓ Coin2↑".to_string(),
        Some(CrossMarketCombination::Coin1UpCoin2Down) => "Coin1↑ Coin2↓".to_string(),
        Some(CrossMarketCombination::BothUp) => "Both↑".to_string(),
        Some(CrossMarketCombination::BothDown) => "Both↓".to_string(),
        None => "ALL".to_string(),
    };

    let bet_str = args
        .fixed_bet_size()
        .map(|v| format!("${}", v))
        .unwrap_or_else(|| format!("Kelly {}%", (args.kelly_fraction * 100.0) as i32));

    // Run based on mode
    match mode {
        ExecutionMode::Paper => {
            let paper_config = PaperExecutorConfig {
                initial_balance: args.paper_balance_decimal(),
                fill_rate: 0.95,
                ..Default::default()
            };
            let executor = PaperExecutor::new(paper_config);

            let dashboard_config = DashboardConfig {
                mode,
                pair: pair_str,
                combination: combo_str,
                bet_size: bet_str,
                kelly_fraction: args.kelly_fraction,
                min_spread: args.min_spread,
                balance: args.paper_balance_decimal(),
                duration,
                persist: args.persist,
            };

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
                dashboard_config,
            )
            .await
        }
        ExecutionMode::Live => {
            let live_config = LiveExecutorConfig::mainnet();
            let executor = LiveExecutor::new(live_config).await?;
            let balance = executor.get_balance().await?;

            let dashboard_config = DashboardConfig {
                mode,
                pair: pair_str,
                combination: combo_str,
                bet_size: bet_str,
                kelly_fraction: args.kelly_fraction,
                min_spread: args.min_spread,
                balance,
                duration,
                persist: args.persist,
            };

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
                dashboard_config,
            )
            .await
        }
    }
}

/// Runs the automated trading loop.
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
    dashboard_config: DashboardConfig,
) -> Result<()> {
    let runner_stop = runner.stop_handle();
    let runner_stats = runner.stats();

    let mut auto_executor = match db_pool {
        Some(pool) => CrossMarketAutoExecutor::with_persistence(executor, config, pool, session_id),
        None => CrossMarketAutoExecutor::new(executor, config),
    };
    let auto_stop = auto_executor.stop_handle();
    let auto_stats = auto_executor.stats();

    // Track system status
    let scanner_ready = Arc::new(AtomicBool::new(false));
    let executor_ready = Arc::new(AtomicBool::new(false));

    // Spawn runner
    let scanner_ready_clone = scanner_ready.clone();
    let runner_handle = tokio::spawn(async move {
        scanner_ready_clone.store(true, Ordering::SeqCst);
        if let Err(e) = runner.run().await {
            error!("Runner error: {}", e);
        }
    });

    // Spawn auto executor
    let executor_ready_clone = executor_ready.clone();
    let executor_handle = tokio::spawn(async move {
        executor_ready_clone.store(true, Ordering::SeqCst);
        if let Err(e) = auto_executor.run(opp_rx).await {
            error!("AutoExecutor error: {}", e);
        }
    });

    // Set up Ctrl+C handler
    let stop_runner = runner_stop.clone();
    let stop_executor = auto_stop.clone();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            shutdown_clone.store(true, Ordering::SeqCst);
            stop_runner.store(true, Ordering::SeqCst);
            stop_executor.store(true, Ordering::SeqCst);
        }
    });

    // Main monitoring loop
    let start_time = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + duration;
    let stats_interval = Duration::from_secs(stats_interval_secs);
    let mut last_stats = tokio::time::Instant::now();

    // Print initial launch banner (once)
    if !verbose {
        print_launch_banner(&dashboard_config);
    }

    loop {
        if runner_stop.load(Ordering::SeqCst) || auto_stop.load(Ordering::SeqCst) {
            break;
        }

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        if last_stats.elapsed() >= stats_interval {
            if verbose {
                print_stats_log(&runner_stats, &auto_stats).await;
            } else {
                print_dashboard(
                    &dashboard_config,
                    &runner_stats,
                    &auto_stats,
                    start_time.elapsed(),
                    duration,
                    scanner_ready.load(Ordering::SeqCst),
                    executor_ready.load(Ordering::SeqCst),
                    shutdown.load(Ordering::SeqCst),
                )
                .await;
            }
            last_stats = tokio::time::Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Stop both components
    runner_stop.store(true, Ordering::SeqCst);
    auto_stop.store(true, Ordering::SeqCst);

    let _ = tokio::time::timeout(Duration::from_secs(5), runner_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), executor_handle).await;

    // Print final summary
    if !verbose {
        // Move cursor below dashboard
        print!("\x1b[20B\n");
    }
    print_summary(&runner_stats, &auto_stats).await;

    Ok(())
}

/// Prints the launch banner (shown once at startup).
fn print_launch_banner(config: &DashboardConfig) {
    // Clear screen and move to top
    print!("\x1b[2J\x1b[H");

    let mode_color = if config.mode == ExecutionMode::Live { "\x1b[91m" } else { "\x1b[93m" };
    let reset = "\x1b[0m";
    let green = "\x1b[92m";
    let cyan = "\x1b[96m";
    let dim = "\x1b[2m";

    println!("{cyan}╔══════════════════════════════════════════════════════════════════╗{reset}");
    println!("{cyan}║{reset}       {green}CROSS-MARKET CORRELATION ARBITRAGE{reset}                        {cyan}║{reset}");
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  Mode: {mode_color}{:6}{reset}  │  Pair: {:8}  │  Strategy: {:12} {cyan}║{reset}",
        config.mode, config.pair, config.combination);
    println!("{cyan}║{reset}  Bet:  {:6}  │  Spread: ${:.2}    │  Balance: ${:14} {cyan}║{reset}",
        config.bet_size, config.min_spread, config.balance);
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  {dim}Starting systems...{reset}                                            {cyan}║{reset}");
    println!("{cyan}╚══════════════════════════════════════════════════════════════════╝{reset}");
    println!();
    let _ = stdout().flush();
}

/// Prints the refreshing dashboard.
async fn print_dashboard(
    config: &DashboardConfig,
    runner_stats: &Arc<RwLock<CrossMarketRunnerStats>>,
    auto_stats: &Arc<RwLock<CrossMarketAutoExecutorStats>>,
    elapsed: Duration,
    total_duration: Duration,
    scanner_ready: bool,
    executor_ready: bool,
    shutting_down: bool,
) {
    let runner = runner_stats.read().await;
    let auto = auto_stats.read().await;

    // ANSI codes
    let reset = "\x1b[0m";
    let green = "\x1b[92m";
    let red = "\x1b[91m";
    let yellow = "\x1b[93m";
    let cyan = "\x1b[96m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";

    let mode_color = if config.mode == ExecutionMode::Live { red } else { yellow };

    // Status indicators
    let scanner_status = if scanner_ready { format!("{green}●{reset}") } else { format!("{dim}○{reset}") };
    let executor_status = if executor_ready { format!("{green}●{reset}") } else { format!("{dim}○{reset}") };
    let ws_status = if runner.scans_performed > 0 { format!("{green}●{reset}") } else { format!("{yellow}○{reset}") };

    // Calculate progress
    let progress_pct = (elapsed.as_secs_f64() / total_duration.as_secs_f64() * 100.0).min(100.0);
    let remaining = total_duration.saturating_sub(elapsed);
    let remaining_str = format_duration(remaining);

    // Win rate
    let total_settled = auto.settled_wins + auto.settled_losses;
    let win_rate = if total_settled > 0 {
        (auto.settled_wins as f64 / total_settled as f64) * 100.0
    } else {
        0.0
    };

    // P&L color
    let pnl_color = if auto.realized_pnl > Decimal::ZERO {
        green
    } else if auto.realized_pnl < Decimal::ZERO {
        red
    } else {
        reset
    };

    // Move cursor to top and redraw
    print!("\x1b[H");

    println!("{cyan}╔══════════════════════════════════════════════════════════════════╗{reset}");
    println!("{cyan}║{reset}       {green}CROSS-MARKET CORRELATION ARBITRAGE{reset}                        {cyan}║{reset}");
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  Mode: {mode_color}{:6}{reset}  │  Pair: {:8}  │  Strategy: {:12} {cyan}║{reset}",
        config.mode, config.pair, config.combination);
    println!("{cyan}║{reset}  Bet:  {:6}  │  Spread: ${:.2}    │  Balance: ${:14} {cyan}║{reset}",
        config.bet_size, config.min_spread, config.balance);
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");

    // Systems status
    let status_msg = if shutting_down {
        format!("{yellow}Shutting down...{reset}")
    } else {
        format!("{green}Running{reset}")
    };
    println!("{cyan}║{reset}  {bold}SYSTEMS{reset}   {scanner_status} Scanner   {executor_status} Executor   {ws_status} WebSocket   {status_msg:16} {cyan}║{reset}");

    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");

    // Trading stats
    println!("{cyan}║{reset}  {bold}TRADING{reset}                                                         {cyan}║{reset}");
    println!("{cyan}║{reset}    Scans: {:6}   Opportunities: {:6}   Executed: {:6}       {cyan}║{reset}",
        runner.scans_performed, runner.opportunities_detected, auto.both_filled);
    println!("{cyan}║{reset}    Pending: {:4}   Settled: {:4}   Skipped: {:4}   Rejected: {:4} {cyan}║{reset}",
        auto.pending_settlement, total_settled, auto.opportunities_skipped, auto.both_rejected);

    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");

    // Performance
    println!("{cyan}║{reset}  {bold}PERFORMANCE{reset}                                                     {cyan}║{reset}");
    println!("{cyan}║{reset}    Wins: {green}{:4}{reset}   Losses: {red}{:4}{reset}   Win Rate: {:5.1}%                   {cyan}║{reset}",
        auto.settled_wins, auto.settled_losses, win_rate);
    println!("{cyan}║{reset}    P&L: {pnl_color}${:12}{reset}   Volume: ${:12}                {cyan}║{reset}",
        auto.realized_pnl, auto.total_volume);

    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");

    // Progress bar
    let bar_width = 40;
    let filled = (progress_pct / 100.0 * bar_width as f64) as usize;
    let empty = bar_width - filled;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    println!("{cyan}║{reset}  {dim}Progress:{reset} [{cyan}{}{reset}] {:5.1}%  Remaining: {:8} {cyan}║{reset}",
        bar, progress_pct, remaining_str);
    println!("{cyan}║{reset}  {dim}Time: {}{reset}   {dim}Press Ctrl+C to stop{reset}                          {cyan}║{reset}",
        Local::now().format("%H:%M:%S"));
    println!("{cyan}╚══════════════════════════════════════════════════════════════════╝{reset}");

    let _ = stdout().flush();
}

/// Format duration as HH:MM:SS or MM:SS.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

/// Prints stats as log lines (verbose mode).
async fn print_stats_log(
    runner_stats: &Arc<RwLock<CrossMarketRunnerStats>>,
    auto_stats: &Arc<RwLock<CrossMarketAutoExecutorStats>>,
) {
    let runner = runner_stats.read().await;
    let auto = auto_stats.read().await;

    tracing::info!(
        "Scans: {} | Opps: {} | Filled: {} | Pending: {} | W/L: {}/{} | P&L: ${}",
        runner.scans_performed,
        auto.opportunities_received,
        auto.both_filled,
        auto.pending_settlement,
        auto.settled_wins,
        auto.settled_losses,
        auto.realized_pnl
    );
}

/// Prints final summary.
async fn print_summary(
    runner_stats: &Arc<RwLock<CrossMarketRunnerStats>>,
    auto_stats: &Arc<RwLock<CrossMarketAutoExecutorStats>>,
) {
    let runner = runner_stats.read().await;
    let auto = auto_stats.read().await;

    let green = "\x1b[92m";
    let red = "\x1b[91m";
    let cyan = "\x1b[96m";
    let reset = "\x1b[0m";
    let bold = "\x1b[1m";

    println!();
    println!("{cyan}╔══════════════════════════════════════════════════════════════════╗{reset}");
    println!("{cyan}║{reset}                      {bold}FINAL SUMMARY{reset}                              {cyan}║{reset}");
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  {bold}Scanner{reset}                                                        {cyan}║{reset}");
    println!("{cyan}║{reset}    Scans performed: {:8}                                     {cyan}║{reset}", runner.scans_performed);
    println!("{cyan}║{reset}    Opportunities detected: {:8}                             {cyan}║{reset}", runner.opportunities_detected);
    if let Some(best) = runner.best_spread {
        println!("{cyan}║{reset}    Best spread seen: ${:.4}                                     {cyan}║{reset}", best);
    }
    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  {bold}Executor{reset}                                                       {cyan}║{reset}");
    println!("{cyan}║{reset}    Opportunities received: {:8}                             {cyan}║{reset}", auto.opportunities_received);
    println!("{cyan}║{reset}    Opportunities skipped: {:8}                              {cyan}║{reset}", auto.opportunities_skipped);
    println!("{cyan}║{reset}    Both legs filled: {:8}                                   {cyan}║{reset}", auto.both_filled);
    println!("{cyan}║{reset}    Partial fills: {:8}                                      {cyan}║{reset}", auto.partial_fills);
    println!("{cyan}║{reset}    Both rejected: {:8}                                      {cyan}║{reset}", auto.both_rejected);
    println!("{cyan}║{reset}    Total volume: ${:12}                                  {cyan}║{reset}", auto.total_volume);

    if auto.executions_attempted > 0 {
        let fill_rate = auto.both_filled as f64 / auto.executions_attempted as f64 * 100.0;
        println!("{cyan}║{reset}    Fill rate: {:5.1}%                                            {cyan}║{reset}", fill_rate);
    }

    println!("{cyan}╠══════════════════════════════════════════════════════════════════╣{reset}");
    println!("{cyan}║{reset}  {bold}Settlement{reset}                                                     {cyan}║{reset}");
    println!("{cyan}║{reset}    Pending: {:8}                                            {cyan}║{reset}", auto.pending_settlement);
    println!("{cyan}║{reset}    Wins: {green}{:8}{reset}   Losses: {red}{:8}{reset}   Double Wins: {:8}   {cyan}║{reset}",
        auto.settled_wins, auto.settled_losses, auto.double_wins);

    let pnl_color = if auto.realized_pnl > Decimal::ZERO { green } else if auto.realized_pnl < Decimal::ZERO { red } else { reset };
    println!("{cyan}║{reset}    Realized P&L: {pnl_color}${:12}{reset}                                 {cyan}║{reset}", auto.realized_pnl);

    if auto.settled_wins + auto.settled_losses > 0 {
        let win_rate = auto.settled_wins as f64 / (auto.settled_wins + auto.settled_losses) as f64 * 100.0;
        println!("{cyan}║{reset}    Win Rate: {:5.1}%                                             {cyan}║{reset}", win_rate);
    }

    println!("{cyan}╚══════════════════════════════════════════════════════════════════╝{reset}");
    println!();
}
