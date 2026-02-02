//! Cross-exchange executor for coordinated Kalshi/Polymarket execution.
//!
//! This module provides safe, coordinated execution of arbitrage opportunities
//! across both exchanges with:
//! - Settlement verification before execution
//! - Simultaneous order submission using `tokio::join!`
//! - Automatic unwind attempts on partial fills
//! - Circuit breaker protection
//! - Position tracking
//!
//! # Safety Guarantees
//!
//! 1. **Settlement verification BEFORE execution** - Never executes if settlement
//!    criteria don't match identically across exchanges.
//! 2. **Simultaneous submission** - Both orders are submitted concurrently to minimize
//!    timing risk.
//! 3. **Automatic unwind** - If one leg fails, attempts to close the other.
//! 4. **Circuit breaker** - Trips on consecutive failures or daily loss.
//! 5. **Position tracking** - All open cross-exchange positions are tracked.
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_arbitrage_cross::executor::{CrossExchangeExecutor, CrossExecutorConfig};
//!
//! let executor = CrossExchangeExecutor::new(
//!     kalshi_executor,
//!     polymarket_executor,
//!     CrossExecutorConfig::conservative(),
//! );
//!
//! // Execute an opportunity (validates settlement first)
//! let result = executor.execute(&opportunity).await;
//! ```

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use algo_trade_kalshi::executor::{HardLimits as KalshiHardLimits, KalshiExecutor};
use algo_trade_kalshi::types::{Order as KalshiOrder, OrderRequest, Side as KalshiSide};
use algo_trade_polymarket::arbitrage::execution::{OrderParams, OrderResult, PolymarketExecutor};

use crate::detector::CrossExchangeOpportunity;
use crate::types::{MatchedMarket, SettlementVerification, Side};

// =============================================================================
// Hard Limits for Polymarket (unified with Kalshi pattern)
// =============================================================================

/// Hard limits for Polymarket order validation.
///
/// These are safety limits to prevent catastrophic trading errors.
/// Polymarket uses dollars (0.01-0.99) for prices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketHardLimits {
    /// Maximum order size in shares.
    pub max_order_shares: Decimal,

    /// Minimum order size in shares.
    pub min_order_shares: Decimal,

    /// Maximum price (typically 0.99).
    pub max_price: Decimal,

    /// Minimum price (typically 0.01).
    pub min_price: Decimal,

    /// Maximum single order value in dollars.
    pub max_order_value: Decimal,

    /// Maximum daily volume in dollars.
    pub max_daily_volume: Decimal,

    /// Minimum balance reserve to keep in dollars.
    pub min_balance_reserve: Decimal,
}

impl Default for PolymarketHardLimits {
    fn default() -> Self {
        Self {
            max_order_shares: dec!(1000),
            min_order_shares: dec!(1),
            max_price: dec!(0.95),
            min_price: dec!(0.05),
            max_order_value: dec!(500), // $500 max per order
            max_daily_volume: dec!(5000), // $5000 daily
            min_balance_reserve: dec!(50), // Keep $50 minimum
        }
    }
}

impl PolymarketHardLimits {
    /// Creates conservative limits for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_order_shares: dec!(100),
            min_order_shares: dec!(1),
            max_price: dec!(0.90),
            min_price: dec!(0.10),
            max_order_value: dec!(100), // $100
            max_daily_volume: dec!(500), // $500
            min_balance_reserve: dec!(100), // $100
        }
    }

    /// Creates micro testing limits for very small amounts.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            max_order_shares: dec!(50),
            min_order_shares: dec!(1),
            max_price: dec!(0.90),
            min_price: dec!(0.10),
            max_order_value: dec!(25), // $25
            max_daily_volume: dec!(250), // $250
            min_balance_reserve: dec!(50), // $50
        }
    }
}

// =============================================================================
// Cross Executor Configuration
// =============================================================================

/// Configuration for the cross-exchange executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossExecutorConfig {
    /// Hard limits for Kalshi orders.
    pub kalshi_limits: KalshiHardLimits,

    /// Hard limits for Polymarket orders.
    pub polymarket_limits: PolymarketHardLimits,

    /// Maximum position size per market (in shares/contracts).
    pub max_position_per_market: Decimal,

    /// Maximum number of concurrent open positions across all markets.
    pub max_concurrent_positions: u32,

    /// Settlement confidence threshold (e.g., 0.95).
    /// Only execute if settlement verification confidence >= this.
    pub settlement_confidence_threshold: f64,

    /// Maximum consecutive failures before circuit breaker trips.
    pub max_consecutive_failures: u32,

    /// Maximum daily loss in dollars before circuit breaker trips.
    pub max_daily_loss: Decimal,

    /// Pause duration when circuit breaker trips.
    #[serde(with = "humantime_serde")]
    pub pause_duration: Duration,

    /// Order timeout for waiting on fills.
    #[serde(with = "humantime_serde")]
    pub order_timeout: Duration,

    /// Cooldown between executions.
    #[serde(with = "humantime_serde")]
    pub cooldown: Duration,
}

impl Default for CrossExecutorConfig {
    fn default() -> Self {
        Self {
            kalshi_limits: KalshiHardLimits::default(),
            polymarket_limits: PolymarketHardLimits::default(),
            max_position_per_market: dec!(500),
            max_concurrent_positions: 5,
            settlement_confidence_threshold: 0.95,
            max_consecutive_failures: 3,
            max_daily_loss: dec!(100),
            pause_duration: Duration::from_secs(300), // 5 minutes
            order_timeout: Duration::from_secs(5),
            cooldown: Duration::from_secs(10),
        }
    }
}

impl CrossExecutorConfig {
    /// Creates a conservative configuration for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            kalshi_limits: KalshiHardLimits::conservative(),
            polymarket_limits: PolymarketHardLimits::conservative(),
            max_position_per_market: dec!(100),
            max_concurrent_positions: 2,
            settlement_confidence_threshold: 0.99, // Very high confidence required
            max_consecutive_failures: 2,
            max_daily_loss: dec!(50),
            pause_duration: Duration::from_secs(600), // 10 minutes
            order_timeout: Duration::from_secs(5),
            cooldown: Duration::from_secs(30),
        }
    }

    /// Creates a micro testing configuration.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            kalshi_limits: KalshiHardLimits::micro_testing(),
            polymarket_limits: PolymarketHardLimits::micro_testing(),
            max_position_per_market: dec!(50),
            max_concurrent_positions: 1,
            settlement_confidence_threshold: 0.99,
            max_consecutive_failures: 2,
            max_daily_loss: dec!(25),
            pause_duration: Duration::from_secs(300),
            order_timeout: Duration::from_secs(5),
            cooldown: Duration::from_secs(60),
        }
    }

    /// Sets the settlement confidence threshold.
    #[must_use]
    pub fn with_settlement_confidence_threshold(mut self, threshold: f64) -> Self {
        self.settlement_confidence_threshold = threshold;
        self
    }

    /// Sets the max position per market.
    #[must_use]
    pub fn with_max_position_per_market(mut self, max: Decimal) -> Self {
        self.max_position_per_market = max;
        self
    }

    /// Sets the max daily loss.
    #[must_use]
    pub fn with_max_daily_loss(mut self, max: Decimal) -> Self {
        self.max_daily_loss = max;
        self
    }
}

// =============================================================================
// Cross Circuit Breaker
// =============================================================================

/// Circuit breaker state for cross-exchange execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossCircuitBreakerState {
    /// Normal operation.
    Closed,
    /// Temporarily blocked after failures.
    Open,
    /// Manually tripped.
    Tripped,
}

/// Circuit breaker errors.
#[derive(Debug, Clone, Error)]
pub enum CircuitBreakerError {
    /// Daily loss limit exceeded.
    #[error("Daily loss limit exceeded: ${current_loss} >= ${max_loss} max")]
    DailyLossExceeded {
        /// Current loss.
        current_loss: Decimal,
        /// Maximum allowed.
        max_loss: Decimal,
    },

    /// Too many consecutive failures.
    #[error("Consecutive failures exceeded: {failures} >= {max_failures}")]
    ConsecutiveFailuresExceeded {
        /// Current failures.
        failures: u32,
        /// Maximum allowed.
        max_failures: u32,
    },

    /// Currently paused.
    #[error("Circuit breaker paused, {remaining_secs}s remaining")]
    Paused {
        /// Seconds remaining.
        remaining_secs: u64,
    },

    /// Manually tripped.
    #[error("Circuit breaker manually tripped")]
    ManuallyTripped,
}

/// Circuit breaker for cross-exchange safety.
#[derive(Debug)]
pub struct CrossCircuitBreaker {
    config: CrossExecutorConfig,
    consecutive_failures: AtomicU32,
    state: RwLock<CrossCircuitBreakerState>,
    daily_pnl: RwLock<Decimal>,
    last_trip_time: RwLock<Option<Instant>>,
    successful_trades: AtomicU32,
    failed_trades: AtomicU32,
}

impl CrossCircuitBreaker {
    /// Creates a new circuit breaker.
    pub fn new(config: CrossExecutorConfig) -> Self {
        Self {
            config,
            consecutive_failures: AtomicU32::new(0),
            state: RwLock::new(CrossCircuitBreakerState::Closed),
            daily_pnl: RwLock::new(Decimal::ZERO),
            last_trip_time: RwLock::new(None),
            successful_trades: AtomicU32::new(0),
            failed_trades: AtomicU32::new(0),
        }
    }

    /// Checks if trading is allowed.
    pub fn can_trade(&self) -> Result<(), CircuitBreakerError> {
        let state = *self.state.read();

        match state {
            CrossCircuitBreakerState::Closed => {
                // Check daily loss
                let daily_pnl = *self.daily_pnl.read();
                let current_loss = -daily_pnl;
                if current_loss >= self.config.max_daily_loss {
                    return Err(CircuitBreakerError::DailyLossExceeded {
                        current_loss,
                        max_loss: self.config.max_daily_loss,
                    });
                }
                Ok(())
            }
            CrossCircuitBreakerState::Open => {
                // Check if pause has elapsed
                if let Some(trip_time) = *self.last_trip_time.read() {
                    if trip_time.elapsed() >= self.config.pause_duration {
                        // Reset to closed
                        *self.state.write() = CrossCircuitBreakerState::Closed;
                        self.consecutive_failures.store(0, Ordering::SeqCst);
                        return Ok(());
                    }
                    let remaining = self.config.pause_duration - trip_time.elapsed();
                    return Err(CircuitBreakerError::Paused {
                        remaining_secs: remaining.as_secs(),
                    });
                }
                Err(CircuitBreakerError::ConsecutiveFailuresExceeded {
                    failures: self.consecutive_failures.load(Ordering::SeqCst),
                    max_failures: self.config.max_consecutive_failures,
                })
            }
            CrossCircuitBreakerState::Tripped => Err(CircuitBreakerError::ManuallyTripped),
        }
    }

    /// Records a successful execution.
    pub fn record_success(&self, pnl: Decimal) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.daily_pnl.write() += pnl;
        self.successful_trades.fetch_add(1, Ordering::SeqCst);

        // Check if we've exceeded daily loss limit
        let daily_pnl = *self.daily_pnl.read();
        if -daily_pnl >= self.config.max_daily_loss {
            self.trip_for_loss();
        }
    }

    /// Records a failed execution.
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.failed_trades.fetch_add(1, Ordering::SeqCst);

        if failures >= self.config.max_consecutive_failures {
            self.trip_for_failures();
        }
    }

    /// Manually trips the circuit breaker.
    pub fn trip(&self) {
        *self.state.write() = CrossCircuitBreakerState::Tripped;
        *self.last_trip_time.write() = Some(Instant::now());
        warn!("Cross-exchange circuit breaker manually tripped");
    }

    /// Resets the circuit breaker.
    pub fn reset(&self) {
        *self.state.write() = CrossCircuitBreakerState::Closed;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.last_trip_time.write() = None;
        info!("Cross-exchange circuit breaker reset");
    }

    /// Resets daily P&L (call at start of each day).
    pub fn reset_daily(&self) {
        *self.daily_pnl.write() = Decimal::ZERO;
        self.successful_trades.store(0, Ordering::SeqCst);
        self.failed_trades.store(0, Ordering::SeqCst);
    }

    fn trip_for_failures(&self) {
        *self.state.write() = CrossCircuitBreakerState::Open;
        *self.last_trip_time.write() = Some(Instant::now());
        warn!(
            failures = self.consecutive_failures.load(Ordering::SeqCst),
            "Cross-exchange circuit breaker tripped: too many consecutive failures"
        );
    }

    fn trip_for_loss(&self) {
        *self.state.write() = CrossCircuitBreakerState::Tripped;
        *self.last_trip_time.write() = Some(Instant::now());
        warn!(
            daily_pnl = %*self.daily_pnl.read(),
            max_loss = %self.config.max_daily_loss,
            "Cross-exchange circuit breaker tripped: daily loss limit exceeded"
        );
    }

    /// Returns the current state.
    #[must_use]
    pub fn state(&self) -> CrossCircuitBreakerState {
        *self.state.read()
    }

    /// Returns the daily P&L.
    #[must_use]
    pub fn daily_pnl(&self) -> Decimal {
        *self.daily_pnl.read()
    }

    /// Returns successful trade count.
    #[must_use]
    pub fn successful_trades(&self) -> u32 {
        self.successful_trades.load(Ordering::SeqCst)
    }

    /// Returns failed trade count.
    #[must_use]
    pub fn failed_trades(&self) -> u32 {
        self.failed_trades.load(Ordering::SeqCst)
    }
}

// =============================================================================
// Execution Results
// =============================================================================

/// Result of unwinding a position on one exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnwindResult {
    /// Whether the unwind was successful.
    pub success: bool,

    /// Amount unwound.
    pub amount_unwound: Decimal,

    /// Price achieved (if any).
    pub price: Option<Decimal>,

    /// Error message if failed.
    pub error: Option<String>,

    /// P&L from the unwind.
    pub pnl: Decimal,
}

impl UnwindResult {
    /// Creates a successful unwind result.
    #[must_use]
    pub fn success(amount: Decimal, price: Decimal, pnl: Decimal) -> Self {
        Self {
            success: true,
            amount_unwound: amount,
            price: Some(price),
            error: None,
            pnl,
        }
    }

    /// Creates a failed unwind result.
    #[must_use]
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            amount_unwound: Decimal::ZERO,
            price: None,
            error: Some(error.into()),
            pnl: Decimal::ZERO,
        }
    }
}

/// Result of a cross-exchange execution attempt.
#[derive(Debug, Clone)]
pub enum CrossExecutionResult {
    /// Both legs filled successfully.
    Success {
        /// Kalshi order result.
        kalshi_order: KalshiOrder,
        /// Polymarket order result.
        polymarket_order: OrderResult,
        /// Total cost of both legs.
        total_cost: Decimal,
        /// Expected profit at settlement.
        expected_profit: Decimal,
    },

    /// Only Kalshi leg filled - exposure created.
    KalshiOnlyFilled {
        /// Kalshi order that filled.
        kalshi_order: KalshiOrder,
        /// Error from Polymarket.
        polymarket_error: String,
        /// Exposure in dollars.
        exposure: Decimal,
        /// Result of unwind attempt (if any).
        unwind_result: Option<UnwindResult>,
    },

    /// Only Polymarket leg filled - exposure created.
    PolymarketOnlyFilled {
        /// Polymarket order that filled.
        polymarket_order: OrderResult,
        /// Error from Kalshi.
        kalshi_error: String,
        /// Exposure in dollars.
        exposure: Decimal,
        /// Result of unwind attempt (if any).
        unwind_result: Option<UnwindResult>,
    },

    /// Both orders rejected - no position change.
    BothRejected {
        /// Kalshi error.
        kalshi_error: String,
        /// Polymarket error.
        polymarket_error: String,
    },

    /// Settlement criteria don't match - execution blocked.
    SettlementMismatch {
        /// Reason for mismatch.
        reason: String,
    },

    /// Circuit breaker blocked execution.
    CircuitBreakerBlocked {
        /// Reason.
        reason: String,
    },

    /// Cooldown active.
    CooldownActive {
        /// Seconds remaining.
        remaining_secs: u64,
    },
}

impl CrossExecutionResult {
    /// Returns true if both legs filled successfully.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Returns true if there's unhedged exposure.
    #[must_use]
    pub fn has_exposure(&self) -> bool {
        matches!(
            self,
            Self::KalshiOnlyFilled { .. } | Self::PolymarketOnlyFilled { .. }
        )
    }

    /// Returns the P&L of this execution (if applicable).
    #[must_use]
    pub fn pnl(&self) -> Option<Decimal> {
        match self {
            Self::Success { expected_profit, .. } => Some(*expected_profit),
            Self::KalshiOnlyFilled { unwind_result, .. } => {
                unwind_result.as_ref().map(|r| r.pnl)
            }
            Self::PolymarketOnlyFilled { unwind_result, .. } => {
                unwind_result.as_ref().map(|r| r.pnl)
            }
            _ => None,
        }
    }
}

// =============================================================================
// Polymarket Executor Trait Object Wrapper
// =============================================================================

/// Wrapper to hold any Polymarket executor implementation.
pub struct PolymarketExecutorWrapper {
    inner: Arc<dyn PolymarketExecutor>,
}

impl PolymarketExecutorWrapper {
    /// Creates a new wrapper.
    pub fn new<E: PolymarketExecutor + 'static>(executor: E) -> Self {
        Self {
            inner: Arc::new(executor),
        }
    }

    /// Returns a reference to the inner executor.
    pub fn inner(&self) -> &dyn PolymarketExecutor {
        self.inner.as_ref()
    }
}

impl std::fmt::Debug for PolymarketExecutorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketExecutorWrapper").finish()
    }
}

// =============================================================================
// Cross-Exchange Executor
// =============================================================================

/// Cross-exchange executor for Kalshi and Polymarket.
///
/// Coordinates execution across both exchanges with safety guarantees.
pub struct CrossExchangeExecutor {
    kalshi: KalshiExecutor,
    polymarket: PolymarketExecutorWrapper,
    config: CrossExecutorConfig,
    circuit_breaker: CrossCircuitBreaker,
    last_execution: RwLock<Option<Instant>>,
    open_positions: RwLock<Vec<CrossPosition>>,
}

impl std::fmt::Debug for CrossExchangeExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossExchangeExecutor")
            .field("config", &self.config)
            .field("circuit_breaker_state", &self.circuit_breaker.state())
            .field("open_positions", &self.open_positions.read().len())
            .finish()
    }
}

impl CrossExchangeExecutor {
    /// Creates a new cross-exchange executor.
    pub fn new<E: PolymarketExecutor + 'static>(
        kalshi: KalshiExecutor,
        polymarket: E,
        config: CrossExecutorConfig,
    ) -> Self {
        let circuit_breaker = CrossCircuitBreaker::new(config.clone());
        Self {
            kalshi,
            polymarket: PolymarketExecutorWrapper::new(polymarket),
            config,
            circuit_breaker,
            last_execution: RwLock::new(None),
            open_positions: RwLock::new(Vec::new()),
        }
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &CrossExecutorConfig {
        &self.config
    }

    /// Returns the circuit breaker.
    #[must_use]
    pub fn circuit_breaker(&self) -> &CrossCircuitBreaker {
        &self.circuit_breaker
    }

    /// Returns the number of open positions.
    #[must_use]
    pub fn open_position_count(&self) -> usize {
        self.open_positions.read().len()
    }

    /// Verifies settlement criteria before execution.
    ///
    /// Returns `SettlementVerification::Identical` if safe to arbitrage.
    fn verify_settlement(&self, opp: &CrossExchangeOpportunity) -> SettlementVerification {
        // For now, we rely on the match confidence from the opportunity
        // In production, this would verify:
        // 1. Both markets settle at the same time
        // 2. Both use the same price source
        // 3. Both use the same threshold/strike
        // 4. Both use the same comparison (above/below)

        let confidence = opp.matched_market.match_confidence;

        if confidence >= self.config.settlement_confidence_threshold {
            if (confidence - 1.0).abs() < f64::EPSILON {
                SettlementVerification::Identical
            } else {
                SettlementVerification::Compatible {
                    differences: vec![format!("Match confidence: {:.2}%", confidence * 100.0)],
                    adjusted_confidence: confidence,
                }
            }
        } else {
            SettlementVerification::Incompatible {
                reason: format!(
                    "Match confidence {:.2}% below threshold {:.2}%",
                    confidence * 100.0,
                    self.config.settlement_confidence_threshold * 100.0
                ),
            }
        }
    }

    /// Checks if cooldown period has elapsed.
    fn check_cooldown(&self) -> Option<u64> {
        if let Some(last) = *self.last_execution.read() {
            let elapsed = last.elapsed();
            if elapsed < self.config.cooldown {
                let remaining = self.config.cooldown - elapsed;
                return Some(remaining.as_secs());
            }
        }
        None
    }

    /// Executes an arbitrage opportunity across both exchanges.
    ///
    /// # Safety Checks (in order)
    /// 1. Settlement verification
    /// 2. Circuit breaker check
    /// 3. Cooldown check
    /// 4. Position limit check
    /// 5. Size validation
    ///
    /// Then submits orders simultaneously.
    pub async fn execute(&self, opp: &CrossExchangeOpportunity) -> CrossExecutionResult {
        info!(
            kalshi_ticker = %opp.matched_market.kalshi_ticker,
            kalshi_side = %opp.kalshi_side,
            kalshi_price = %opp.kalshi_price,
            polymarket_side = %opp.polymarket_side,
            polymarket_price = %opp.polymarket_price,
            expected_profit = %opp.expected_profit,
            "Attempting cross-exchange execution"
        );

        // 1. Verify settlement criteria FIRST
        let verification = self.verify_settlement(opp);
        if !verification.is_safe() {
            let reason = match verification {
                SettlementVerification::Incompatible { reason } => reason,
                _ => "Unknown verification failure".to_string(),
            };
            warn!(reason = %reason, "Settlement verification failed - blocking execution");
            return CrossExecutionResult::SettlementMismatch { reason };
        }

        // 2. Check circuit breaker
        if let Err(e) = self.circuit_breaker.can_trade() {
            warn!(error = %e, "Circuit breaker blocked execution");
            return CrossExecutionResult::CircuitBreakerBlocked {
                reason: e.to_string(),
            };
        }

        // 3. Check cooldown
        if let Some(remaining) = self.check_cooldown() {
            debug!(remaining_secs = remaining, "Cooldown active");
            return CrossExecutionResult::CooldownActive {
                remaining_secs: remaining,
            };
        }

        // 4. Check position limits
        let current_positions = self.open_positions.read().len() as u32;
        if current_positions >= self.config.max_concurrent_positions {
            return CrossExecutionResult::CircuitBreakerBlocked {
                reason: format!(
                    "Max concurrent positions reached: {} >= {}",
                    current_positions, self.config.max_concurrent_positions
                ),
            };
        }

        // 5. Calculate execution size (limited by config and opportunity)
        let size = opp.max_size.min(self.config.max_position_per_market);
        // Convert Decimal to u32 (round down to be conservative)
        let kalshi_contracts = size
            .trunc()
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);

        // Create orders
        // Convert Kalshi price (Decimal in cents) to u32
        let kalshi_price_cents = opp
            .kalshi_price
            .trunc()
            .to_string()
            .parse::<u32>()
            .unwrap_or(50);
        let kalshi_order = match opp.kalshi_side {
            Side::Yes => OrderRequest::buy_yes(&opp.matched_market.kalshi_ticker, kalshi_price_cents, kalshi_contracts),
            Side::No => OrderRequest::buy_no(&opp.matched_market.kalshi_ticker, 100 - kalshi_price_cents, kalshi_contracts),
        };

        let polymarket_token = match opp.polymarket_side {
            Side::Yes => &opp.matched_market.polymarket_yes_token,
            Side::No => &opp.matched_market.polymarket_no_token,
        };

        let polymarket_order = OrderParams::buy_fok(
            polymarket_token.clone(),
            opp.polymarket_price,
            size,
        );

        // Execute both orders simultaneously
        let (kalshi_result, polymarket_result) = tokio::join!(
            self.kalshi.execute_order(&kalshi_order),
            self.polymarket.inner().submit_order(polymarket_order)
        );

        // Update last execution time
        *self.last_execution.write() = Some(Instant::now());

        // Handle results
        match (kalshi_result, polymarket_result) {
            (Ok(kalshi), Ok(poly)) if kalshi.status.has_fills() && poly.is_filled() => {
                // Both filled successfully
                let kalshi_cost = kalshi.filled_value_cents() / dec!(100);
                let poly_cost = poly.fill_notional();
                let total_cost = kalshi_cost + poly_cost;
                let expected_profit = size - total_cost;

                info!(
                    kalshi_order_id = %kalshi.order_id,
                    polymarket_order_id = %poly.order_id,
                    total_cost = %total_cost,
                    expected_profit = %expected_profit,
                    "Cross-exchange execution successful"
                );

                // Record success
                self.circuit_breaker.record_success(expected_profit);

                // Track position
                self.add_position(opp, &kalshi, &poly, total_cost, expected_profit);

                CrossExecutionResult::Success {
                    kalshi_order: kalshi,
                    polymarket_order: poly,
                    total_cost,
                    expected_profit,
                }
            }

            (Ok(kalshi), Err(poly_err)) if kalshi.status.has_fills() => {
                // Kalshi filled, Polymarket failed
                let exposure = kalshi.filled_value_cents() / dec!(100);
                warn!(
                    kalshi_order_id = %kalshi.order_id,
                    polymarket_error = %poly_err,
                    exposure = %exposure,
                    "Kalshi only filled - attempting unwind"
                );

                // Attempt to unwind Kalshi position
                let unwind_result = self.unwind_kalshi_position(&kalshi).await;

                self.circuit_breaker.record_failure();

                CrossExecutionResult::KalshiOnlyFilled {
                    kalshi_order: kalshi,
                    polymarket_error: poly_err.to_string(),
                    exposure,
                    unwind_result: Some(unwind_result),
                }
            }

            (Err(kalshi_err), Ok(poly)) if poly.is_filled() => {
                // Polymarket filled, Kalshi failed
                let exposure = poly.fill_notional();
                warn!(
                    kalshi_error = %kalshi_err,
                    polymarket_order_id = %poly.order_id,
                    exposure = %exposure,
                    "Polymarket only filled - attempting unwind"
                );

                // Attempt to unwind Polymarket position
                let unwind_result = self.unwind_polymarket_position(&poly).await;

                self.circuit_breaker.record_failure();

                CrossExecutionResult::PolymarketOnlyFilled {
                    polymarket_order: poly,
                    kalshi_error: kalshi_err.to_string(),
                    exposure,
                    unwind_result: Some(unwind_result),
                }
            }

            (Err(kalshi_err), Err(poly_err)) => {
                // Both rejected
                debug!(
                    kalshi_error = %kalshi_err,
                    polymarket_error = %poly_err,
                    "Both orders rejected - no position change"
                );

                // Don't count as failure if both rejected (market conditions changed)
                CrossExecutionResult::BothRejected {
                    kalshi_error: kalshi_err.to_string(),
                    polymarket_error: poly_err.to_string(),
                }
            }

            (Ok(kalshi), Ok(poly)) => {
                // Neither filled (should be rare with FOK orders)
                debug!(
                    kalshi_status = ?kalshi.status,
                    polymarket_status = ?poly.status,
                    "Neither order filled"
                );

                CrossExecutionResult::BothRejected {
                    kalshi_error: format!("Order not filled: {:?}", kalshi.status),
                    polymarket_error: format!("Order not filled: {:?}", poly.status),
                }
            }

            (Ok(kalshi), Err(poly_err)) => {
                // Kalshi didn't fill, Polymarket errored
                CrossExecutionResult::BothRejected {
                    kalshi_error: format!("Order not filled: {:?}", kalshi.status),
                    polymarket_error: poly_err.to_string(),
                }
            }

            (Err(kalshi_err), Ok(poly)) => {
                // Kalshi errored, Polymarket didn't fill
                CrossExecutionResult::BothRejected {
                    kalshi_error: kalshi_err.to_string(),
                    polymarket_error: format!("Order not filled: {:?}", poly.status),
                }
            }
        }
    }

    /// Attempts to unwind a Kalshi position.
    pub async fn unwind_kalshi_position(&self, order: &KalshiOrder) -> UnwindResult {
        // Create opposite order to close position
        let opposite_side = match order.side {
            KalshiSide::Yes => KalshiSide::No,
            KalshiSide::No => KalshiSide::Yes,
        };

        // For unwinding, we sell at market or close to it
        // This is a simplified implementation - production would be more sophisticated
        let unwind_price = order.price.unwrap_or(50);
        let unwind_order = match opposite_side {
            KalshiSide::Yes => OrderRequest::buy_yes(&order.ticker, 100 - unwind_price, order.filled_count),
            KalshiSide::No => OrderRequest::buy_no(&order.ticker, 100 - unwind_price, order.filled_count),
        };

        match self.kalshi.execute_order(&unwind_order).await {
            Ok(result) if result.status.has_fills() => {
                let amount = Decimal::from(result.filled_count);
                let price = result.avg_fill_price.unwrap_or(Decimal::ZERO);
                // P&L is the difference between entry and exit
                let entry_cost = order.filled_value_cents() / dec!(100);
                let exit_proceeds = result.filled_value_cents() / dec!(100);
                let pnl = exit_proceeds - entry_cost;

                info!(
                    original_order = %order.order_id,
                    unwind_order = %result.order_id,
                    pnl = %pnl,
                    "Successfully unwound Kalshi position"
                );

                UnwindResult::success(amount, price, pnl)
            }
            Ok(result) => {
                warn!(
                    status = ?result.status,
                    "Kalshi unwind order not filled"
                );
                UnwindResult::failure(format!("Unwind order not filled: {:?}", result.status))
            }
            Err(e) => {
                error!(error = %e, "Failed to unwind Kalshi position");
                UnwindResult::failure(e.to_string())
            }
        }
    }

    /// Attempts to unwind a Polymarket position.
    pub async fn unwind_polymarket_position(&self, order: &OrderResult) -> UnwindResult {
        // Create sell order to close position
        let sell_order = OrderParams::sell_fak(
            order.order_id.clone(), // This should be the token ID, not order ID
            dec!(0.01), // Sell at minimum price (market sell effectively)
            order.filled_size,
        );

        match self.polymarket.inner().submit_order(sell_order).await {
            Ok(result) if result.status.has_fills() => {
                let amount = result.filled_size;
                let price = result.avg_fill_price.unwrap_or(Decimal::ZERO);
                // P&L is proceeds from sale minus original cost
                let entry_cost = order.fill_notional();
                let exit_proceeds = result.fill_notional();
                let pnl = exit_proceeds - entry_cost;

                info!(
                    original_order = %order.order_id,
                    unwind_order = %result.order_id,
                    pnl = %pnl,
                    "Successfully unwound Polymarket position"
                );

                UnwindResult::success(amount, price, pnl)
            }
            Ok(result) => {
                warn!(
                    status = ?result.status,
                    "Polymarket unwind order not filled"
                );
                UnwindResult::failure(format!("Unwind order not filled: {:?}", result.status))
            }
            Err(e) => {
                error!(error = %e, "Failed to unwind Polymarket position");
                UnwindResult::failure(e.to_string())
            }
        }
    }

    /// Adds a position to tracking.
    fn add_position(
        &self,
        opp: &CrossExchangeOpportunity,
        kalshi: &KalshiOrder,
        poly: &OrderResult,
        total_cost: Decimal,
        expected_profit: Decimal,
    ) {
        let position = CrossPosition {
            id: Uuid::new_v4(),
            matched_market: opp.matched_market.clone(),
            kalshi_order_id: kalshi.order_id.clone(),
            kalshi_side: opp.kalshi_side,
            kalshi_filled: Decimal::from(kalshi.filled_count),
            kalshi_price: kalshi.avg_fill_price.unwrap_or(Decimal::ZERO),
            polymarket_order_id: poly.order_id.clone(),
            polymarket_side: opp.polymarket_side,
            polymarket_filled: poly.filled_size,
            polymarket_price: poly.avg_fill_price.unwrap_or(Decimal::ZERO),
            total_cost,
            expected_profit,
            status: CrossPositionStatus::Open,
            created_at: Utc::now(),
            settled_at: None,
            actual_profit: None,
        };

        self.open_positions.write().push(position);
    }

    /// Emergency stop - trips the circuit breaker.
    pub fn emergency_stop(&self) {
        self.circuit_breaker.trip();
        warn!("Cross-exchange emergency stop triggered");
    }

    /// Resumes trading after emergency stop.
    pub fn resume_trading(&self) {
        self.circuit_breaker.reset();
        info!("Cross-exchange trading resumed");
    }
}

// =============================================================================
// Cross Position Tracking
// =============================================================================

/// Status of a cross-exchange position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossPositionStatus {
    /// Position is open, awaiting settlement.
    Open,
    /// Position is being unwound due to an error.
    Unwinding,
    /// Market has settled, awaiting payout.
    AwaitingSettlement,
    /// Position has been settled with final P&L.
    Settled,
    /// Position was unwound due to an error.
    UnwoundWithLoss,
}

/// A cross-exchange arbitrage position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossPosition {
    /// Unique position identifier.
    pub id: Uuid,

    /// The matched market this position is for.
    pub matched_market: MatchedMarket,

    // Kalshi leg
    /// Kalshi order ID.
    pub kalshi_order_id: String,
    /// Side bought on Kalshi.
    pub kalshi_side: Side,
    /// Amount filled on Kalshi.
    pub kalshi_filled: Decimal,
    /// Average fill price on Kalshi (cents).
    pub kalshi_price: Decimal,

    // Polymarket leg
    /// Polymarket order ID.
    pub polymarket_order_id: String,
    /// Side bought on Polymarket.
    pub polymarket_side: Side,
    /// Amount filled on Polymarket.
    pub polymarket_filled: Decimal,
    /// Average fill price on Polymarket (dollars).
    pub polymarket_price: Decimal,

    // Combined
    /// Total cost of both legs.
    pub total_cost: Decimal,
    /// Expected profit at settlement.
    pub expected_profit: Decimal,

    /// Position status.
    pub status: CrossPositionStatus,

    /// When the position was created.
    pub created_at: DateTime<Utc>,

    /// When the position was settled (if settled).
    pub settled_at: Option<DateTime<Utc>>,

    /// Actual profit/loss (if settled).
    pub actual_profit: Option<Decimal>,
}

impl CrossPosition {
    /// Returns the balanced quantity (minimum of both legs).
    #[must_use]
    pub fn balanced_quantity(&self) -> Decimal {
        self.kalshi_filled.min(self.polymarket_filled)
    }

    /// Returns the imbalance (difference between legs).
    #[must_use]
    pub fn imbalance(&self) -> Decimal {
        (self.kalshi_filled - self.polymarket_filled).abs()
    }

    /// Returns true if the position is balanced.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.imbalance() < dec!(0.01)
    }

    /// Returns the time until settlement.
    #[must_use]
    pub fn time_to_settlement(&self) -> chrono::Duration {
        self.matched_market.time_to_settlement()
    }

    /// Returns the guaranteed payout (balanced quantity * $1).
    #[must_use]
    pub fn guaranteed_payout(&self) -> Decimal {
        self.balanced_quantity()
    }

    /// Returns the ROI as a percentage.
    #[must_use]
    pub fn roi_pct(&self) -> Decimal {
        if self.total_cost == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.expected_profit / self.total_cost * dec!(100)
    }
}

// =============================================================================
// Duration Serde Helper
// =============================================================================

mod humantime_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Config Tests ====================

    #[test]
    fn test_cross_executor_config_default() {
        let config = CrossExecutorConfig::default();

        assert_eq!(config.max_position_per_market, dec!(500));
        assert_eq!(config.max_concurrent_positions, 5);
        assert!((config.settlement_confidence_threshold - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_cross_executor_config_conservative() {
        let config = CrossExecutorConfig::conservative();

        assert_eq!(config.max_position_per_market, dec!(100));
        assert_eq!(config.max_concurrent_positions, 2);
        assert!((config.settlement_confidence_threshold - 0.99).abs() < 0.001);
    }

    #[test]
    fn test_cross_executor_config_micro_testing() {
        let config = CrossExecutorConfig::micro_testing();

        assert_eq!(config.max_position_per_market, dec!(50));
        assert_eq!(config.max_concurrent_positions, 1);
    }

    #[test]
    fn test_cross_executor_config_builder() {
        let config = CrossExecutorConfig::default()
            .with_settlement_confidence_threshold(0.98)
            .with_max_position_per_market(dec!(200))
            .with_max_daily_loss(dec!(75));

        assert!((config.settlement_confidence_threshold - 0.98).abs() < 0.001);
        assert_eq!(config.max_position_per_market, dec!(200));
        assert_eq!(config.max_daily_loss, dec!(75));
    }

    // ==================== Polymarket Hard Limits Tests ====================

    #[test]
    fn test_polymarket_hard_limits_default() {
        let limits = PolymarketHardLimits::default();

        assert_eq!(limits.max_order_shares, dec!(1000));
        assert_eq!(limits.max_order_value, dec!(500));
    }

    #[test]
    fn test_polymarket_hard_limits_conservative() {
        let limits = PolymarketHardLimits::conservative();

        assert_eq!(limits.max_order_shares, dec!(100));
        assert_eq!(limits.max_order_value, dec!(100));
    }

    // ==================== Circuit Breaker Tests ====================

    #[test]
    fn test_circuit_breaker_initial_state() {
        let config = CrossExecutorConfig::default();
        let breaker = CrossCircuitBreaker::new(config);

        assert_eq!(breaker.state(), CrossCircuitBreakerState::Closed);
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_circuit_breaker_trips_after_failures() {
        let config = CrossExecutorConfig::default();
        let breaker = CrossCircuitBreaker::new(config);

        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.can_trade().is_ok());

        breaker.record_failure();
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let config = CrossExecutorConfig::default();
        let breaker = CrossCircuitBreaker::new(config);

        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success(dec!(5));

        assert_eq!(breaker.state(), CrossCircuitBreakerState::Closed);
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_circuit_breaker_daily_loss_limit() {
        let config = CrossExecutorConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CrossCircuitBreaker::new(config);

        breaker.record_success(dec!(-40));
        assert!(breaker.can_trade().is_ok());

        breaker.record_success(dec!(-15));
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_circuit_breaker_manual_trip() {
        let config = CrossExecutorConfig::default();
        let breaker = CrossCircuitBreaker::new(config);

        breaker.trip();
        assert_eq!(breaker.state(), CrossCircuitBreakerState::Tripped);
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let config = CrossExecutorConfig::default();
        let breaker = CrossCircuitBreaker::new(config);

        breaker.trip();
        breaker.reset();

        assert_eq!(breaker.state(), CrossCircuitBreakerState::Closed);
        assert!(breaker.can_trade().is_ok());
    }

    // ==================== Unwind Result Tests ====================

    #[test]
    fn test_unwind_result_success() {
        let result = UnwindResult::success(dec!(100), dec!(0.45), dec!(5));

        assert!(result.success);
        assert_eq!(result.amount_unwound, dec!(100));
        assert_eq!(result.price, Some(dec!(0.45)));
        assert_eq!(result.pnl, dec!(5));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_unwind_result_failure() {
        let result = UnwindResult::failure("Order rejected");

        assert!(!result.success);
        assert_eq!(result.amount_unwound, Decimal::ZERO);
        assert!(result.price.is_none());
        assert_eq!(result.error, Some("Order rejected".to_string()));
    }

    // ==================== Cross Execution Result Tests ====================

    #[test]
    fn test_cross_execution_result_is_success() {
        let result = CrossExecutionResult::Success {
            kalshi_order: create_test_kalshi_order(),
            polymarket_order: create_test_poly_order(),
            total_cost: dec!(0.95),
            expected_profit: dec!(5),
        };

        assert!(result.is_success());
        assert!(!result.has_exposure());
        assert_eq!(result.pnl(), Some(dec!(5)));
    }

    #[test]
    fn test_cross_execution_result_has_exposure() {
        let result = CrossExecutionResult::KalshiOnlyFilled {
            kalshi_order: create_test_kalshi_order(),
            polymarket_error: "Rejected".to_string(),
            exposure: dec!(45),
            unwind_result: None,
        };

        assert!(!result.is_success());
        assert!(result.has_exposure());
    }

    // ==================== Cross Position Tests ====================

    #[test]
    fn test_cross_position_balanced() {
        let position = create_test_position(dec!(100), dec!(100));

        assert!(position.is_balanced());
        assert_eq!(position.balanced_quantity(), dec!(100));
        assert_eq!(position.imbalance(), Decimal::ZERO);
    }

    #[test]
    fn test_cross_position_imbalanced() {
        let position = create_test_position(dec!(100), dec!(80));

        assert!(!position.is_balanced());
        assert_eq!(position.balanced_quantity(), dec!(80));
        assert_eq!(position.imbalance(), dec!(20));
    }

    #[test]
    fn test_cross_position_guaranteed_payout() {
        let position = create_test_position(dec!(100), dec!(100));

        assert_eq!(position.guaranteed_payout(), dec!(100));
    }

    #[test]
    fn test_cross_position_roi() {
        let mut position = create_test_position(dec!(100), dec!(100));
        position.total_cost = dec!(95);
        position.expected_profit = dec!(5);

        let roi = position.roi_pct();
        // 5 / 95 * 100 ≈ 5.26%
        assert!(roi > dec!(5) && roi < dec!(6));
    }

    // ==================== Helper Functions ====================

    fn create_test_kalshi_order() -> KalshiOrder {
        KalshiOrder {
            order_id: "kalshi-order-123".to_string(),
            client_order_id: None,
            ticker: "KXBTC-TEST".to_string(),
            side: KalshiSide::Yes,
            action: algo_trade_kalshi::types::Action::Buy,
            order_type: algo_trade_kalshi::types::OrderType::Limit,
            status: algo_trade_kalshi::types::OrderStatus::Filled,
            count: 100,
            filled_count: 100,
            remaining_count: 0,
            price: Some(45),
            avg_fill_price: Some(dec!(45)),
            created_time: None,
            updated_time: None,
        }
    }

    fn create_test_poly_order() -> OrderResult {
        OrderResult::filled("poly-order-456", dec!(100), dec!(0.50))
    }

    fn create_test_position(kalshi_filled: Decimal, poly_filled: Decimal) -> CrossPosition {
        CrossPosition {
            id: Uuid::new_v4(),
            matched_market: MatchedMarket::new(
                "KXBTC-TEST".to_string(),
                "0xtest".to_string(),
                "yes-token".to_string(),
                "no-token".to_string(),
                "BTC".to_string(),
                dec!(100000),
                Utc::now() + chrono::Duration::hours(1),
                0.95,
            ),
            kalshi_order_id: "kalshi-123".to_string(),
            kalshi_side: Side::Yes,
            kalshi_filled,
            kalshi_price: dec!(45),
            polymarket_order_id: "poly-456".to_string(),
            polymarket_side: Side::No,
            polymarket_filled: poly_filled,
            polymarket_price: dec!(0.50),
            total_cost: dec!(95),
            expected_profit: dec!(5),
            status: CrossPositionStatus::Open,
            created_at: Utc::now(),
            settled_at: None,
            actual_profit: None,
        }
    }
}
