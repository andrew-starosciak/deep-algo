//! CLI command for automated cross-market arbitrage trading.
//!
//! This command runs the full automated trading system, connecting the
//! `CrossMarketRunner` (signal detection) to `CrossMarketAutoExecutor` (order execution).

use algo_trade_polymarket::arbitrage::{
    CrossMarketAutoExecutor, CrossMarketAutoExecutorConfig, CrossMarketAutoExecutorStats,
    CrossMarketCombination, CrossMarketRunner, CrossMarketRunnerConfig, CrossMarketRunnerStats,
    LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig, PendingTradeDisplay,
    PolymarketExecutor, RecentTradeDisplay,
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

    /// Maximum position per window in USDC (paper default: 200, live default: 10).
    #[arg(long)]
    pub max_position: Option<f64>,

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
    max_position: Decimal,
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

    // Build auto executor config.
    // Live mode uses micro_testing() as safe base; paper uses Default.
    let mut auto_config = if mode == ExecutionMode::Live {
        CrossMarketAutoExecutorConfig::micro_testing()
    } else {
        CrossMarketAutoExecutorConfig::default()
    };

    // Apply CLI overrides
    auto_config.filter_pair = pair.clone();
    auto_config.filter_combination = combination;
    auto_config.kelly_fraction = args.kelly_fraction;
    auto_config.min_spread = Decimal::from_str(&format!("{:.4}", args.min_spread))?;
    auto_config.min_win_probability = args.min_win_prob;
    if let Some(max_pos) = args.max_position {
        auto_config.max_position_per_window = Decimal::from_str(&format!("{:.2}", max_pos))?;
    }
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
                max_position: auto_config.max_position_per_window,
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
            let mut live_config = LiveExecutorConfig::micro_testing();
            // Set taker fee to match the 15-min crypto market's taker_base_fee
            live_config.clob_config.taker_fee_bps = 1000;
            let mut executor = LiveExecutor::new(live_config).await?;
            executor.authenticate().await?;
            let balance = executor.get_balance().await?;

            let dashboard_config = DashboardConfig {
                mode,
                pair: pair_str,
                combination: combo_str,
                bet_size: bet_str,
                kelly_fraction: args.kelly_fraction,
                min_spread: args.min_spread,
                max_position: auto_config.max_position_per_window,
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
            // Copy live prices from runner to executor for settlement
            {
                let runner = runner_stats.read().await;
                let mut auto = auto_stats.write().await;
                auto.live_prices = runner.current_prices.clone();
            }

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
    print_summary(&runner_stats, &auto_stats).await;

    Ok(())
}

/// Prints the launch banner (shown once at startup).
fn print_launch_banner(_config: &DashboardConfig) {
    // Clear screen
    print!("\x1b[2J\x1b[H");
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
    let clear_line = "\x1b[2K";

    let mode_color = if config.mode == ExecutionMode::Live { red } else { yellow };

    // Status indicators
    let scanner_dot = if scanner_ready { green } else { dim };
    let executor_dot = if executor_ready { green } else { dim };
    let ws_dot = if runner.scans_performed > 0 { green } else { yellow };

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

    // Latency display (only available after trades execute)
    let latency_str = if auto.latency_samples > 0 {
        format!("{}ms (avg {}ms)", auto.last_latency_ms, auto.avg_latency_ms)
    } else {
        "waiting for trade".to_string()
    };

    // Get current prices for the pair
    let (coin1, coin2) = config.pair.split_once('/').unwrap_or(("BTC", "ETH"));
    let c1_prices = runner.current_prices.get(coin1);
    let c2_prices = runner.current_prices.get(coin2);

    // Move cursor to top
    print!("\x1b[H");

    // Clear and print each line
    println!("{clear_line}");
    println!("{clear_line}{bold}{cyan}  CROSS-MARKET CORRELATION ARBITRAGE{reset}");
    println!("{clear_line}");

    // Config line
    println!("{clear_line}  {dim}Mode:{reset} {mode_color}{:6}{reset}   {dim}Pair:{reset} {:8}   {dim}Strategy:{reset} {}",
        config.mode, config.pair, config.combination);
    println!("{clear_line}  {dim}Bet:{reset}  {:7}  {dim}Spread:{reset} ${:.2}   {dim}Max/Window:{reset} ${:.0}   {dim}Balance:{reset} ${:.2}",
        config.bet_size, config.min_spread, config.max_position, config.balance);
    println!("{clear_line}");

    // Systems status
    let status_text = if shutting_down { format!("{yellow}Stopping{reset}") } else { format!("{green}Running{reset}") };
    println!("{clear_line}  {bold}Systems{reset}  {scanner_dot}●{reset} Scanner  {executor_dot}●{reset} Executor  {ws_dot}●{reset} WebSocket  [{status_text}]");
    println!("{clear_line}  {dim}Latency:{reset} {latency_str}");
    println!("{clear_line}");

    // Live prices
    println!("{clear_line}  {bold}Live Prices{reset}");
    if let Some((up, down)) = c1_prices {
        println!("{clear_line}    {coin1}:  {green}UP ${:.2}{reset}  {red}DOWN ${:.2}{reset}", up, down);
    } else {
        println!("{clear_line}    {coin1}:  {dim}waiting...{reset}");
    }
    if let Some((up, down)) = c2_prices {
        println!("{clear_line}    {coin2}:  {green}UP ${:.2}{reset}  {red}DOWN ${:.2}{reset}", up, down);
    } else {
        println!("{clear_line}    {coin2}:  {dim}waiting...{reset}");
    }
    // Show combined cost for the strategy based on combination
    let strategy_cost = match config.combination.as_str() {
        "Coin1↓ Coin2↑" => c1_prices.zip(c2_prices).map(|((_, c1_down), (c2_up, _))| (c1_down + c2_up, format!("{coin1}↓ + {coin2}↑"))),
        "Coin1↑ Coin2↓" => c1_prices.zip(c2_prices).map(|((c1_up, _), (_, c2_down))| (c1_up + c2_down, format!("{coin1}↑ + {coin2}↓"))),
        "Both↑" => c1_prices.zip(c2_prices).map(|((c1_up, _), (c2_up, _))| (c1_up + c2_up, format!("{coin1}↑ + {coin2}↑"))),
        "Both↓" => c1_prices.zip(c2_prices).map(|((_, c1_down), (_, c2_down))| (c1_down + c2_down, format!("{coin1}↓ + {coin2}↓"))),
        _ => c1_prices.zip(c2_prices).map(|((_, c1_down), (c2_up, _))| (c1_down + c2_up, format!("{coin1}↓ + {coin2}↑"))),
    };
    if let Some((total, combo_str)) = strategy_cost {
        let spread = Decimal::ONE - total;
        let spread_color = if spread >= Decimal::from_str("0.03").unwrap_or_default() { green } else { yellow };
        println!("{clear_line}    {dim}Strategy:{reset} {combo_str} = ${:.2}  {dim}Spread:{reset} {spread_color}${:.2}{reset}", total, spread);
    }
    println!("{clear_line}");

    // Trading stats
    println!("{clear_line}  {bold}Trading{reset}");
    println!("{clear_line}    Scans: {:<6}  Opportunities: {:<6}  Executed: {}",
        runner.scans_performed, runner.opportunities_detected, auto.both_filled);
    println!("{clear_line}    Pending: {:<4}  Settled: {:<4}  Skipped: {:<4}  Rejected: {}",
        auto.pending_settlement, total_settled, auto.opportunities_skipped, auto.both_rejected);
    println!("{clear_line}");

    // Performance
    println!("{clear_line}  {bold}Performance{reset}");
    println!("{clear_line}    Wins: {green}{}{reset}  Losses: {red}{}{reset}  Win Rate: {:.1}%",
        auto.settled_wins, auto.settled_losses, win_rate);
    println!("{clear_line}    P&L: {pnl_color}${:.2}{reset}  Volume: ${:.2}",
        auto.realized_pnl, auto.total_volume);
    println!("{clear_line}");

    // Trades this window
    if !auto.recent_trades.is_empty() || !auto.pending_trades.is_empty() {
        println!("{clear_line}  {bold}Trades This Window{reset}");
        for trade in auto.recent_trades.iter().rev().take(3) {
            // Convert to local time for display
            let local_time: chrono::DateTime<chrono::Local> = trade.executed_at.into();
            println!("{clear_line}    {dim}[{}]{reset} {}: {}${:.2} + {}${:.2} = ${:.2}",
                local_time.format("%H:%M:%S"),
                trade.pair,
                trade.leg1_dir.chars().next().unwrap_or('-'), trade.leg1_price,
                trade.leg2_dir.chars().next().unwrap_or('-'), trade.leg2_price,
                trade.total_cost);
        }
        for pending in auto.pending_trades.iter().take(2) {
            let remaining = (pending.window_end - chrono::Utc::now()).num_seconds().max(0);
            let status = if remaining > 0 {
                format!("{yellow}settles in {}s{reset}", remaining)
            } else {
                format!("{green}settling...{reset}")
            };
            println!("{clear_line}    {dim}[pending]{reset} {}: {}+{} ${:.2} {status}",
                pending.pair, pending.leg1_dir.chars().next().unwrap_or('-'),
                pending.leg2_dir.chars().next().unwrap_or('-'), pending.total_cost);
        }
        println!("{clear_line}");
    }

    // Window progress (15-minute market window)
    let (window_pct, window_remaining) = get_window_progress();
    let window_bar_width = 15;
    let window_filled = (window_pct / 100.0 * window_bar_width as f64) as usize;
    let window_empty = window_bar_width - window_filled;
    let window_bar = format!("{}{}", "█".repeat(window_filled), "░".repeat(window_empty));
    let window_mins = window_remaining / 60;
    let window_secs = window_remaining % 60;

    // Session progress bar
    let bar_width = 20;
    let filled = (progress_pct / 100.0 * bar_width as f64) as usize;
    let empty = bar_width - filled;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    println!("{clear_line}  {dim}Window:{reset}  [{yellow}{window_bar}{reset}] {window_mins}:{window_secs:02} left   {dim}Session:{reset} [{cyan}{bar}{reset}] {remaining_str} left");
    println!("{clear_line}");
    println!("{clear_line}  {dim}Time: {}  |  Ctrl+C to stop  |  Logs: /tmp/cross_market_auto.log{reset}",
        Local::now().format("%H:%M:%S"));

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

/// Calculate the current 15-minute window progress.
/// Returns (progress_percent, seconds_remaining).
fn get_window_progress() -> (f64, u64) {
    let now = chrono::Utc::now().timestamp();
    let window_start = (now / 900) * 900;
    let elapsed_in_window = now - window_start;
    let progress = elapsed_in_window as f64 / 900.0 * 100.0;
    let remaining = 900 - elapsed_in_window as u64;
    (progress, remaining)
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
    let dim = "\x1b[2m";

    // Clear screen and print summary
    print!("\x1b[2J\x1b[H");

    println!();
    println!("  {bold}{cyan}SESSION COMPLETE{reset}");
    println!();
    println!("  {bold}Scanner{reset}");
    println!("    Scans: {}  Opportunities: {}", runner.scans_performed, runner.opportunities_detected);
    if let Some(best) = runner.best_spread {
        println!("    Best spread: ${:.4}", best);
    }
    println!();
    println!("  {bold}Executor{reset}");
    println!("    Received: {}  Skipped: {}  Executed: {}",
        auto.opportunities_received, auto.opportunities_skipped, auto.both_filled);
    println!("    Partial: {}  Rejected: {}  Volume: ${:.2}",
        auto.partial_fills, auto.both_rejected, auto.total_volume);
    if auto.incomplete_trades > 0 || auto.incomplete_recovered > 0 {
        println!("    Incomplete: {} pending, {} recovered, {} expired",
            auto.incomplete_trades, auto.incomplete_recovered, auto.incomplete_expired);
    }
    if auto.executions_attempted > 0 {
        let fill_rate = auto.both_filled as f64 / auto.executions_attempted as f64 * 100.0;
        println!("    Fill rate: {:.1}%", fill_rate);
    }
    if auto.latency_samples > 0 {
        println!("    Avg latency: {}ms ({} samples)", auto.avg_latency_ms, auto.latency_samples);
    }
    println!();
    println!("  {bold}Settlement{reset}");
    println!("    Pending: {}  Wins: {green}{}{reset}  Losses: {red}{}{reset}  Double: {}",
        auto.pending_settlement, auto.settled_wins, auto.settled_losses, auto.double_wins);

    let pnl_color = if auto.realized_pnl > Decimal::ZERO { green } else if auto.realized_pnl < Decimal::ZERO { red } else { reset };
    println!("    {bold}P&L: {pnl_color}${:.2}{reset}", auto.realized_pnl);

    if auto.settled_wins + auto.settled_losses > 0 {
        let win_rate = auto.settled_wins as f64 / (auto.settled_wins + auto.settled_losses) as f64 * 100.0;
        println!("    Win Rate: {:.1}%", win_rate);
    }
    println!();
    println!("  {dim}Full logs: /tmp/cross_market_auto.log{reset}");
    println!();
}
