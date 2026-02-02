//! Execution layer traits and types for Polymarket arbitrage.
//!
//! This module defines the core abstractions for order execution in Polymarket's CLOB.
//! The actual implementation requires EIP-712 signing which is Phase 3 work.
//!
//! # Overview
//!
//! The execution layer provides:
//! - Order submission (single and batch)
//! - Order status polling and terminal state waiting
//! - Position and balance queries
//! - Risk controls and execution configuration
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::execution::*;
//! use rust_decimal_macros::dec;
//!
//! async fn execute_arbitrage(executor: impl PolymarketExecutor) -> Result<(), ExecutionError> {
//!     // Check balance first
//!     let balance = executor.get_balance().await?;
//!     println!("Available balance: {}", balance);
//!
//!     // Submit a FOK order
//!     let order = OrderParams {
//!         token_id: "abc123".to_string(),
//!         side: Side::Buy,
//!         price: dec!(0.45),
//!         size: dec!(100),
//!         order_type: OrderType::Fok,
//!         neg_risk: true,
//!     };
//!
//!     let result = executor.submit_order(order).await?;
//!     println!("Order {} status: {:?}", result.order_id, result.status);
//!
//!     Ok(())
//! }
//! ```

use async_trait::async_trait;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

// =============================================================================
// Order Types
// =============================================================================

/// Side of an order (buy/sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    /// Buy shares (go long).
    Buy,
    /// Sell shares (go short or close position).
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Order type determining fill behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    /// Fill-or-Kill: Must fill entirely or cancel immediately.
    /// Required for arbitrage to prevent partial fills.
    Fok,
    /// Fill-and-Kill: Fill what's available, cancel the rest.
    /// Used for unwinding positions.
    Fak,
    /// Good-til-Cancelled: Remains on book until filled or cancelled.
    /// Not recommended for arbitrage due to exposure risk.
    Gtc,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Fok => write!(f, "FOK"),
            OrderType::Fak => write!(f, "FAK"),
            OrderType::Gtc => write!(f, "GTC"),
        }
    }
}

// =============================================================================
// Order Parameters and Results
// =============================================================================

/// Parameters for order submission.
///
/// Contains all required fields for submitting an order to Polymarket's CLOB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderParams {
    /// Token ID for the outcome (YES or NO token).
    pub token_id: String,

    /// Order side (buy or sell).
    pub side: Side,

    /// Limit price (0.01 to 0.99 for probability markets).
    /// For buys: maximum price willing to pay.
    /// For sells: minimum price willing to receive.
    pub price: Decimal,

    /// Number of shares to trade.
    pub size: Decimal,

    /// Order type determining fill behavior.
    pub order_type: OrderType,

    /// Neg-risk flag required for certain market types.
    /// Must be true for BTC 15-minute binary markets.
    pub neg_risk: bool,
}

impl OrderParams {
    /// Creates a new FOK buy order (most common for arbitrage).
    #[must_use]
    pub fn buy_fok(token_id: impl Into<String>, price: Decimal, size: Decimal) -> Self {
        Self {
            token_id: token_id.into(),
            side: Side::Buy,
            price,
            size,
            order_type: OrderType::Fok,
            neg_risk: true,
        }
    }

    /// Creates a new FAK sell order (for unwinding).
    #[must_use]
    pub fn sell_fak(token_id: impl Into<String>, price: Decimal, size: Decimal) -> Self {
        Self {
            token_id: token_id.into(),
            side: Side::Sell,
            price,
            size,
            order_type: OrderType::Fak,
            neg_risk: true,
        }
    }

    /// Sets the neg_risk flag.
    #[must_use]
    pub fn with_neg_risk(mut self, neg_risk: bool) -> Self {
        self.neg_risk = neg_risk;
        self
    }

    /// Total cost/proceeds of this order if fully filled.
    #[must_use]
    pub fn notional_value(&self) -> Decimal {
        self.price * self.size
    }
}

/// Status of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    /// Order submitted but not yet processed.
    Pending,
    /// Order partially filled, remainder still on book.
    PartiallyFilled,
    /// Order completely filled.
    Filled,
    /// Order cancelled by user.
    Cancelled,
    /// Order rejected by exchange (insufficient balance, invalid params, etc.).
    Rejected,
    /// Order expired (for time-limited orders).
    Expired,
}

impl OrderStatus {
    /// Returns true if this status is terminal (no further changes expected).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderStatus::Filled
                | OrderStatus::Cancelled
                | OrderStatus::Rejected
                | OrderStatus::Expired
        )
    }

    /// Returns true if the order resulted in any fills.
    #[must_use]
    pub fn has_fills(&self) -> bool {
        matches!(self, OrderStatus::Filled | OrderStatus::PartiallyFilled)
    }
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::Pending => write!(f, "PENDING"),
            OrderStatus::PartiallyFilled => write!(f, "PARTIALLY_FILLED"),
            OrderStatus::Filled => write!(f, "FILLED"),
            OrderStatus::Cancelled => write!(f, "CANCELLED"),
            OrderStatus::Rejected => write!(f, "REJECTED"),
            OrderStatus::Expired => write!(f, "EXPIRED"),
        }
    }
}

/// Result of an order submission or status query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResult {
    /// Unique order identifier from the exchange.
    pub order_id: String,

    /// Current order status.
    pub status: OrderStatus,

    /// Amount filled so far.
    pub filled_size: Decimal,

    /// Volume-weighted average fill price (if any fills occurred).
    pub avg_fill_price: Option<Decimal>,

    /// Error message if order was rejected or failed.
    pub error: Option<String>,
}

impl OrderResult {
    /// Creates a successful filled order result.
    #[must_use]
    pub fn filled(order_id: impl Into<String>, size: Decimal, price: Decimal) -> Self {
        Self {
            order_id: order_id.into(),
            status: OrderStatus::Filled,
            filled_size: size,
            avg_fill_price: Some(price),
            error: None,
        }
    }

    /// Creates a rejected order result.
    #[must_use]
    pub fn rejected(order_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            order_id: order_id.into(),
            status: OrderStatus::Rejected,
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            error: Some(reason.into()),
        }
    }

    /// Creates a pending order result.
    #[must_use]
    pub fn pending(order_id: impl Into<String>) -> Self {
        Self {
            order_id: order_id.into(),
            status: OrderStatus::Pending,
            filled_size: Decimal::ZERO,
            avg_fill_price: None,
            error: None,
        }
    }

    /// Returns true if the order was fully filled.
    #[must_use]
    pub fn is_filled(&self) -> bool {
        self.status == OrderStatus::Filled
    }

    /// Returns true if the order reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Returns the total notional value of fills.
    #[must_use]
    pub fn fill_notional(&self) -> Decimal {
        self.avg_fill_price.unwrap_or(Decimal::ZERO) * self.filled_size
    }
}

// =============================================================================
// Position Types
// =============================================================================

/// A position in a single token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Token ID.
    pub token_id: String,

    /// Number of shares held (negative for short).
    pub size: Decimal,

    /// Average entry price.
    pub avg_price: Decimal,

    /// Current market price (if available).
    pub current_price: Option<Decimal>,

    /// Unrealized P&L (if current_price available).
    pub unrealized_pnl: Option<Decimal>,
}

impl Position {
    /// Creates a new position.
    #[must_use]
    pub fn new(token_id: impl Into<String>, size: Decimal, avg_price: Decimal) -> Self {
        Self {
            token_id: token_id.into(),
            size,
            avg_price,
            current_price: None,
            unrealized_pnl: None,
        }
    }

    /// Updates position with current market price and calculates unrealized P&L.
    #[must_use]
    pub fn with_current_price(mut self, price: Decimal) -> Self {
        self.current_price = Some(price);
        self.unrealized_pnl = Some((price - self.avg_price) * self.size);
        self
    }

    /// Total cost basis of this position.
    #[must_use]
    pub fn cost_basis(&self) -> Decimal {
        self.avg_price * self.size
    }
}

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur during order execution.
#[derive(Debug, Clone, Error)]
pub enum ExecutionError {
    /// Order was rejected by the exchange.
    #[error("Order rejected: {reason}")]
    Rejected {
        /// Rejection reason from exchange.
        reason: String,
    },

    /// Insufficient balance to execute order.
    #[error("Insufficient balance: need {required}, have {available}")]
    InsufficientBalance {
        /// Balance required for the order.
        required: Decimal,
        /// Currently available balance.
        available: Decimal,
    },

    /// Timed out waiting for order to reach terminal state.
    #[error("Timeout waiting for order: {order_id}")]
    Timeout {
        /// Order ID that timed out.
        order_id: String,
    },

    /// Order was only partially filled (for FOK orders that shouldn't partial fill,
    /// this indicates a serious issue).
    #[error("Partial fill: filled {filled} of {requested} for order {order_id}")]
    PartialFill {
        /// Order ID that partially filled.
        order_id: String,
        /// Amount that was filled.
        filled: Decimal,
        /// Amount originally requested.
        requested: Decimal,
    },

    /// API communication error.
    #[error("API error: {0}")]
    Api(String),

    /// Error during order signing (EIP-712).
    #[error("Signing error: {0}")]
    Signing(String),

    /// Rate limit exceeded.
    #[error("Rate limit exceeded: retry after {retry_after_secs}s")]
    RateLimited {
        /// Seconds to wait before retrying.
        retry_after_secs: u64,
    },

    /// Invalid order parameters.
    #[error("Invalid order: {0}")]
    InvalidOrder(String),

    /// Network/connection error.
    #[error("Network error: {0}")]
    Network(String),
}

impl ExecutionError {
    /// Creates a rejected error with the given reason.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }

    /// Creates an insufficient balance error.
    #[must_use]
    pub fn insufficient_balance(required: Decimal, available: Decimal) -> Self {
        Self::InsufficientBalance {
            required,
            available,
        }
    }

    /// Creates a timeout error.
    #[must_use]
    pub fn timeout(order_id: impl Into<String>) -> Self {
        Self::Timeout {
            order_id: order_id.into(),
        }
    }

    /// Creates a partial fill error.
    #[must_use]
    pub fn partial_fill(order_id: impl Into<String>, filled: Decimal, requested: Decimal) -> Self {
        Self::PartialFill {
            order_id: order_id.into(),
            filled,
            requested,
        }
    }

    /// Returns true if this error is retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ExecutionError::Timeout { .. }
                | ExecutionError::RateLimited { .. }
                | ExecutionError::Network(_)
        )
    }
}

// =============================================================================
// Executor Trait
// =============================================================================

/// Core trait for executing orders on Polymarket's CLOB.
///
/// Implementations must handle:
/// - EIP-712 order signing
/// - API authentication
/// - Order submission and polling
/// - Position and balance queries
///
/// # Implementation Notes
///
/// TODO(Phase 3): Implement EIP-712 signing using ethers-rs or alloy.
/// The signing flow is:
/// 1. Construct order struct with all parameters
/// 2. Compute EIP-712 struct hash
/// 3. Sign with private key
/// 4. Submit signed order to POST /order endpoint
///
/// Reference: https://docs.polymarket.com/#signing-orders
#[async_trait]
pub trait PolymarketExecutor: Send + Sync {
    /// Signs and submits a single order.
    ///
    /// # Arguments
    /// * `order` - Order parameters to submit
    ///
    /// # Returns
    /// Order result with ID and initial status, or error.
    ///
    /// # Errors
    /// - `ExecutionError::InsufficientBalance` - Not enough USDC
    /// - `ExecutionError::Signing` - Failed to sign order
    /// - `ExecutionError::Rejected` - Exchange rejected order
    /// - `ExecutionError::Api` - API communication failure
    async fn submit_order(&self, order: OrderParams) -> Result<OrderResult, ExecutionError>;

    /// Pre-signs and submits multiple orders in batch.
    ///
    /// Batch submission is faster than individual orders because:
    /// 1. Orders are pre-signed in parallel
    /// 2. Single API call for submission
    /// 3. Reduces network round-trips
    ///
    /// # Arguments
    /// * `orders` - Vector of order parameters
    ///
    /// # Returns
    /// Vector of order results in same order as input.
    ///
    /// # Errors
    /// Returns error if ANY order fails to sign. Individual order rejections
    /// are returned as rejected OrderResults, not errors.
    async fn submit_orders_batch(
        &self,
        orders: Vec<OrderParams>,
    ) -> Result<Vec<OrderResult>, ExecutionError>;

    /// Cancels an order by ID.
    ///
    /// # Arguments
    /// * `order_id` - Exchange order ID to cancel
    ///
    /// # Errors
    /// - `ExecutionError::Api` - Order not found or already terminal
    async fn cancel_order(&self, order_id: &str) -> Result<(), ExecutionError>;

    /// Gets current status of an order.
    ///
    /// # Arguments
    /// * `order_id` - Exchange order ID to query
    ///
    /// # Returns
    /// Current order result with status and fill information.
    async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ExecutionError>;

    /// Polls order until it reaches a terminal state.
    ///
    /// Terminal states: Filled, Cancelled, Rejected, Expired
    ///
    /// # Arguments
    /// * `order_id` - Exchange order ID to poll
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// Final order result.
    ///
    /// # Errors
    /// - `ExecutionError::Timeout` - Order didn't reach terminal state in time
    async fn wait_for_terminal(
        &self,
        order_id: &str,
        timeout: Duration,
    ) -> Result<OrderResult, ExecutionError>;

    /// Gets all current positions.
    ///
    /// # Returns
    /// Vector of positions (may be empty).
    async fn get_positions(&self) -> Result<Vec<Position>, ExecutionError>;

    /// Gets available USDC balance.
    ///
    /// # Returns
    /// Available balance for trading.
    async fn get_balance(&self) -> Result<Decimal, ExecutionError>;
}

// =============================================================================
// Executor Configuration
// =============================================================================

/// Configuration for the arbitrage executor.
///
/// Controls risk limits, timing, and safety margins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Safety margin multiplier for balance checks (e.g., 1.20 for 20% margin).
    /// Required balance = order_cost * balance_margin
    pub balance_margin: Decimal,

    /// Minimum time between executions.
    pub cooldown: Duration,

    /// Maximum time to wait for order to fill.
    pub order_timeout: Duration,

    /// Maximum allowed imbalance between YES and NO shares.
    /// Positions exceeding this trigger unwind attempts.
    pub max_imbalance: Decimal,

    /// Maximum daily loss before stopping trading.
    pub max_daily_loss: Decimal,

    /// Maximum consecutive failures before pausing.
    pub max_consecutive_failures: u32,

    /// Pause duration after max consecutive failures.
    pub failure_pause: Duration,

    /// Minimum profit per pair to execute (after fees).
    pub min_profit_threshold: Decimal,

    /// Maximum position size per market.
    pub max_position_size: Decimal,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            balance_margin: dec!(1.20),             // 20% safety margin
            cooldown: Duration::from_secs(5),       // 5 second cooldown
            order_timeout: Duration::from_secs(3),  // 3 second timeout
            max_imbalance: dec!(50),                // Max 50 share imbalance
            max_daily_loss: dec!(50),               // $50 max daily loss
            max_consecutive_failures: 3,            // Pause after 3 failures
            failure_pause: Duration::from_secs(60), // 60 second pause
            min_profit_threshold: dec!(0.005),      // $0.005 min profit
            max_position_size: dec!(1000),          // Max 1000 shares
        }
    }
}

impl ExecutorConfig {
    /// Creates config with conservative settings for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            balance_margin: dec!(1.50), // 50% safety margin
            cooldown: Duration::from_secs(10),
            order_timeout: Duration::from_secs(5),
            max_imbalance: dec!(25),
            max_daily_loss: dec!(25),
            max_consecutive_failures: 2,
            failure_pause: Duration::from_secs(120),
            min_profit_threshold: dec!(0.01),
            max_position_size: dec!(500),
        }
    }

    /// Creates config for aggressive trading (higher risk).
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            balance_margin: dec!(1.10), // 10% safety margin
            cooldown: Duration::from_secs(2),
            order_timeout: Duration::from_secs(2),
            max_imbalance: dec!(100),
            max_daily_loss: dec!(100),
            max_consecutive_failures: 5,
            failure_pause: Duration::from_secs(30),
            min_profit_threshold: dec!(0.002),
            max_position_size: dec!(2000),
        }
    }

    /// Sets the balance margin.
    #[must_use]
    pub fn with_balance_margin(mut self, margin: Decimal) -> Self {
        self.balance_margin = margin;
        self
    }

    /// Sets the order timeout.
    #[must_use]
    pub fn with_order_timeout(mut self, timeout: Duration) -> Self {
        self.order_timeout = timeout;
        self
    }

    /// Sets the maximum daily loss.
    #[must_use]
    pub fn with_max_daily_loss(mut self, max_loss: Decimal) -> Self {
        self.max_daily_loss = max_loss;
        self
    }

    /// Sets the maximum position size.
    #[must_use]
    pub fn with_max_position_size(mut self, max_size: Decimal) -> Self {
        self.max_position_size = max_size;
        self
    }
}

// =============================================================================
// Execution Result
// =============================================================================

/// Outcome of an arbitrage execution attempt.
///
/// Represents the full result of trying to open a paired arbitrage position.
/// Large variants are boxed to keep the enum size reasonable.
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    /// Both legs filled successfully, arbitrage position opened.
    Success {
        /// The opened arbitrage position (boxed to reduce enum size).
        position: Box<ArbitragePositionSnapshot>,
        /// YES order result (boxed to reduce enum size).
        yes_order: Box<OrderResult>,
        /// NO order result (boxed to reduce enum size).
        no_order: Box<OrderResult>,
    },

    /// Only one leg filled, creating exposure.
    /// Unwind should be attempted immediately.
    PartialFill {
        /// Which side filled.
        filled_side: Side,
        /// The filled order result (boxed to reduce enum size).
        order: Box<OrderResult>,
        /// Whether an unwind was attempted.
        unwind_attempted: bool,
    },

    /// Both orders rejected, no position change.
    Rejected {
        /// Rejection reason.
        reason: String,
    },

    /// Risk limit prevented execution.
    RiskLimitHit {
        /// Which limit was hit.
        limit: RiskLimit,
    },
}

/// Types of risk limits that can prevent execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskLimit {
    /// Cooldown between trades not elapsed.
    Cooldown {
        /// Seconds remaining.
        remaining_secs: u64,
    },
    /// Daily loss limit reached.
    DailyLoss {
        /// Current daily P&L.
        current_pnl: Decimal,
        /// Configured limit.
        limit: Decimal,
    },
    /// Too many consecutive failures.
    ConsecutiveFailures {
        /// Number of failures.
        failures: u32,
    },
    /// Position size would exceed limit.
    PositionSize {
        /// Current size.
        current: Decimal,
        /// Would-be size.
        proposed: Decimal,
        /// Maximum allowed.
        max: Decimal,
    },
    /// Insufficient balance.
    InsufficientBalance {
        /// Required amount.
        required: Decimal,
        /// Available amount.
        available: Decimal,
    },
    /// Imbalance would exceed limit.
    Imbalance {
        /// Current imbalance.
        current: Decimal,
        /// Maximum allowed.
        max: Decimal,
    },
}

impl std::fmt::Display for RiskLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLimit::Cooldown { remaining_secs } => {
                write!(f, "Cooldown active: {}s remaining", remaining_secs)
            }
            RiskLimit::DailyLoss { current_pnl, limit } => {
                write!(f, "Daily loss limit: {} / {} limit", current_pnl, limit)
            }
            RiskLimit::ConsecutiveFailures { failures } => {
                write!(f, "Too many consecutive failures: {}", failures)
            }
            RiskLimit::PositionSize {
                current,
                proposed,
                max,
            } => {
                write!(
                    f,
                    "Position size limit: {} + proposed = {} > {} max",
                    current, proposed, max
                )
            }
            RiskLimit::InsufficientBalance {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient balance: need {}, have {}",
                    required, available
                )
            }
            RiskLimit::Imbalance { current, max } => {
                write!(f, "Imbalance limit: {} > {} max", current, max)
            }
        }
    }
}

/// Snapshot of an arbitrage position for execution results.
///
/// Lighter weight than full ArbitragePosition, used for immediate execution feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitragePositionSnapshot {
    /// Market condition ID.
    pub market_id: String,

    /// YES shares acquired.
    pub yes_shares: Decimal,

    /// Total cost of YES shares.
    pub yes_cost: Decimal,

    /// NO shares acquired.
    pub no_shares: Decimal,

    /// Total cost of NO shares.
    pub no_cost: Decimal,

    /// Combined pair cost per share.
    pub pair_cost: Decimal,

    /// Guaranteed profit (assuming settlement).
    pub guaranteed_profit: Decimal,

    /// Share imbalance (yes - no).
    pub imbalance: Decimal,
}

impl ArbitragePositionSnapshot {
    /// Creates a new position snapshot from order results.
    #[must_use]
    pub fn from_orders(
        market_id: impl Into<String>,
        yes_result: &OrderResult,
        no_result: &OrderResult,
        yes_price: Decimal,
        no_price: Decimal,
    ) -> Self {
        let yes_shares = yes_result.filled_size;
        let no_shares = no_result.filled_size;
        let yes_cost = yes_result.fill_notional();
        let no_cost = no_result.fill_notional();

        let min_shares = yes_shares.min(no_shares);
        let pair_cost = if min_shares > Decimal::ZERO {
            (yes_cost + no_cost) / min_shares
        } else {
            yes_price + no_price
        };

        let guaranteed_profit = min_shares - (yes_cost + no_cost);
        let imbalance = yes_shares - no_shares;

        Self {
            market_id: market_id.into(),
            yes_shares,
            yes_cost,
            no_shares,
            no_cost,
            pair_cost,
            guaranteed_profit,
            imbalance,
        }
    }

    /// Returns true if the position is balanced (no imbalance).
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.imbalance == Decimal::ZERO
    }

    /// Returns true if the position has profitable arbitrage (pair_cost < 1).
    #[must_use]
    pub fn is_profitable(&self) -> bool {
        self.pair_cost < Decimal::ONE
    }

    /// Total capital invested.
    #[must_use]
    pub fn total_investment(&self) -> Decimal {
        self.yes_cost + self.no_cost
    }

    /// ROI as a decimal (e.g., 0.03 for 3%).
    #[must_use]
    pub fn roi(&self) -> Decimal {
        let investment = self.total_investment();
        if investment > Decimal::ZERO {
            self.guaranteed_profit / investment
        } else {
            Decimal::ZERO
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ==================== Side Tests ====================

    #[test]
    fn test_side_display() {
        assert_eq!(Side::Buy.to_string(), "BUY");
        assert_eq!(Side::Sell.to_string(), "SELL");
    }

    #[test]
    fn test_side_serialization() {
        let buy_json = serde_json::to_string(&Side::Buy).unwrap();
        assert_eq!(buy_json, "\"BUY\"");

        let sell: Side = serde_json::from_str("\"SELL\"").unwrap();
        assert_eq!(sell, Side::Sell);
    }

    // ==================== OrderType Tests ====================

    #[test]
    fn test_order_type_display() {
        assert_eq!(OrderType::Fok.to_string(), "FOK");
        assert_eq!(OrderType::Fak.to_string(), "FAK");
        assert_eq!(OrderType::Gtc.to_string(), "GTC");
    }

    #[test]
    fn test_order_type_serialization() {
        let fok_json = serde_json::to_string(&OrderType::Fok).unwrap();
        assert_eq!(fok_json, "\"FOK\"");
    }

    // ==================== OrderParams Tests ====================

    #[test]
    fn test_order_params_buy_fok() {
        let order = OrderParams::buy_fok("token123", dec!(0.45), dec!(100));

        assert_eq!(order.token_id, "token123");
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.price, dec!(0.45));
        assert_eq!(order.size, dec!(100));
        assert_eq!(order.order_type, OrderType::Fok);
        assert!(order.neg_risk);
    }

    #[test]
    fn test_order_params_sell_fak() {
        let order = OrderParams::sell_fak("token456", dec!(0.01), dec!(50));

        assert_eq!(order.side, Side::Sell);
        assert_eq!(order.order_type, OrderType::Fak);
    }

    #[test]
    fn test_order_params_notional_value() {
        let order = OrderParams::buy_fok("token", dec!(0.50), dec!(200));
        assert_eq!(order.notional_value(), dec!(100));
    }

    #[test]
    fn test_order_params_with_neg_risk() {
        let order = OrderParams::buy_fok("token", dec!(0.50), dec!(100)).with_neg_risk(false);
        assert!(!order.neg_risk);
    }

    // ==================== OrderStatus Tests ====================

    #[test]
    fn test_order_status_is_terminal() {
        assert!(OrderStatus::Filled.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(OrderStatus::Rejected.is_terminal());
        assert!(OrderStatus::Expired.is_terminal());

        assert!(!OrderStatus::Pending.is_terminal());
        assert!(!OrderStatus::PartiallyFilled.is_terminal());
    }

    #[test]
    fn test_order_status_has_fills() {
        assert!(OrderStatus::Filled.has_fills());
        assert!(OrderStatus::PartiallyFilled.has_fills());

        assert!(!OrderStatus::Pending.has_fills());
        assert!(!OrderStatus::Cancelled.has_fills());
        assert!(!OrderStatus::Rejected.has_fills());
        assert!(!OrderStatus::Expired.has_fills());
    }

    #[test]
    fn test_order_status_display() {
        assert_eq!(OrderStatus::Filled.to_string(), "FILLED");
        assert_eq!(OrderStatus::PartiallyFilled.to_string(), "PARTIALLY_FILLED");
    }

    // ==================== OrderResult Tests ====================

    #[test]
    fn test_order_result_filled() {
        let result = OrderResult::filled("order123", dec!(100), dec!(0.45));

        assert_eq!(result.order_id, "order123");
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_size, dec!(100));
        assert_eq!(result.avg_fill_price, Some(dec!(0.45)));
        assert!(result.error.is_none());
        assert!(result.is_filled());
        assert!(result.is_terminal());
    }

    #[test]
    fn test_order_result_rejected() {
        let result = OrderResult::rejected("order456", "Insufficient balance");

        assert_eq!(result.status, OrderStatus::Rejected);
        assert_eq!(result.filled_size, Decimal::ZERO);
        assert!(result.avg_fill_price.is_none());
        assert_eq!(result.error, Some("Insufficient balance".to_string()));
        assert!(!result.is_filled());
        assert!(result.is_terminal());
    }

    #[test]
    fn test_order_result_pending() {
        let result = OrderResult::pending("order789");

        assert_eq!(result.status, OrderStatus::Pending);
        assert!(!result.is_filled());
        assert!(!result.is_terminal());
    }

    #[test]
    fn test_order_result_fill_notional() {
        let result = OrderResult::filled("order", dec!(100), dec!(0.45));
        assert_eq!(result.fill_notional(), dec!(45));

        let rejected = OrderResult::rejected("order", "reason");
        assert_eq!(rejected.fill_notional(), Decimal::ZERO);
    }

    // ==================== Position Tests ====================

    #[test]
    fn test_position_new() {
        let pos = Position::new("token123", dec!(100), dec!(0.45));

        assert_eq!(pos.token_id, "token123");
        assert_eq!(pos.size, dec!(100));
        assert_eq!(pos.avg_price, dec!(0.45));
        assert!(pos.current_price.is_none());
        assert!(pos.unrealized_pnl.is_none());
    }

    #[test]
    fn test_position_cost_basis() {
        let pos = Position::new("token", dec!(100), dec!(0.45));
        assert_eq!(pos.cost_basis(), dec!(45));
    }

    #[test]
    fn test_position_with_current_price() {
        let pos = Position::new("token", dec!(100), dec!(0.45)).with_current_price(dec!(0.50));

        assert_eq!(pos.current_price, Some(dec!(0.50)));
        assert_eq!(pos.unrealized_pnl, Some(dec!(5))); // (0.50 - 0.45) * 100
    }

    // ==================== ExecutionError Tests ====================

    #[test]
    fn test_execution_error_rejected() {
        let err = ExecutionError::rejected("Bad order");
        assert!(matches!(err, ExecutionError::Rejected { reason } if reason == "Bad order"));
    }

    #[test]
    fn test_execution_error_insufficient_balance() {
        let err = ExecutionError::insufficient_balance(dec!(100), dec!(50));
        assert!(
            matches!(err, ExecutionError::InsufficientBalance { required, available }
                if required == dec!(100) && available == dec!(50))
        );
    }

    #[test]
    fn test_execution_error_timeout() {
        let err = ExecutionError::timeout("order123");
        assert!(matches!(err, ExecutionError::Timeout { order_id } if order_id == "order123"));
    }

    #[test]
    fn test_execution_error_partial_fill() {
        let err = ExecutionError::partial_fill("order456", dec!(50), dec!(100));
        assert!(
            matches!(err, ExecutionError::PartialFill { order_id, filled, requested }
                if order_id == "order456" && filled == dec!(50) && requested == dec!(100))
        );
    }

    #[test]
    fn test_execution_error_is_retryable() {
        assert!(ExecutionError::timeout("order").is_retryable());
        assert!(ExecutionError::RateLimited {
            retry_after_secs: 5
        }
        .is_retryable());
        assert!(ExecutionError::Network("connection failed".to_string()).is_retryable());

        assert!(!ExecutionError::rejected("bad order").is_retryable());
        assert!(!ExecutionError::insufficient_balance(dec!(100), dec!(50)).is_retryable());
        assert!(!ExecutionError::Signing("invalid key".to_string()).is_retryable());
    }

    #[test]
    fn test_execution_error_display() {
        let err = ExecutionError::insufficient_balance(dec!(100), dec!(50));
        let msg = err.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("50"));
    }

    // ==================== ExecutorConfig Tests ====================

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();

        assert_eq!(config.balance_margin, dec!(1.20));
        assert_eq!(config.cooldown, Duration::from_secs(5));
        assert_eq!(config.order_timeout, Duration::from_secs(3));
        assert_eq!(config.max_imbalance, dec!(50));
        assert_eq!(config.max_daily_loss, dec!(50));
        assert_eq!(config.max_consecutive_failures, 3);
    }

    #[test]
    fn test_executor_config_conservative() {
        let config = ExecutorConfig::conservative();

        assert_eq!(config.balance_margin, dec!(1.50));
        assert_eq!(config.max_daily_loss, dec!(25));
        assert_eq!(config.max_consecutive_failures, 2);
    }

    #[test]
    fn test_executor_config_aggressive() {
        let config = ExecutorConfig::aggressive();

        assert_eq!(config.balance_margin, dec!(1.10));
        assert_eq!(config.max_daily_loss, dec!(100));
        assert_eq!(config.max_consecutive_failures, 5);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = ExecutorConfig::default()
            .with_balance_margin(dec!(1.30))
            .with_order_timeout(Duration::from_secs(10))
            .with_max_daily_loss(dec!(75))
            .with_max_position_size(dec!(500));

        assert_eq!(config.balance_margin, dec!(1.30));
        assert_eq!(config.order_timeout, Duration::from_secs(10));
        assert_eq!(config.max_daily_loss, dec!(75));
        assert_eq!(config.max_position_size, dec!(500));
    }

    // ==================== RiskLimit Tests ====================

    #[test]
    fn test_risk_limit_display() {
        let cooldown = RiskLimit::Cooldown { remaining_secs: 3 };
        assert!(cooldown.to_string().contains("3s"));

        let daily = RiskLimit::DailyLoss {
            current_pnl: dec!(-40),
            limit: dec!(50),
        };
        assert!(daily.to_string().contains("-40"));

        let failures = RiskLimit::ConsecutiveFailures { failures: 3 };
        assert!(failures.to_string().contains("3"));
    }

    // ==================== ArbitragePositionSnapshot Tests ====================

    #[test]
    fn test_position_snapshot_from_orders() {
        let yes_result = OrderResult::filled("yes-order", dec!(100), dec!(0.45));
        let no_result = OrderResult::filled("no-order", dec!(100), dec!(0.52));

        let snapshot = ArbitragePositionSnapshot::from_orders(
            "market123",
            &yes_result,
            &no_result,
            dec!(0.45),
            dec!(0.52),
        );

        assert_eq!(snapshot.market_id, "market123");
        assert_eq!(snapshot.yes_shares, dec!(100));
        assert_eq!(snapshot.no_shares, dec!(100));
        assert_eq!(snapshot.yes_cost, dec!(45)); // 100 * 0.45
        assert_eq!(snapshot.no_cost, dec!(52)); // 100 * 0.52
        assert_eq!(snapshot.pair_cost, dec!(0.97)); // (45 + 52) / 100
        assert!(snapshot.is_balanced());
        assert!(snapshot.is_profitable()); // 0.97 < 1.00
    }

    #[test]
    fn test_position_snapshot_guaranteed_profit() {
        let yes_result = OrderResult::filled("yes", dec!(100), dec!(0.45));
        let no_result = OrderResult::filled("no", dec!(100), dec!(0.52));

        let snapshot = ArbitragePositionSnapshot::from_orders(
            "market",
            &yes_result,
            &no_result,
            dec!(0.45),
            dec!(0.52),
        );

        // Profit = min_shares (100) - total_cost (45 + 52 = 97) = 3
        assert_eq!(snapshot.guaranteed_profit, dec!(3));
    }

    #[test]
    fn test_position_snapshot_imbalanced() {
        let yes_result = OrderResult::filled("yes", dec!(100), dec!(0.45));
        let no_result = OrderResult::filled("no", dec!(80), dec!(0.52));

        let snapshot = ArbitragePositionSnapshot::from_orders(
            "market",
            &yes_result,
            &no_result,
            dec!(0.45),
            dec!(0.52),
        );

        assert!(!snapshot.is_balanced());
        assert_eq!(snapshot.imbalance, dec!(20)); // 100 - 80
    }

    #[test]
    fn test_position_snapshot_total_investment() {
        let yes_result = OrderResult::filled("yes", dec!(100), dec!(0.45));
        let no_result = OrderResult::filled("no", dec!(100), dec!(0.50));

        let snapshot = ArbitragePositionSnapshot::from_orders(
            "market",
            &yes_result,
            &no_result,
            dec!(0.45),
            dec!(0.50),
        );

        assert_eq!(snapshot.total_investment(), dec!(95)); // 45 + 50
    }

    #[test]
    fn test_position_snapshot_roi() {
        let yes_result = OrderResult::filled("yes", dec!(100), dec!(0.45));
        let no_result = OrderResult::filled("no", dec!(100), dec!(0.52));

        let snapshot = ArbitragePositionSnapshot::from_orders(
            "market",
            &yes_result,
            &no_result,
            dec!(0.45),
            dec!(0.52),
        );

        // ROI = profit / investment = 3 / 97 ≈ 0.0309
        let roi = snapshot.roi();
        assert!(roi > dec!(0.03) && roi < dec!(0.032));
    }
}
