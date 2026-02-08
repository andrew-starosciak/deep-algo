//! CLI command for automated cross-market arbitrage trading.
//!
//! This command runs the full automated trading system, connecting the
//! `CrossMarketRunner` (signal detection) to `CrossMarketAutoExecutor` (order execution).

use algo_trade_polymarket::arbitrage::{
    CrossMarketAutoExecutor, CrossMarketAutoExecutorConfig, CrossMarketAutoExecutorStats,
    CrossMarketCombination, CrossMarketRunner, CrossMarketRunnerConfig, CrossMarketRunnerStats,
    EventKind, LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig,
    PendingTradeDisplay, PolymarketExecutor, RecentTradeDisplay,
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
    /// Observe mode: detects all opportunities with relaxed filters,
    /// tracks real market outcomes via settlement, but never risks capital.
    /// Used to measure true correlation rates, spread distributions,
    /// and both-lose probabilities before committing to live trading.
    Observe,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Paper => write!(f, "PAPER"),
            ExecutionMode::Live => write!(f, "LIVE"),
            ExecutionMode::Observe => write!(f, "OBSERVE"),
        }
    }
}

impl FromStr for ExecutionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "paper" => Ok(ExecutionMode::Paper),
            "live" => Ok(ExecutionMode::Live),
            "observe" => Ok(ExecutionMode::Observe),
            _ => Err(format!(
                "Invalid mode '{}'. Valid options: paper, live, observe",
                s
            )),
        }
    }
}

/// Arguments for the cross-market-auto command.
#[derive(Args, Debug)]
pub struct CrossMarketAutoArgs {
    /// Execution mode: paper (default), live, or observe.
    /// Observe mode tracks all opportunities and real outcomes without trading.
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

    /// Minimum spread required to execute. Default: 0.20 (20%).
    #[arg(long, default_value = "0.20")]
    pub min_spread: f64,

    /// Minimum win probability required. Default: 0.85 (85%).
    #[arg(long, default_value = "0.85")]
    pub min_win_prob: f64,

    /// Maximum implied loss probability (divergence filter). Default: 0.50 (50%).
    /// Rejects trades where (1-p1)*(1-p2) exceeds this — both prices too low means
    /// the big spread is a trap (high chance neither leg wins).
    #[arg(long, default_value = "0.50")]
    pub max_loss_prob: f64,

    /// Entry window start: minutes before window close to START trading. Default: 10.
    /// Data shows 8-10 min before close is the optimal entry zone.
    #[arg(long, default_value = "10")]
    pub entry_start_mins: i64,

    /// Entry window end: minutes before window close to STOP trading. Default: 4.
    /// BTC/ETH maintains 87%+ win rate down to 4 min; drop-off starts at 2 min.
    #[arg(long, default_value = "4")]
    pub entry_end_mins: i64,

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
        if !(0.0..=1.0).contains(&self.max_loss_prob) {
            anyhow::bail!("--max-loss-prob must be between 0.0 and 1.0");
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
    auto_config.max_loss_prob = args.max_loss_prob;
    auto_config.entry_window_start_secs = args.entry_start_mins * 60;
    auto_config.entry_window_end_secs = args.entry_end_mins * 60;
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
        ExecutionMode::Observe => {
            // Observe mode: 100% fill rate, infinite balance, relaxed filters.
            // Captures ALL opportunities and their real market outcomes.
            let paper_config = PaperExecutorConfig {
                initial_balance: Decimal::from(1_000_000),
                fill_rate: 1.0,
                ..Default::default()
            };
            let executor = PaperExecutor::new(paper_config);

            // Relax ALL filters to capture maximum data points
            auto_config.min_spread = Decimal::from_str("0.01").unwrap_or_default();
            auto_config.min_win_probability = 0.01;
            auto_config.max_loss_prob = 0.99;
            auto_config.max_position_per_window = Decimal::from(1_000_000);
            auto_config.max_trades_per_window = 100;
            auto_config.fixed_bet_size = Some(Decimal::ONE); // $1 per leg (nominal)
            auto_config.entry_window_start_secs = 870; // Observe from 14:30 onward
            auto_config.entry_window_end_secs = 30; // Observe almost to window end
            auto_config.filter_combination = None; // Observe ALL combinations
            auto_config.observe_mode = true; // Persist ALL detected opportunities

            let dashboard_config = DashboardConfig {
                mode,
                pair: pair_str.clone(),
                combination: combo_str.clone(),
                bet_size: "OBSERVE".to_string(),
                kelly_fraction: 0.0,
                min_spread: 0.01,
                max_position: auto_config.max_position_per_window,
                balance: Decimal::from(1_000_000),
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
            // Copy live prices and snapshots from runner to executor
            {
                let runner = runner_stats.read().await;
                let mut auto = auto_stats.write().await;
                auto.live_prices = runner.current_prices.clone();
                auto.live_snapshots = runner.current_snapshots.clone();
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
    let magenta = "\x1b[95m";

    let mode_color = match config.mode {
        ExecutionMode::Live => red,
        ExecutionMode::Observe => cyan,
        ExecutionMode::Paper => yellow,
    };

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

    // Fill rate
    let fill_rate = if auto.executions_attempted > 0 {
        (auto.both_filled as f64 / auto.executions_attempted as f64) * 100.0
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

    // Latency display
    let latency_str = if auto.latency_samples > 0 {
        format!("{}ms avg", auto.avg_latency_ms)
    } else {
        "-".to_string()
    };

    // Move cursor to top
    print!("\x1b[H");

    // Header
    println!("{clear_line}");
    println!("{clear_line}{bold}{cyan}  CROSS-MARKET CORRELATION ARBITRAGE{reset}  {mode_color}[{}]{reset}", config.mode);
    println!("{clear_line}{dim}  ────────────────────────────────────────────────────────────────{reset}");

    // Compact config + systems line
    let status_text = if shutting_down { format!("{yellow}STOP{reset}") } else { format!("{green}RUN{reset}") };
    println!("{clear_line}  {dim}Pair:{reset} {:7} {dim}Bet:{reset} {:7} {dim}Spread:{reset} >${:.2}  {dim}Latency:{reset} {:8}  [{status_text}]",
        config.pair, config.bet_size, config.min_spread, latency_str);
    println!("{clear_line}  {scanner_dot}●{reset}Scan {executor_dot}●{reset}Exec {ws_dot}●{reset}WS   {dim}Max:{reset} ${:.0}/win  {dim}Bal:{reset} ${:.2}",
        config.max_position, config.balance);
    println!("{clear_line}");

    // Live prices - all 4 coins in a 2x2 grid
    let all_coins = ["BTC", "ETH", "SOL", "XRP"];
    println!("{clear_line}  {bold}Prices{reset}");
    for row in all_coins.chunks(2) {
        print!("{clear_line}  ");
        for coin in row {
            if let Some((up, down)) = runner.current_prices.get(*coin) {
                print!("  {:>3}  {green}▲{:.2}{reset}  {red}▼{:.2}{reset}      ", coin, up, down);
            } else {
                print!("  {:>3}  {dim}waiting...{reset}          ", coin);
            }
        }
        println!();
    }
    println!("{clear_line}");

    // Pairings table - all 6 pairs with spreads for each combination
    let spread_ok = Decimal::from_str("0.03").unwrap_or_default();
    println!("{clear_line}  {bold}Spreads{reset}        {dim}↓↑      ↑↓      ↑↑      ↓↓{reset}");
    let pairs: &[(&str, &str)] = &[
        ("BTC", "ETH"), ("BTC", "SOL"), ("BTC", "XRP"),
        ("ETH", "SOL"), ("ETH", "XRP"), ("SOL", "XRP"),
    ];
    for (c1, c2) in pairs {
        let p1 = runner.current_prices.get(*c1);
        let p2 = runner.current_prices.get(*c2);
        if let (Some((u1, d1)), Some((u2, d2))) = (p1, p2) {
            let spreads = [
                Decimal::ONE - (*d1 + *u2),  // ↓↑: coin1 down + coin2 up
                Decimal::ONE - (*u1 + *d2),  // ↑↓: coin1 up + coin2 down
                Decimal::ONE - (*u1 + *u2),  // ↑↑: both up
                Decimal::ONE - (*d1 + *d2),  // ↓↓: both down
            ];
            print!("{clear_line}    {c1}/{c2}    ");
            for s in &spreads {
                let color = if *s >= spread_ok { green }
                           else if *s > Decimal::ZERO { yellow }
                           else { dim };
                let sign = if *s > Decimal::ZERO { "+" } else { "" };
                print!("{color}{sign}{:.3}{reset}   ", s);
            }
            println!();
        } else {
            println!("{clear_line}    {c1}/{c2}    {dim}...{reset}");
        }
    }
    println!("{clear_line}");

    // ── Trading Stats ──
    println!("{clear_line}  {bold}Trading{reset}                              {bold}Performance{reset}");
    println!("{clear_line}    Scans: {:<6} Opps: {:<5} Exec: {:<4}    Wins: {green}{}{reset}  Losses: {red}{}{reset}  Rate: {:.0}%",
        runner.scans_performed, runner.opportunities_detected, auto.both_filled,
        auto.settled_wins, auto.settled_losses, win_rate);
    println!("{clear_line}    Partial: {:<4} Reject: {:<4} Fill: {:.0}%    P&L: {pnl_color}${:.2}{reset}  Vol: ${:.2}",
        auto.partial_fills, auto.both_rejected, fill_rate,
        auto.realized_pnl, auto.total_volume);

    // Extra stats row (trims, early exits, recovery)
    let extras: Vec<String> = [
        (auto.trim_count > 0).then(|| format!("Trims:{}", auto.trim_count)),
        (auto.early_exits > 0).then(|| format!("EarlyExits:{}", auto.early_exits)),
        (auto.incomplete_recovered > 0).then(|| format!("Recovered:{}", auto.incomplete_recovered)),
        (auto.incomplete_escaped > 0).then(|| format!("Escaped:{}", auto.incomplete_escaped)),
        (auto.pending_settlement > 0).then(|| format!("Pending:{}", auto.pending_settlement)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !extras.is_empty() {
        println!("{clear_line}    {dim}{}{reset}", extras.join("  "));
    }
    println!("{clear_line}");

    // ── Holdings (positions we're currently holding) ──
    if !auto.pending_trades.is_empty() {
        let mut total_cost = Decimal::ZERO;
        let mut total_value = Decimal::ZERO;
        let mut total_proceeds = Decimal::ZERO;

        println!("{clear_line}  {bold}Holdings{reset}");
        println!("{clear_line}    {dim}Pair        Leg1    Now     Shares   Leg2    Now     Shares   Cost    Value   P&L     Timer{reset}");
        for p in auto.pending_trades.iter().take(5) {
            let remaining_secs = (p.window_end - chrono::Utc::now()).num_seconds().max(0);
            let status = if p.partially_exited {
                format!("{magenta}exiting{reset}")
            } else if remaining_secs > 0 {
                let mins = remaining_secs / 60;
                let secs = remaining_secs % 60;
                format!("{yellow}{mins}:{secs:02}{reset}")
            } else {
                format!("{green}settle{reset}")
            };

            let l1_dir_char = p.leg1_dir.chars().next().unwrap_or('-');
            let l2_dir_char = p.leg2_dir.chars().next().unwrap_or('-');

            // Look up current bid prices from live feed
            let c1_prices = runner.current_prices.get(p.coin1.as_str());
            let c2_prices = runner.current_prices.get(p.coin2.as_str());

            // Get current price for each leg based on direction
            let leg1_now = c1_prices.map(|(up, down)| {
                if p.leg1_dir == "UP" { *up } else { *down }
            });
            let leg2_now = c2_prices.map(|(up, down)| {
                if p.leg2_dir == "UP" { *up } else { *down }
            });

            // Calculate current value = shares * current_price for each leg
            let leg1_val = leg1_now.map(|px| p.shares_leg1 * px).unwrap_or(Decimal::ZERO);
            let leg2_val = leg2_now.map(|px| p.shares_leg2 * px).unwrap_or(Decimal::ZERO);
            let current_value = leg1_val + leg2_val + p.early_exit_proceeds;

            // Cost = entry * shares for each leg
            let cost = p.entry_price_leg1 * p.shares_leg1 + p.entry_price_leg2 * p.shares_leg2;
            let pnl = current_value - cost;

            total_cost += cost;
            total_value += current_value;
            total_proceeds += p.early_exit_proceeds;

            let pnl_c = if pnl > Decimal::ZERO { green } else if pnl < Decimal::ZERO { red } else { reset };
            let l1_now_str = leg1_now.map(|p| format!("{:.2}", p)).unwrap_or_else(|| "  - ".to_string());
            let l2_now_str = leg2_now.map(|p| format!("{:.2}", p)).unwrap_or_else(|| "  - ".to_string());

            println!(
                "{clear_line}    {:<10}  {l1_dir_char}@{:.2}  {l1_now_str}  {:>5.1}sh   {l2_dir_char}@{:.2}  {l2_now_str}  {:>5.1}sh   ${:<5.2}  ${:<5.2}  {pnl_c}{:+.2}{reset}  {status}",
                p.pair, p.entry_price_leg1, p.shares_leg1,
                p.entry_price_leg2, p.shares_leg2,
                cost, current_value, pnl,
            );
        }
        if auto.pending_trades.len() > 5 {
            println!("{clear_line}    {dim}... and {} more{reset}", auto.pending_trades.len() - 5);
        }

        // Totals row
        let total_pnl = total_value - total_cost;
        let total_pnl_c = if total_pnl > Decimal::ZERO { green } else if total_pnl < Decimal::ZERO { red } else { reset };
        println!("{clear_line}    {dim}─────────────────────────────────────────────────────────────────────────────────{reset}");
        println!("{clear_line}    {bold}Total{reset}                                                     ${:<5.2}  ${:<5.2}  {total_pnl_c}{bold}{:+.2}{reset}",
            total_cost, total_value, total_pnl);
        println!("{clear_line}");
    }

    // ── Event Log (last N key events) ──
    if !auto.event_log.is_empty() {
        println!("{clear_line}  {bold}Events{reset}");
        for event in auto.event_log.iter().rev().take(8) {
            let local_time: chrono::DateTime<chrono::Local> = event.time.into();
            let time_str = local_time.format("%H:%M:%S");
            let color = match event.kind {
                EventKind::Fill => green,
                EventKind::PartialFill => yellow,
                EventKind::Reject => dim,
                EventKind::Trim => magenta,
                EventKind::EarlyExit => cyan,
                EventKind::Settlement => if event.message.contains("WIN") { green } else { red },
                EventKind::Recovery => yellow,
                EventKind::Error => red,
            };
            println!("{clear_line}    {dim}{time_str}{reset} {color}{}{reset}", event.message);
        }
        println!("{clear_line}");
    }

    // ── Progress bars ──
    let (window_pct, window_remaining) = get_window_progress();
    let window_bar_width = 15;
    let window_filled = (window_pct / 100.0 * window_bar_width as f64) as usize;
    let window_empty = window_bar_width - window_filled;
    let window_bar = format!("{}{}", "█".repeat(window_filled), "░".repeat(window_empty));
    let window_mins = window_remaining / 60;
    let window_secs = window_remaining % 60;

    let bar_width = 20;
    let filled = (progress_pct / 100.0 * bar_width as f64) as usize;
    let empty = bar_width - filled;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    println!("{clear_line}  {dim}Window:{reset} [{yellow}{window_bar}{reset}] {window_mins}:{window_secs:02}   {dim}Session:{reset} [{cyan}{bar}{reset}] {remaining_str}");
    println!("{clear_line}  {dim}{}  |  Ctrl+C to stop{reset}",
        Local::now().format("%H:%M:%S"));

    // Clear any leftover lines from previous render
    for _ in 0..10 {
        println!("{clear_line}");
    }

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
    if auto.incomplete_trades > 0 || auto.incomplete_recovered > 0 || auto.incomplete_escaped > 0 {
        println!("    Recovery: {} pending, {} recovered, {} expired, {} escaped",
            auto.incomplete_trades, auto.incomplete_recovered,
            auto.incomplete_expired, auto.incomplete_escaped);
    }
    if auto.executions_attempted > 0 {
        let fill_rate = auto.both_filled as f64 / auto.executions_attempted as f64 * 100.0;
        println!("    Fill rate: {:.1}%", fill_rate);
    }
    if auto.latency_samples > 0 {
        println!("    Avg latency: {}ms ({} samples)", auto.avg_latency_ms, auto.latency_samples);
    }
    if auto.trim_count > 0 {
        println!("    Trims: {} ({:.1} shares trimmed)", auto.trim_count, auto.trim_shares);
    }
    if auto.early_exits > 0 {
        println!("    Early exits: {} (${:.2} proceeds)", auto.early_exits, auto.early_exit_proceeds);
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
    // Show last few events
    if !auto.event_log.is_empty() {
        println!("  {bold}Last Events{reset}");
        for event in auto.event_log.iter().rev().take(5) {
            let local_time: chrono::DateTime<chrono::Local> = event.time.into();
            println!("    {dim}{}{reset} {}", local_time.format("%H:%M:%S"), event.message);
        }
        println!();
    }
    println!("  {dim}Full logs available via: RUST_LOG=info (see script --verbose flag){reset}");
    println!();
}
