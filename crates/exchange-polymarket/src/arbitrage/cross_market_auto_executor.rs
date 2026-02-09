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
use rust_decimal::prelude::ToPrimitive;
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
use tracing::{debug, error, info, trace, warn};

use super::cross_market_types::{CrossMarketCombination, CrossMarketOpportunity};
use super::execution::{
    ExecutionError, OrderParams, OrderResult, OrderStatus, OrderType, PolymarketExecutor, Side,
};
use algo_trade_data::chainlink::ChainlinkWindowTracker;
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

    /// Slippage tolerance added to buy prices to improve fill rate.
    /// With 500-700ms latency, the book moves between detection and order arrival.
    /// E.g., 0.02 means buy up to 2 cents above detected price.
    /// The effective spread shrinks by 2x this value (both legs).
    /// Default: 0.02 ($0.02 per share).
    pub buy_slippage: Decimal,

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

    /// Minimum excess shares to trigger a trim sell after parallel fill (default: 0.5).
    /// Below this threshold, asymmetric fills are accepted as negligible.
    pub trim_threshold: Decimal,

    /// Earliest entry time: seconds before window end to START accepting trades.
    /// Data shows 8-10 min before close is the optimal entry zone.
    /// Default: 600 (start trading at 10 min before close).
    pub entry_window_start_secs: i64,

    /// Latest entry time: seconds before window end to STOP accepting trades.
    /// Below this, prices diverge sharply and the 4-6 min "dead zone" drags win rate.
    /// Also used as recovery-buy cutoff (stop recovery attempts near window end).
    /// Default: 360 (stop trading at 6 min before close).
    pub entry_window_end_secs: i64,

    /// Maximum implied loss probability (divergence filter).
    /// Calculated as (1 - leg1_price) * (1 - leg2_price) — the probability that
    /// NEITHER leg wins under independence. When both leg prices are low, this
    /// value is high, meaning the "big spread" is a trap (high cost of ruin).
    /// Default: 0.50 (reject if >50% chance of total loss).
    pub max_loss_prob: f64,

    /// Combined leg-drop threshold to trigger a divergence exit.
    /// When BOTH legs drop below their entry prices, the combined drop percentage
    /// (leg1_drop + leg2_drop) must exceed this to trigger a loss-cutting exit.
    /// E.g., 0.20 means exit when combined drop >= 20% (leg1 -12% + leg2 -10%).
    /// Set to 0 to disable. Default: 0 (DISABLED — paired positions should always
    /// be held to settlement since one leg must win).
    pub divergence_exit_threshold: Decimal,

    /// Maximum number of paired trades per 15-minute window. Once we have this many
    /// filled trades in a window, no new entries are allowed until the next window.
    /// Default: 1 (one trade per window to avoid over-concentration).
    pub max_trades_per_window: u32,

    /// Observe mode: persist ALL detected opportunities (even filtered ones) and
    /// record CLOB snapshots every scan interval. Default: false.
    pub observe_mode: bool,
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
            min_spread: dec!(0.20),
            buy_slippage: dec!(0.02),
            min_win_probability: 0.80,
            max_history: 1000,
            early_exit_enabled: false,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
            trim_threshold: dec!(0.5),
            entry_window_start_secs: 600,
            entry_window_end_secs: 240,
            max_loss_prob: 0.50,
            divergence_exit_threshold: Decimal::ZERO,
            max_trades_per_window: 5,
            observe_mode: false,
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
            min_spread: dec!(0.20),
            buy_slippage: dec!(0.02),
            min_win_probability: 0.85,
            max_history: 1000,
            early_exit_enabled: false,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
            trim_threshold: dec!(0.5),
            entry_window_start_secs: 600,
            entry_window_end_secs: 240,
            max_loss_prob: 0.50,
            divergence_exit_threshold: Decimal::ZERO,
            max_trades_per_window: 5,
            observe_mode: false,
        }
    }

    /// Creates a micro testing configuration with tight limits.
    /// Designed for high-frequency small FOK orders ($1-2 per leg) that
    /// fill reliably on thin books and accumulate edge through volume.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            filter_pair: Some((Coin::Btc, Coin::Eth)),
            filter_combination: Some(CrossMarketCombination::Coin1DownCoin2Up),
            kelly_fraction: 0.10,
            fixed_bet_size: Some(dec!(1)),
            min_bet_size: dec!(1),
            max_bet_size: dec!(5),
            max_position_per_window: dec!(10),
            min_spread: dec!(0.20),
            buy_slippage: dec!(0.02),
            min_win_probability: 0.75,
            max_history: 1000,
            early_exit_enabled: false,
            early_exit_profit_threshold: dec!(0.10),
            early_exit_depth_fraction: dec!(0.50),
            trim_threshold: dec!(0.5),
            entry_window_start_secs: 600,
            entry_window_end_secs: 240,
            max_loss_prob: 0.50,
            divergence_exit_threshold: Decimal::ZERO,
            max_trades_per_window: 10,
            observe_mode: false,
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

    // === Trim Stats ===
    /// Number of trim sells executed after asymmetric parallel fills.
    pub trim_count: u64,

    /// Total shares trimmed.
    pub trim_shares: Decimal,

    // === Event Log (for dashboard) ===
    /// Recent key events for dashboard display (max 20).
    pub event_log: VecDeque<EventLogEntry>,

    /// Last known on-chain balance (USDC + redeemable positions).
    /// Updated from actual API calls during trading, used by dashboard.
    pub live_balance: Option<Decimal>,

    // === Live prices from WebSocket (for settlement) ===
    /// Current prices from WebSocket feed: coin -> (up_price, down_price).
    /// Updated by the CLI from runner stats, used for fast settlement.
    pub live_prices: std::collections::HashMap<String, (Decimal, Decimal)>,

    // === Live snapshots from scanner (for CLOB persistence) ===
    /// Latest market snapshots from scanner, updated by CLI stats sync.
    /// Used for persisting CLOB price history to database.
    pub live_snapshots: Vec<super::cross_market_types::CoinMarketSnapshot>,
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

/// Pending trade info for dashboard display (includes holding details).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingTradeDisplay {
    pub trade_id: String,
    pub pair: String,
    /// Coin 1 slug (e.g., "BTC") for live price lookup.
    pub coin1: String,
    /// Coin 2 slug (e.g., "ETH") for live price lookup.
    pub coin2: String,
    pub leg1_dir: String,
    pub leg2_dir: String,
    pub total_cost: Decimal,
    pub window_end: DateTime<Utc>,
    /// Remaining shares on leg 1.
    pub shares_leg1: Decimal,
    /// Remaining shares on leg 2.
    pub shares_leg2: Decimal,
    /// Entry price for leg 1.
    pub entry_price_leg1: Decimal,
    /// Entry price for leg 2.
    pub entry_price_leg2: Decimal,
    /// Cumulative early exit proceeds collected so far.
    pub early_exit_proceeds: Decimal,
    /// Whether any early exit sells have occurred.
    pub partially_exited: bool,
}

/// Key event entry for the dashboard event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogEntry {
    pub time: DateTime<Utc>,
    pub kind: EventKind,
    pub message: String,
}

/// Type of key event for color-coding in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Fill,
    PartialFill,
    Reject,
    Trim,
    EarlyExit,
    Settlement,
    Recovery,
    Error,
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
    /// Entry price for leg 1 (probability/share).
    pub leg1_entry_price: Decimal,
    /// Entry price for leg 2 (probability/share).
    pub leg2_entry_price: Decimal,
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
    /// Last early exit attempt time (for cooldown after failures).
    #[serde(default)]
    pub last_exit_attempt: Option<DateTime<Utc>>,
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

/// Minimum leg price for the correlation arbitrage to be meaningful.
/// At very low prices (e.g. $0.07), the leg is essentially dead weight — the market
/// is saying "this won't happen" so there's no real hedge benefit from correlation.
/// Both legs need meaningful probability for the arbitrage to work.
const MIN_LEG_PRICE: Decimal = dec!(0.15);

/// Minimum order value per leg in USDC. With MIN_LEG_PRICE=$0.15 and small bet sizes,
/// the cheaper leg can be ~$0.38, so $0.25 accommodates asymmetric pairs.
const MIN_ORDER_VALUE: Decimal = dec!(0.25);

/// Maximum retry attempts before triggering the escape hatch (sell filled leg).
/// Recovery runs every 5 seconds, so 60 retries ≈ 5 minutes of actual attempts.
const MAX_RECOVERY_RETRIES: u32 = 60;

/// Maximum age of an incomplete trade before triggering the escape hatch.
/// 5 minutes gives liquidity time to return without blocking the bot too long.
const MAX_RECOVERY_AGE_SECS: u64 = 300;

/// Maximum escape sell attempts before hard-abandoning an incomplete trade.
/// After this many failed escape sells, the trade is removed from the queue
/// and the loss is accepted (shares may remain in portfolio, recoverable manually).
const MAX_ESCAPE_ATTEMPTS: u32 = 10;

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

    /// Number of escape sell attempts (after should_escape() triggers).
    pub escape_attempts: u32,

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

    /// Chainlink oracle price tracker for settlement (replaces Binance).
    chainlink_tracker: Arc<RwLock<ChainlinkWindowTracker>>,

    /// Initial balance at session start, used to derive P&L from actual on-chain state.
    initial_balance: Option<Decimal>,
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
        let rpc_url = std::env::var("POLYGON_RPC_URL")
            .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

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
            chainlink_tracker: Arc::new(RwLock::new(ChainlinkWindowTracker::new(&rpc_url))),
            initial_balance: None,
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
        let rpc_url = std::env::var("POLYGON_RPC_URL")
            .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

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
            chainlink_tracker: Arc::new(RwLock::new(ChainlinkWindowTracker::new(&rpc_url))),
            initial_balance: None,
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

        // Startup: settle any pending DB trades (orphans + partial fills), then expire stale ones
        self.settle_db_trades().await;
        self.expire_stale_pending().await;

        // Startup: create session record in database
        self.create_session_record().await;

        // Capture initial USDC balance for P&L derivation from actual on-chain state.
        // Uses get_balance() (USDC only) — not get_effective_balance() — so open
        // positions don't inflate P&L before they're actually redeemed.
        match self.executor.get_balance().await {
            Ok(bal) => {
                self.initial_balance = Some(bal);
                let mut stats = self.stats.write().await;
                stats.live_balance = Some(bal);
                info!(initial_balance = %bal, "Session starting balance captured for P&L tracking");
            }
            Err(e) => {
                warn!(error = %e, "Failed to capture initial balance — P&L will use first trade query");
            }
        }

        // Track last settlement check time - check every 5 seconds for faster settlement
        let mut last_settlement_check = std::time::Instant::now();
        let settlement_check_interval = std::time::Duration::from_secs(5);

        // Track Chainlink polling (every 10 seconds)
        let mut last_chainlink_poll = std::time::Instant::now();
        let chainlink_poll_interval = std::time::Duration::from_secs(10);

        // Track last CLOB/Chainlink DB persistence (every 30 seconds)
        let mut last_price_persist = std::time::Instant::now();
        let price_persist_interval = std::time::Duration::from_secs(30);

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("CrossMarketAutoExecutor stopping");
                break;
            }

            // Poll Chainlink oracle prices at window boundaries
            if last_chainlink_poll.elapsed() >= chainlink_poll_interval {
                let mut tracker = self.chainlink_tracker.write().await;
                tracker.poll().await;
                last_chainlink_poll = std::time::Instant::now();
            }

            // Persist CLOB snapshots and Chainlink window prices periodically
            if last_price_persist.elapsed() >= price_persist_interval {
                self.persist_clob_snapshots().await;
                self.persist_chainlink_windows().await;
                last_price_persist = std::time::Instant::now();
            }

            // Always check settlement if interval has passed (before waiting for opportunities)
            if last_settlement_check.elapsed() >= settlement_check_interval {
                debug!("Running periodic settlement check...");

                // Settle DB trades (handles partial fills + orphans from any session)
                self.settle_db_trades().await;

                // Settle in-memory pending trades (paired trades from current session)
                if let Err(e) = self.check_pending_settlements().await {
                    warn!(error = %e, "Settlement check error");
                }

                // Try early exit on profitable positions (before window closes)
                if self.config.early_exit_enabled {
                    if let Err(e) = self.try_early_exit().await {
                        debug!(error = %e, "Early exit check error (non-fatal)");
                    }
                }

                // Sync pending display with actual settlement state (shares, early exits)
                self.sync_pending_display().await;

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
                        Ok(n) => {
                            info!(redeemed = n, "Auto-redeemed {} resolved positions", n);
                            // Refresh USDC balance after redemption and derive P&L
                            if let Ok(bal) = self.executor.get_balance().await {
                                let mut stats = self.stats.write().await;
                                stats.live_balance = Some(bal);
                                if let Some(initial) = self.initial_balance {
                                    stats.realized_pnl = bal - initial;
                                }
                            }
                        }
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

        // Final persistence flush
        self.persist_clob_snapshots().await;
        self.persist_chainlink_windows().await;

        // Update session record with final stats
        self.update_session_record().await;

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

        // Observe mode: persist ALL detected opportunities before filtering
        if self.config.observe_mode {
            self.persist_detected_opportunity(&opp).await;
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

                // Settle any pending DB trades from the previous window so balance/P&L
                // are up to date before we start trading in the new window.
                drop(pos);
                self.settle_db_trades().await;
                // Re-acquire the position lock after settlement
                pos = self.position.write().await;
            }

            // Per-window entry limit: only allow max_trades_per_window paired trades
            if pos.position_count >= self.config.max_trades_per_window {
                debug!(
                    position_count = pos.position_count,
                    max = self.config.max_trades_per_window,
                    "Skipping opportunity — already entered this window"
                );
                drop(pos);
                self.stats.write().await.opportunities_skipped += 1;
                return Ok(());
            }
        }

        // Calculate bet size using effective balance (USDC + redeemable positions)
        let balance = self.executor.get_effective_balance().await?;
        // Update live balance with USDC only (matches MetaMask / on-chain truth)
        if let Ok(usdc) = self.executor.get_balance().await {
            self.stats.write().await.live_balance = Some(usdc);
        }
        // Record initial balance on first query for P&L derivation
        if self.initial_balance.is_none() {
            self.initial_balance = Some(balance);
        }
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
        // Floor at 5 shares — Polymarket's minimum order size. Without this,
        // partial-fill sell-backs get rejected and we're stuck holding naked exposure.
        let avg_leg_price = opp.total_cost / dec!(2);
        let shares = (bet_per_leg / avg_leg_price).max(dec!(5));

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

        // Check actual USDC balance covers both legs.
        // get_effective_balance() includes redeemable positions which can't be
        // spent directly, so verify real USDC covers the order.
        {
            let usdc_balance = self.executor.get_balance().await?;
            if usdc_balance < estimated_cost {
                debug!(
                    usdc_balance = %usdc_balance,
                    estimated_cost = %estimated_cost,
                    "Insufficient USDC balance for both legs"
                );
                self.stats.write().await.opportunities_skipped += 1;
                return Ok(());
            }
        }

        // Execute both legs
        let result = self.execute_both_legs(&opp, shares).await?;

        // Record trade
        self.record_trade(&opp, &result, shares).await;

        // Update position tracker — count ALL attempts to prevent retry loops.
        // Even partial fills that get sold back must increment the count,
        // otherwise the bot retries endlessly within the same window.
        {
            let mut pos = self.position.write().await;
            match &result {
                CrossMarketExecutionResult::Success { total_cost, .. } => {
                    pos.record_position(*total_cost);
                }
                CrossMarketExecutionResult::Leg1OnlyFilled { .. }
                | CrossMarketExecutionResult::Leg2OnlyFilled { .. } => {
                    // Record attempt (zero cost since we sell back) to block retries.
                    pos.record_position(Decimal::ZERO);
                }
                CrossMarketExecutionResult::BothRejected { .. } => {
                    // Also count rejections — if the book can't fill us, don't keep trying.
                    pos.record_position(Decimal::ZERO);
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
                    let fill_price = leg1_result.avg_fill_price.unwrap_or(opp.leg1_price);
                    let filled_size = leg1_result.filled_size;
                    warn!(
                        filled_size = %filled_size,
                        fill_price = %fill_price,
                        "Partial fill - leg 1 only. Selling back immediately."
                    );
                    drop(stats); // Release lock before sell-back

                    // Immediately sell back the filled leg to avoid naked directional exposure.
                    // Accept a small spread loss (~2-5%) rather than holding a 50/50 bet.
                    self.sell_back_partial_fill(
                        &opp.leg1_token_id,
                        filled_size,
                        fill_price,
                        "leg1",
                    )
                    .await;
                }
                CrossMarketExecutionResult::Leg2OnlyFilled { leg2_result, .. } => {
                    stats.partial_fills += 1;
                    let fill_price = leg2_result.avg_fill_price.unwrap_or(opp.leg2_price);
                    let filled_size = leg2_result.filled_size;
                    warn!(
                        filled_size = %filled_size,
                        fill_price = %fill_price,
                        "Partial fill - leg 2 only. Selling back immediately."
                    );
                    drop(stats); // Release lock before sell-back

                    // Immediately sell back the filled leg to avoid naked directional exposure.
                    self.sell_back_partial_fill(
                        &opp.leg2_token_id,
                        filled_size,
                        fill_price,
                        "leg2",
                    )
                    .await;
                }
                CrossMarketExecutionResult::BothRejected { .. } => {
                    stats.both_rejected += 1;
                    drop(stats);
                    // Start cooldown to avoid spamming API when there's no liquidity
                    self.last_both_rejected_at = Some(std::time::Instant::now());
                }
            }
        }

        // Push key events to dashboard event log
        let pair = format!("{}/{}", opp.coin1, opp.coin2);
        match &result {
            CrossMarketExecutionResult::Success {
                total_cost,
                leg1_result,
                leg2_result,
                ..
            } => {
                let paired = leg1_result.filled_size.min(leg2_result.filled_size);
                self.push_event(
                    EventKind::Fill,
                    format!(
                        "FILLED {} {:.1}sh @ ${:.2} ({}↓${:.2} + {}↑${:.2})",
                        pair, paired, total_cost, opp.coin1, opp.leg1_price, opp.coin2, opp.leg2_price,
                    ),
                )
                .await;
            }
            CrossMarketExecutionResult::Leg1OnlyFilled { leg1_result, .. } => {
                self.push_event(
                    EventKind::PartialFill,
                    format!(
                        "PARTIAL {} leg1 only {:.1}sh — sold back immediately",
                        pair, leg1_result.filled_size,
                    ),
                )
                .await;
            }
            CrossMarketExecutionResult::Leg2OnlyFilled { leg2_result, .. } => {
                self.push_event(
                    EventKind::PartialFill,
                    format!(
                        "PARTIAL {} leg2 only {:.1}sh — sold back immediately",
                        pair, leg2_result.filled_size,
                    ),
                )
                .await;
            }
            CrossMarketExecutionResult::BothRejected { .. } => {
                self.push_event(
                    EventKind::Reject,
                    format!("REJECTED {} — both legs rejected", pair),
                )
                .await;
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

        // Recovery max price: only allow recovery if total cost stays within
        // the original opportunity's combined cost + small buffer (5%).
        // This prevents recovery from destroying the spread by paying way more
        // than the original opportunity price for the missing leg.
        let original_pair_cost = opp.leg1_price + opp.leg2_price;
        let max_missing_price = (original_pair_cost * dec!(1.05)) - filled_price;

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
                escape_attempts: 0,
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
                escape_attempts: 0,
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

            // Trading cutoff: if we're near window end, don't try to recover —
            // prices have diverged and the missing leg is likely worthless or overpriced.
            // Force escape instead.
            {
                let secs_to_end = (trade.window_end - Utc::now()).num_seconds();
                if secs_to_end <= self.config.entry_window_end_secs {
                    warn!(
                        trade_id = %trade.trade_id,
                        secs_to_end = secs_to_end,
                        "Window cutoff reached — skipping recovery, will escape"
                    );
                    let mut incomplete = self.incomplete_trades.write().await;
                    if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                        t.retry_count = MAX_RECOVERY_RETRIES;
                    }
                    continue;
                }
            }

            // Escape hatch: if too many retries or too old, sell the filled leg
            // to recover capital rather than staying stuck indefinitely.
            if trade.should_escape() {
                // Hard abandon: if escape sell has failed too many times, give up
                // and remove from queue. Shares remain in portfolio for manual recovery.
                if trade.escape_attempts >= MAX_ESCAPE_ATTEMPTS {
                    error!(
                        trade_id = %trade.trade_id,
                        escape_attempts = trade.escape_attempts,
                        shares = %trade.shares,
                        filled_token = %trade.filled_token_id,
                        "HARD ABANDON: escape sell failed {} times, removing from queue. \
                         Orphaned shares remain in portfolio — manual recovery needed.",
                        trade.escape_attempts,
                    );
                    {
                        let mut incomplete = self.incomplete_trades.write().await;
                        incomplete.retain(|t| t.trade_id != trade.trade_id);
                    }
                    {
                        let mut stats = self.stats.write().await;
                        stats.incomplete_escaped += 1;
                        stats.incomplete_trades = stats.incomplete_trades.saturating_sub(1);
                    }
                    self.push_event(
                        EventKind::Error,
                        format!(
                            "ABANDON {}/{} {}sh orphaned — manual sell needed",
                            trade.coin1, trade.coin2, trade.shares,
                        ),
                    )
                    .await;
                    continue;
                }

                warn!(
                    trade_id = %trade.trade_id,
                    retry_count = trade.retry_count,
                    escape_attempt = trade.escape_attempts + 1,
                    age_secs = (Utc::now() - trade.created_at).num_seconds(),
                    "Escape hatch triggered - selling filled leg to recover capital"
                );

                if let Err(e) = self.escape_sell_filled_leg(&trade, &live_prices).await {
                    let err_str = e.to_string();

                    // If the orderbook no longer exists, the market has expired.
                    // Retrying will never work — abandon immediately and let
                    // on-chain auto-redeem handle these shares.
                    if err_str.contains("does not exist") {
                        error!(
                            trade_id = %trade.trade_id,
                            error = %e,
                            "Orderbook expired — market closed, abandoning to on-chain redemption"
                        );
                        let mut incomplete = self.incomplete_trades.write().await;
                        incomplete.retain(|t| t.trade_id != trade.trade_id);
                        drop(incomplete);
                        let mut stats = self.stats.write().await;
                        stats.incomplete_escaped += 1;
                        stats.incomplete_trades = stats.incomplete_trades.saturating_sub(1);
                        self.push_event(
                            EventKind::Recovery,
                            format!(
                                "ABANDON {}/{} orderbook expired — shares await on-chain redemption",
                                trade.coin1, trade.coin2,
                            ),
                        )
                        .await;
                        continue;
                    }

                    warn!(
                        trade_id = %trade.trade_id,
                        error = %e,
                        escape_attempt = trade.escape_attempts + 1,
                        "Escape sell failed, will retry next cycle"
                    );
                    let mut incomplete = self.incomplete_trades.write().await;
                    if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                        t.escape_attempts += 1;
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
                self.push_event(
                    EventKind::Recovery,
                    format!("ESCAPE {}/{} sold filled leg to free capital", trade.coin1, trade.coin2),
                )
                .await;
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

            // Check position limit before recovery buy
            {
                let recovery_cost = current_price * trade.shares;
                let pos = self.position.read().await;
                let remaining = pos.remaining_capacity(self.config.max_position_per_window);
                if remaining < recovery_cost {
                    warn!(
                        trade_id = %trade.trade_id,
                        recovery_cost = %recovery_cost,
                        remaining_capacity = %remaining,
                        "Recovery would exceed window position limit — triggering escape instead"
                    );
                    // Force escape by setting retry_count past threshold
                    let mut incomplete = self.incomplete_trades.write().await;
                    if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                        t.retry_count = MAX_RECOVERY_RETRIES;
                    }
                    continue;
                }
            }

            // Check Polymarket minimums for recovery order
            if current_price < MIN_LEG_PRICE {
                debug!(
                    trade_id = %trade.trade_id,
                    current_price = %current_price,
                    "Recovery price below Polymarket minimum $0.05"
                );
                continue;
            }
            let recovery_order_value = current_price * trade.shares;
            if recovery_order_value < MIN_ORDER_VALUE {
                debug!(
                    trade_id = %trade.trade_id,
                    order_value = %recovery_order_value,
                    "Recovery order value below Polymarket minimum $1"
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
            self.push_event(
                EventKind::Recovery,
                format!(
                    "RECOVERED {}/{} missing leg filled @ ${:.2}",
                    trade.coin1, trade.coin2, missing_fill_price,
                ),
            )
            .await;

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
                leg1_entry_price: if trade.filled_leg == FilledLeg::Leg1 {
                    trade.filled_price
                } else {
                    missing_fill_price
                },
                leg2_entry_price: if trade.filled_leg == FilledLeg::Leg2 {
                    trade.filled_price
                } else {
                    missing_fill_price
                },
                shares: trade.shares,
                window_end: trade.window_end,
                executed_at: Utc::now(),
                remaining_shares_leg1: trade.shares,
                remaining_shares_leg2: trade.shares,
                early_exit_proceeds: Decimal::ZERO,
                partially_exited: false,
                last_exit_attempt: None,
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

        let market_price = match live_prices.get(&filled_coin) {
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

        // Progressive slippage: more aggressive with each escape attempt
        // Attempt 0: 95% market, Attempt 1: 92%, Attempt 2: 89%, ..., clamped at 80%
        let discount = dec!(0.95) - dec!(0.03) * Decimal::from(trade.escape_attempts);
        let discount = discount.max(dec!(0.80));
        // Round DOWN to cents — Polymarket requires tick-aligned prices ($0.01)
        let sell_price = ((market_price * discount) * dec!(100)).floor() / dec!(100);
        let sell_price = sell_price.max(dec!(0.01));

        info!(
            trade_id = %trade.trade_id,
            filled_token = %trade.filled_token_id,
            shares = %trade.shares,
            market_price = %market_price,
            sell_price = %sell_price,
            discount = %discount,
            escape_attempt = trade.escape_attempts + 1,
            original_buy_price = %trade.filled_price,
            "Escape hatch: selling filled leg to recover capital"
        );

        // Use FAK (Fill-and-Kill / IOC) instead of FOK — accept partial fills
        let order = OrderParams {
            token_id: trade.filled_token_id.clone(),
            side: Side::Sell,
            price: sell_price,
            size: trade.shares,
            order_type: OrderType::Fak,
            neg_risk: false,
            presigned: None,
        };

        let result = self.executor.submit_order(order).await?;

        // Accept any fill (full or partial) as success
        if result.filled_size > Decimal::ZERO {
            let recovered = result.fill_notional();
            let loss = (trade.filled_price - sell_price) * result.filled_size;
            let remaining = trade.shares - result.filled_size;
            warn!(
                trade_id = %trade.trade_id,
                recovered = %recovered,
                loss = %loss,
                filled = %result.filled_size,
                remaining = %remaining,
                "Escape hatch: sold filled leg, accepted loss to free capital"
            );
            // If partially filled, update remaining shares in the incomplete trade
            if remaining > dec!(0.5) {
                let mut incomplete = self.incomplete_trades.write().await;
                if let Some(t) = incomplete.iter_mut().find(|t| t.trade_id == trade.trade_id) {
                    t.shares = remaining;
                    // Don't count this as a fully successful escape — we'll retry
                    // the rest next cycle
                    return Err(CrossMarketAutoExecutorError::Filtered {
                        reason: format!(
                            "Escape partial fill: sold {}, {} remaining",
                            result.filled_size, remaining
                        ),
                    });
                }
            }
            Ok(())
        } else {
            warn!(
                trade_id = %trade.trade_id,
                status = ?result.status,
                sell_price = %sell_price,
                "Escape sell not filled, will retry with more slippage"
            );
            Err(CrossMarketAutoExecutorError::Filtered {
                reason: "Escape sell not filled".to_string(),
            })
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

        // Entry timing window: only trade within the optimal time range.
        // Data shows 8-10 min before close = best, 4-6 min = dead zone.
        {
            let ts = opp.detected_at.timestamp();
            let window_secs: i64 = 900;
            let window_start = (ts / window_secs) * window_secs;
            let secs_into_window = ts - window_start;
            let secs_remaining = window_secs - secs_into_window;

            if secs_remaining > self.config.entry_window_start_secs {
                trace!(
                    secs_remaining = secs_remaining,
                    entry_start = self.config.entry_window_start_secs,
                    "Too early in window — waiting for entry window"
                );
                return false;
            }

            if secs_remaining <= self.config.entry_window_end_secs {
                debug!(
                    secs_remaining = secs_remaining,
                    entry_end = self.config.entry_window_end_secs,
                    "Past entry window — too close to window end"
                );
                return false;
            }
        }

        // Check spread
        if opp.spread < self.config.min_spread {
            trace!(
                spread = %opp.spread,
                min = %self.config.min_spread,
                "Filter: spread too low"
            );
            return false;
        }

        // Check win probability
        if opp.win_probability < self.config.min_win_probability {
            trace!(
                win_prob = format!("{:.1}%", opp.win_probability * 100.0),
                min = format!("{:.1}%", self.config.min_win_probability * 100.0),
                leg1 = %opp.leg1_price,
                leg2 = %opp.leg2_price,
                spread = %opp.spread,
                "Filter: win probability too low"
            );
            return false;
        }

        // Check minimum leg price (Polymarket rejects orders below $0.05)
        if opp.leg1_price < MIN_LEG_PRICE || opp.leg2_price < MIN_LEG_PRICE {
            trace!(
                leg1 = %opp.leg1_price,
                leg2 = %opp.leg2_price,
                min = %MIN_LEG_PRICE,
                "Filter: leg price below minimum"
            );
            return false;
        }

        // Divergence filter: reject when implied loss probability is too high.
        // P(neither leg wins) = (1 - p1) * (1 - p2) under independence.
        // When both prices are low, this is high → the big spread is a trap.
        // Correlation helps but can't save us when this is >50%.
        {
            let p1 = opp.leg1_price.to_f64().unwrap_or(0.0);
            let p2 = opp.leg2_price.to_f64().unwrap_or(0.0);
            let loss_prob = (1.0 - p1) * (1.0 - p2);
            if loss_prob > self.config.max_loss_prob {
                trace!(
                    leg1_price = %opp.leg1_price,
                    leg2_price = %opp.leg2_price,
                    loss_prob = format!("{:.1}%", loss_prob * 100.0),
                    max = format!("{:.1}%", self.config.max_loss_prob * 100.0),
                    "Filter: implied loss probability too high"
                );
                return false;
            }
        }

        true
    }

    /// Executes both legs of the cross-market opportunity in parallel.
    ///
    /// Both legs are submitted concurrently via `tokio::join!` to minimize
    /// latency and improve fill rates. If fills are asymmetric (different
    /// sizes), the excess on the over-filled leg is trimmed via a FAK sell.
    async fn execute_both_legs(
        &self,
        opp: &CrossMarketOpportunity,
        shares: Decimal,
    ) -> Result<CrossMarketExecutionResult, CrossMarketAutoExecutorError> {
        // Validate minimum order value ($1.00 Polymarket minimum)
        let leg1_value = shares * opp.leg1_price;
        let leg2_value = shares * opp.leg2_price;
        if leg1_value < MIN_ORDER_VALUE || leg2_value < MIN_ORDER_VALUE {
            return Err(CrossMarketAutoExecutorError::Filtered {
                reason: format!(
                    "Order value below $1 minimum (leg1=${}, leg2=${})",
                    leg1_value.round_dp(2),
                    leg2_value.round_dp(2),
                ),
            });
        }

        // Create orders for both legs.
        // Add slippage tolerance to buy prices — matching engine takes 500-2000ms
        // so the book can move between detection and fill. Round up to $0.01 tick grid.
        let slippage = self.config.buy_slippage;
        let leg1_buy_price = ((opp.leg1_price + slippage) * dec!(100)).ceil() / dec!(100);
        let leg2_buy_price = ((opp.leg2_price + slippage) * dec!(100)).ceil() / dec!(100);
        let leg1_buy_price = leg1_buy_price.min(dec!(0.99));
        let leg2_buy_price = leg2_buy_price.min(dec!(0.99));

        let leg1_order = OrderParams {
            token_id: opp.leg1_token_id.clone(),
            side: Side::Buy,
            price: leg1_buy_price,
            size: shares,
            order_type: OrderType::Fok,
            neg_risk: false,
            presigned: None,
        };

        let leg2_order = OrderParams {
            token_id: opp.leg2_token_id.clone(),
            side: Side::Buy,
            price: leg2_buy_price,
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
            "Executing cross-market trade (parallel)"
        );

        // Submit both legs concurrently
        let (leg1_raw, leg2_raw) = tokio::join!(
            self.executor.submit_order(leg1_order),
            self.executor.submit_order(leg2_order),
        );

        // Convert errors to rejected results
        let leg1_result = leg1_raw.unwrap_or_else(|e| {
            warn!(error = %e, "Leg 1 submission error");
            OrderResult::rejected("leg1-error", e.to_string())
        });
        let leg2_result = leg2_raw.unwrap_or_else(|e| {
            warn!(error = %e, "Leg 2 submission error");
            OrderResult::rejected("leg2-error", e.to_string())
        });

        // Use HTTP round-trip latency from the order results (max of both legs)
        let latency_ms = leg1_result
            .latency_ms
            .unwrap_or(0)
            .max(leg2_result.latency_ms.unwrap_or(0));
        if latency_ms > 0 {
            self.update_latency_stats(latency_ms).await;
        }

        let leg1_filled = leg1_result.status == OrderStatus::Filled;
        let leg2_filled = leg2_result.status == OrderStatus::Filled;

        let result = match (leg1_filled, leg2_filled) {
            (true, true) if leg1_result.filled_size <= Decimal::ZERO
                || leg2_result.filled_size <= Decimal::ZERO =>
            {
                // Edge case: status says Filled but zero actual size — treat as rejected
                warn!(
                    leg1_filled_size = %leg1_result.filled_size,
                    leg2_filled_size = %leg2_result.filled_size,
                    "Both legs report Filled but zero fill size — treating as rejected"
                );
                CrossMarketExecutionResult::BothRejected {
                    leg1_result,
                    leg2_result,
                }
            }
            (true, true) => {
                // Both filled — check for asymmetry and trim excess
                let paired = leg1_result.filled_size.min(leg2_result.filled_size);
                let excess_leg1 = (leg1_result.filled_size - paired).max(Decimal::ZERO);
                let excess_leg2 = (leg2_result.filled_size - paired).max(Decimal::ZERO);

                if excess_leg1 > self.config.trim_threshold {
                    info!(
                        excess = %excess_leg1,
                        leg1_filled = %leg1_result.filled_size,
                        leg2_filled = %leg2_result.filled_size,
                        "Asymmetric fill — trimming leg1 excess"
                    );
                    self.trim_excess_shares(
                        &opp.leg1_token_id,
                        excess_leg1,
                        opp.leg1_price,
                    )
                    .await;
                }
                if excess_leg2 > self.config.trim_threshold {
                    info!(
                        excess = %excess_leg2,
                        leg1_filled = %leg1_result.filled_size,
                        leg2_filled = %leg2_result.filled_size,
                        "Asymmetric fill — trimming leg2 excess"
                    );
                    self.trim_excess_shares(
                        &opp.leg2_token_id,
                        excess_leg2,
                        opp.leg2_price,
                    )
                    .await;
                }

                // Use paired size for cost calculation
                let leg1_price = leg1_result.avg_fill_price.unwrap_or(opp.leg1_price);
                let leg2_price = leg2_result.avg_fill_price.unwrap_or(opp.leg2_price);
                let total_cost = paired * (leg1_price + leg2_price);

                info!(
                    paired = %paired,
                    total_cost = %total_cost,
                    latency_ms = latency_ms,
                    "Both legs filled (parallel)"
                );

                CrossMarketExecutionResult::Success {
                    leg1_result,
                    leg2_result,
                    total_cost,
                    expected_payout: Decimal::ONE,
                }
            }
            (true, false) => {
                warn!(
                    leg1_filled_size = %leg1_result.filled_size,
                    leg2_status = ?leg2_result.status,
                    "Parallel: leg1 filled, leg2 rejected — directional exposure"
                );
                CrossMarketExecutionResult::Leg1OnlyFilled {
                    leg1_result,
                    leg2_result,
                }
            }
            (false, true) => {
                warn!(
                    leg1_status = ?leg1_result.status,
                    leg2_filled_size = %leg2_result.filled_size,
                    "Parallel: leg2 filled, leg1 rejected — directional exposure"
                );
                CrossMarketExecutionResult::Leg2OnlyFilled {
                    leg1_result,
                    leg2_result,
                }
            }
            (false, false) => {
                info!(
                    leg1_status = ?leg1_result.status,
                    leg2_status = ?leg2_result.status,
                    "Both legs rejected"
                );
                CrossMarketExecutionResult::BothRejected {
                    leg1_result,
                    leg2_result,
                }
            }
        };

        Ok(result)
    }

    /// Trims excess shares from an over-filled leg by selling at best bid.
    ///
    /// When parallel leg submission results in asymmetric fills (e.g., 26.2 vs 24.5 shares),
    /// the excess on the over-filled leg is sold via FAK to restore delta-neutral pairing.
    /// Uses a 2% discount from reference price to ensure the sell fills quickly.
    async fn trim_excess_shares(
        &self,
        token_id: &str,
        excess: Decimal,
        reference_price: Decimal,
    ) {
        // Sell at 2% below reference to ensure fill, round to cent tick, floor at 0.01
        let sell_price = ((reference_price * dec!(0.98)) * dec!(100)).floor() / dec!(100);
        let sell_price = sell_price.max(dec!(0.01));

        let sell_order = OrderParams {
            token_id: token_id.to_string(),
            side: Side::Sell,
            price: sell_price,
            size: excess,
            order_type: OrderType::Fak,
            neg_risk: false,
            presigned: None,
        };

        info!(
            token_id = %token_id,
            excess = %excess,
            sell_price = %sell_price,
            "Trimming excess shares to match paired position"
        );

        match self.executor.submit_order(sell_order).await {
            Ok(result) => {
                if result.filled_size > Decimal::ZERO {
                    info!(
                        filled = %result.filled_size,
                        "Trim sell filled"
                    );
                    let mut stats = self.stats.write().await;
                    stats.trim_count += 1;
                    stats.trim_shares += result.filled_size;
                    drop(stats);
                    self.push_event(
                        EventKind::Trim,
                        format!("TRIM {:.1}sh @ ${:.2}", result.filled_size, sell_price),
                    )
                    .await;
                } else {
                    warn!("Trim sell not filled — small unhedged exposure remains");
                }
            }
            Err(e) => {
                warn!(error = %e, "Trim sell failed — small unhedged exposure remains");
            }
        }
    }

    /// Immediately sells back a partially-filled leg to avoid naked directional exposure.
    ///
    /// When only one leg of a paired trade fills, we're left with an unhedged directional
    /// bet (50/50 with no edge). Instead of trying to recover the missing leg (which usually
    /// fails as the book has moved), we sell the filled leg back immediately. This accepts
    /// a small known spread loss (~2-5%) rather than risking a 50% chance of total loss.
    async fn sell_back_partial_fill(
        &self,
        token_id: &str,
        filled_size: Decimal,
        fill_price: Decimal,
        leg_label: &str,
    ) {
        // Sell at 3% below fill price to ensure it fills quickly via FAK
        let sell_price = ((fill_price * dec!(0.97)) * dec!(100)).floor() / dec!(100);
        let sell_price = sell_price.max(dec!(0.01));

        // Check Polymarket minimums
        let sell_value = sell_price * filled_size;
        if sell_value < MIN_ORDER_VALUE {
            warn!(
                leg = leg_label,
                sell_value = %sell_value,
                "Sell-back value below $1 minimum — accepting small directional exposure"
            );
            return;
        }

        let sell_order = OrderParams {
            token_id: token_id.to_string(),
            side: Side::Sell,
            price: sell_price,
            size: filled_size,
            order_type: OrderType::Fak,
            neg_risk: false,
            presigned: None,
        };

        info!(
            leg = leg_label,
            token_id = %token_id,
            filled_size = %filled_size,
            fill_price = %fill_price,
            sell_price = %sell_price,
            "Selling back partial fill to avoid directional exposure"
        );

        // Retry sell-back with delays — the buy may still be settling on-chain.
        // Polymarket returns "matched" before the Polygon tx confirms, so shares
        // aren't immediately available to sell.
        let delays = [
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(5),
        ];
        for (attempt, delay) in std::iter::once(std::time::Duration::ZERO)
            .chain(delays.iter().copied())
            .enumerate()
        {
            if delay > std::time::Duration::ZERO {
                info!(
                    leg = leg_label,
                    attempt = attempt + 1,
                    delay_secs = delay.as_secs(),
                    "Retrying sell-back after delay (waiting for on-chain settlement)"
                );
                tokio::time::sleep(delay).await;
            }

            match self.executor.submit_order(sell_order.clone()).await {
                Ok(result) => {
                    let recovered = result.filled_size * sell_price;
                    let cost = filled_size * fill_price;
                    let loss = cost - recovered;
                    if result.filled_size > Decimal::ZERO {
                        info!(
                            leg = leg_label,
                            sold = %result.filled_size,
                            recovered = %recovered,
                            loss = %loss,
                            attempt = attempt + 1,
                            "Sell-back filled — small spread loss accepted"
                        );
                        self.push_event(
                            EventKind::Trim,
                            format!(
                                "SELL-BACK {} {:.1}sh @ ${:.2} (loss ${:.2})",
                                leg_label, result.filled_size, sell_price, loss,
                            ),
                        )
                        .await;
                        return;
                    }
                    warn!(
                        leg = leg_label,
                        attempt = attempt + 1,
                        "Sell-back not filled — will retry"
                    );
                }
                Err(e) => {
                    let is_balance_error = e.to_string().contains("not enough balance");
                    if is_balance_error && attempt < delays.len() {
                        debug!(
                            leg = leg_label,
                            attempt = attempt + 1,
                            error = %e,
                            "Sell-back failed (shares not yet settled) — will retry"
                        );
                        continue;
                    }
                    warn!(
                        leg = leg_label,
                        attempt = attempt + 1,
                        error = %e,
                        "Sell-back failed — directional exposure remains"
                    );
                    return;
                }
            }
        }

        warn!(
            leg = leg_label,
            "Sell-back exhausted all retries — directional exposure remains"
        );
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

    /// Pushes a key event to the dashboard event log.
    async fn push_event(&self, kind: EventKind, message: String) {
        let mut stats = self.stats.write().await;
        stats.event_log.push_back(EventLogEntry {
            time: Utc::now(),
            kind,
            message,
        });
        // Keep max 20 events
        while stats.event_log.len() > 20 {
            stats.event_log.pop_front();
        }
    }

    /// Syncs pending trade display with actual pending settlements state.
    async fn sync_pending_display(&self) {
        let pending = self.pending_settlements.read().await;
        let mut stats = self.stats.write().await;
        stats.pending_trades = pending
            .iter()
            .map(|s| PendingTradeDisplay {
                trade_id: s.trade_id.clone(),
                pair: format!("{}/{}", s.coin1, s.coin2),
                coin1: s.coin1.clone(),
                coin2: s.coin2.clone(),
                leg1_dir: s.leg1_direction.clone(),
                leg2_dir: s.leg2_direction.clone(),
                total_cost: s.total_cost,
                window_end: s.window_end,
                shares_leg1: s.remaining_shares_leg1,
                shares_leg2: s.remaining_shares_leg2,
                entry_price_leg1: s.leg1_entry_price,
                entry_price_leg2: s.leg2_entry_price,
                early_exit_proceeds: s.early_exit_proceeds,
                partially_exited: s.partially_exited,
            })
            .collect();
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
        // Use actual paired size (min of both fills) instead of requested shares,
        // since parallel execution may produce asymmetric fills that get trimmed.
        if let CrossMarketExecutionResult::Success {
            ref leg1_result,
            ref leg2_result,
            ..
        } = result
        {
            let paired_shares = leg1_result.filled_size.min(leg2_result.filled_size);
            self.add_pending_settlement(opp, paired_shares).await;
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
    // Database Lifecycle Methods (startup, periodic, shutdown)
    // =========================================================================

    /// Settles pending DB trades from any session whose windows have closed.
    ///
    /// This runs at startup, on window transitions, and periodically. It handles:
    /// - Orphaned trades from previous sessions (killed before settlement)
    /// - Current-session trades not yet in the in-memory pending list (partial fills)
    /// - Partial fills correctly: only the filled leg contributes to PnL
    async fn settle_db_trades(&self) {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return,
        };

        // Load pending executed trades where window has closed.
        // Skip current-session fully-paired trades (both fills present) — those are
        // handled by the in-memory settlement in check_pending_settlements().
        // We DO pick up:
        //   - All trades from other sessions (orphans)
        //   - Current-session partial fills (one NULL fill price) that never enter the
        //     in-memory pending list
        let rows = match sqlx::query_as::<_, (
            i32,                    // id
            String,                 // session_id
            DateTime<Utc>,          // timestamp (executed_at)
            String,                 // coin1
            String,                 // coin2
            String,                 // combination
            String,                 // leg1_direction
            Decimal,                // leg1_price
            String,                 // leg1_token_id
            String,                 // leg2_direction
            Decimal,                // leg2_price
            String,                 // leg2_token_id
            Decimal,                // total_cost
            DateTime<Utc>,          // window_end
            Option<Decimal>,        // leg1_fill_price
            Option<Decimal>,        // leg2_fill_price
        )>(
            r#"
            SELECT id, session_id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, window_end,
                   leg1_fill_price, leg2_fill_price
            FROM cross_market_opportunities
            WHERE status = 'pending'
              AND executed = true
              AND window_end < NOW() - INTERVAL '30 seconds'
              AND (
                  session_id != $1
                  OR leg1_fill_price IS NULL
                  OR leg2_fill_price IS NULL
              )
            ORDER BY window_end ASC
            "#,
        )
        .bind(&self.session_id)
        .fetch_all(pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Failed to load pending trades for DB settlement");
                return;
            }
        };

        if rows.is_empty() {
            return;
        }

        info!(
            count = rows.len(),
            "Found {} pending DB trades to settle", rows.len()
        );

        let mut settled = 0u64;
        let mut failed = 0u64;

        for (id, original_session_id, executed_at, coin1, coin2, _combination,
             leg1_direction, leg1_price, leg1_token_id,
             leg2_direction, leg2_price, leg2_token_id,
             total_cost, window_end,
             leg1_fill_price, leg2_fill_price) in &rows
        {
            // Detect partial fills: one fill price is NULL
            let is_partial = leg1_fill_price.is_none() != leg2_fill_price.is_none();

            // Reconstruct a PendingPaperSettlement for the existing settle methods
            let settlement = PendingPaperSettlement {
                trade_id: format!("db-settle-{}", id),
                coin1: coin1.clone(),
                coin2: coin2.clone(),
                leg1_direction: leg1_direction.clone(),
                leg2_direction: leg2_direction.clone(),
                leg1_token_id: leg1_token_id.clone(),
                leg2_token_id: leg2_token_id.clone(),
                total_cost: *total_cost,
                leg1_entry_price: *leg1_price,
                leg2_entry_price: *leg2_price,
                shares: Decimal::ONE, // Nominal — pnl recalculated from actual fills
                window_end: *window_end,
                executed_at: *executed_at,
                remaining_shares_leg1: Decimal::ONE,
                remaining_shares_leg2: Decimal::ONE,
                early_exit_proceeds: Decimal::ZERO,
                partially_exited: false,
                last_exit_attempt: None,
            };

            // Try Gamma API first (most reliable for past windows), then CLOB
            let outcome = match self.try_settle_via_gamma(&settlement).await {
                Ok(result) => {
                    debug!(trade_id = %settlement.trade_id, "DB trade settled via Gamma API");
                    Some(result)
                }
                Err(gamma_err) => {
                    debug!(
                        trade_id = %settlement.trade_id,
                        error = %gamma_err,
                        "Gamma API failed for orphan, trying Chainlink oracle"
                    );
                    match self.try_settle_via_chainlink(&settlement).await {
                        Ok(result) => {
                            info!(trade_id = %settlement.trade_id, "Orphan settled via Chainlink oracle");
                            Some(result)
                        }
                        Err(chainlink_err) => {
                            warn!(
                                trade_id = %settlement.trade_id,
                                gamma_error = %gamma_err,
                                chainlink_error = %chainlink_err,
                                "Could not settle orphan via Gamma or Chainlink"
                            );
                            None
                        }
                    }
                }
            };

            if let Some((leg1_won, leg2_won)) = outcome {
                // Derive outcomes
                let c1_out = if leg1_won {
                    leg1_direction.clone()
                } else if leg1_direction == "UP" {
                    "DOWN".to_string()
                } else {
                    "UP".to_string()
                };
                let c2_out = if leg2_won {
                    leg2_direction.clone()
                } else if leg2_direction == "UP" {
                    "DOWN".to_string()
                } else {
                    "UP".to_string()
                };

                // Calculate PnL differently for partial fills vs full paired trades
                let (trade_result, pnl) = if is_partial {
                    // Partial fill: only the filled leg matters.
                    // The sell-back may have failed, so the position was held to settlement.
                    let (filled_leg_won, fill_price) = if leg1_fill_price.is_some() {
                        (leg1_won, leg1_fill_price.unwrap_or(*leg1_price))
                    } else {
                        (leg2_won, leg2_fill_price.unwrap_or(*leg2_price))
                    };

                    // Shares = total_cost / fill_price (single leg cost)
                    let shares = if fill_price > Decimal::ZERO {
                        *total_cost / fill_price
                    } else {
                        Decimal::ONE
                    };
                    let payout = if filled_leg_won { shares } else { Decimal::ZERO };
                    let fees = payout * self.fee_rate;
                    let pnl = payout - fees - *total_cost;

                    // Partial fills are either win or lose based on the single filled leg
                    let result = if filled_leg_won { "WIN" } else { "LOSE" };
                    (result, pnl)
                } else {
                    // Full paired trade
                    let result = match (leg1_won, leg2_won) {
                        (true, true) => "DOUBLE_WIN",
                        (true, false) | (false, true) => "WIN",
                        (false, false) => "LOSE",
                    };
                    let shares = if (*leg1_price + *leg2_price) > Decimal::ZERO {
                        *total_cost / (*leg1_price + *leg2_price)
                    } else {
                        Decimal::ONE
                    };
                    let leg1_payout = if leg1_won { shares } else { Decimal::ZERO };
                    let leg2_payout = if leg2_won { shares } else { Decimal::ZERO };
                    let gross_payout = leg1_payout + leg2_payout;
                    let fees = gross_payout * self.fee_rate;
                    let pnl = gross_payout - fees - *total_cost;
                    (result, pnl)
                };

                // Update DB
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
                    WHERE id = $6
                    "#,
                )
                .bind(&c1_out)
                .bind(&c2_out)
                .bind(trade_result)
                .bind(pnl)
                .bind(c1_out == c2_out)
                .bind(id)
                .execute(pool)
                .await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        settled += 1;

                        // Update in-memory stats so dashboard reflects this settlement
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
                        }

                        info!(
                            id = id,
                            session = %original_session_id,
                            pair = %format!("{}/{}", coin1, coin2),
                            partial = is_partial,
                            result = trade_result,
                            pnl = %pnl,
                            "Settled DB trade"
                        );

                        // Push settlement event to dashboard
                        let pnl_sign = if pnl >= Decimal::ZERO { "+" } else { "" };
                        let result_label = if is_partial {
                            format!("{} (partial)", trade_result)
                        } else {
                            trade_result.to_string()
                        };
                        self.push_event(
                            EventKind::Settlement,
                            format!(
                                "SETTLED {}/{} {} {}{:.2} ({}↑ {}↑)",
                                coin1, coin2, result_label, pnl_sign, pnl, c1_out, c2_out,
                            ),
                        )
                        .await;
                    }
                    Ok(_) => {
                        warn!(id = id, "DB settlement update matched no rows");
                        failed += 1;
                    }
                    Err(e) => {
                        warn!(id = id, error = %e, "Failed to update DB settlement");
                        failed += 1;
                    }
                }
            } else {
                failed += 1;
            }
        }

        if settled > 0 || failed > 0 {
            info!(
                settled = settled,
                failed = failed,
                "DB trade settlement complete"
            );
        }
    }


    /// Expires stale pending records from previous sessions on startup.
    /// Any opportunity still 'pending' with window_end > 30 minutes ago is marked 'expired'.
    async fn expire_stale_pending(&self) {
        if let Some(pool) = &self.db_pool {
            let result = sqlx::query(
                r#"
                UPDATE cross_market_opportunities
                SET status = 'expired', settled_at = NOW()
                WHERE status = 'pending'
                  AND window_end < NOW() - INTERVAL '30 minutes'
                "#,
            )
            .execute(pool)
            .await;

            match result {
                Ok(r) if r.rows_affected() > 0 => {
                    info!(
                        expired = r.rows_affected(),
                        "Expired stale pending records from previous sessions"
                    );
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "Failed to expire stale pending records"),
            }
        }
    }

    /// Creates a session record in cross_market_sessions at startup.
    async fn create_session_record(&self) {
        if let Some(pool) = &self.db_pool {
            let coins: Vec<String> = self
                .config
                .filter_pair
                .map(|(c1, c2)| {
                    vec![
                        c1.slug_prefix().to_uppercase(),
                        c2.slug_prefix().to_uppercase(),
                    ]
                })
                .unwrap_or_else(|| {
                    vec![
                        "BTC".to_string(),
                        "ETH".to_string(),
                        "SOL".to_string(),
                        "XRP".to_string(),
                    ]
                });

            let result = sqlx::query(
                r#"
                INSERT INTO cross_market_sessions
                    (session_id, started_at, min_spread_threshold,
                     assumed_correlation, coins_scanned, status)
                VALUES ($1, NOW(), $2, $3, $4, 'active')
                ON CONFLICT (session_id) DO NOTHING
                "#,
            )
            .bind(&self.session_id)
            .bind(self.config.min_spread)
            .bind(Decimal::from_f64_retain(self.config.min_win_probability).unwrap_or(Decimal::ZERO))
            .bind(&coins)
            .execute(pool)
            .await;

            match result {
                Ok(_) => info!(session_id = %self.session_id, "Session record created"),
                Err(e) => warn!(error = %e, "Failed to create session record"),
            }
        }
    }

    /// Updates the session record with final stats at shutdown.
    async fn update_session_record(&self) {
        if let Some(pool) = &self.db_pool {
            let stats = self.stats.read().await;
            let total_settled = stats.settled_wins + stats.settled_losses + stats.double_wins;
            let total_wins = stats.settled_wins + stats.double_wins;
            let actual_win_rate = if total_settled > 0 {
                Decimal::from(total_wins) / Decimal::from(total_settled)
            } else {
                Decimal::ZERO
            };

            let result = sqlx::query(
                r#"
                UPDATE cross_market_sessions
                SET ended_at = NOW(),
                    total_opportunities = $2,
                    opportunities_settled = $3,
                    total_wins = $4,
                    total_losses = $5,
                    double_wins = $6,
                    actual_win_rate = $7,
                    total_pnl = $8,
                    status = 'completed'
                WHERE session_id = $1
                "#,
            )
            .bind(&self.session_id)
            .bind(stats.opportunities_received as i32)
            .bind(total_settled as i32)
            .bind(stats.settled_wins as i32)
            .bind(stats.settled_losses as i32)
            .bind(stats.double_wins as i32)
            .bind(actual_win_rate)
            .bind(stats.realized_pnl)
            .execute(pool)
            .await;

            match result {
                Ok(_) => info!(session_id = %self.session_id, "Session record updated"),
                Err(e) => warn!(error = %e, "Failed to update session record"),
            }
        }
    }

    /// Persists CLOB price snapshots from the latest scanner data.
    async fn persist_clob_snapshots(&self) {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return,
        };

        let snapshots = {
            let stats = self.stats.read().await;
            stats.live_snapshots.clone()
        };

        if snapshots.is_empty() {
            return;
        }

        let now = Utc::now();
        for snap in &snapshots {
            let coin = snap.coin.slug_prefix().to_uppercase();
            let (up_bid, up_ask) = snap
                .up_depth
                .as_ref()
                .map(|d| (Some(d.bid_depth), Some(d.ask_depth)))
                .unwrap_or((None, None));
            let (down_bid, down_ask) = snap
                .down_depth
                .as_ref()
                .map(|d| (Some(d.bid_depth), Some(d.ask_depth)))
                .unwrap_or((None, None));

            let result = sqlx::query(
                r#"
                INSERT INTO clob_price_snapshots
                    (timestamp, coin, up_price, down_price,
                     up_token_id, down_token_id,
                     up_bid_depth, up_ask_depth, down_bid_depth, down_ask_depth,
                     session_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (timestamp, coin) DO NOTHING
                "#,
            )
            .bind(now)
            .bind(&coin)
            .bind(snap.up_price)
            .bind(snap.down_price)
            .bind(&snap.up_token_id)
            .bind(&snap.down_token_id)
            .bind(up_bid)
            .bind(up_ask)
            .bind(down_bid)
            .bind(down_ask)
            .bind(&self.session_id)
            .execute(pool)
            .await;

            if let Err(e) = result {
                debug!(error = %e, coin = %coin, "Failed to persist CLOB snapshot");
            }
        }
    }

    /// Persists Chainlink window price records to the database.
    async fn persist_chainlink_windows(&self) {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return,
        };

        let tracker = self.chainlink_tracker.read().await;
        let windows = tracker.get_all_windows();

        for (coin, window_start_ts, record) in &windows {
            let window_start =
                DateTime::from_timestamp(*window_start_ts, 0).unwrap_or_else(Utc::now);

            let result = sqlx::query(
                r#"
                INSERT INTO chainlink_window_prices
                    (window_start, coin, start_price, end_price, outcome,
                     closed, poll_count, first_polled_at, last_polled_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (window_start, coin) DO UPDATE SET
                    end_price = EXCLUDED.end_price,
                    outcome = EXCLUDED.outcome,
                    closed = EXCLUDED.closed,
                    poll_count = EXCLUDED.poll_count,
                    last_polled_at = EXCLUDED.last_polled_at
                "#,
            )
            .bind(window_start)
            .bind(coin)
            .bind(record.start_price)
            .bind(record.latest_price)
            .bind(if record.closed {
                if record.latest_price >= record.start_price {
                    Some("UP")
                } else {
                    Some("DOWN")
                }
            } else {
                None
            })
            .bind(record.closed)
            .bind(record.poll_count as i32)
            .bind(record.first_polled_at)
            .bind(record.last_polled_at)
            .execute(pool)
            .await;

            if let Err(e) = result {
                debug!(error = %e, coin = %coin, "Failed to persist Chainlink window");
            }
        }
    }

    /// Persists a detected opportunity without execution (observe mode).
    /// Records all opportunities with executed=false so we can track what we *could* have traded.
    async fn persist_detected_opportunity(&self, opp: &CrossMarketOpportunity) {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return,
        };

        let window_end = {
            let ts = opp.detected_at.timestamp();
            let window_secs = 900;
            let window_start = (ts / window_secs) * window_secs;
            let window_end_ts = window_start + window_secs;
            DateTime::from_timestamp(window_end_ts, 0).unwrap_or(opp.detected_at)
        };

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
                 executed)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18,
                    $19, $20, $21, $22, $23)
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
        .bind("pending")
        .bind(window_end)
        .bind(opp.leg1_bid_depth)
        .bind(opp.leg1_ask_depth)
        .bind(opp.leg2_bid_depth)
        .bind(opp.leg2_ask_depth)
        .bind(false) // Not executed — just detected
        .execute(pool)
        .await;

        if let Err(e) = result {
            debug!(error = %e, "Failed to persist detected opportunity");
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
            leg1_entry_price: opp.leg1_price,
            leg2_entry_price: opp.leg2_price,
            shares,
            window_end,
            executed_at: opp.detected_at, // Must match DB timestamp for settlement updates
            remaining_shares_leg1: shares,
            remaining_shares_leg2: shares,
            early_exit_proceeds: Decimal::ZERO,
            partially_exited: false,
            last_exit_attempt: None,
        };

        let mut pending = self.pending_settlements.write().await;
        pending.push(settlement.clone());

        let mut stats = self.stats.write().await;
        stats.pending_settlement = pending.len() as u64;

        // Update pending trades display
        stats.pending_trades.push(PendingTradeDisplay {
            trade_id: settlement.trade_id,
            pair: format!("{}/{}", opp.coin1, opp.coin2),
            coin1: opp.coin1.to_string(),
            coin2: opp.coin2.to_string(),
            leg1_dir: settlement.leg1_direction,
            leg2_dir: settlement.leg2_direction,
            total_cost: settlement.total_cost,
            window_end,
            shares_leg1: shares,
            shares_leg2: shares,
            entry_price_leg1: opp.leg1_price,
            entry_price_leg2: opp.leg2_price,
            early_exit_proceeds: Decimal::ZERO,
            partially_exited: false,
        });
    }

    /// Tries fast settlement using CLOB token prices for closed windows.
    ///
    /// Queries the specific token IDs from the CLOB API. If a token's price
    /// is > $0.50, that outcome won.
    ///
    /// NOTE: Previously this used live scanner prices (coin-level) which was
    /// wrong — those are for the NEXT window's markets, not the resolved one.
    /// Now we only use token-specific CLOB prices which are correct.
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

        debug!(
            count = trades_to_check.len(),
            "Checking {} pending trades for fast settlement via CLOB token prices",
            trades_to_check.len()
        );

        let mut settled_count = 0u64;

        for settlement in trades_to_check {
            // Fetch CLOB prices for the specific token IDs of this trade
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
                debug!(
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
                        debug!(error = %e, "Failed to parse CLOB prices JSON");
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
                // Token price > 0.50 means that outcome won
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

        // Find trades with open windows that could be exited early.
        // Cooldown: skip trades where we attempted exit in the last 30 seconds
        // to avoid spamming the API after failures (circuit breaker, no balance).
        let exit_cooldown = chrono::Duration::seconds(30);

        let candidates: Vec<PendingPaperSettlement> = {
            let pending = self.pending_settlements.read().await;
            pending
                .iter()
                .filter(|s| now < s.window_end) // Window still open
                .filter(|s| s.remaining_shares_leg1 > Decimal::ZERO || s.remaining_shares_leg2 > Decimal::ZERO)
                .filter(|s| {
                    // Skip if we recently attempted an exit on this trade
                    s.last_exit_attempt
                        .map(|t| now - t > exit_cooldown)
                        .unwrap_or(true)
                })
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

            // Decide: take profit, cut losses on divergence, or skip
            let is_profitable = profit_pct >= self.config.early_exit_profit_threshold;

            // Divergence detection: both legs dropping below entry = wrong-way move.
            // This means BTC is pumping (BTC DOWN dropping) AND ETH is dumping
            // (ETH UP dropping) simultaneously — the one scenario that kills us.
            //
            // IMPORTANT: Only check divergence when BOTH legs still have shares.
            // If one leg has been sold (remaining = 0), this is a directional position
            // and divergence logic doesn't apply — let it ride to settlement.
            let both_legs_held = settlement.remaining_shares_leg1 > Decimal::ZERO
                && settlement.remaining_shares_leg2 > Decimal::ZERO;

            let is_diverging = if both_legs_held
                && self.config.divergence_exit_threshold > Decimal::ZERO
            {
                let leg1_vs_entry = if settlement.leg1_entry_price > Decimal::ZERO {
                    leg1_bid / settlement.leg1_entry_price
                } else {
                    Decimal::ONE
                };
                let leg2_vs_entry = if settlement.leg2_entry_price > Decimal::ZERO {
                    leg2_bid / settlement.leg2_entry_price
                } else {
                    Decimal::ONE
                };

                // Both legs must be below entry (divergence, not just one-sided)
                let both_dropping = leg1_vs_entry < Decimal::ONE && leg2_vs_entry < Decimal::ONE;
                // Combined loss magnitude
                let combined_loss = (Decimal::ONE - leg1_vs_entry) + (Decimal::ONE - leg2_vs_entry);

                if both_dropping && combined_loss >= self.config.divergence_exit_threshold {
                    warn!(
                        trade_id = %settlement.trade_id,
                        leg1_entry = %settlement.leg1_entry_price,
                        leg2_entry = %settlement.leg2_entry_price,
                        leg1_bid = %leg1_bid,
                        leg2_bid = %leg2_bid,
                        leg1_drop = format!("{:.1}%", (Decimal::ONE - leg1_vs_entry) * dec!(100)),
                        leg2_drop = format!("{:.1}%", (Decimal::ONE - leg2_vs_entry) * dec!(100)),
                        combined_loss = format!("{:.1}%", combined_loss * dec!(100)),
                        "Divergence detected — both legs dropping, cutting losses"
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !is_profitable && !is_diverging {
                debug!(
                    trade_id = %settlement.trade_id,
                    profit_pct = %profit_pct,
                    threshold = %self.config.early_exit_profit_threshold,
                    leg1_bid = %leg1_bid,
                    leg2_bid = %leg2_bid,
                    "Early exit: no trigger (profit below threshold, no divergence)"
                );
                continue;
            }

            let exit_reason = if is_diverging { "DIVERGENCE" } else { "PROFIT" };
            info!(
                trade_id = %settlement.trade_id,
                reason = exit_reason,
                profit_pct = %profit_pct,
                current_value = %current_value,
                cost_basis = %cost_basis,
                leg1_bid = %leg1_bid,
                leg2_bid = %leg2_bid,
                remaining_leg1 = %settlement.remaining_shares_leg1,
                remaining_leg2 = %settlement.remaining_shares_leg2,
                "Early exit triggered, attempting sell"
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

            // Helper: determine sell size and price from orderbook, falling back to
            // WebSocket live price when REST orderbook returns empty bids.
            let calc_sell = |book: &Option<(Decimal, Option<Decimal>)>,
                             ws_bid: Decimal,
                             remaining: Decimal| -> (Decimal, Decimal) {
                // Try orderbook first
                if let Some((bid_depth, Some(best_bid))) = book {
                    if *bid_depth > Decimal::ZERO && *best_bid >= MIN_LEG_PRICE {
                        let max_sell = *bid_depth * depth_fraction;
                        let sell_size = remaining.min(max_sell);
                        let sell_value = sell_size * *best_bid;
                        if sell_size >= dec!(0.1) && sell_value >= MIN_ORDER_VALUE {
                            return (sell_size, *best_bid);
                        }
                    }
                }
                // Fallback: use WebSocket live price with conservative discount
                if ws_bid >= MIN_LEG_PRICE {
                    // Round to cent tick — Polymarket requires tick-aligned prices
                    let sell_price = ((ws_bid * dec!(0.95)) * dec!(100)).floor() / dec!(100);
                    let sell_price = sell_price.max(MIN_LEG_PRICE);
                    let sell_size = remaining;
                    let sell_value = sell_size * sell_price;
                    if sell_size >= dec!(0.1) && sell_value >= MIN_ORDER_VALUE {
                        return (sell_size, sell_price);
                    }
                }
                (Decimal::ZERO, Decimal::ZERO)
            };

            // Smart leg selection: only sell losing legs, keep winning legs for
            // potential $1.00 settlement. A "winning" leg has bid >= entry price.
            // Exception: if both are winning (profit exit), sell both to lock in.
            let leg1_winning = leg1_bid >= settlement.leg1_entry_price;
            let leg2_winning = leg2_bid >= settlement.leg2_entry_price;

            let (sell_leg1, sell_leg2) = match (leg1_winning, leg2_winning) {
                // Both winning: sell both to lock in profit
                (true, true) => (true, true),
                // One winning, one losing: only sell the loser, keep winner for settlement
                (true, false) => {
                    info!(
                        trade_id = %settlement.trade_id,
                        winning_leg = "leg1",
                        leg1_bid = %leg1_bid,
                        leg1_entry = %settlement.leg1_entry_price,
                        "Keeping winning leg for settlement, selling losing leg only"
                    );
                    (false, true)
                }
                (false, true) => {
                    info!(
                        trade_id = %settlement.trade_id,
                        winning_leg = "leg2",
                        leg2_bid = %leg2_bid,
                        leg2_entry = %settlement.leg2_entry_price,
                        "Keeping winning leg for settlement, selling losing leg only"
                    );
                    (true, false)
                }
                // Both losing: sell both to cut losses
                (false, false) => (true, true),
            };

            let (leg1_sell_size, leg1_sell_price) = if sell_leg1 {
                calc_sell(&leg1_book, leg1_bid, settlement.remaining_shares_leg1)
            } else {
                (Decimal::ZERO, Decimal::ZERO)
            };
            let (leg2_sell_size, leg2_sell_price) = if sell_leg2 {
                calc_sell(&leg2_book, leg2_bid, settlement.remaining_shares_leg2)
            } else {
                (Decimal::ZERO, Decimal::ZERO)
            };

            if leg1_sell_size <= Decimal::ZERO && leg2_sell_size <= Decimal::ZERO {
                warn!(
                    trade_id = %settlement.trade_id,
                    leg1_book = ?leg1_book,
                    leg2_book = ?leg2_book,
                    "Early exit: no sellable depth (bids empty or below minimums)"
                );
                continue;
            }

            // Mark exit attempt timestamp (cooldown on failure)
            {
                let mut pending = self.pending_settlements.write().await;
                if let Some(s) = pending.iter_mut().find(|s| s.trade_id == settlement.trade_id) {
                    s.last_exit_attempt = Some(now);
                }
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
                            let trade_id = settlement.trade_id.clone();

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

                // Credit balance and log outside lock scope
                if let Some((early_proceeds, ref trade_id, _)) = fully_exited_data {
                    let pnl = early_proceeds - settlement.total_cost;
                    info!(
                        trade_id = %trade_id,
                        proceeds = %early_proceeds,
                        cost = %settlement.total_cost,
                        pnl = %pnl,
                        "Early exit: FULLY EXITED"
                    );

                    if let Err(e) = self.executor.credit_balance(early_proceeds).await {
                        warn!(error = %e, "Failed to credit early exit proceeds");
                    }
                }

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
                    drop(stats);
                    let pnl_sign = if pnl >= Decimal::ZERO { "+" } else { "" };
                    let label = if is_diverging { "DIVERGENCE EXIT" } else { "EARLY EXIT" };
                    self.push_event(
                        EventKind::EarlyExit,
                        format!(
                            "{} {}/{} ${:.2} → {}{:.2}",
                            label, settlement.coin1, settlement.coin2, early_proceeds, pnl_sign, pnl,
                        ),
                    )
                    .await;
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
                    "Gamma API settlement not available, falling back to Chainlink oracle"
                );
                // CLOB returns 400 for resolved markets, skip straight to Chainlink
                // (same price source Polymarket uses for settlement)
                self.try_settle_via_chainlink(settlement).await?
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

        // Push settlement event to dashboard
        let pnl_sign = if pnl >= Decimal::ZERO { "+" } else { "" };
        let result_emoji = match trade_result {
            "DOUBLE_WIN" => "WIN",
            "WIN" => "WIN",
            _ => "LOSS",
        };
        self.push_event(
            EventKind::Settlement,
            format!(
                "SETTLED {}/{} {} {}{:.2} ({}↑ {}↑)",
                settlement.coin1, settlement.coin2, result_emoji, pnl_sign, pnl, c1_out, c2_out,
            ),
        )
        .await;

        // Credit the executor's balance back (essential for paper trading)
        // In live mode, this is a no-op (balance comes from chain)
        if let Err(e) = self.executor.credit_balance(net_payout).await {
            warn!(error = %e, "Failed to credit executor balance");
        }

        // In live mode, refresh USDC balance and derive P&L from actual state.
        // Uses get_balance() (USDC only) so open positions don't inflate P&L.
        if let Ok(bal) = self.executor.get_balance().await {
            let mut stats = self.stats.write().await;
            stats.live_balance = Some(bal);
            if let Some(initial) = self.initial_balance {
                let balance_pnl = bal - initial;
                info!(
                    balance_pnl = %balance_pnl,
                    estimated_pnl = %stats.realized_pnl,
                    live_balance = %bal,
                    initial_balance = %initial,
                    "P&L reconciliation: using balance-derived P&L"
                );
                stats.realized_pnl = balance_pnl;
            }
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

    /// Tries to settle via Chainlink oracle prices recorded at window boundaries.
    ///
    /// This is the correct fallback since Polymarket uses Chainlink for resolution.
    /// The tracker records oracle prices every ~10 seconds and compares
    /// start vs end price for each window.
    async fn try_settle_via_chainlink(
        &self,
        settlement: &PendingPaperSettlement,
    ) -> Result<(bool, bool), CrossMarketAutoExecutorError> {
        let window_end = settlement.window_end;
        let window_start_ts = (window_end - chrono::Duration::minutes(15)).timestamp();

        let tracker = self.chainlink_tracker.read().await;

        let c1_outcome = tracker.get_outcome(&settlement.coin1, window_start_ts);
        let c2_outcome = tracker.get_outcome(&settlement.coin2, window_start_ts);
        let c1_available = c1_outcome.is_some();
        let c2_available = c2_outcome.is_some();
        let tracked = tracker.tracked_window_count();
        let closed = tracker.closed_window_count();

        match (c1_outcome, c2_outcome) {
            (Some(c1), Some(c2)) => {
                let leg1_won = settlement.leg1_direction == c1;
                let leg2_won = settlement.leg2_direction == c2;

                info!(
                    trade_id = %settlement.trade_id,
                    coin1_outcome = %c1,
                    coin2_outcome = %c2,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "Settled via Chainlink oracle prices"
                );
                Ok((leg1_won, leg2_won))
            }
            _ => {
                debug!(
                    trade_id = %settlement.trade_id,
                    c1_available = c1_available,
                    c2_available = c2_available,
                    tracked_windows = tracked,
                    closed_windows = closed,
                    "Chainlink window data not available for settlement"
                );
                Err(CrossMarketAutoExecutorError::Execution(
                    ExecutionError::rejected(
                        "Chainlink window prices not available (bot may not have been running during this window)".to_string(),
                    ),
                ))
            }
        }
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
        // Use a timestamp 7 minutes into a 15-minute window (8 min remaining)
        // to land inside the default entry window (6-10 min before close).
        let now = Utc::now();
        let window_start = (now.timestamp() / 900) * 900;
        let safe_time = DateTime::from_timestamp(window_start + 420, 0).unwrap_or(now);
        CrossMarketOpportunity {
            coin1: "BTC".to_string(),
            coin2: "ETH".to_string(),
            combination: CrossMarketCombination::Coin1DownCoin2Up,
            leg1_direction: "DOWN".to_string(),
            leg1_price: dec!(0.40),
            leg1_token_id: "btc-down-token".to_string(),
            leg2_direction: "UP".to_string(),
            leg2_price: dec!(0.40),
            leg2_token_id: "eth-up-token".to_string(),
            total_cost: dec!(0.80),
            spread: dec!(0.20),
            expected_value: dec!(0.10),
            assumed_correlation: 0.73,
            win_probability: 0.92,
            detected_at: safe_time,
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
