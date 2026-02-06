//! Automated execution for cross-market correlation arbitrage.
//!
//! This module connects the signal detection pipeline (`CrossMarketRunner`) to the
//! order execution layer, enabling automated cross-market trading.
//!
//! # Strategy
//!
//! For BTC/ETH correlation arbitrage with Coin1DownCoin2Up:
//! - Leg 1: Buy BTC DOWN
//! - Leg 2: Buy ETH UP
//! - Win if BTC and ETH move together (91.6% historical win rate)
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────────┐         ┌────────────────────────┐
//! │   CrossMarketRunner    │ opps    │ CrossMarketAutoExecutor│
//! │   (detects opps)       │─────────▶ - Filter pairs/combos  │
//! └────────────────────────┘  mpsc   │ - Kelly sizing         │
//!                                    │ - Execute both legs    │
//!                                    │ - Persist to DB        │
//!                                    │         ↓              │
//!                                    ┌────────────────────────┐
//!                                    │  PolymarketExecutor    │
//!                                    │  submit_orders_batch() │
//!                                    └────────────────────────┘
//! ```

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::cross_market_types::{CrossMarketCombination, CrossMarketOpportunity};
use super::execution::{
    ExecutionError, OrderParams, OrderResult, OrderStatus, OrderType, PolymarketExecutor, Side,
};
use crate::gamma::GammaClient;
use crate::models::Coin;

// =============================================================================
// Errors
// =============================================================================

/// Errors from the cross-market auto executor.
#[derive(Error, Debug)]
pub enum CrossMarketAutoExecutorError {
    /// Execution error from underlying executor.
    #[error("Execution error: {0}")]
    Execution(#[from] ExecutionError),

    /// Position limit exceeded.
    #[error("Position limit exceeded: current {current}, limit {limit}")]
    PositionLimit { current: Decimal, limit: Decimal },

    /// Opportunity filtered out.
    #[error("Opportunity filtered: {reason}")]
    Filtered { reason: String },

    /// Signal channel closed.
    #[error("Signal channel closed")]
    ChannelClosed,
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the cross-market auto executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMarketAutoExecutorConfig {
    /// Only execute opportunities for this specific coin pair (e.g., BTC/ETH).
    /// If None, executes all pairs.
    pub filter_pair: Option<(Coin, Coin)>,

    /// Only execute this specific combination.
    /// If None, executes all combinations.
    pub filter_combination: Option<CrossMarketCombination>,

    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    pub kelly_fraction: f64,

    /// Fixed bet size in USDC per leg (overrides Kelly if set).
    pub fixed_bet_size: Option<Decimal>,

    /// Minimum bet size in USDC per leg.
    pub min_bet_size: Decimal,

    /// Maximum bet size in USDC per leg.
    pub max_bet_size: Decimal,

    /// Maximum total position value per window across all pairs.
    pub max_position_per_window: Decimal,

    /// Minimum spread required to execute.
    pub min_spread: Decimal,

    /// Minimum win probability required to execute.
    pub min_win_probability: f64,

    /// Maximum trade history to keep in memory.
    pub max_history: usize,

    /// Enable early exit when positions are profitable (default: true).
    pub early_exit_enabled: bool,

    /// Minimum profit percentage to trigger early exit (default: 0.10 = 10%).
    pub early_exit_profit_threshold: Decimal,

    /// Fraction of visible bid depth to sell into per cycle (default: 0.50).
    pub early_exit_depth_fraction: Decimal,
}

impl Default for CrossMarketAutoExecutorConfig {
    fn default() -> Self {
        Self {
            filter_pair: None,
            filter_combination: None,
            kelly_fraction: 0.25,
            fixed_bet_size: None,
            min_bet_size: dec!(5),
            max_bet_size: dec!(50),
            max_position_per_window: dec!(200),
            min_spread: dec!(0.03),
            min_win_probability: 0.80,
            max_history: 1000,
            early_exit_enabled: true,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
        }
    }
}

impl CrossMarketAutoExecutorConfig {
    /// Creates a BTC/ETH focused configuration (highest correlation pair).
    #[must_use]
    pub fn btc_eth_only() -> Self {
        Self {
            filter_pair: Some((Coin::Btc, Coin::Eth)),
            filter_combination: Some(CrossMarketCombination::Coin1DownCoin2Up),
            kelly_fraction: 0.25,
            fixed_bet_size: None,
            min_bet_size: dec!(5),
            max_bet_size: dec!(50),
            max_position_per_window: dec!(200),
            min_spread: dec!(0.03),
            min_win_probability: 0.85,
            max_history: 1000,
            early_exit_enabled: true,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
        }
    }

    /// Creates a micro testing configuration with tight limits.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            filter_pair: Some((Coin::Btc, Coin::Eth)),
            filter_combination: Some(CrossMarketCombination::Coin1DownCoin2Up),
            kelly_fraction: 0.10,
            fixed_bet_size: Some(dec!(2.5)),
            min_bet_size: dec!(1),
            max_bet_size: dec!(5),
            max_position_per_window: dec!(10),
            min_spread: dec!(0.02),
            min_win_probability: 0.75,
            max_history: 100,
            early_exit_enabled: true,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
        }
    }

    /// Sets the pair filter.
    #[must_use]
    pub fn with_pair_filter(mut self, coin1: Coin, coin2: Coin) -> Self {
        self.filter_pair = Some((coin1, coin2));
        self
    }

    /// Sets the combination filter.
    #[must_use]
    pub fn with_combination_filter(mut self, combo: CrossMarketCombination) -> Self {
        self.filter_combination = Some(combo);
        self
    }

    /// Sets the fixed bet size.
    #[must_use]
    pub fn with_fixed_bet(mut self, size: Decimal) -> Self {
        self.fixed_bet_size = Some(size);
        self
    }

    /// Sets the Kelly fraction.
    #[must_use]
    pub fn with_kelly_fraction(mut self, fraction: f64) -> Self {
        self.kelly_fraction = fraction;
        self
    }
}

// =============================================================================
// Trade Record
// =============================================================================

/// Result of a cross-market execution attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrossMarketExecutionResult {
    /// Both legs filled successfully.
    Success {
        leg1_result: OrderResult,
        leg2_result: OrderResult,
        total_cost: Decimal,
        expected_payout: Decimal,
    },
    /// Only leg 1 filled (exposure created).
    Leg1OnlyFilled {
        leg1_result: OrderResult,
        leg2_result: OrderResult,
    },
    /// Only leg 2 filled (exposure created).
    Leg2OnlyFilled {
        leg1_result: OrderResult,
        leg2_result: OrderResult,
    },
    /// Both legs rejected (no exposure).
    BothRejected {
        leg1_result: OrderResult,
        leg2_result: OrderResult,
    },
}

impl CrossMarketExecutionResult {
    /// Returns true if execution was successful (both legs filled).
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Returns true if there was a partial fill (exposure created).
    #[must_use]
    pub fn is_partial(&self) -> bool {
        matches!(
            self,
            Self::Leg1OnlyFilled { .. } | Self::Leg2OnlyFilled { .. }
        )
    }
}

/// A record of an executed cross-market trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMarketTradeRecord {
    /// Unique trade ID.
    pub trade_id: String,

    /// Coin pair (e.g., "BTC/ETH").
    pub pair: String,

    /// Combination type.
    pub combination: CrossMarketCombination,

    /// Leg 1 token ID.
    pub leg1_token_id: String,

    /// Leg 2 token ID.
    pub leg2_token_id: String,

    /// Leg 1 requested price.
    pub leg1_price: Decimal,

    /// Leg 2 requested price.
    pub leg2_price: Decimal,

    /// Total cost (both legs).
    pub total_cost: Decimal,

    /// Shares per leg.
    pub shares: Decimal,

    /// Execution result.
    pub result: CrossMarketExecutionResult,

    /// Timestamp of detection.
    pub detected_at: DateTime<Utc>,

    /// Timestamp of execution.
    pub executed_at: DateTime<Utc>,

    /// Win probability at time of signal.
    pub win_probability: f64,

    /// Expected value at time of signal.
    pub expected_value: Decimal,
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics for the cross-market auto executor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrossMarketAutoExecutorStats {
    /// Total opportunities received.
    pub opportunities_received: u64,

    /// Opportunities skipped (filtered, below threshold, etc.).
    pub opportunities_skipped: u64,

    /// Executions attempted.
    pub executions_attempted: u64,

    /// Both legs filled.
    pub both_filled: u64,

    /// Only one leg filled (partial).
    pub partial_fills: u64,

    /// Incomplete trades awaiting completion.
    pub incomplete_trades: u64,

    /// Incomplete trades successfully completed.
    pub incomplete_recovered: u64,

    /// Incomplete trades that expired (window closed).
    pub incomplete_expired: u64,

    /// Incomplete trades resolved via escape hatch (sold filled leg).
    pub incomplete_escaped: u64,

    /// Both legs rejected.
    pub both_rejected: u64,

    /// Total volume traded (USDC).
    pub total_volume: Decimal,

    /// Current window position value.
    pub current_position_value: Decimal,

    /// Start time.
    pub started_at: Option<DateTime<Utc>>,

    /// Last trade time.
    pub last_trade_time: Option<DateTime<Utc>>,

    // === Settlement Stats (Paper Trading) ===
    /// Trades pending settlement.
    pub pending_settlement: u64,

    /// Trades settled as wins.
    pub settled_wins: u64,

    /// Trades settled as losses.
    pub settled_losses: u64,

    // === Latency Stats ===
    /// Last API latency in milliseconds.
    pub last_latency_ms: u64,

    /// Average API latency in milliseconds.
    pub avg_latency_ms: u64,

    /// Total latency samples.
    pub latency_samples: u64,

    /// Double wins (both legs won - rare but possible).
    pub double_wins: u64,

    /// Realized P&L from settled trades.
    pub realized_pnl: Decimal,

    /// Current paper balance (for paper trading).
    pub paper_balance: Decimal,

    // === Recent Trades (for dashboard display) ===
    /// Recent trades for display (trade_id, pair, leg1_price, leg2_price, total_cost, timestamp).
    pub recent_trades: Vec<RecentTradeDisplay>,

    /// Pending settlements for display.
    pub pending_trades: Vec<PendingTradeDisplay>,

    // === Early Exit Stats ===
    /// Trades exited early (sold before settlement).
    pub early_exits: u64,

    /// Total USDC received from early exit sells.
    pub early_exit_proceeds: Decimal,

    // === Live prices from WebSocket (for settlement) ===
    /// Current prices from WebSocket feed: coin -> (up_price, down_price).
    /// Updated by the CLI from runner stats, used for fast settlement.
    pub live_prices: std::collections::HashMap<String, (Decimal, Decimal)>,
}

/// Simplified trade info for dashboard display.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecentTradeDisplay {
    pub trade_id: String,
    pub pair: String,
    pub leg1_dir: String,
    pub leg1_price: Decimal,
    pub leg2_dir: String,
    pub leg2_price: Decimal,
    pub total_cost: Decimal,
    pub executed_at: DateTime<Utc>,
}

/// Pending trade info for dashboard display.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingTradeDisplay {
    pub trade_id: String,
    pub pair: String,
    pub leg1_dir: String,
    pub leg2_dir: String,
    pub total_cost: Decimal,
    pub window_end: DateTime<Utc>,
}

// =============================================================================
// Pending Paper Settlement
// =============================================================================

/// A trade awaiting settlement (paper trading).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPaperSettlement {
    /// Trade ID.
    pub trade_id: String,
    /// Coin 1 (e.g., "btc").
    pub coin1: String,
    /// Coin 2 (e.g., "eth").
    pub coin2: String,
    /// Leg 1 direction ("UP" or "DOWN").
    pub leg1_direction: String,
    /// Leg 2 direction ("UP" or "DOWN").
    pub leg2_direction: String,
    /// Leg 1 token ID (for Polymarket price query).
    pub leg1_token_id: String,
    /// Leg 2 token ID (for Polymarket price query).
    pub leg2_token_id: String,
    /// Total cost of the trade.
    pub total_cost: Decimal,
    /// Shares bought (same for both legs).
    pub shares: Decimal,
    /// Window end time (when settlement becomes possible).
    pub window_end: DateTime<Utc>,
    /// When the trade was executed.
    pub executed_at: DateTime<Utc>,
    /// Shares remaining for leg 1 (decreases as we partially exit).
    pub remaining_shares_leg1: Decimal,
    /// Shares remaining for leg 2.
    pub remaining_shares_leg2: Decimal,
    /// USDC received from early exits so far.
    pub early_exit_proceeds: Decimal,
    /// Whether this trade has been partially exited.
    pub partially_exited: bool,
}

// =============================================================================
// Incomplete Trade (Partial Fill Recovery)
// =============================================================================

/// Which leg was filled in a partial fill scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilledLeg {
    /// Leg 1 (coin 1) was filled.
    Leg1,
    /// Leg 2 (coin 2) was filled.
    Leg2,
}

/// Maximum retry attempts before triggering the escape hatch (sell filled leg).
/// Recovery runs every 5 seconds, so 60 retries ≈ 5 minutes of actual attempts.
const MAX_RECOVERY_RETRIES: u32 = 60;

/// Maximum age of an incomplete trade before triggering the escape hatch.
/// 5 minutes gives liquidity time to return without blocking the bot too long.
const MAX_RECOVERY_AGE_SECS: u64 = 300;

/// An incomplete trade where one leg filled but the other didn't.
///
/// This tracks partial fills so we can attempt to complete the missing leg
/// when market conditions become favorable again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncompleteTrade {
    /// Unique ID for this incomplete trade.
    pub trade_id: String,

    /// Which leg was successfully filled.
    pub filled_leg: FilledLeg,

    /// Coin 1 symbol (e.g., "btc").
    pub coin1: String,

    /// Coin 2 symbol (e.g., "eth").
    pub coin2: String,

    /// The filled leg's direction ("UP" or "DOWN").
    pub filled_direction: String,

    /// The filled leg's token ID.
    pub filled_token_id: String,

    /// Price at which the filled leg executed.
    pub filled_price: Decimal,

    /// Number of shares filled.
    pub shares: Decimal,

    /// The missing leg's direction ("UP" or "DOWN").
    pub missing_direction: String,

    /// The missing leg's token ID.
    pub missing_token_id: String,

    /// Maximum price we're willing to pay for the missing leg.
    /// Calculated as: target_pair_cost - filled_price
    pub max_missing_price: Decimal,

    /// Window end time (must complete before this).
    pub window_end: DateTime<Utc>,

    /// When the partial fill occurred.
    pub created_at: DateTime<Utc>,

    /// Number of retry attempts.
    pub retry_count: u32,

    /// Combination type for this trade.
    pub combination: CrossMarketCombination,
}

impl IncompleteTrade {
    /// Returns true if the window has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.window_end
    }

    /// Returns true if the given price is acceptable for completing the trade.
    pub fn is_price_acceptable(&self, current_price: Decimal) -> bool {
        current_price <= self.max_missing_price
    }

    /// Returns the coin symbol for the missing leg.
    pub fn missing_coin(&self) -> &str {
        match self.filled_leg {
            FilledLeg::Leg1 => &self.coin2,
            FilledLeg::Leg2 => &self.coin1,
        }
    }

    /// Returns true if the escape hatch should trigger.
    ///
    /// The escape hatch fires when either:
    /// - Retry count exceeds `MAX_RECOVERY_RETRIES`
    /// - Trade age exceeds `MAX_RECOVERY_AGE_SECS`
    pub fn should_escape(&self) -> bool {
        if self.retry_count >= MAX_RECOVERY_RETRIES {
            return true;
        }
        let age = Utc::now() - self.created_at;
        age.num_seconds() >= MAX_RECOVERY_AGE_SECS as i64
    }
}

// =============================================================================
// Kelly Sizer for Cross-Market
// =============================================================================

/// Calculates position size using Kelly criterion for cross-market bets.
pub struct CrossMarketKellySizer {
    /// Kelly fraction to use (0.0 to 1.0).
    fraction: f64,
    /// Minimum bet size per leg.
    min_size: Decimal,
    /// Maximum bet size per leg.
    max_size: Decimal,
}

impl CrossMarketKellySizer {
    /// Creates a new Kelly sizer.
    #[must_use]
    pub fn new(fraction: f64, min_size: Decimal, max_size: Decimal) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
            min_size,
            max_size,
        }
    }

    /// Calculates the optimal bet size per leg.
    ///
    /// For cross-market bets:
    /// - Win: Get $1.00 back (from either leg)
    /// - Lose: Lose total_cost (both legs)
    /// - Kelly: f* = (p * b - q) / b where b = (1 - cost) / cost
    ///
    /// # Returns
    /// Bet size per leg in USDC, or None if no bet recommended.
    #[must_use]
    pub fn size(
        &self,
        win_probability: f64,
        total_cost: Decimal,
        bankroll: Decimal,
    ) -> Option<Decimal> {
        // Convert to f64 for calculation
        let cost_f64 = total_cost.to_string().parse::<f64>().unwrap_or(1.0);

        // Net odds: if we risk `cost` to win `1 - cost`, b = (1 - cost) / cost
        if cost_f64 <= 0.0 || cost_f64 >= 1.0 {
            return None;
        }
        let b = (1.0 - cost_f64) / cost_f64;

        // Full Kelly: f* = (p * b - q) / b = (p * b - (1-p)) / b
        let q = 1.0 - win_probability;
        let full_kelly = (win_probability * b - q) / b;

        // No bet if Kelly is negative (no edge)
        if full_kelly <= 0.0 {
            return None;
        }

        // Apply fraction
        let kelly_fraction = full_kelly * self.fraction;

        // Convert bankroll to f64
        let bankroll_f64 = bankroll.to_string().parse::<f64>().unwrap_or(0.0);

        // Calculate bet size (this is total bet, need to divide by 2 for per-leg)
        let total_bet_f64 = bankroll_f64 * kelly_fraction;
        let per_leg_bet_f64 = total_bet_f64 / 2.0;

        // Convert back to Decimal
        let per_leg_bet = Decimal::from_f64_retain(per_leg_bet_f64)?;

        // If calculated bet is below minimum, no bet
        if per_leg_bet < self.min_size {
            return None;
        }

        // Apply maximum and bankroll limits
        let per_leg_bet = per_leg_bet.min(self.max_size).min(bankroll / dec!(2));

        Some(per_leg_bet)
    }
}

// =============================================================================
// Window Position Tracker
// =============================================================================

/// Tracks cross-market positions for the current window.
#[derive(Debug, Clone, Default)]
pub struct CrossMarketWindowTracker {
    /// Current window start timestamp (ms).
    pub window_start_ms: i64,

    /// Total cost invested this window.
    pub total_cost: Decimal,

    /// Number of positions this window.
    pub position_count: u32,
}

impl CrossMarketWindowTracker {
    /// Creates a new tracker for the given window.
    #[must_use]
    pub fn new(window_start_ms: i64) -> Self {
        Self {
            window_start_ms,
            total_cost: Decimal::ZERO,
            position_count: 0,
        }
    }

    /// Records a new position.
    pub fn record_position(&mut self, cost: Decimal) {
        self.total_cost += cost;
        self.position_count += 1;
    }

    /// Clears positions (on window transition).
    pub fn clear(&mut self) {
        self.total_cost = Decimal::ZERO;
        self.position_count = 0;
    }

    /// Returns remaining capacity for this window.
    #[must_use]
    pub fn remaining_capacity(&self, max_position: Decimal) -> Decimal {
        (max_position - self.total_cost).max(Decimal::ZERO)
    }
}

// =============================================================================
// Cross-Market Auto Executor
// =============================================================================

/// Automated execution bridge for cross-market opportunities.
///
/// Consumes opportunities from `CrossMarketRunner` and executes them via `PolymarketExecutor`.
pub struct CrossMarketAutoExecutor<E: PolymarketExecutor> {
    /// The underlying executor.
    executor: E,

    /// Configuration.
    config: CrossMarketAutoExecutorConfig,

    /// Kelly position sizer.
    sizer: CrossMarketKellySizer,

    /// Current window position tracker.
    position: Arc<RwLock<CrossMarketWindowTracker>>,

    /// Execution statistics.
    stats: Arc<RwLock<CrossMarketAutoExecutorStats>>,

    /// Trade history.
    history: Arc<RwLock<VecDeque<CrossMarketTradeRecord>>>,

    /// Pending settlements (paper trading).
    pending_settlements: Arc<RwLock<Vec<PendingPaperSettlement>>>,

    /// Incomplete trades awaiting completion (partial fills).
    incomplete_trades: Arc<RwLock<Vec<IncompleteTrade>>>,

    /// HTTP client for settlement price checks.
    http_client: reqwest::Client,

    /// Gamma API client for fetching market outcomes.
    gamma_client: GammaClient,

    /// Fee rate on winnings (default 2%).
    fee_rate: Decimal,

    /// Stop flag.
    should_stop: Arc<AtomicBool>,

    /// Optional database pool for persistence.
    db_pool: Option<PgPool>,

    /// Session ID for grouping trades.
    session_id: String,

    /// Cooldown after execution failures to avoid wasting API calls.
    /// Tracks when the last execution attempt failed (FOK not filled, etc.)
    last_both_rejected_at: Option<std::time::Instant>,

    /// Tracks the last time we checked for redeemable positions.
    last_redeem_check: Option<std::time::Instant>,
}

impl<E: PolymarketExecutor> CrossMarketAutoExecutor<E> {
    /// Creates a new cross-market auto executor.
    pub fn new(executor: E, config: CrossMarketAutoExecutorConfig) -> Self {
        let sizer = CrossMarketKellySizer::new(
            config.kelly_fraction,
            config.min_bet_size,
            config.max_bet_size,
        );

        let session_id = format!("auto-{}", Utc::now().format("%Y%m%d-%H%M%S"));

        Self {
            executor,
            config,
            sizer,
            position: Arc::new(RwLock::new(CrossMarketWindowTracker::default())),
            stats: Arc::new(RwLock::new(CrossMarketAutoExecutorStats::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            pending_settlements: Arc::new(RwLock::new(Vec::new())),
            incomplete_trades: Arc::new(RwLock::new(Vec::new())),
            http_client: reqwest::Client::new(),
            gamma_client: GammaClient::new(),
            fee_rate: dec!(0.02), // 2% fee
            should_stop: Arc::new(AtomicBool::new(false)),
            db_pool: None,
            session_id,
            last_both_rejected_at: None,
            last_redeem_check: None,
        }
    }

    /// Creates a new cross-market auto executor with database persistence.
    pub fn with_persistence(
        executor: E,
        config: CrossMarketAutoExecutorConfig,
        db_pool: PgPool,
        session_id: Option<String>,
    ) -> Self {
        let sizer = CrossMarketKellySizer::new(
            config.kelly_fraction,
            config.min_bet_size,
            config.max_bet_size,
        );

        let session_id =
            session_id.unwrap_or_else(|| format!("auto-{}", Utc::now().format("%Y%m%d-%H%M%S")));

        Self {
            executor,
            config,
            sizer,
            position: Arc::new(RwLock::new(CrossMarketWindowTracker::default())),
            stats: Arc::new(RwLock::new(CrossMarketAutoExecutorStats::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            pending_settlements: Arc::new(RwLock::new(Vec::new())),
            incomplete_trades: Arc::new(RwLock::new(Vec::new())),
            http_client: reqwest::Client::new(),
            gamma_client: GammaClient::new(),
            fee_rate: dec!(0.02), // 2% fee
            should_stop: Arc::new(AtomicBool::new(false)),
            db_pool: Some(db_pool),
            session_id,
            last_both_rejected_at: None,
            last_redeem_check: None,
        }
    }

    /// Returns a handle to stop the executor.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.should_stop.clone()
    }

    /// Returns the shared stats.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<CrossMarketAutoExecutorStats>> {
        self.stats.clone()
    }

    /// Returns the trade history.
    #[must_use]
    pub fn history(&self) -> Arc<RwLock<VecDeque<CrossMarketTradeRecord>>> {
        self.history.clone()
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &CrossMarketAutoExecutorConfig {
        &self.config
    }

    /// Runs the auto executor, consuming opportunities and executing trades.
    pub async fn run(
        &mut self,
        mut opp_rx: mpsc::Receiver<CrossMarketOpportunity>,
    ) -> Result<(), CrossMarketAutoExecutorError> {
        info!(
            kelly = self.config.kelly_fraction,
            min_spread = %self.config.min_spread,
            max_position = %self.config.max_position_per_window,
            "CrossMarketAutoExecutor starting"
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Track last settlement check time - check every 5 seconds for faster settlement
        let mut last_settlement_check = std::time::Instant::now();
        let settlement_check_interval = std::time::Duration::from_secs(5);

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("CrossMarketAutoExecutor stopping");
                break;
            }

            // Always check settlement if interval has passed (before waiting for opportunities)
            if last_settlement_check.elapsed() >= settlement_check_interval {
                debug!("Running periodic settlement check...");
                if let Err(e) = self.check_pending_settlements().await {
                    warn!(error = %e, "Settlement check error");
                }

                // Try early exit on profitable positions (before window closes)
                if self.config.early_exit_enabled {
                    if let Err(e) = self.try_early_exit().await {
                        debug!(error = %e, "Early exit check error (non-fatal)");
                    }
                }

                // Also try to complete any incomplete trades (partial fill recovery)
                if let Err(e) = self.try_complete_incomplete_trades().await {
                    warn!(error = %e, "Incomplete trade recovery error");
                }

                last_settlement_check = std::time::Instant::now();
            }

            // Auto-redeem: check for redeemable positions every 60 seconds
            {
                let should_check = match self.last_redeem_check {
                    None => true,
                    Some(last) => last.elapsed() >= std::time::Duration::from_secs(60),
                };
                if should_check {
                    match self.executor.redeem_resolved_positions().await {
                        Ok(0) => {}
                        Ok(n) => info!(redeemed = n, "Auto-redeemed {} resolved positions", n),
                        Err(e) => debug!(error = %e, "Auto-redeem check failed (non-fatal)"),
                    }
                    self.last_redeem_check = Some(std::time::Instant::now());
                }
            }

            // Wait for next opportunity with short timeout
            tokio::select! {
                opp = opp_rx.recv() => {
                    match opp {
                        Some(o) => {
                            if let Err(e) = self.handle_opportunity(o).await {
                                error!(error = %e, "Error handling opportunity");
                            }
                        }
                        None => {
                            info!("Opportunity channel closed");
                            return Err(CrossMarketAutoExecutorError::ChannelClosed);
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // Just a short sleep to avoid busy-looping
                }
            }
        }

        // Final settlement check before stopping
        info!("Running final settlement check...");
        if let Err(e) = self.check_pending_settlements().await {
            warn!(error = %e, "Final settlement check error");
        }

        // Log any remaining incomplete trades
        {
            let incomplete = self.incomplete_trades.read().await;
            if !incomplete.is_empty() {
                warn!(
                    count = incomplete.len(),
                    "Stopping with {} incomplete trades still pending",
                    incomplete.len()
                );
            }
        }

        Ok(())
    }

    /// Handles a single opportunity.
    async fn handle_opportunity(
        &mut self,
        opp: CrossMarketOpportunity,
    ) -> Result<(), CrossMarketAutoExecutorError> {
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.opportunities_received += 1;
        }

        // Cooldown after both legs rejected (e.g., FOK not filled due to low liquidity).
        // Wait 30 seconds before retrying to avoid wasting API calls on illiquid markets.
        if let Some(rejected_at) = self.last_both_rejected_at {
            let cooldown = std::time::Duration::from_secs(30);
            if rejected_at.elapsed() < cooldown {
                debug!(
                    remaining_secs = (cooldown - rejected_at.elapsed()).as_secs(),
                    "Skipping opportunity - cooldown after both legs rejected"
                );
                self.stats.write().await.opportunities_skipped += 1;
                return Ok(());
            }
            // Cooldown expired
            self.last_both_rejected_at = None;
        }

        // Block new trades while incomplete trades are pending recovery.
        // One-sided positions are directional bets (~50/50), not the hedged arb (~96%).
        // Don't stack more exposure until existing positions are fully hedged.
        {
            let incomplete = self.incomplete_trades.read().await;
            if !incomplete.is_empty() {
                debug!(
                    count = incomplete.len(),
                    "Skipping opportunity - incomplete trades pending recovery"
                );
                self.stats.write().await.opportunities_skipped += 1;
                return Ok(());
            }
        }

        // Apply filters
        if !self.should_execute(&opp) {
            debug!(
                pair = %format!("{}/{}", opp.coin1, opp.coin2),
                combo = ?opp.combination,
                "Opportunity filtered"
            );
            self.stats.write().await.opportunities_skipped += 1;
            return Ok(());
        }

        // Handle window transitions
        {
            let mut pos = self.position.write().await;
            let opp_window_ms = (opp.detected_at.timestamp_millis() / 900_000) * 900_000;

            if opp_window_ms != pos.window_start_ms {
                info!(
                    old_window = pos.window_start_ms,
                    new_window = opp_window_ms,
                    "Window transition - clearing position tracker"
                );
                pos.window_start_ms = opp_window_ms;
                pos.clear();
            }
        }

        // Calculate bet size using effective balance (USDC + redeemable positions)
        let balance = self.executor.get_effective_balance().await?;
        let bet_per_leg = if let Some(fixed) = self.config.fixed_bet_size {
            fixed
        } else {
            match self
                .sizer
                .size(opp.win_probability, opp.total_cost, balance)
            {
                Some(size) => size,
                None => {
                    debug!("Kelly recommends no bet");
                    self.stats.write().await.opportunities_skipped += 1;
                    return Ok(());
                }
            }
        };

        // Calculate shares (same for both legs)
        // shares = bet_per_leg / leg_price
        // For simplicity, use the average leg price
        let avg_leg_price = opp.total_cost / dec!(2);
        let shares = bet_per_leg / avg_leg_price;

        // Estimate actual USDC cost for both legs
        let estimated_cost = shares * opp.leg1_price + shares * opp.leg2_price;

        // Check position limits against actual USDC cost (not per-share price)
        {
            let pos = self.position.read().await;
            let remaining = pos.remaining_capacity(self.config.max_position_per_window);
            if remaining < estimated_cost {
                debug!(
                    remaining = %remaining,
                    estimated_cost = %estimated_cost,
                    "Position limit would be exceeded"
                );
                self.stats.write().await.opportunities_skipped += 1;
                return Ok(());
            }
        }

        // Execute both legs
        let result = self.execute_both_legs(&opp, shares).await?;

        // Record trade
        self.record_trade(&opp, &result, shares).await;

        // Update position tracker for any fills (not just both-filled)
        {
            let mut pos = self.position.write().await;
            match &result {
                CrossMarketExecutionResult::Success { total_cost, .. } => {
                    pos.record_position(*total_cost);
                }
                CrossMarketExecutionResult::Leg1OnlyFilled { leg1_result, .. } => {
                    // Record the filled leg's cost — capital is deployed
                    let leg1_cost = shares * leg1_result.avg_fill_price.unwrap_or(opp.leg1_price);
                    pos.record_position(leg1_cost);
                }
                CrossMarketExecutionResult::Leg2OnlyFilled { leg2_result, .. } => {
                    let leg2_cost = shares * leg2_result.avg_fill_price.unwrap_or(opp.leg2_price);
                    pos.record_position(leg2_cost);
                }
                CrossMarketExecutionResult::BothRejected { .. } => {
                    // No capital deployed
                }
            }
        }

        // Update stats and handle partial fills
        {
            let mut stats = self.stats.write().await;
            stats.executions_attempted += 1;
            match &result {
                CrossMarketExecutionResult::Success { total_cost, .. } => {
                    stats.both_filled += 1;
                    stats.total_volume += *total_cost;
                    stats.last_trade_time = Some(Utc::now());
                }
                CrossMarketExecutionResult::Leg1OnlyFilled { leg1_result, .. } => {
                    stats.partial_fills += 1;
                    stats.incomplete_trades += 1;
                    warn!("Partial fill - leg 1 only. Adding to recovery queue.");

                    // Create incomplete trade for recovery
                    let incomplete = self.create_incomplete_trade(
                        &opp,
                        FilledLeg::Leg1,
                        leg1_result.avg_fill_price.unwrap_or(opp.leg1_price),
                        shares,
                    );
                    drop(stats); // Release lock before acquiring another
                    self.incomplete_trades.write().await.push(incomplete);
                }
                CrossMarketExecutionResult::Leg2OnlyFilled { leg2_result, .. } => {
                    stats.partial_fills += 1;
                    stats.incomplete_trades += 1;
                    warn!("Partial fill - leg 2 only. Adding to recovery queue.");

                    // Create incomplete trade for recovery
                    let incomplete = self.create_incomplete_trade(
                        &opp,
                        FilledLeg::Leg2,
                        leg2_result.avg_fill_price.unwrap_or(opp.leg2_price),
                        shares,
                    );
                    drop(stats); // Release lock before acquiring another
                    self.incomplete_trades.write().await.push(incomplete);
                }
                CrossMarketExecutionResult::BothRejected { .. } => {
                    stats.both_rejected += 1;
                    drop(stats);
                    // Start cooldown to avoid spamming API when there's no liquidity
                    self.last_both_rejected_at = Some(std::time::Instant::now());
                }
            }
        }

        Ok(())
    }

    /// Creates an IncompleteTrade from a partial fill.
    fn create_incomplete_trade(
        &self,
        opp: &CrossMarketOpportunity,
        filled_leg: FilledLeg,
        filled_price: Decimal,
        shares: Decimal,
    ) -> IncompleteTrade {
        // Calculate window end time
        let window_end = {
            let ts = opp.detected_at.timestamp();
            let window_secs = 900; // 15 minutes
            let window_start = (ts / window_secs) * window_secs;
            let window_end_ts = window_start + window_secs;
            DateTime::from_timestamp(window_end_ts, 0).unwrap_or(opp.detected_at)
        };

        // Target pair cost with some buffer (allow slightly worse price)
        // Use 0.97 as max pair cost (giving at least $0.03 spread)
        let target_pair_cost = dec!(0.97);
        let max_missing_price = target_pair_cost - filled_price;

        match filled_leg {
            FilledLeg::Leg1 => IncompleteTrade {
                trade_id: format!(
                    "incomplete-{}-{}-{}",
                    opp.coin1,
                    opp.coin2,
                    opp.detected_at.timestamp_millis()
                ),
                filled_leg,
                coin1: opp.coin1.clone(),
                coin2: opp.coin2.clone(),
                filled_direction: opp.leg1_direction.clone(),
                filled_token_id: opp.leg1_token_id.clone(),
                filled_price,
                shares,
                missing_direction: opp.leg2_direction.clone(),
                missing_token_id: opp.leg2_token_id.clone(),
                max_missing_price,
                window_end,
                created_at: Utc::now(),
                retry_count: 0,
                combination: opp.combination,
            },
            FilledLeg::Leg2 => IncompleteTrade {
                trade_id: format!(
                    "incomplete-{}-{}-{}",
                    opp.coin1,
                    opp.coin2,
                    opp.detected_at.timestamp_millis()
                ),
                filled_leg,
                coin1: opp.coin1.clone(),
                coin2: opp.coin2.clone(),
                filled_direction: opp.leg2_direction.clone(),
                filled_token_id: opp.leg2_token_id.clone(),
                filled_price,
                shares,
                missing_direction: opp.leg1_direction.clone(),
                missing_token_id: opp.leg1_token_id.clone(),
                max_missing_price,
                window_end,
                created_at: Utc::now(),
                retry_count: 0,
                combination: opp.combination,
            },
        }
    }

    /// Attempts to complete incomplete trades (partial fill recovery).
    ///
    /// This method:
    /// 1. Removes expired incomplete trades (window closed)
    /// 2. Checks if the missing leg can be filled at an acceptable price
    /// 3. Attempts to fill the missing leg
    /// 4. On success, creates a full trade for settlement
    pub async fn try_complete_incomplete_trades(
        &self,
    ) -> Result<u64, CrossMarketAutoExecutorError> {
        let mut completed_count = 0u64;
        let mut expired_count = 0u64;

        // Get current live prices for checking
        let live_prices = {
            let stats = self.stats.read().await;
            stats.live_prices.clone()
        };

        if live_prices.is_empty() {
            debug!("No live prices available for incomplete trade recovery");
            return Ok(0);
        }

        // Get incomplete trades to process
        let trades_to_check: Vec<IncompleteTrade> = {
            let incomplete = self.incomplete_trades.read().await;
            incomplete.clone()
        };

        if trades_to_check.is_empty() {
            return Ok(0);
        }

        info!(
            count = trades_to_check.len(),
            "Checking {} incomplete trades for recovery",
            trades_to_check.len()
        );

        for trade in trades_to_check {
            // Check if expired
            if trade.is_expired() {
                warn!(
                    trade_id = %trade.trade_id,
                    "Incomplete trade expired - window closed"
                );
                expired_count += 1;

                // Remove from list and update stats
                {
                    let mut incomplete = self.incomplete_trades.write().await;
                    incomplete.retain(|t| t.trade_id != trade.trade_id);
                }
                {
                    let mut stats = self.stats.write().await;
                    stats.incomplete_expired += 1;
                    stats.incomplete_trades = stats.incomplete_trades.saturating_sub(1);
                }
                continue;
            }

            // Escape hatch: if too many retries or too old, sell the filled leg
            // to recover capital rather than staying stuck indefinitely.
            if trade.should_escape() {
                warn!(
                    trade_id = %trade.trade_id,
                    retry_count = trade.retry_count,
                    age_secs = (Utc::now() - trade.created_at).num_seconds(),
                    "Escape hatch triggered - selling filled leg to recover capital"
                );

                if let Err(e) = self.escape_sell_filled_leg(&trade, &live_prices).await {
                    warn!(
                        trade_id = %trade.trade_id,
                        error = %e,
                        "Escape sell failed, will retry next cycle"
                    );
                    // Increment retry count so it doesn't loop forever
                    let mut incomplete = self.incomplete_trades.write().await;
                    if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                        t.retry_count += 1;
                    }
                    continue;
                }

                // Remove from list and update stats
                {
                    let mut incomplete = self.incomplete_trades.write().await;
                    incomplete.retain(|t| t.trade_id != trade.trade_id);
                }
                {
                    let mut stats = self.stats.write().await;
                    stats.incomplete_escaped += 1;
                    stats.incomplete_trades = stats.incomplete_trades.saturating_sub(1);
                }
                continue;
            }

            // Get current price for the missing leg's coin
            let missing_coin = trade.missing_coin().to_uppercase();
            let current_price = match live_prices.get(&missing_coin) {
                Some(&(up_price, down_price)) => {
                    // Get the price for the direction we need
                    if trade.missing_direction == "UP" {
                        up_price
                    } else {
                        down_price
                    }
                }
                None => {
                    debug!(
                        trade_id = %trade.trade_id,
                        missing_coin = %missing_coin,
                        "No price available for missing leg"
                    );
                    continue;
                }
            };

            // Check if price is acceptable
            if !trade.is_price_acceptable(current_price) {
                debug!(
                    trade_id = %trade.trade_id,
                    current_price = %current_price,
                    max_price = %trade.max_missing_price,
                    "Price not acceptable for recovery"
                );
                continue;
            }

            info!(
                trade_id = %trade.trade_id,
                missing_coin = %missing_coin,
                missing_direction = %trade.missing_direction,
                current_price = %current_price,
                max_price = %trade.max_missing_price,
                filled_price = %trade.filled_price,
                "Attempting to complete incomplete trade"
            );

            // Try to fill the missing leg
            let order = OrderParams {
                token_id: trade.missing_token_id.clone(),
                side: Side::Buy,
                price: current_price,
                size: trade.shares,
                order_type: OrderType::Fok,
                neg_risk: false,
                presigned: None,
            };

            let results = match self.executor.submit_orders_batch(vec![order]).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        trade_id = %trade.trade_id,
                        error = %e,
                        "Failed to submit recovery order"
                    );
                    // Increment retry count
                    {
                        let mut incomplete = self.incomplete_trades.write().await;
                        if let Some(t) =
                            incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id)
                        {
                            t.retry_count += 1;
                        }
                    }
                    continue;
                }
            };

            if results.is_empty() || results[0].status != OrderStatus::Filled {
                debug!(
                    trade_id = %trade.trade_id,
                    "Recovery order not filled"
                );
                // Increment retry count
                {
                    let mut incomplete = self.incomplete_trades.write().await;
                    if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                        t.retry_count += 1;
                    }
                }
                continue;
            }

            // Successfully completed!
            let missing_fill_price = results[0].avg_fill_price.unwrap_or(current_price);
            let total_cost = (trade.filled_price + missing_fill_price) * trade.shares;

            info!(
                trade_id = %trade.trade_id,
                filled_leg_price = %trade.filled_price,
                missing_leg_price = %missing_fill_price,
                total_cost = %total_cost,
                "Successfully recovered incomplete trade!"
            );

            // Remove from incomplete list
            {
                let mut incomplete = self.incomplete_trades.write().await;
                incomplete.retain(|t| t.trade_id != trade.trade_id);
            }

            // Record recovered leg's cost in position tracker
            {
                let recovery_cost = missing_fill_price * trade.shares;
                let mut pos = self.position.write().await;
                pos.record_position(recovery_cost);
            }

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.incomplete_recovered += 1;
                stats.incomplete_trades = stats.incomplete_trades.saturating_sub(1);
                stats.both_filled += 1;
                stats.total_volume += total_cost;
            }

            // Add to pending settlements for paper trading
            let settlement = PendingPaperSettlement {
                trade_id: format!("recovered-{}", trade.trade_id),
                coin1: trade.coin1.clone(),
                coin2: trade.coin2.clone(),
                leg1_direction: if trade.filled_leg == FilledLeg::Leg1 {
                    trade.filled_direction.clone()
                } else {
                    trade.missing_direction.clone()
                },
                leg2_direction: if trade.filled_leg == FilledLeg::Leg2 {
                    trade.filled_direction.clone()
                } else {
                    trade.missing_direction.clone()
                },
                leg1_token_id: if trade.filled_leg == FilledLeg::Leg1 {
                    trade.filled_token_id.clone()
                } else {
                    trade.missing_token_id.clone()
                },
                leg2_token_id: if trade.filled_leg == FilledLeg::Leg2 {
                    trade.filled_token_id.clone()
                } else {
                    trade.missing_token_id.clone()
                },
                total_cost,
                shares: trade.shares,
                window_end: trade.window_end,
                executed_at: Utc::now(),
                remaining_shares_leg1: trade.shares,
                remaining_shares_leg2: trade.shares,
                early_exit_proceeds: Decimal::ZERO,
                partially_exited: false,
            };

            {
                let mut pending = self.pending_settlements.write().await;
                pending.push(settlement);
            }

            completed_count += 1;
        }

        if completed_count > 0 || expired_count > 0 {
            info!(
                completed = completed_count,
                expired = expired_count,
                "Incomplete trade recovery: {} completed, {} expired",
                completed_count,
                expired_count
            );
        }

        Ok(completed_count)
    }

    /// Escape hatch: sells the filled leg at market price to recover capital.
    ///
    /// When recovery fails after MAX_RECOVERY_RETRIES or MAX_RECOVERY_AGE_SECS,
    /// we sell the filled position rather than staying stuck indefinitely.
    /// This accepts a small loss to free up capital for new trades.
    async fn escape_sell_filled_leg(
        &self,
        trade: &IncompleteTrade,
        live_prices: &std::collections::HashMap<String, (Decimal, Decimal)>,
    ) -> Result<(), CrossMarketAutoExecutorError> {
        // Get the current sell price for the filled leg
        let filled_coin = match trade.filled_leg {
            FilledLeg::Leg1 => trade.coin1.to_uppercase(),
            FilledLeg::Leg2 => trade.coin2.to_uppercase(),
        };

        let sell_price = match live_prices.get(&filled_coin) {
            Some(&(up_price, down_price)) => {
                // We're selling, so we want the bid price for our direction
                if trade.filled_direction == "UP" {
                    up_price
                } else {
                    down_price
                }
            }
            None => {
                return Err(CrossMarketAutoExecutorError::Filtered {
                    reason: format!("No price for {} to sell filled leg", filled_coin),
                });
            }
        };

        // Sell at slightly below market to ensure fill
        let sell_price = (sell_price * dec!(0.95)).max(dec!(0.01));

        info!(
            trade_id = %trade.trade_id,
            filled_token = %trade.filled_token_id,
            shares = %trade.shares,
            sell_price = %sell_price,
            original_buy_price = %trade.filled_price,
            "Escape hatch: selling filled leg to recover capital"
        );

        let order = OrderParams {
            token_id: trade.filled_token_id.clone(),
            side: Side::Sell,
            price: sell_price,
            size: trade.shares,
            order_type: OrderType::Fok,
            neg_risk: false,
            presigned: None,
        };

        let result = self.executor.submit_order(order).await?;

        match result.status {
            OrderStatus::Filled => {
                let recovered = result.fill_notional();
                let loss = (trade.filled_price - sell_price) * trade.shares;
                warn!(
                    trade_id = %trade.trade_id,
                    recovered = %recovered,
                    loss = %loss,
                    "Escape hatch: sold filled leg, accepted loss to free capital"
                );
                Ok(())
            }
            _ => {
                warn!(
                    trade_id = %trade.trade_id,
                    status = ?result.status,
                    "Escape sell not filled, will retry"
                );
                Err(CrossMarketAutoExecutorError::Filtered {
                    reason: "Escape sell not filled".to_string(),
                })
            }
        }
    }

    /// Parses a coin string to Coin enum.
    fn parse_coin(s: &str) -> Option<Coin> {
        match s.to_uppercase().as_str() {
            "BTC" | "BITCOIN" => Some(Coin::Btc),
            "ETH" | "ETHEREUM" => Some(Coin::Eth),
            "SOL" | "SOLANA" => Some(Coin::Sol),
            "XRP" | "RIPPLE" => Some(Coin::Xrp),
            _ => None,
        }
    }

    /// Checks if this opportunity should be executed.
    fn should_execute(&self, opp: &CrossMarketOpportunity) -> bool {
        // Check pair filter
        if let Some((c1, c2)) = &self.config.filter_pair {
            let opp_coin1 = Self::parse_coin(&opp.coin1);
            let opp_coin2 = Self::parse_coin(&opp.coin2);
            if let (Some(oc1), Some(oc2)) = (opp_coin1, opp_coin2) {
                if !((oc1 == *c1 && oc2 == *c2) || (oc1 == *c2 && oc2 == *c1)) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check combination filter
        if let Some(combo) = &self.config.filter_combination {
            if opp.combination != *combo {
                return false;
            }
        }

        // Check spread
        if opp.spread < self.config.min_spread {
            return false;
        }

        // Check win probability
        if opp.win_probability < self.config.min_win_probability {
            return false;
        }

        true
    }

    /// Executes both legs of the cross-market opportunity.
    async fn execute_both_legs(
        &self,
        opp: &CrossMarketOpportunity,
        shares: Decimal,
    ) -> Result<CrossMarketExecutionResult, CrossMarketAutoExecutorError> {
        // Create orders for both legs
        let leg1_order = OrderParams {
            token_id: opp.leg1_token_id.clone(),
            side: Side::Buy,
            price: opp.leg1_price,
            size: shares,
            order_type: OrderType::Fok,
            neg_risk: false,
            presigned: None,
        };

        let leg2_order = OrderParams {
            token_id: opp.leg2_token_id.clone(),
            side: Side::Buy,
            price: opp.leg2_price,
            size: shares,
            order_type: OrderType::Fok,
            neg_risk: false,
            presigned: None,
        };

        info!(
            pair = %format!("{}/{}", opp.coin1, opp.coin2),
            combo = ?opp.combination,
            leg1_price = %opp.leg1_price,
            leg2_price = %opp.leg2_price,
            shares = %shares,
            total_cost = %opp.total_cost,
            win_prob = opp.win_probability,
            "Executing cross-market trade"
        );

        // Sequential leg execution to prevent one-sided exposure:
        // 1. Try leg1 first
        // 2. Only submit leg2 if leg1 fills
        // This prevents buying one side without the other.
        let start = std::time::Instant::now();

        // Submit leg 1
        let leg1_result = match self.executor.submit_order(leg1_order).await {
            Ok(result) => result,
            Err(e) => {
                let leg1_rejected = OrderResult::rejected("leg1-error", e.to_string());
                let leg2_skipped = OrderResult::rejected("leg2-skipped", "Skipped: leg1 failed");
                warn!("Leg 1 error, skipping leg 2: {}", e);
                return Ok(CrossMarketExecutionResult::BothRejected {
                    leg1_result: leg1_rejected,
                    leg2_result: leg2_skipped,
                });
            }
        };

        // If leg 1 didn't fill, skip leg 2 entirely
        if leg1_result.status != OrderStatus::Filled {
            let leg2_skipped =
                OrderResult::rejected("leg2-skipped", "Skipped: leg1 not filled (no liquidity)");
            info!(
                leg1_status = ?leg1_result.status,
                "Leg 1 not filled, skipping leg 2 to prevent one-sided exposure"
            );
            let latency_ms = start.elapsed().as_millis() as u64;
            self.update_latency_stats(latency_ms).await;
            return Ok(CrossMarketExecutionResult::BothRejected {
                leg1_result,
                leg2_result: leg2_skipped,
            });
        }

        info!(
            leg1_filled = %leg1_result.filled_size,
            "Leg 1 filled, proceeding to leg 2"
        );

        // Submit leg 2
        let leg2_result = match self.executor.submit_order(leg2_order).await {
            Ok(result) => result,
            Err(e) => OrderResult::rejected("leg2-error", e.to_string()),
        };

        let latency_ms = start.elapsed().as_millis() as u64;
        self.update_latency_stats(latency_ms).await;

        let leg2_filled = leg2_result.status == OrderStatus::Filled;

        let result = if leg2_filled {
            let total_cost = leg1_result.fill_notional() + leg2_result.fill_notional();
            info!(total_cost = %total_cost, "Both legs filled successfully");
            CrossMarketExecutionResult::Success {
                leg1_result,
                leg2_result,
                total_cost,
                expected_payout: Decimal::ONE,
            }
        } else {
            warn!("Leg 1 filled but leg 2 failed - directional exposure!");
            CrossMarketExecutionResult::Leg1OnlyFilled {
                leg1_result,
                leg2_result,
            }
        };

        Ok(result)
    }

    /// Updates latency statistics.
    async fn update_latency_stats(&self, latency_ms: u64) {
        let mut stats = self.stats.write().await;
        stats.last_latency_ms = latency_ms;
        stats.latency_samples += 1;
        stats.avg_latency_ms = ((stats.avg_latency_ms * (stats.latency_samples - 1))
            + latency_ms)
            / stats.latency_samples;
    }

    /// Records a trade in history.
    async fn record_trade(
        &self,
        opp: &CrossMarketOpportunity,
        result: &CrossMarketExecutionResult,
        shares: Decimal,
    ) {
        let trade_id = format!(
            "cm-{}-{}",
            opp.detected_at.timestamp_millis(),
            opp.combination.to_string().to_lowercase()
        );

        let record = CrossMarketTradeRecord {
            trade_id: trade_id.clone(),
            pair: format!("{}/{}", opp.coin1, opp.coin2),
            combination: opp.combination,
            leg1_token_id: opp.leg1_token_id.clone(),
            leg2_token_id: opp.leg2_token_id.clone(),
            leg1_price: opp.leg1_price,
            leg2_price: opp.leg2_price,
            total_cost: opp.total_cost,
            shares,
            result: result.clone(),
            detected_at: opp.detected_at,
            executed_at: Utc::now(),
            win_probability: opp.win_probability,
            expected_value: opp.expected_value,
        };

        let mut history = self.history.write().await;
        history.push_back(record);
        while history.len() > self.config.max_history {
            history.pop_front();
        }

        // Update recent trades display in stats
        if result.is_success() {
            let mut stats = self.stats.write().await;
            let display = RecentTradeDisplay {
                trade_id: trade_id.clone(),
                pair: format!("{}/{}", opp.coin1, opp.coin2),
                leg1_dir: opp.leg1_direction.clone(),
                leg1_price: opp.leg1_price,
                leg2_dir: opp.leg2_direction.clone(),
                leg2_price: opp.leg2_price,
                total_cost: opp.total_cost,
                executed_at: Utc::now(),
            };
            stats.recent_trades.push(display);
            // Keep only last 10 trades
            if stats.recent_trades.len() > 10 {
                stats.recent_trades.remove(0);
            }
        }

        // Add to pending settlements for paper trading (if successful)
        if result.is_success() {
            self.add_pending_settlement(opp, shares).await;
        }

        // Persist to database if configured
        if let Some(pool) = &self.db_pool {
            if let Err(e) = self.persist_trade(pool, opp, result, shares).await {
                error!(error = %e, "Failed to persist trade to database");
            }
        }
    }

    /// Persists an executed trade to the database.
    async fn persist_trade(
        &self,
        pool: &PgPool,
        opp: &CrossMarketOpportunity,
        result: &CrossMarketExecutionResult,
        _shares: Decimal,
    ) -> Result<(), CrossMarketAutoExecutorError> {
        // Extract fill prices from result
        let (leg1_fill, leg2_fill, executed) = match result {
            CrossMarketExecutionResult::Success {
                leg1_result,
                leg2_result,
                ..
            } => (leg1_result.avg_fill_price, leg2_result.avg_fill_price, true),
            CrossMarketExecutionResult::Leg1OnlyFilled { leg1_result, .. } => {
                (leg1_result.avg_fill_price, None, true)
            }
            CrossMarketExecutionResult::Leg2OnlyFilled { leg2_result, .. } => {
                (None, leg2_result.avg_fill_price, true)
            }
            CrossMarketExecutionResult::BothRejected { .. } => {
                // Don't persist rejected trades
                return Ok(());
            }
        };

        // Calculate slippage if we have fill prices
        let slippage = match (leg1_fill, leg2_fill) {
            (Some(f1), Some(f2)) => {
                let expected = opp.leg1_price + opp.leg2_price;
                let actual = f1 + f2;
                Some(actual - expected)
            }
            _ => None,
        };

        // Calculate window end
        let window_end = {
            let ts = opp.detected_at.timestamp();
            let window_secs = 900; // 15 minutes
            let window_start = (ts / window_secs) * window_secs;
            let window_end_ts = window_start + window_secs;
            DateTime::from_timestamp(window_end_ts, 0).unwrap_or(opp.detected_at)
        };

        // Insert into database
        let result = sqlx::query(
            r#"
            INSERT INTO cross_market_opportunities
                (timestamp, coin1, coin2, combination,
                 leg1_direction, leg1_price, leg1_token_id,
                 leg2_direction, leg2_price, leg2_token_id,
                 total_cost, spread, expected_value, win_probability,
                 assumed_correlation, session_id, status, window_end,
                 leg1_bid_depth, leg1_ask_depth,
                 leg2_bid_depth, leg2_ask_depth,
                 executed, leg1_fill_price, leg2_fill_price, slippage)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18,
                    $19, $20, $21, $22, $23, $24, $25, $26)
            "#,
        )
        .bind(opp.detected_at)
        .bind(&opp.coin1)
        .bind(&opp.coin2)
        .bind(opp.combination.to_string())
        .bind(&opp.leg1_direction)
        .bind(opp.leg1_price)
        .bind(&opp.leg1_token_id)
        .bind(&opp.leg2_direction)
        .bind(opp.leg2_price)
        .bind(&opp.leg2_token_id)
        .bind(opp.total_cost)
        .bind(opp.spread)
        .bind(opp.expected_value)
        .bind(Decimal::from_f64_retain(opp.win_probability).unwrap_or(Decimal::ZERO))
        .bind(Decimal::from_f64_retain(opp.assumed_correlation).unwrap_or(Decimal::ZERO))
        .bind(&self.session_id)
        .bind("pending") // Will be settled later
        .bind(window_end)
        .bind(opp.leg1_bid_depth)
        .bind(opp.leg1_ask_depth)
        .bind(opp.leg2_bid_depth)
        .bind(opp.leg2_ask_depth)
        .bind(executed)
        .bind(leg1_fill)
        .bind(leg2_fill)
        .bind(slippage)
        .execute(pool)
        .await;

        match result {
            Ok(_) => {
                debug!(
                    pair = %format!("{}/{}", opp.coin1, opp.coin2),
                    session = %self.session_id,
                    "Trade persisted to database"
                );
                Ok(())
            }
            Err(e) => Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected(format!("Database error: {}", e)),
            )),
        }
    }

    // =========================================================================
    // Paper Settlement Methods
    // =========================================================================

    /// Adds a successful trade to pending settlements for paper trading.
    async fn add_pending_settlement(&self, opp: &CrossMarketOpportunity, shares: Decimal) {
        // Calculate window end
        let window_end = {
            let ts = opp.detected_at.timestamp();
            let window_secs = 900; // 15 minutes
            let window_start = (ts / window_secs) * window_secs;
            let window_end_ts = window_start + window_secs;
            DateTime::from_timestamp(window_end_ts, 0).unwrap_or(opp.detected_at)
        };

        let settlement = PendingPaperSettlement {
            trade_id: format!(
                "paper-{}-{}-{}",
                opp.coin1,
                opp.coin2,
                opp.detected_at.timestamp_millis()
            ),
            coin1: opp.coin1.clone(),
            coin2: opp.coin2.clone(),
            leg1_direction: opp.leg1_direction.clone(),
            leg2_direction: opp.leg2_direction.clone(),
            leg1_token_id: opp.leg1_token_id.clone(),
            leg2_token_id: opp.leg2_token_id.clone(),
            total_cost: opp.total_cost * shares, // Actual USDC spent, not pair ratio
            shares,
            window_end,
            executed_at: Utc::now(),
            remaining_shares_leg1: shares,
            remaining_shares_leg2: shares,
            early_exit_proceeds: Decimal::ZERO,
            partially_exited: false,
        };

        let mut pending = self.pending_settlements.write().await;
        pending.push(settlement.clone());

        let mut stats = self.stats.write().await;
        stats.pending_settlement = pending.len() as u64;

        // Update pending trades display
        stats.pending_trades.push(PendingTradeDisplay {
            trade_id: settlement.trade_id,
            pair: format!("{}/{}", opp.coin1, opp.coin2),
            leg1_dir: settlement.leg1_direction,
            leg2_dir: settlement.leg2_direction,
            total_cost: settlement.total_cost,
            window_end,
        });
    }

    /// Tries fast settlement using live WebSocket prices first, then CLOB.
    ///
    /// For trades where the window has closed:
    /// 1. Try using live_prices from WebSocket (most reliable)
    /// 2. Fall back to fetching CLOB prices directly
    ///
    /// Simple logic: If UP price > $0.50, coin went UP. Otherwise DOWN.
    async fn try_fast_settle_via_clob(&self) -> Result<u64, CrossMarketAutoExecutorError> {
        let now = Utc::now();
        let half = dec!(0.50);

        // Find trades with closed windows
        let trades_to_check: Vec<PendingPaperSettlement> = {
            let pending = self.pending_settlements.read().await;
            pending
                .iter()
                .filter(|s| now > s.window_end) // Window has closed
                .cloned()
                .collect()
        };

        if trades_to_check.is_empty() {
            return Ok(0);
        }

        // Get live prices from stats (updated by CLI from runner)
        let live_prices = {
            let stats = self.stats.read().await;
            stats.live_prices.clone()
        };

        info!(
            count = trades_to_check.len(),
            live_prices_available = !live_prices.is_empty(),
            "Checking {} pending trades for fast settlement",
            trades_to_check.len()
        );

        let mut settled_count = 0u64;

        for settlement in trades_to_check {
            let secs_since_close = (now - settlement.window_end).num_seconds();

            // Try to settle using live WebSocket prices first
            let c1_upper = settlement.coin1.to_uppercase();
            let c2_upper = settlement.coin2.to_uppercase();

            if let (Some(&(c1_up, _c1_down)), Some(&(c2_up, _c2_down))) =
                (live_prices.get(&c1_upper), live_prices.get(&c2_upper))
            {
                // Simple: if UP > 0.50, coin went UP. Otherwise DOWN.
                let c1_outcome = if c1_up > half { "UP" } else { "DOWN" };
                let c2_outcome = if c2_up > half { "UP" } else { "DOWN" };

                // Determine if our legs won
                let leg1_won = settlement.leg1_direction == c1_outcome;
                let leg2_won = settlement.leg2_direction == c2_outcome;

                info!(
                    trade_id = %settlement.trade_id,
                    secs_since_close = secs_since_close,
                    c1 = %c1_upper,
                    c1_up = %c1_up,
                    c1_outcome = %c1_outcome,
                    c2 = %c2_upper,
                    c2_up = %c2_up,
                    c2_outcome = %c2_outcome,
                    leg1_dir = %settlement.leg1_direction,
                    leg2_dir = %settlement.leg2_direction,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "SETTLING via live WebSocket prices"
                );

                // Finalize
                self.finalize_settlement(&settlement, leg1_won, leg2_won)
                    .await;

                // Remove from pending
                {
                    let mut pending = self.pending_settlements.write().await;
                    pending.retain(|s| s.trade_id != settlement.trade_id);

                    let mut stats = self.stats.write().await;
                    stats.pending_settlement = pending.len() as u64;
                    stats
                        .pending_trades
                        .retain(|t| t.trade_id != settlement.trade_id);
                }

                settled_count += 1;
                continue; // Move to next trade
            } else {
                info!(
                    trade_id = %settlement.trade_id,
                    c1 = %c1_upper,
                    c2 = %c2_upper,
                    available_coins = ?live_prices.keys().collect::<Vec<_>>(),
                    "No live prices for coins, trying CLOB API"
                );
            }

            // Fall back to CLOB API if live prices not available or not decisive
            let token_ids = vec![
                settlement.leg1_token_id.clone(),
                settlement.leg2_token_id.clone(),
            ];

            let url = format!(
                "https://clob.polymarket.com/prices?token_ids={}",
                token_ids.join(",")
            );

            let response = match self
                .http_client
                .get(&url)
                .header("Accept", "application/json")
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, trade_id = %settlement.trade_id, "Failed to fetch CLOB prices");
                    continue;
                }
            };

            let status = response.status();
            if !status.is_success() {
                warn!(
                    status = %status,
                    trade_id = %settlement.trade_id,
                    "CLOB API returned error - tokens may be expired"
                );
                continue;
            }

            #[derive(Deserialize, Debug)]
            struct PriceResponse {
                price: String,
            }

            let body = match response.text().await {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "Failed to read CLOB response body");
                    continue;
                }
            };

            let prices: std::collections::HashMap<String, PriceResponse> =
                match serde_json::from_str(&body) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(error = %e, body = %body, "Failed to parse CLOB prices JSON");
                        continue;
                    }
                };

            // Parse prices - these are the specific token prices
            let leg1_price = prices
                .get(&settlement.leg1_token_id)
                .and_then(|p| Decimal::from_str(&p.price).ok());
            let leg2_price = prices
                .get(&settlement.leg2_token_id)
                .and_then(|p| Decimal::from_str(&p.price).ok());

            if let (Some(p1), Some(p2)) = (leg1_price, leg2_price) {
                // Simple: if token price > 0.50, that token won
                let leg1_won = p1 > half;
                let leg2_won = p2 > half;

                info!(
                    trade_id = %settlement.trade_id,
                    leg1_token_price = %p1,
                    leg2_token_price = %p2,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "SETTLING via CLOB token prices"
                );

                self.finalize_settlement(&settlement, leg1_won, leg2_won)
                    .await;

                {
                    let mut pending = self.pending_settlements.write().await;
                    pending.retain(|s| s.trade_id != settlement.trade_id);

                    let mut stats = self.stats.write().await;
                    stats.pending_settlement = pending.len() as u64;
                    stats
                        .pending_trades
                        .retain(|t| t.trade_id != settlement.trade_id);
                }

                settled_count += 1;
            }
        }

        Ok(settled_count)
    }

    /// Tries fast settlement using live WebSocket prices.
    ///
    /// If prices are decisive (>$0.90 or <$0.10), we can infer the outcome
    /// without waiting for official resolution. This is faster and more reliable.
    ///
    /// # Arguments
    /// * `current_prices` - Map of coin -> (up_price, down_price) from WebSocket
    pub async fn try_fast_settle_with_prices(
        &self,
        current_prices: &std::collections::HashMap<String, (Decimal, Decimal)>,
    ) -> Result<u64, CrossMarketAutoExecutorError> {
        let now = Utc::now();
        let threshold_high = dec!(0.90); // If price > 0.90, consider it a winner
        let threshold_low = dec!(0.10); // If price < 0.10, consider it a loser

        let mut settled_count = 0u64;

        // Collect trades ready for fast settlement
        let mut to_settle = Vec::new();
        {
            let pending = self.pending_settlements.read().await;
            for settlement in pending.iter() {
                // Only try fast settle after window has closed
                if now <= settlement.window_end {
                    continue;
                }

                // Get prices for both coins
                let c1_prices = current_prices.get(&settlement.coin1.to_uppercase());
                let c2_prices = current_prices.get(&settlement.coin2.to_uppercase());

                if let (Some((c1_up, c1_down)), Some((c2_up, c2_down))) = (c1_prices, c2_prices) {
                    // Determine outcomes from decisive prices
                    let c1_outcome = if *c1_up > threshold_high {
                        Some("UP")
                    } else if *c1_down > threshold_high {
                        Some("DOWN")
                    } else if *c1_up < threshold_low {
                        Some("DOWN") // If UP is near 0, DOWN won
                    } else if *c1_down < threshold_low {
                        Some("UP") // If DOWN is near 0, UP won
                    } else {
                        None // Prices not decisive yet
                    };

                    let c2_outcome = if *c2_up > threshold_high {
                        Some("UP")
                    } else if *c2_down > threshold_high {
                        Some("DOWN")
                    } else if *c2_up < threshold_low {
                        Some("DOWN")
                    } else if *c2_down < threshold_low {
                        Some("UP")
                    } else {
                        None
                    };

                    if let (Some(c1), Some(c2)) = (c1_outcome, c2_outcome) {
                        to_settle.push((settlement.clone(), c1.to_string(), c2.to_string()));
                    }
                }
            }
        }

        // Settle the decisive trades
        for (settlement, c1_outcome, c2_outcome) in to_settle {
            let leg1_won = settlement.leg1_direction == c1_outcome;
            let leg2_won = settlement.leg2_direction == c2_outcome;

            info!(
                trade_id = %settlement.trade_id,
                c1 = &settlement.coin1,
                c1_outcome = %c1_outcome,
                c2 = &settlement.coin2,
                c2_outcome = %c2_outcome,
                leg1_won = leg1_won,
                leg2_won = leg2_won,
                "Fast settlement via live prices"
            );

            // Calculate P&L and update stats
            self.finalize_settlement(&settlement, leg1_won, leg2_won)
                .await;

            // Remove from pending
            {
                let mut pending = self.pending_settlements.write().await;
                pending.retain(|s| s.trade_id != settlement.trade_id);

                let mut stats = self.stats.write().await;
                stats.pending_settlement = pending.len() as u64;
                // Also remove from pending display
                stats
                    .pending_trades
                    .retain(|t| t.trade_id != settlement.trade_id);
            }

            settled_count += 1;
        }

        Ok(settled_count)
    }

    /// Attempts early exit on profitable positions before settlement.
    ///
    /// For each pending trade where the window is still open:
    /// 1. Checks if the combined position value exceeds cost by the profit threshold
    /// 2. Fetches order book depth for each leg
    /// 3. Sells chunks sized to available bid-side liquidity
    /// 4. Tracks partial exits and removes fully exited positions
    async fn try_early_exit(&self) -> Result<u64, CrossMarketAutoExecutorError> {
        let now = Utc::now();

        // Get live prices
        let live_prices = {
            let stats = self.stats.read().await;
            stats.live_prices.clone()
        };

        if live_prices.is_empty() {
            return Ok(0);
        }

        // Find trades with open windows that could be exited early
        let candidates: Vec<PendingPaperSettlement> = {
            let pending = self.pending_settlements.read().await;
            pending
                .iter()
                .filter(|s| now < s.window_end) // Window still open
                .filter(|s| s.remaining_shares_leg1 > Decimal::ZERO || s.remaining_shares_leg2 > Decimal::ZERO)
                .cloned()
                .collect()
        };

        if candidates.is_empty() {
            return Ok(0);
        }

        let mut exit_count = 0u64;

        for settlement in &candidates {
            let c1_upper = settlement.coin1.to_uppercase();
            let c2_upper = settlement.coin2.to_uppercase();

            // Look up live bid prices for our legs
            let (leg1_bid, leg2_bid) = match (live_prices.get(&c1_upper), live_prices.get(&c2_upper)) {
                (Some(&(c1_up, c1_down)), Some(&(c2_up, c2_down))) => {
                    // Map leg direction to the correct price
                    let l1_bid = if settlement.leg1_direction == "UP" { c1_up } else { c1_down };
                    let l2_bid = if settlement.leg2_direction == "UP" { c2_up } else { c2_down };
                    (l1_bid, l2_bid)
                }
                _ => continue, // No live prices for these coins
            };

            // Calculate current value and profit
            let current_value = settlement.remaining_shares_leg1 * leg1_bid
                + settlement.remaining_shares_leg2 * leg2_bid;

            // Proportional cost basis for remaining shares
            let original_shares = settlement.shares;
            let remaining_fraction = if original_shares > Decimal::ZERO {
                (settlement.remaining_shares_leg1 + settlement.remaining_shares_leg2)
                    / (Decimal::TWO * original_shares)
            } else {
                Decimal::ZERO
            };
            let cost_basis = settlement.total_cost * remaining_fraction;

            if cost_basis <= Decimal::ZERO {
                continue;
            }

            let profit_pct = (current_value - cost_basis) / cost_basis;

            if profit_pct < self.config.early_exit_profit_threshold {
                debug!(
                    trade_id = %settlement.trade_id,
                    profit_pct = %profit_pct,
                    threshold = %self.config.early_exit_profit_threshold,
                    leg1_bid = %leg1_bid,
                    leg2_bid = %leg2_bid,
                    "Early exit: profit below threshold"
                );
                continue;
            }

            info!(
                trade_id = %settlement.trade_id,
                profit_pct = %profit_pct,
                current_value = %current_value,
                cost_basis = %cost_basis,
                leg1_bid = %leg1_bid,
                leg2_bid = %leg2_bid,
                remaining_leg1 = %settlement.remaining_shares_leg1,
                remaining_leg2 = %settlement.remaining_shares_leg2,
                "Early exit: profit threshold met, attempting sell"
            );

            // Fetch order books for both tokens to determine sell size
            let (leg1_book, leg2_book) = {
                let url1 = format!(
                    "https://clob.polymarket.com/book?token_id={}",
                    settlement.leg1_token_id
                );
                let url2 = format!(
                    "https://clob.polymarket.com/book?token_id={}",
                    settlement.leg2_token_id
                );

                let (r1, r2) = tokio::join!(
                    self.http_client.get(&url1).timeout(std::time::Duration::from_secs(5)).send(),
                    self.http_client.get(&url2).timeout(std::time::Duration::from_secs(5)).send(),
                );

                let parse_book = |resp: Result<reqwest::Response, reqwest::Error>, _token_id: &str| async move {
                    let resp = resp.ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let body: serde_json::Value = resp.json().await.ok()?;
                    let bids = body.get("bids")?.as_array()?;
                    let total_bid_depth: Decimal = bids.iter().filter_map(|b| {
                        b.get("size")?.as_str()?.parse::<Decimal>().ok()
                    }).sum();
                    let best_bid: Option<Decimal> = bids.first()
                        .and_then(|b| b.get("price")?.as_str()?.parse::<Decimal>().ok());
                    Some((total_bid_depth, best_bid))
                };

                let b1 = parse_book(r1, &settlement.leg1_token_id).await;
                let b2 = parse_book(r2, &settlement.leg2_token_id).await;
                (b1, b2)
            };

            // Size sells based on bid depth
            let depth_fraction = self.config.early_exit_depth_fraction;

            let (leg1_sell_size, leg1_sell_price) = match leg1_book {
                Some((bid_depth, Some(best_bid))) if bid_depth > Decimal::ZERO => {
                    let max_sell = bid_depth * depth_fraction;
                    let sell_size = settlement.remaining_shares_leg1.min(max_sell);
                    if sell_size >= dec!(0.1) {
                        (sell_size, best_bid)
                    } else {
                        (Decimal::ZERO, Decimal::ZERO)
                    }
                }
                _ => (Decimal::ZERO, Decimal::ZERO),
            };

            let (leg2_sell_size, leg2_sell_price) = match leg2_book {
                Some((bid_depth, Some(best_bid))) if bid_depth > Decimal::ZERO => {
                    let max_sell = bid_depth * depth_fraction;
                    let sell_size = settlement.remaining_shares_leg2.min(max_sell);
                    if sell_size >= dec!(0.1) {
                        (sell_size, best_bid)
                    } else {
                        (Decimal::ZERO, Decimal::ZERO)
                    }
                }
                _ => (Decimal::ZERO, Decimal::ZERO),
            };

            if leg1_sell_size <= Decimal::ZERO && leg2_sell_size <= Decimal::ZERO {
                debug!(
                    trade_id = %settlement.trade_id,
                    "Early exit: insufficient bid depth for either leg"
                );
                continue;
            }

            // Submit FAK sell orders
            let mut leg1_filled = Decimal::ZERO;
            let mut leg2_filled = Decimal::ZERO;
            let mut leg1_proceeds = Decimal::ZERO;
            let mut leg2_proceeds = Decimal::ZERO;

            if leg1_sell_size > Decimal::ZERO {
                let sell_order = OrderParams::sell_fak(
                    &settlement.leg1_token_id,
                    leg1_sell_price,
                    leg1_sell_size,
                );
                match self.executor.submit_order(sell_order).await {
                    Ok(result) => {
                        if result.status == OrderStatus::Filled || result.status == OrderStatus::PartiallyFilled {
                            leg1_filled = result.filled_size;
                            leg1_proceeds = leg1_filled * result.avg_fill_price.unwrap_or(leg1_sell_price);
                            info!(
                                trade_id = %settlement.trade_id,
                                leg = "leg1",
                                filled = %leg1_filled,
                                proceeds = %leg1_proceeds,
                                "Early exit: leg1 sell filled"
                            );
                        } else {
                            debug!(
                                trade_id = %settlement.trade_id,
                                status = ?result.status,
                                "Early exit: leg1 sell not filled"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, trade_id = %settlement.trade_id, "Early exit: leg1 sell failed");
                    }
                }
            }

            if leg2_sell_size > Decimal::ZERO {
                let sell_order = OrderParams::sell_fak(
                    &settlement.leg2_token_id,
                    leg2_sell_price,
                    leg2_sell_size,
                );
                match self.executor.submit_order(sell_order).await {
                    Ok(result) => {
                        if result.status == OrderStatus::Filled || result.status == OrderStatus::PartiallyFilled {
                            leg2_filled = result.filled_size;
                            leg2_proceeds = leg2_filled * result.avg_fill_price.unwrap_or(leg2_sell_price);
                            info!(
                                trade_id = %settlement.trade_id,
                                leg = "leg2",
                                filled = %leg2_filled,
                                proceeds = %leg2_proceeds,
                                "Early exit: leg2 sell filled"
                            );
                        } else {
                            debug!(
                                trade_id = %settlement.trade_id,
                                status = ?result.status,
                                "Early exit: leg2 sell not filled"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, trade_id = %settlement.trade_id, "Early exit: leg2 sell failed");
                    }
                }
            }

            let total_proceeds = leg1_proceeds + leg2_proceeds;

            if leg1_filled > Decimal::ZERO || leg2_filled > Decimal::ZERO {
                // Check if fully exited (extract data, then drop lock before acquiring stats)
                let fully_exited_data: Option<(Decimal, String, u64)> = {
                    let mut pending = self.pending_settlements.write().await;
                    if let Some(s) = pending.iter_mut().find(|s| s.trade_id == settlement.trade_id) {
                        // Clamp to zero to avoid negative from rounding
                        s.remaining_shares_leg1 = (s.remaining_shares_leg1 - leg1_filled).max(Decimal::ZERO);
                        s.remaining_shares_leg2 = (s.remaining_shares_leg2 - leg2_filled).max(Decimal::ZERO);
                        s.early_exit_proceeds += total_proceeds;
                        s.partially_exited = true;

                        info!(
                            trade_id = %settlement.trade_id,
                            remaining_leg1 = %s.remaining_shares_leg1,
                            remaining_leg2 = %s.remaining_shares_leg2,
                            total_early_proceeds = %s.early_exit_proceeds,
                            "Early exit: position updated"
                        );

                        // Check if fully exited
                        if s.remaining_shares_leg1 <= Decimal::ZERO
                            && s.remaining_shares_leg2 <= Decimal::ZERO
                        {
                            let early_proceeds = s.early_exit_proceeds;
                            let pnl = early_proceeds - settlement.total_cost;
                            let trade_id = settlement.trade_id.clone();

                            info!(
                                trade_id = %trade_id,
                                proceeds = %early_proceeds,
                                cost = %settlement.total_cost,
                                pnl = %pnl,
                                "Early exit: FULLY EXITED"
                            );

                            // Credit balance while holding pending lock (no other lock needed)
                            if let Err(e) = self.executor.credit_balance(early_proceeds).await {
                                warn!(error = %e, "Failed to credit early exit proceeds");
                            }

                            // Remove from pending
                            pending.retain(|s| s.trade_id != trade_id);
                            let pending_len = pending.len();

                            Some((early_proceeds, trade_id, pending_len as u64))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }; // pending lock dropped here

                // Update stats without holding pending lock (avoids deadlock)
                if let Some((early_proceeds, trade_id, pending_len)) = fully_exited_data {
                    let pnl = early_proceeds - settlement.total_cost;
                    let mut stats = self.stats.write().await;
                    stats.pending_settlement = pending_len;
                    stats.pending_trades.retain(|t| t.trade_id != trade_id);
                    stats.early_exits += 1;
                    stats.early_exit_proceeds += early_proceeds;
                    stats.realized_pnl += pnl;
                    if pnl > Decimal::ZERO {
                        stats.settled_wins += 1;
                    } else {
                        stats.settled_losses += 1;
                    }
                }

                exit_count += 1;
            }
        }

        if exit_count > 0 {
            info!(
                count = exit_count,
                "Early exit: processed {} trade(s)",
                exit_count
            );
        }

        Ok(exit_count)
    }

    /// Checks and settles any pending paper trades whose windows have closed.
    async fn check_pending_settlements(&self) -> Result<(), CrossMarketAutoExecutorError> {
        // First, try fast settlement via CLOB prices for any closed windows
        if let Err(e) = self.try_fast_settle_via_clob().await {
            debug!(error = %e, "Fast settlement check failed (non-fatal)");
        }

        let now = Utc::now();
        let settlement_delay = chrono::Duration::seconds(30); // Reduced from 120s since we try fast settle first

        // Collect trades ready for settlement
        let mut to_settle = Vec::new();
        let mut waiting_count = 0;
        {
            let pending = self.pending_settlements.read().await;
            for settlement in pending.iter() {
                let ready_at = settlement.window_end + settlement_delay;
                if now > ready_at {
                    to_settle.push(settlement.clone());
                } else {
                    waiting_count += 1;
                    let wait_secs = (ready_at - now).num_seconds();
                    debug!(
                        trade_id = %settlement.trade_id,
                        window_end = %settlement.window_end,
                        ready_at = %ready_at,
                        wait_secs = wait_secs,
                        "Trade not yet ready for settlement"
                    );
                }
            }
        }

        if to_settle.is_empty() {
            if waiting_count > 0 {
                debug!(
                    waiting = waiting_count,
                    "No trades ready, {} still waiting", waiting_count
                );
            }
            return Ok(());
        }

        info!(
            ready = to_settle.len(),
            waiting = waiting_count,
            "Settling ready trades"
        );

        for settlement in to_settle {
            info!(
                trade_id = %settlement.trade_id,
                pair = %format!("{}/{}", settlement.coin1, settlement.coin2),
                window_end = %settlement.window_end,
                "Attempting settlement"
            );

            match self.settle_paper_trade(&settlement).await {
                Ok(()) => {
                    // Remove from pending
                    let mut pending = self.pending_settlements.write().await;
                    pending.retain(|s| s.trade_id != settlement.trade_id);

                    let mut stats = self.stats.write().await;
                    stats.pending_settlement = pending.len() as u64;
                }
                Err(e) => {
                    // Keep in pending for retry
                    warn!(
                        trade_id = %settlement.trade_id,
                        error = %e,
                        "Settlement failed, will retry"
                    );
                }
            }
        }

        Ok(())
    }

    /// Settles a single paper trade by checking Polymarket token prices.
    ///
    /// After market resolution:
    /// - Winning tokens trade at $1.00 (or very close)
    /// - Losing tokens trade at $0.00 (or very close)
    ///
    /// We query the actual token prices to determine outcomes, which matches
    /// Polymarket's Chainlink-based resolution exactly.
    ///
    /// Settlement flow:
    /// 1. Try CLOB token prices (works for recently closed markets)
    /// 2. Fall back to Binance klines if CLOB fails (for resolved markets)
    async fn settle_paper_trade(
        &self,
        settlement: &PendingPaperSettlement,
    ) -> Result<(), CrossMarketAutoExecutorError> {
        // Try to get outcomes - Gamma API first (official), then CLOB prices, then Binance fallback
        let (leg1_won, leg2_won) = match self.try_settle_via_gamma(settlement).await {
            Ok(result) => {
                info!(trade_id = %settlement.trade_id, "Settled via Gamma API (official outcomes)");
                result
            }
            Err(e) => {
                debug!(
                    trade_id = %settlement.trade_id,
                    error = %e,
                    "Gamma API settlement not available, trying CLOB"
                );
                // Try CLOB prices
                match self.try_settle_via_clob(settlement).await {
                    Ok(result) => {
                        info!(trade_id = %settlement.trade_id, "Settled via CLOB prices");
                        result
                    }
                    Err(e2) => {
                        info!(
                            trade_id = %settlement.trade_id,
                            error = %e2,
                            "CLOB settlement failed, trying Binance fallback"
                        );
                        // Fall back to Binance
                        self.try_settle_via_binance(settlement).await?
                    }
                }
            }
        };

        self.finalize_settlement(settlement, leg1_won, leg2_won)
            .await;
        Ok(())
    }

    /// Finalizes settlement by calculating P&L and updating stats.
    async fn finalize_settlement(
        &self,
        settlement: &PendingPaperSettlement,
        leg1_won: bool,
        leg2_won: bool,
    ) {
        // Derive coin outcomes from leg results
        // If leg won, coin moved in the direction we bet on
        let c1_out = if leg1_won {
            settlement.leg1_direction.clone()
        } else if settlement.leg1_direction == "UP" {
            "DOWN".to_string()
        } else {
            "UP".to_string()
        };
        let c2_out = if leg2_won {
            settlement.leg2_direction.clone()
        } else if settlement.leg2_direction == "UP" {
            "DOWN".to_string()
        } else {
            "UP".to_string()
        };

        // Calculate payout
        let trade_result = match (leg1_won, leg2_won) {
            (true, true) => "DOUBLE_WIN",
            (true, false) | (false, true) => "WIN",
            (false, false) => "LOSE",
        };

        // For partially exited trades, only settle remaining shares
        // Leg 1 settles at $1 if won, $0 if lost. Same for leg 2.
        let leg1_payout_rate = if leg1_won { Decimal::ONE } else { Decimal::ZERO };
        let leg2_payout_rate = if leg2_won { Decimal::ONE } else { Decimal::ZERO };
        let remaining_payout = settlement.remaining_shares_leg1 * leg1_payout_rate
            + settlement.remaining_shares_leg2 * leg2_payout_rate;
        let fees = remaining_payout * self.fee_rate;
        let net_payout = remaining_payout - fees + settlement.early_exit_proceeds;

        // Cost basis is the full original cost
        let pnl = net_payout - settlement.total_cost;

        info!(
            trade_id = %settlement.trade_id,
            pair = %format!("{}/{}", settlement.coin1, settlement.coin2),
            c1_outcome = %c1_out,
            c2_outcome = %c2_out,
            result = trade_result,
            payout = %net_payout,
            pnl = %pnl,
            partially_exited = settlement.partially_exited,
            early_exit_proceeds = %settlement.early_exit_proceeds,
            "Paper trade settled"
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.realized_pnl += pnl;

            match trade_result {
                "DOUBLE_WIN" => {
                    stats.settled_wins += 1;
                    stats.double_wins += 1;
                }
                "WIN" => {
                    stats.settled_wins += 1;
                }
                _ => {
                    stats.settled_losses += 1;
                }
            }

            // Update paper balance tracking
            stats.paper_balance += net_payout;
        }

        // Credit the executor's balance back (essential for paper trading)
        // In live mode, this is a no-op (balance comes from chain)
        if let Err(e) = self.executor.credit_balance(net_payout).await {
            warn!(error = %e, "Failed to credit executor balance");
        }

        // Update database status if persistence is enabled
        if let Some(pool) = &self.db_pool {
            let result = sqlx::query(
                r#"
                UPDATE cross_market_opportunities
                SET status = 'settled',
                    coin1_outcome = $1,
                    coin2_outcome = $2,
                    trade_result = $3,
                    actual_pnl = $4,
                    correlation_correct = $5,
                    settled_at = NOW()
                WHERE session_id = $6
                  AND timestamp = $7
                "#,
            )
            .bind(&c1_out)
            .bind(&c2_out)
            .bind(trade_result) // 'WIN', 'DOUBLE_WIN', or 'LOSE'
            .bind(pnl)
            .bind(c1_out == c2_out) // correlation correct if same direction
            .bind(&self.session_id)
            .bind(settlement.executed_at)
            .execute(pool)
            .await;

            if let Err(e) = result {
                warn!(error = %e, "Failed to update settlement status in database");
            }
        }
    }

    /// Fetches coin outcome (UP or DOWN) from Binance using 1m candles.
    ///
    /// Uses 1m candles instead of 15m because Binance candles don't align with
    /// Polymarket's ET-based windows. We fetch candles for the entire window
    /// and compare the open of the first candle to the close of the last.
    async fn get_coin_outcome(
        &self,
        coin: &str,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Option<String>, CrossMarketAutoExecutorError> {
        // Convert coin to Binance symbol
        let symbol = match coin.to_uppercase().as_str() {
            "BTC" => "BTCUSDT",
            "ETH" => "ETHUSDT",
            "SOL" => "SOLUSDT",
            "XRP" => "XRPUSDT",
            other => {
                return Err(CrossMarketAutoExecutorError::Execution(
                    ExecutionError::rejected(format!("Unknown coin: {}", other)),
                ));
            }
        };

        let start_ms = window_start.timestamp_millis();
        let end_ms = window_end.timestamp_millis();

        // Use 1m candles to get precise window boundaries
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol={}&interval=1m&startTime={}&endTime={}&limit=20",
            symbol, start_ms, end_ms
        );

        debug!(
            symbol = symbol,
            window_start = %window_start,
            window_end = %window_end,
            url = %url,
            "Fetching Binance candles for settlement"
        );

        let response = self
            .http_client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                    "HTTP error: {}",
                    e
                )))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            warn!(symbol = symbol, status = %status, "Binance API error");
            return Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected(format!("Binance API error: {}", status)),
            ));
        }

        // Parse klines: [open_time, open, high, low, close, volume, close_time, ...]
        let klines: Vec<Vec<serde_json::Value>> = response.json().await.map_err(|e| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                "JSON parse error: {}",
                e
            )))
        })?;

        if klines.is_empty() {
            warn!(symbol = symbol, "No candles returned from Binance");
            return Ok(None);
        }

        // Get the first candle's open price
        let first_kline = &klines[0];
        let open_str = first_kline.get(1).and_then(|v| v.as_str()).ok_or_else(|| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(
                "Invalid first kline open price".to_string(),
            ))
        })?;

        // Get the last candle's close price
        let last_kline = &klines[klines.len() - 1];

        // Check if the last kline is closed (close_time < now)
        let close_time_ms = last_kline.get(6).and_then(|v| v.as_i64()).ok_or_else(|| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(
                "Invalid kline close_time".to_string(),
            ))
        })?;

        let now_ms = Utc::now().timestamp_millis();
        if close_time_ms > now_ms {
            debug!(
                symbol = symbol,
                close_time_ms = close_time_ms,
                now_ms = now_ms,
                "Last candle not yet closed"
            );
            return Ok(None); // Window not fully closed yet
        }

        let close_str = last_kline.get(4).and_then(|v| v.as_str()).ok_or_else(|| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(
                "Invalid last kline close price".to_string(),
            ))
        })?;

        let open: f64 = open_str.parse().map_err(|_| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(
                "Invalid open price format".to_string(),
            ))
        })?;
        let close: f64 = close_str.parse().map_err(|_| {
            CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(
                "Invalid close price format".to_string(),
            ))
        })?;

        // UP if close > open, DOWN otherwise
        let outcome = if close > open { "UP" } else { "DOWN" };

        info!(
            symbol = symbol,
            open = open,
            close = close,
            candle_count = klines.len(),
            outcome = outcome,
            "Determined coin outcome from Binance"
        );

        Ok(Some(outcome.to_string()))
    }

    /// Tries to settle via Gamma API (official market outcomes).
    ///
    /// This is the most reliable source as it uses Polymarket's official
    /// resolution data from the `winner` field on tokens.
    async fn try_settle_via_gamma(
        &self,
        settlement: &PendingPaperSettlement,
    ) -> Result<(bool, bool), CrossMarketAutoExecutorError> {
        // Parse coins from settlement
        let coin1 = match settlement.coin1.to_uppercase().as_str() {
            "BTC" => Coin::Btc,
            "ETH" => Coin::Eth,
            "SOL" => Coin::Sol,
            "XRP" => Coin::Xrp,
            other => {
                return Err(CrossMarketAutoExecutorError::Execution(
                    ExecutionError::rejected(format!("Unknown coin: {}", other)),
                ));
            }
        };

        let coin2 = match settlement.coin2.to_uppercase().as_str() {
            "BTC" => Coin::Btc,
            "ETH" => Coin::Eth,
            "SOL" => Coin::Sol,
            "XRP" => Coin::Xrp,
            other => {
                return Err(CrossMarketAutoExecutorError::Execution(
                    ExecutionError::rejected(format!("Unknown coin: {}", other)),
                ));
            }
        };

        // Get outcomes from Gamma API
        let c1_outcome = self
            .gamma_client
            .get_market_outcome(coin1, settlement.window_end)
            .await
            .map_err(|e| {
                CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                    "Gamma API error for {}: {}",
                    coin1.slug_prefix(),
                    e
                )))
            })?;

        let c2_outcome = self
            .gamma_client
            .get_market_outcome(coin2, settlement.window_end)
            .await
            .map_err(|e| {
                CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                    "Gamma API error for {}: {}",
                    coin2.slug_prefix(),
                    e
                )))
            })?;

        match (c1_outcome, c2_outcome) {
            (Some(c1), Some(c2)) => {
                // Leg won if our bet direction matches actual outcome
                let leg1_won = settlement.leg1_direction == c1;
                let leg2_won = settlement.leg2_direction == c2;

                info!(
                    trade_id = %settlement.trade_id,
                    coin1_outcome = %c1,
                    coin2_outcome = %c2,
                    leg1_direction = %settlement.leg1_direction,
                    leg2_direction = %settlement.leg2_direction,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "Got official outcomes from Gamma API"
                );
                Ok((leg1_won, leg2_won))
            }
            _ => Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected(
                    "Market outcomes not yet available from Gamma".to_string(),
                ),
            )),
        }
    }

    /// Tries to settle via CLOB token prices.
    async fn try_settle_via_clob(
        &self,
        settlement: &PendingPaperSettlement,
    ) -> Result<(bool, bool), CrossMarketAutoExecutorError> {
        let token_ids = vec![
            settlement.leg1_token_id.clone(),
            settlement.leg2_token_id.clone(),
        ];

        let url = format!(
            "https://clob.polymarket.com/prices?token_ids={}",
            token_ids.join(",")
        );

        let response = self
            .http_client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                    "HTTP error: {}",
                    e
                )))
            })?;

        if !response.status().is_success() {
            return Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected(format!("CLOB API error: {}", response.status())),
            ));
        }

        let prices: std::collections::HashMap<String, serde_json::Value> =
            response.json().await.map_err(|e| {
                CrossMarketAutoExecutorError::Execution(ExecutionError::rejected(format!(
                    "JSON parse error: {}",
                    e
                )))
            })?;

        let leg1_price = prices
            .get(&settlement.leg1_token_id)
            .and_then(|v| v.get("price").or(v.get("mid")))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| {
                prices
                    .get(&settlement.leg1_token_id)
                    .and_then(|v| v.as_f64())
            });

        let leg2_price = prices
            .get(&settlement.leg2_token_id)
            .and_then(|v| v.get("price").or(v.get("mid")))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| {
                prices
                    .get(&settlement.leg2_token_id)
                    .and_then(|v| v.as_f64())
            });

        match (leg1_price, leg2_price) {
            (Some(p1), Some(p2)) => {
                let l1_won = p1 >= 0.95;
                let l2_won = p2 >= 0.95;
                debug!(
                    trade_id = %settlement.trade_id,
                    leg1_price = p1,
                    leg2_price = p2,
                    leg1_won = l1_won,
                    leg2_won = l2_won,
                    "Settled via CLOB prices"
                );
                Ok((l1_won, l2_won))
            }
            _ => Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected("Token prices not available in CLOB".to_string()),
            )),
        }
    }

    /// Tries to settle via Binance klines (fallback when CLOB fails).
    async fn try_settle_via_binance(
        &self,
        settlement: &PendingPaperSettlement,
    ) -> Result<(bool, bool), CrossMarketAutoExecutorError> {
        let window_end = settlement.window_end;
        let window_start = window_end - chrono::Duration::minutes(15);

        let c1_outcome = self
            .get_coin_outcome(&settlement.coin1, window_start, window_end)
            .await?;
        let c2_outcome = self
            .get_coin_outcome(&settlement.coin2, window_start, window_end)
            .await?;

        match (c1_outcome, c2_outcome) {
            (Some(c1), Some(c2)) => {
                // Leg won if our bet direction matches actual outcome
                let leg1_won = settlement.leg1_direction == c1;
                let leg2_won = settlement.leg2_direction == c2;

                info!(
                    trade_id = %settlement.trade_id,
                    coin1_outcome = %c1,
                    coin2_outcome = %c2,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "Settled via Binance fallback"
                );
                Ok((leg1_won, leg2_won))
            }
            _ => Err(CrossMarketAutoExecutorError::Execution(
                ExecutionError::rejected("Coin outcomes not available from Binance".to_string()),
            )),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::paper_executor::{PaperExecutor, PaperExecutorConfig};

    // =========================================================================
    // Kelly Sizer Tests
    // =========================================================================

    #[test]
    fn test_kelly_sizer_positive_edge() {
        let sizer = CrossMarketKellySizer::new(0.25, dec!(5), dec!(50));

        // 95% win probability, 0.85 total cost = strong edge
        // EV = 0.95 * (1.00 - 0.85) - 0.05 * 0.85 = 0.95 * 0.15 - 0.0425 = 0.1425 - 0.0425 = 0.10 (10% EV)
        let size = sizer.size(0.95, dec!(0.85), dec!(1000));

        assert!(size.is_some(), "Should recommend a bet with strong edge");
        let size = size.unwrap();
        assert!(size >= dec!(5), "Size {} should be >= $5", size);
        assert!(size <= dec!(50), "Size {} should be <= $50", size);
    }

    #[test]
    fn test_kelly_sizer_no_edge() {
        let sizer = CrossMarketKellySizer::new(0.25, dec!(5), dec!(50));

        // 50% win probability, 0.96 total cost = negative edge (EV = 0.50 - 0.96 < 0)
        let size = sizer.size(0.50, dec!(0.96), dec!(1000));

        assert!(size.is_none(), "Should recommend no bet with negative edge");
    }

    #[test]
    fn test_kelly_sizer_high_correlation() {
        let sizer = CrossMarketKellySizer::new(0.25, dec!(5), dec!(50));

        // 92% win probability (like BTC/ETH), 0.80 cost = good spread
        // EV = 0.92 * 0.20 - 0.08 * 0.80 = 0.184 - 0.064 = 0.12 (12% EV)
        let size = sizer.size(0.92, dec!(0.80), dec!(500));

        assert!(size.is_some(), "Should recommend a bet with 12% edge");
        let size = size.unwrap();
        // With high win prob and good cost, should recommend a bet
        assert!(size >= dec!(5));
    }

    #[test]
    fn test_kelly_sizer_respects_min() {
        let sizer = CrossMarketKellySizer::new(0.001, dec!(10), dec!(50)); // Very tiny fraction

        // Small Kelly with small bankroll = below minimum
        let size = sizer.size(0.90, dec!(0.96), dec!(100));

        assert!(size.is_none(), "Should return None when below minimum");
    }

    #[test]
    fn test_kelly_sizer_respects_max() {
        let sizer = CrossMarketKellySizer::new(1.0, dec!(5), dec!(25)); // Full Kelly, max $25

        // Huge edge would suggest large bet, but capped
        let size = sizer.size(0.99, dec!(0.80), dec!(10000));

        assert!(size.is_some());
        assert!(size.unwrap() <= dec!(25), "Should be capped at max");
    }

    // =========================================================================
    // Window Tracker Tests
    // =========================================================================

    #[test]
    fn test_window_tracker_empty() {
        let tracker = CrossMarketWindowTracker::new(0);

        assert_eq!(tracker.total_cost, Decimal::ZERO);
        assert_eq!(tracker.position_count, 0);
        assert_eq!(tracker.remaining_capacity(dec!(200)), dec!(200));
    }

    #[test]
    fn test_window_tracker_record_position() {
        let mut tracker = CrossMarketWindowTracker::new(0);

        tracker.record_position(dec!(50));
        assert_eq!(tracker.total_cost, dec!(50));
        assert_eq!(tracker.position_count, 1);
        assert_eq!(tracker.remaining_capacity(dec!(200)), dec!(150));

        tracker.record_position(dec!(75));
        assert_eq!(tracker.total_cost, dec!(125));
        assert_eq!(tracker.position_count, 2);
        assert_eq!(tracker.remaining_capacity(dec!(200)), dec!(75));
    }

    #[test]
    fn test_window_tracker_clear() {
        let mut tracker = CrossMarketWindowTracker::new(0);
        tracker.record_position(dec!(100));

        tracker.clear();

        assert_eq!(tracker.total_cost, Decimal::ZERO);
        assert_eq!(tracker.position_count, 0);
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = CrossMarketAutoExecutorConfig::default();

        assert!(config.filter_pair.is_none());
        assert!(config.filter_combination.is_none());
        assert!((config.kelly_fraction - 0.25).abs() < 0.001);
        assert_eq!(config.min_bet_size, dec!(5));
        assert_eq!(config.max_bet_size, dec!(50));
    }

    #[test]
    fn test_config_btc_eth_only() {
        let config = CrossMarketAutoExecutorConfig::btc_eth_only();

        assert_eq!(config.filter_pair, Some((Coin::Btc, Coin::Eth)));
        assert_eq!(
            config.filter_combination,
            Some(CrossMarketCombination::Coin1DownCoin2Up)
        );
    }

    #[test]
    fn test_config_builder() {
        let config = CrossMarketAutoExecutorConfig::default()
            .with_pair_filter(Coin::Btc, Coin::Sol)
            .with_combination_filter(CrossMarketCombination::Coin1UpCoin2Down)
            .with_fixed_bet(dec!(25))
            .with_kelly_fraction(0.5);

        assert_eq!(config.filter_pair, Some((Coin::Btc, Coin::Sol)));
        assert_eq!(
            config.filter_combination,
            Some(CrossMarketCombination::Coin1UpCoin2Down)
        );
        assert_eq!(config.fixed_bet_size, Some(dec!(25)));
        assert!((config.kelly_fraction - 0.5).abs() < 0.001);
    }

    // =========================================================================
    // Stats Tests
    // =========================================================================

    #[test]
    fn test_stats_default() {
        let stats = CrossMarketAutoExecutorStats::default();

        assert_eq!(stats.opportunities_received, 0);
        assert_eq!(stats.executions_attempted, 0);
        assert_eq!(stats.total_volume, Decimal::ZERO);
        assert!(stats.started_at.is_none());
    }

    // =========================================================================
    // Execution Result Tests
    // =========================================================================

    #[test]
    fn test_execution_result_success() {
        let result = CrossMarketExecutionResult::Success {
            leg1_result: OrderResult::filled("leg1", dec!(100), dec!(0.48)),
            leg2_result: OrderResult::filled("leg2", dec!(100), dec!(0.48)),
            total_cost: dec!(96),
            expected_payout: dec!(1),
        };

        assert!(result.is_success());
        assert!(!result.is_partial());
    }

    #[test]
    fn test_execution_result_partial() {
        let result = CrossMarketExecutionResult::Leg1OnlyFilled {
            leg1_result: OrderResult::filled("leg1", dec!(100), dec!(0.48)),
            leg2_result: OrderResult::rejected("leg2", "No fill"),
        };

        assert!(!result.is_success());
        assert!(result.is_partial());
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_auto_executor_creation() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only();
        let auto = CrossMarketAutoExecutor::new(executor, auto_config);

        assert_eq!(auto.config().filter_pair, Some((Coin::Btc, Coin::Eth)));
    }

    #[tokio::test]
    async fn test_auto_executor_stop_handle() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        let auto = CrossMarketAutoExecutor::new(executor, CrossMarketAutoExecutorConfig::default());

        let stop = auto.stop_handle();
        assert!(!stop.load(Ordering::SeqCst));

        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }

    fn create_test_opportunity() -> CrossMarketOpportunity {
        CrossMarketOpportunity {
            coin1: "BTC".to_string(),
            coin2: "ETH".to_string(),
            combination: CrossMarketCombination::Coin1DownCoin2Up,
            leg1_direction: "DOWN".to_string(),
            leg1_price: dec!(0.48),
            leg1_token_id: "btc-down-token".to_string(),
            leg2_direction: "UP".to_string(),
            leg2_price: dec!(0.48),
            leg2_token_id: "eth-up-token".to_string(),
            total_cost: dec!(0.96),
            spread: dec!(0.04),
            expected_value: dec!(0.02),
            assumed_correlation: 0.73,
            win_probability: 0.92,
            detected_at: Utc::now(),
            leg1_bid_depth: None,
            leg1_ask_depth: Some(dec!(1000)),
            leg1_spread_bps: None,
            leg2_bid_depth: None,
            leg2_ask_depth: Some(dec!(1000)),
            leg2_spread_bps: None,
        }
    }

    #[tokio::test]
    async fn test_auto_executor_handles_opportunity() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0, // Always fill
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only().with_fixed_bet(dec!(20));

        let mut auto = CrossMarketAutoExecutor::new(executor, auto_config);

        let opp = create_test_opportunity();

        // Handle the opportunity directly
        let result = auto.handle_opportunity(opp).await;
        assert!(result.is_ok());

        // Check stats
        let stats = auto.stats.read().await;
        assert_eq!(stats.opportunities_received, 1);
        assert_eq!(stats.executions_attempted, 1);
        assert_eq!(stats.both_filled, 1);
        assert!(stats.total_volume > Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_auto_executor_filters_wrong_pair() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        // Config only allows BTC/ETH
        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only();
        let mut auto = CrossMarketAutoExecutor::new(executor, auto_config);

        // Create SOL/XRP opportunity (wrong pair)
        let mut opp = create_test_opportunity();
        opp.coin1 = "SOL".to_string();
        opp.coin2 = "XRP".to_string();

        let result = auto.handle_opportunity(opp).await;
        assert!(result.is_ok());

        // Should be skipped
        let stats = auto.stats.read().await;
        assert_eq!(stats.opportunities_received, 1);
        assert_eq!(stats.opportunities_skipped, 1);
        assert_eq!(stats.executions_attempted, 0);
    }

    #[tokio::test]
    async fn test_auto_executor_filters_wrong_combination() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        // Config only allows Coin1DownCoin2Up
        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only();
        let mut auto = CrossMarketAutoExecutor::new(executor, auto_config);

        // Create BothUp opportunity (wrong combo)
        let mut opp = create_test_opportunity();
        opp.combination = CrossMarketCombination::BothUp;

        let result = auto.handle_opportunity(opp).await;
        assert!(result.is_ok());

        // Should be skipped
        let stats = auto.stats.read().await;
        assert_eq!(stats.opportunities_skipped, 1);
        assert_eq!(stats.executions_attempted, 0);
    }

    #[tokio::test]
    async fn test_auto_executor_filters_low_win_probability() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only();
        let mut auto = CrossMarketAutoExecutor::new(executor, auto_config);

        // Create opportunity with low win probability
        let mut opp = create_test_opportunity();
        opp.win_probability = 0.50; // Below 0.85 threshold

        let result = auto.handle_opportunity(opp).await;
        assert!(result.is_ok());

        // Should be skipped
        let stats = auto.stats.read().await;
        assert_eq!(stats.opportunities_skipped, 1);
    }

    #[tokio::test]
    async fn test_auto_executor_position_limit() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        // Fixed bet of $20 per leg = $40 total per opportunity
        let auto_config = CrossMarketAutoExecutorConfig::btc_eth_only().with_fixed_bet(dec!(20));

        let mut auto = CrossMarketAutoExecutor::new(executor, auto_config);

        // Create opportunity first to get its window
        let opp = create_test_opportunity();
        let opp_window_ms = (opp.detected_at.timestamp_millis() / 900_000) * 900_000;

        // Set position tracker to exceed limit, IN THE SAME WINDOW
        {
            let mut pos = auto.position.write().await;
            pos.window_start_ms = opp_window_ms; // Same window as opportunity
            pos.total_cost = dec!(200); // At the $200 limit, so remaining = 0
        }

        // Try to execute - should fail because remaining (0) < total_cost (0.96)
        let result = auto.handle_opportunity(opp).await;
        assert!(result.is_ok());

        // Should be skipped due to position limit
        let stats = auto.stats.read().await;
        assert_eq!(
            stats.opportunities_skipped, 1,
            "Should skip due to position limit"
        );
    }
}
