//! Directional executor: bridges signals to execution with Kelly sizing,
//! settlement tracking, and a live dashboard.
//!
//! # Architecture
//!
//! ```text
//! DirectionalSignal (from runner)
//!         │
//!         ▼
//! DirectionalExecutor
//! ├── Observe mode: log only
//! ├── Check position limits
//! ├── Kelly size → FOK order
//! ├── Track pending settlements
//! └── Render dashboard (or log in verbose mode)
//! ```

use crate::arbitrage::directional_detector::{Direction, DirectionalSignal};
use crate::arbitrage::directional_runner::DirectionalRunnerStats;
use crate::arbitrage::execution::{
    ExecutionError, OrderParams, OrderStatus, PolymarketExecutor,
};
use crate::GammaClient;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

// =============================================================================
// Errors
// =============================================================================

/// Errors from the directional executor.
#[derive(Error, Debug)]
pub enum DirectionalExecutorError {
    /// Execution error from underlying executor.
    #[error("Execution error: {0}")]
    Execution(#[from] ExecutionError),

    /// Signal channel closed.
    #[error("Signal channel closed")]
    ChannelClosed,

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the directional executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalExecutorConfig {
    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    pub kelly_fraction: f64,

    /// Fixed bet size in USDC (overrides Kelly if set).
    pub fixed_bet_size: Option<Decimal>,

    /// Minimum bet size in USDC.
    pub min_bet_size: Decimal,

    /// Maximum bet size in USDC.
    pub max_bet_size: Decimal,

    /// Minimum edge required to execute (0.0 to 1.0).
    pub min_edge: f64,

    /// Maximum position (cost) per 15-minute window.
    pub max_position_per_window: Decimal,

    /// Maximum number of trades per 15-minute window.
    pub max_trades_per_window: u32,

    /// Observe mode: log signals but don't execute.
    pub observe_mode: bool,

    /// Fee rate on winning trades (e.g., 0.02 = 2%).
    pub fee_rate: Decimal,

    /// Dashboard refresh interval in seconds.
    pub stats_interval_secs: u64,

    /// Settlement check interval in seconds.
    pub settlement_interval_secs: u64,
}

impl Default for DirectionalExecutorConfig {
    fn default() -> Self {
        Self {
            kelly_fraction: 0.25,
            fixed_bet_size: None,
            min_bet_size: dec!(5),
            max_bet_size: dec!(100),
            min_edge: 0.03,
            max_position_per_window: dec!(200),
            max_trades_per_window: 1,
            observe_mode: false,
            fee_rate: dec!(0.02),
            stats_interval_secs: 5,
            settlement_interval_secs: 30,
        }
    }
}

// =============================================================================
// Trade Record
// =============================================================================

/// A record of an executed directional trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalTradeRecord {
    /// Unique trade ID.
    pub trade_id: String,
    /// Coin traded.
    pub coin: String,
    /// Direction.
    pub direction: Direction,
    /// Token ID bought.
    pub token_id: String,
    /// Entry price paid per share.
    pub entry_price: Decimal,
    /// Number of shares bought.
    pub shares: Decimal,
    /// Total cost (entry_price * shares).
    pub cost: Decimal,
    /// Order status.
    pub status: OrderStatus,
    /// Estimated edge at signal time.
    pub estimated_edge: f64,
    /// Win probability at signal time.
    pub win_probability: f64,
    /// Spot delta at signal time.
    pub delta_pct: f64,
    /// Signal timestamp.
    pub signal_timestamp: DateTime<Utc>,
    /// Execution timestamp.
    pub execution_timestamp: DateTime<Utc>,
    /// Settlement result (None if pending).
    pub settlement: Option<SettlementResult>,
}

/// Result of a settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// Whether the trade won.
    pub won: bool,
    /// P&L for this trade.
    pub pnl: Decimal,
    /// Settlement timestamp.
    pub settled_at: DateTime<Utc>,
}

// =============================================================================
// Window Position Tracking
// =============================================================================

/// Tracks position limits per 15-minute window.
#[derive(Debug, Clone, Default)]
struct WindowTracker {
    /// Window start time (ms).
    window_start_ms: i64,
    /// Total cost committed this window.
    total_cost: Decimal,
    /// Number of trades this window.
    trade_count: u32,
}

impl WindowTracker {
    /// Resets for a new window.
    fn reset(&mut self, window_start_ms: i64) {
        self.window_start_ms = window_start_ms;
        self.total_cost = Decimal::ZERO;
        self.trade_count = 0;
    }

    /// Records a trade.
    fn record_trade(&mut self, cost: Decimal) {
        self.total_cost += cost;
        self.trade_count += 1;
    }

    /// Checks if we can trade more this window.
    fn can_trade(&self, max_cost: Decimal, max_trades: u32) -> bool {
        self.total_cost < max_cost && self.trade_count < max_trades
    }
}

// =============================================================================
// Kelly Sizer (duplicated from auto_executor to avoid coupling)
// =============================================================================

/// Calculates position size using Kelly criterion for directional bets.
struct KellySizer {
    fraction: f64,
    min_size: Decimal,
    max_size: Decimal,
}

impl KellySizer {
    fn new(fraction: f64, min_size: Decimal, max_size: Decimal) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
            min_size,
            max_size,
        }
    }

    /// Calculates bet size.
    ///
    /// For directional bets: f* = (win_prob - price) / (1 - price) * kelly_fraction
    fn size(&self, win_prob: f64, price: Decimal, bankroll: Decimal) -> Option<Decimal> {
        let price_f64 = price.to_string().parse::<f64>().unwrap_or(0.5);

        if price_f64 <= 0.0 || price_f64 >= 1.0 {
            return None;
        }

        // Net odds: b = (1 - price) / price
        let b = (1.0 - price_f64) / price_f64;

        // Full Kelly: f* = (p(b+1) - 1) / b
        let full_kelly = (win_prob * (b + 1.0) - 1.0) / b;

        if full_kelly <= 0.0 {
            return None;
        }

        let kelly_fraction = full_kelly * self.fraction;
        let bankroll_f64 = bankroll.to_string().parse::<f64>().unwrap_or(0.0);
        let bet_f64 = bankroll_f64 * kelly_fraction;

        let bet = Decimal::from_f64_retain(bet_f64)?;

        if bet < self.min_size {
            return None;
        }

        Some(bet.min(self.max_size).min(bankroll))
    }
}

// =============================================================================
// Executor Statistics
// =============================================================================

/// Statistics for the directional executor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectionalExecutorStats {
    /// Total signals received.
    pub signals_received: u64,
    /// Signals skipped (observe mode, limits, etc.).
    pub signals_skipped: u64,
    /// Orders attempted.
    pub orders_attempted: u64,
    /// Orders filled.
    pub orders_filled: u64,
    /// Orders rejected/failed.
    pub orders_failed: u64,
    /// Total volume traded (USDC cost).
    pub total_volume: Decimal,
    /// Trades won.
    pub wins: u64,
    /// Trades lost.
    pub losses: u64,
    /// Trades pending settlement.
    pub pending_settlements: u64,
    /// Realized P&L from settled trades.
    pub realized_pnl: Decimal,
    /// Current bankroll.
    pub current_balance: Decimal,
    /// Start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Last trade time.
    pub last_trade_at: Option<DateTime<Utc>>,
}

impl DirectionalExecutorStats {
    /// Returns the win rate (0.0 to 1.0).
    pub fn win_rate(&self) -> f64 {
        let total = self.wins + self.losses;
        if total == 0 {
            return 0.0;
        }
        self.wins as f64 / total as f64
    }

    /// Returns the fill rate (0.0 to 1.0).
    pub fn fill_rate(&self) -> f64 {
        if self.orders_attempted == 0 {
            return 0.0;
        }
        self.orders_filled as f64 / self.orders_attempted as f64
    }
}

// =============================================================================
// Directional Executor
// =============================================================================

/// Directional trading executor.
///
/// Consumes signals from `DirectionalRunner`, sizes via Kelly criterion,
/// executes FOK orders, and tracks settlements.
pub struct DirectionalExecutor<E: PolymarketExecutor> {
    /// The underlying executor (paper or live).
    executor: E,
    /// Configuration.
    config: DirectionalExecutorConfig,
    /// Kelly position sizer.
    sizer: KellySizer,
    /// Window position tracker.
    window_tracker: WindowTracker,
    /// Execution statistics.
    stats: Arc<RwLock<DirectionalExecutorStats>>,
    /// Trade history.
    trades: VecDeque<DirectionalTradeRecord>,
    /// Pending settlements (trades waiting for window resolution).
    pending_settlements: Vec<DirectionalTradeRecord>,
    /// Gamma client for settlement resolution checks.
    gamma_client: GammaClient,
    /// Runner stats (for dashboard).
    runner_stats: Option<Arc<RwLock<DirectionalRunnerStats>>>,
    /// Stop flag.
    should_stop: Arc<AtomicBool>,
    /// Trade counter for ID generation.
    trade_counter: u64,
}

impl<E: PolymarketExecutor> DirectionalExecutor<E> {
    /// Creates a new directional executor.
    pub fn new(executor: E, config: DirectionalExecutorConfig) -> Self {
        let sizer = KellySizer::new(
            config.kelly_fraction,
            config.min_bet_size,
            config.max_bet_size,
        );

        Self {
            executor,
            config,
            sizer,
            window_tracker: WindowTracker::default(),
            stats: Arc::new(RwLock::new(DirectionalExecutorStats::default())),
            trades: VecDeque::with_capacity(100),
            pending_settlements: Vec::new(),
            gamma_client: GammaClient::new(),
            runner_stats: None,
            should_stop: Arc::new(AtomicBool::new(false)),
            trade_counter: 0,
        }
    }

    /// Sets the runner stats handle for dashboard display.
    pub fn set_runner_stats(&mut self, stats: Arc<RwLock<DirectionalRunnerStats>>) {
        self.runner_stats = Some(stats);
    }

    /// Returns the executor stats handle.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<DirectionalExecutorStats>> {
        Arc::clone(&self.stats)
    }

    /// Returns a handle to stop the executor.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.should_stop)
    }

    /// Runs the executor main loop.
    ///
    /// Consumes signals from the channel, executes orders, checks settlements,
    /// and renders the dashboard on a timer.
    pub async fn run(
        &mut self,
        mut signal_rx: mpsc::Receiver<DirectionalSignal>,
    ) -> Result<(), DirectionalExecutorError> {
        info!(
            observe = self.config.observe_mode,
            kelly = self.config.kelly_fraction,
            max_position = %self.config.max_position_per_window,
            max_trades = self.config.max_trades_per_window,
            "Starting directional executor"
        );

        // Initialize stats
        {
            let balance = self.executor.get_balance().await.unwrap_or(dec!(0));
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
            stats.current_balance = balance;
        }

        let mut settlement_tick =
            tokio::time::interval(Duration::from_secs(self.config.settlement_interval_secs));
        let mut dashboard_tick =
            tokio::time::interval(Duration::from_secs(self.config.stats_interval_secs));

        loop {
            tokio::select! {
                biased;

                // Handle incoming signal
                signal = signal_rx.recv() => {
                    match signal {
                        Some(sig) => self.handle_signal(sig).await,
                        None => {
                            info!("Signal channel closed, executor stopping");
                            break;
                        }
                    }
                }

                // Check settlements
                _ = settlement_tick.tick() => {
                    self.check_settlements().await;
                }

                // Render dashboard
                _ = dashboard_tick.tick() => {
                    self.render_dashboard().await;
                }
            }

            if self.should_stop.load(Ordering::SeqCst) {
                info!("Executor stopped via stop handle");
                break;
            }
        }

        // Final summary
        self.print_summary().await;
        Ok(())
    }

    /// Handles a directional signal.
    async fn handle_signal(&mut self, signal: DirectionalSignal) {
        {
            let mut stats = self.stats.write().await;
            stats.signals_received += 1;
        }

        // Observe mode: log only
        if self.config.observe_mode {
            info!(
                coin = signal.coin,
                direction = %signal.direction,
                edge = format!("{:.4}", signal.estimated_edge),
                entry_price = %signal.entry_price,
                confidence = format!("{:.3}", signal.confidence),
                win_prob = format!("{:.3}", signal.win_probability),
                delta = format!("{:+.4}%", signal.delta_pct * 100.0),
                time_left = format!("{}s", signal.time_remaining_secs),
                "OBSERVE: directional signal"
            );
            let mut stats = self.stats.write().await;
            stats.signals_skipped += 1;
            return;
        }

        // Check edge threshold
        if signal.estimated_edge < self.config.min_edge {
            let mut stats = self.stats.write().await;
            stats.signals_skipped += 1;
            return;
        }

        // Check window position limits
        // Detect window from signal timestamp
        let window_start_ms = crate::arbitrage::reference_tracker::ReferenceTracker::window_start_for_time(
            signal.timestamp.timestamp_millis(),
        );
        if self.window_tracker.window_start_ms != window_start_ms {
            self.window_tracker.reset(window_start_ms);
        }
        if !self.window_tracker.can_trade(
            self.config.max_position_per_window,
            self.config.max_trades_per_window,
        ) {
            let mut stats = self.stats.write().await;
            stats.signals_skipped += 1;
            return;
        }

        // Size the bet
        let balance = self
            .executor
            .get_balance()
            .await
            .unwrap_or(dec!(0));

        let bet_size = if let Some(fixed) = self.config.fixed_bet_size {
            if fixed > balance {
                warn!(
                    fixed = %fixed,
                    balance = %balance,
                    "Insufficient balance for fixed bet"
                );
                let mut stats = self.stats.write().await;
                stats.signals_skipped += 1;
                return;
            }
            fixed
        } else {
            match self.sizer.size(
                signal.win_probability,
                signal.entry_price,
                balance,
            ) {
                Some(size) => size,
                None => {
                    let mut stats = self.stats.write().await;
                    stats.signals_skipped += 1;
                    return;
                }
            }
        };

        // Calculate shares: shares = bet_size / entry_price
        let shares = if signal.entry_price > Decimal::ZERO {
            (bet_size / signal.entry_price).round_dp(2)
        } else {
            let mut stats = self.stats.write().await;
            stats.signals_skipped += 1;
            return;
        };

        if shares < dec!(1) {
            let mut stats = self.stats.write().await;
            stats.signals_skipped += 1;
            return;
        }

        // Submit FOK buy order
        let order = OrderParams::buy_fok(
            &signal.entry_token_id,
            signal.entry_price,
            shares,
        );

        info!(
            coin = signal.coin,
            direction = %signal.direction,
            price = %signal.entry_price,
            shares = %shares,
            cost = %bet_size,
            edge = format!("{:.4}", signal.estimated_edge),
            "Submitting directional order"
        );

        {
            let mut stats = self.stats.write().await;
            stats.orders_attempted += 1;
        }

        let result = match self.executor.submit_order(order).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "Order submission failed");
                let mut stats = self.stats.write().await;
                stats.orders_failed += 1;
                return;
            }
        };

        // Process result
        if result.status == OrderStatus::Filled || result.status == OrderStatus::PartiallyFilled {
            let actual_cost = result
                .avg_fill_price
                .unwrap_or(signal.entry_price)
                * result.filled_size;

            self.trade_counter += 1;
            let trade = DirectionalTradeRecord {
                trade_id: format!("dir-{}", self.trade_counter),
                coin: signal.coin.clone(),
                direction: signal.direction,
                token_id: signal.entry_token_id.clone(),
                entry_price: result.avg_fill_price.unwrap_or(signal.entry_price),
                shares: result.filled_size,
                cost: actual_cost,
                status: result.status,
                estimated_edge: signal.estimated_edge,
                win_probability: signal.win_probability,
                delta_pct: signal.delta_pct,
                signal_timestamp: signal.timestamp,
                execution_timestamp: Utc::now(),
                settlement: None,
            };

            info!(
                trade_id = trade.trade_id,
                coin = trade.coin,
                direction = %trade.direction,
                price = %trade.entry_price,
                shares = %trade.shares,
                cost = %trade.cost,
                "FILLED directional trade"
            );

            self.window_tracker.record_trade(actual_cost);
            self.pending_settlements.push(trade.clone());

            // Keep last 100 trades in history
            self.trades.push_back(trade);
            while self.trades.len() > 100 {
                self.trades.pop_front();
            }

            let mut stats = self.stats.write().await;
            stats.orders_filled += 1;
            stats.total_volume += actual_cost;
            stats.pending_settlements = self.pending_settlements.len() as u64;
            stats.current_balance = self
                .executor
                .get_balance()
                .await
                .unwrap_or(stats.current_balance);
            stats.last_trade_at = Some(Utc::now());
        } else {
            let mut stats = self.stats.write().await;
            stats.orders_failed += 1;
        }
    }

    /// Checks pending trades for settlement resolution.
    async fn check_settlements(&mut self) {
        if self.pending_settlements.is_empty() {
            return;
        }

        let now_ms = Utc::now().timestamp_millis();
        let mut settled_indices: Vec<usize> = Vec::new();

        for (i, trade) in self.pending_settlements.iter_mut().enumerate() {
            // Only check trades whose window has closed (15 min = 900_000 ms)
            let window_start = crate::arbitrage::reference_tracker::ReferenceTracker::window_start_for_time(
                trade.signal_timestamp.timestamp_millis(),
            );
            let window_end = window_start + 15 * 60 * 1000;

            // Wait at least 2 minutes after window close for resolution
            if now_ms < window_end + 120_000 {
                continue;
            }

            // Try to resolve via Gamma API
            let coin = crate::models::Coin::from_slug(&trade.coin);
            let coin = match coin {
                Some(c) => c,
                None => continue,
            };

            match self.gamma_client.get_current_15min_market(coin).await {
                Ok(market) => {
                    // Check if the market for the trade's window has resolved
                    let won = match trade.direction {
                        Direction::Up => {
                            market.up_token().and_then(|t| t.winner).unwrap_or(false)
                        }
                        Direction::Down => {
                            market.down_token().and_then(|t| t.winner).unwrap_or(false)
                        }
                    };

                    // Check if resolution is available (either token has winner set)
                    let resolved = market
                        .up_token()
                        .and_then(|t| t.winner)
                        .is_some()
                        || market
                            .down_token()
                            .and_then(|t| t.winner)
                            .is_some();

                    if !resolved {
                        continue; // Not yet resolved
                    }

                    // Calculate P&L
                    let pnl = if won {
                        // Win: receive 1.00 per share, minus entry cost and fee
                        let gross = (Decimal::ONE - trade.entry_price) * trade.shares;
                        gross * (Decimal::ONE - self.config.fee_rate)
                    } else {
                        // Loss: lose entire cost
                        -trade.cost
                    };

                    trade.settlement = Some(SettlementResult {
                        won,
                        pnl,
                        settled_at: Utc::now(),
                    });

                    info!(
                        trade_id = trade.trade_id,
                        coin = trade.coin,
                        direction = %trade.direction,
                        won = won,
                        pnl = %pnl,
                        "SETTLED directional trade"
                    );

                    // Credit balance for wins
                    if won {
                        let winnings = trade.cost + pnl;
                        let _ = self.executor.credit_balance(winnings).await;
                    }

                    // Update stats
                    {
                        let mut stats = self.stats.write().await;
                        if won {
                            stats.wins += 1;
                        } else {
                            stats.losses += 1;
                        }
                        stats.realized_pnl += pnl;
                        stats.current_balance = self
                            .executor
                            .get_balance()
                            .await
                            .unwrap_or(stats.current_balance);
                    }

                    settled_indices.push(i);
                }
                Err(e) => {
                    warn!(
                        coin = trade.coin,
                        error = %e,
                        "Failed to check settlement"
                    );
                }
            }
        }

        // Remove settled trades (iterate in reverse to preserve indices)
        for i in settled_indices.into_iter().rev() {
            self.pending_settlements.remove(i);
        }

        let mut stats = self.stats.write().await;
        stats.pending_settlements = self.pending_settlements.len() as u64;
    }

    /// Renders the live dashboard to stdout.
    async fn render_dashboard(&self) {
        let stats = self.stats.read().await;
        let runner_stats = if let Some(ref rs) = self.runner_stats {
            Some(rs.read().await.clone())
        } else {
            None
        };

        // Clear screen and move cursor to top
        print!("\x1b[2J\x1b[H");

        println!("\x1b[36m╔══════════════════════════════════════════════════════════════════╗\x1b[0m");
        println!("\x1b[36m║\x1b[0m        \x1b[1;37mDirectional Trading Bot\x1b[0m                               \x1b[36m║\x1b[0m");
        println!("\x1b[36m╚══════════════════════════════════════════════════════════════════╝\x1b[0m");
        println!();

        // Mode
        if self.config.observe_mode {
            println!("  \x1b[2mMode:\x1b[0m          \x1b[36mOBSERVE (no trading)\x1b[0m");
        } else if self.config.fixed_bet_size.is_some() {
            println!(
                "  \x1b[2mMode:\x1b[0m          LIVE (fixed ${})",
                self.config.fixed_bet_size.unwrap()
            );
        } else {
            println!(
                "  \x1b[2mMode:\x1b[0m          LIVE (Kelly {:.0}%)",
                self.config.kelly_fraction * 100.0
            );
        }

        // Uptime
        if let Some(started) = stats.started_at {
            let uptime = Utc::now() - started;
            let mins = uptime.num_minutes();
            let secs = uptime.num_seconds() % 60;
            println!("  \x1b[2mUptime:\x1b[0m        {}m {}s", mins, secs);
        }
        println!();

        // Spot prices from runner
        if let Some(ref rs) = runner_stats {
            println!("\x1b[1;37m  Spot Prices:\x1b[0m");
            for (coin, price) in &rs.current_spot_prices {
                let ref_price = rs.current_reference_prices.get(coin);
                let delta = ref_price.map(|r| (price - r) / r * 100.0);
                let up_ask = rs.current_up_asks.get(coin);
                let down_ask = rs.current_down_asks.get(coin);

                print!(
                    "    {:<4} ${:<10.2}",
                    coin,
                    price,
                );
                if let Some(d) = delta {
                    if d > 0.0 {
                        print!("  \x1b[32m{:+.3}%\x1b[0m", d);
                    } else {
                        print!("  \x1b[31m{:+.3}%\x1b[0m", d);
                    }
                }
                if let (Some(u), Some(d)) = (up_ask, down_ask) {
                    print!("  Up:{} Dn:{}", u, d);
                }
                println!();
            }
            println!();
        }

        // Execution stats
        println!("\x1b[1;37m  Execution:\x1b[0m");
        println!(
            "    Signals:    {} received, {} skipped",
            stats.signals_received, stats.signals_skipped
        );
        println!(
            "    Orders:     {} filled / {} attempted ({:.0}% fill rate)",
            stats.orders_filled,
            stats.orders_attempted,
            stats.fill_rate() * 100.0
        );
        println!(
            "    Volume:     ${}",
            stats.total_volume.round_dp(2)
        );
        println!();

        // P&L
        println!("\x1b[1;37m  P&L:\x1b[0m");
        let pnl_color = if stats.realized_pnl >= Decimal::ZERO {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };
        println!(
            "    Realized:   {}${}\x1b[0m",
            pnl_color,
            stats.realized_pnl.round_dp(2)
        );
        println!(
            "    Win Rate:   {:.1}% ({}/{} settled)",
            stats.win_rate() * 100.0,
            stats.wins,
            stats.wins + stats.losses
        );
        println!(
            "    Pending:    {} settlements",
            stats.pending_settlements
        );
        println!(
            "    Balance:    ${}",
            stats.current_balance.round_dp(2)
        );
        println!();

        // Recent trades
        if !self.trades.is_empty() {
            println!("\x1b[1;37m  Recent Trades:\x1b[0m");
            let recent: Vec<_> = self.trades.iter().rev().take(10).collect();
            for trade in recent {
                let status = match &trade.settlement {
                    Some(s) if s.won => format!("\x1b[32mWIN  {}\x1b[0m", s.pnl.round_dp(2)),
                    Some(s) => format!("\x1b[31mLOSS {}\x1b[0m", s.pnl.round_dp(2)),
                    None => "\x1b[33mPENDING\x1b[0m".to_string(),
                };
                println!(
                    "    {} {:<4} {:<4} @ {} ({} shares) → {}",
                    trade.execution_timestamp.format("%H:%M:%S"),
                    trade.coin.to_uppercase(),
                    trade.direction,
                    trade.entry_price,
                    trade.shares,
                    status,
                );
            }
        }
        println!();
        println!("  \x1b[2mPress Ctrl+C to stop\x1b[0m");
    }

    /// Prints final session summary.
    async fn print_summary(&self) {
        let stats = self.stats.read().await;

        println!();
        println!("\x1b[36m═══════════════════════════════════════════════════════════════════\x1b[0m");
        println!("\x1b[1;37mSession Summary\x1b[0m");
        println!();

        if let Some(started) = stats.started_at {
            let duration = Utc::now() - started;
            println!("  Duration:     {}m", duration.num_minutes());
        }

        println!("  Signals:      {} received", stats.signals_received);
        println!(
            "  Trades:       {} filled / {} attempted",
            stats.orders_filled, stats.orders_attempted
        );
        println!(
            "  Volume:       ${}",
            stats.total_volume.round_dp(2)
        );
        println!(
            "  Win Rate:     {:.1}% ({}/{})",
            stats.win_rate() * 100.0,
            stats.wins,
            stats.wins + stats.losses
        );

        let pnl_color = if stats.realized_pnl >= Decimal::ZERO {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };
        println!(
            "  Realized P&L: {}${}\x1b[0m",
            pnl_color,
            stats.realized_pnl.round_dp(2)
        );
        println!(
            "  Final Balance: ${}",
            stats.current_balance.round_dp(2)
        );
        println!(
            "  Pending:      {} unsettled trades",
            stats.pending_settlements
        );
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = DirectionalExecutorConfig::default();
        assert!((config.kelly_fraction - 0.25).abs() < 0.001);
        assert_eq!(config.min_bet_size, dec!(5));
        assert_eq!(config.max_bet_size, dec!(100));
        assert!((config.min_edge - 0.03).abs() < 0.001);
        assert_eq!(config.max_position_per_window, dec!(200));
        assert_eq!(config.max_trades_per_window, 1);
        assert!(!config.observe_mode);
        assert_eq!(config.fee_rate, dec!(0.02));
    }

    #[test]
    fn test_kelly_sizer_positive_edge() {
        let sizer = KellySizer::new(0.25, dec!(5), dec!(100));

        // Win prob 0.60 at price 0.45 → edge = 0.15
        let bet = sizer.size(0.60, dec!(0.45), dec!(1000));
        assert!(bet.is_some());
        let bet = bet.unwrap();
        assert!(bet >= dec!(5)); // Above minimum
        assert!(bet <= dec!(100)); // Below maximum
    }

    #[test]
    fn test_kelly_sizer_no_edge() {
        let sizer = KellySizer::new(0.25, dec!(5), dec!(100));

        // Win prob 0.40 at price 0.50 → negative edge
        let bet = sizer.size(0.40, dec!(0.50), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_sizer_below_minimum() {
        let sizer = KellySizer::new(0.01, dec!(50), dec!(100));

        // Very small Kelly fraction with small bankroll → below min
        let bet = sizer.size(0.55, dec!(0.45), dec!(100));
        // At 0.01 Kelly fraction, the bet would be tiny
        assert!(bet.is_none());
    }

    #[test]
    fn test_window_tracker() {
        let mut tracker = WindowTracker::default();
        tracker.reset(1000);

        assert!(tracker.can_trade(dec!(100), 5));

        tracker.record_trade(dec!(50));
        assert!(tracker.can_trade(dec!(100), 5));
        assert_eq!(tracker.trade_count, 1);

        tracker.record_trade(dec!(60));
        assert!(!tracker.can_trade(dec!(100), 5)); // Over 100 limit

        tracker.reset(2000);
        assert!(tracker.can_trade(dec!(100), 5)); // Reset
    }

    #[test]
    fn test_window_tracker_trade_limit() {
        let mut tracker = WindowTracker::default();
        tracker.reset(1000);

        for _ in 0..3 {
            tracker.record_trade(dec!(10));
        }

        assert!(!tracker.can_trade(dec!(1000), 3)); // At trade limit
        assert!(tracker.can_trade(dec!(1000), 4)); // Below trade limit
    }

    #[test]
    fn test_stats_win_rate() {
        let mut stats = DirectionalExecutorStats::default();
        assert_eq!(stats.win_rate(), 0.0);

        stats.wins = 3;
        stats.losses = 7;
        assert!((stats.win_rate() - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_stats_fill_rate() {
        let mut stats = DirectionalExecutorStats::default();
        assert_eq!(stats.fill_rate(), 0.0);

        stats.orders_attempted = 10;
        stats.orders_filled = 8;
        assert!((stats.fill_rate() - 0.8).abs() < 0.001);
    }
}
