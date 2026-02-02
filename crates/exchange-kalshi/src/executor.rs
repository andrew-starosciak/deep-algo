//! Order execution with hard limits for Kalshi.
//!
//! Provides a safe order execution layer with:
//! - Hard limits on order size, price, and daily volume
//! - Circuit breaker for consecutive failures or losses
//! - Daily volume tracking
//! - Balance reserve protection
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_kalshi::{KalshiExecutor, KalshiExecutorConfig, HardLimits};
//!
//! let executor = KalshiExecutor::demo()?;
//!
//! // Submit order (validates against hard limits first)
//! let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);
//! let result = executor.execute_order(&order).await?;
//! ```

use crate::client::{KalshiClient, KalshiClientConfig};
use crate::error::{KalshiError, Result};
use crate::types::{Balance, Order, OrderRequest};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

// =============================================================================
// Hard Limits
// =============================================================================

/// Hard limits for order validation.
///
/// These are safety limits to prevent catastrophic trading errors.
/// All orders are validated against these limits before submission.
/// Kalshi uses cents (1-99) for prices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardLimits {
    /// Maximum order size in contracts.
    pub max_order_contracts: u32,

    /// Minimum order size in contracts.
    pub min_order_contracts: u32,

    /// Maximum price in cents.
    pub max_price_cents: u32,

    /// Minimum price in cents.
    pub min_price_cents: u32,

    /// Maximum single order value in cents.
    pub max_order_value_cents: i64,

    /// Maximum daily volume in cents.
    pub max_daily_volume_cents: i64,

    /// Minimum balance reserve to keep in cents.
    pub min_balance_reserve_cents: i64,
}

impl Default for HardLimits {
    fn default() -> Self {
        Self {
            max_order_contracts: 1000,       // Max 1000 contracts per order
            min_order_contracts: 1,          // Min 1 contract
            max_price_cents: 95,             // Max 95 cents
            min_price_cents: 5,              // Min 5 cents
            max_order_value_cents: 50000,    // Max $500 per order
            max_daily_volume_cents: 500000,  // Max $5000 daily volume
            min_balance_reserve_cents: 5000, // Keep $50 minimum
        }
    }
}

impl HardLimits {
    /// Creates conservative hard limits for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_order_contracts: 100,
            min_order_contracts: 1,
            max_price_cents: 90,
            min_price_cents: 10,
            max_order_value_cents: 10000,     // $100
            max_daily_volume_cents: 50000,    // $500
            min_balance_reserve_cents: 10000, // $100
        }
    }

    /// Creates micro testing hard limits for very small amounts.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            max_order_contracts: 50,
            min_order_contracts: 1,
            max_price_cents: 90,
            min_price_cents: 10,
            max_order_value_cents: 2500,     // $25
            max_daily_volume_cents: 25000,   // $250
            min_balance_reserve_cents: 5000, // $50
        }
    }

    /// Creates aggressive limits for production.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            max_order_contracts: 5000,
            min_order_contracts: 1,
            max_price_cents: 99,
            min_price_cents: 1,
            max_order_value_cents: 200000,    // $2000
            max_daily_volume_cents: 2000000,  // $20000
            min_balance_reserve_cents: 50000, // $500
        }
    }

    /// Validates an order against these limits.
    ///
    /// # Errors
    /// Returns an error if validation fails.
    pub fn validate_order(&self, order: &OrderRequest) -> Result<()> {
        // Validate contract count
        if order.count < self.min_order_contracts {
            return Err(KalshiError::InvalidOrder(format!(
                "Order count {} below minimum {}",
                order.count, self.min_order_contracts
            )));
        }
        if order.count > self.max_order_contracts {
            return Err(KalshiError::InvalidOrder(format!(
                "Order count {} exceeds maximum {}",
                order.count, self.max_order_contracts
            )));
        }

        // Validate price
        let price = order.yes_price.or(order.no_price).unwrap_or(50);
        if price < self.min_price_cents {
            return Err(KalshiError::InvalidOrder(format!(
                "Price {} below minimum {}",
                price, self.min_price_cents
            )));
        }
        if price > self.max_price_cents {
            return Err(KalshiError::InvalidOrder(format!(
                "Price {} exceeds maximum {}",
                price, self.max_price_cents
            )));
        }

        // Validate order value
        let order_value = order.order_value_cents() as i64;
        if order_value > self.max_order_value_cents {
            return Err(KalshiError::InvalidOrder(format!(
                "Order value {} cents exceeds maximum {} cents",
                order_value, self.max_order_value_cents
            )));
        }

        Ok(())
    }

    /// Validates that an order doesn't exceed remaining daily volume.
    pub fn validate_daily_volume(
        &self,
        order: &OrderRequest,
        current_daily_volume: i64,
    ) -> Result<()> {
        let order_value = order.order_value_cents() as i64;
        let new_total = current_daily_volume + order_value;

        if new_total > self.max_daily_volume_cents {
            return Err(KalshiError::daily_limit_exceeded(
                "volume",
                format!("{} cents", current_daily_volume),
                format!("{} cents", self.max_daily_volume_cents),
            ));
        }

        Ok(())
    }

    /// Validates that balance after order would meet minimum reserve.
    pub fn validate_balance_reserve(
        &self,
        order: &OrderRequest,
        current_balance: i64,
    ) -> Result<()> {
        let order_value = order.order_value_cents() as i64;
        let remaining = current_balance - order_value;

        if remaining < self.min_balance_reserve_cents {
            return Err(KalshiError::insufficient_balance(
                order_value + self.min_balance_reserve_cents,
                current_balance,
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Circuit Breaker
// =============================================================================

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerState {
    /// Normal operation.
    Closed,
    /// Temporarily blocked after failures.
    Open,
    /// Manually tripped.
    Tripped,
}

/// Circuit breaker configuration.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Maximum consecutive failures before tripping.
    pub max_consecutive_failures: u32,

    /// Maximum daily loss in cents before tripping.
    pub max_daily_loss_cents: i64,

    /// Pause duration when circuit breaker trips.
    pub pause_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 5,
            max_daily_loss_cents: 25000, // $250
            pause_duration: Duration::from_secs(300),
        }
    }
}

impl CircuitBreakerConfig {
    /// Creates a micro testing configuration.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            max_consecutive_failures: 3,
            max_daily_loss_cents: 5000, // $50
            pause_duration: Duration::from_secs(60),
        }
    }
}

/// Circuit breaker for safety.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    consecutive_failures: AtomicU32,
    state: RwLock<CircuitBreakerState>,
    daily_pnl_cents: RwLock<i64>,
    last_trip_time: RwLock<Option<std::time::Instant>>,
}

impl CircuitBreaker {
    /// Creates a new circuit breaker.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            consecutive_failures: AtomicU32::new(0),
            state: RwLock::new(CircuitBreakerState::Closed),
            daily_pnl_cents: RwLock::new(0),
            last_trip_time: RwLock::new(None),
        }
    }

    /// Checks if trading is allowed.
    pub fn can_trade(&self) -> Result<()> {
        let state = *self.state.read();

        match state {
            CircuitBreakerState::Closed => Ok(()),
            CircuitBreakerState::Open => {
                // Check if pause duration has elapsed
                if let Some(trip_time) = *self.last_trip_time.read() {
                    if trip_time.elapsed() >= self.config.pause_duration {
                        // Auto-reset after pause
                        *self.state.write() = CircuitBreakerState::Closed;
                        self.consecutive_failures.store(0, Ordering::SeqCst);
                        return Ok(());
                    }
                }
                Err(KalshiError::circuit_breaker_open(
                    "too many consecutive failures",
                ))
            }
            CircuitBreakerState::Tripped => {
                Err(KalshiError::circuit_breaker_open("manually tripped"))
            }
        }
    }

    /// Records a successful operation.
    pub fn record_success(&self, pnl_cents: i64) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.daily_pnl_cents.write() += pnl_cents;

        // Check daily loss limit
        if *self.daily_pnl_cents.read() < -self.config.max_daily_loss_cents {
            self.trip_for_loss();
        }
    }

    /// Records a failed operation.
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

        if failures >= self.config.max_consecutive_failures {
            self.trip_for_failures();
        }
    }

    /// Manually trips the circuit breaker.
    pub fn trip(&self) {
        *self.state.write() = CircuitBreakerState::Tripped;
        *self.last_trip_time.write() = Some(std::time::Instant::now());
        tracing::warn!("Circuit breaker manually tripped");
    }

    /// Resets the circuit breaker.
    pub fn reset(&self) {
        *self.state.write() = CircuitBreakerState::Closed;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.last_trip_time.write() = None;
        tracing::info!("Circuit breaker reset");
    }

    /// Resets daily P&L tracking (call at start of each day).
    pub fn reset_daily(&self) {
        *self.daily_pnl_cents.write() = 0;
    }

    fn trip_for_failures(&self) {
        *self.state.write() = CircuitBreakerState::Open;
        *self.last_trip_time.write() = Some(std::time::Instant::now());
        tracing::warn!(
            failures = self.consecutive_failures.load(Ordering::SeqCst),
            "Circuit breaker tripped: too many failures"
        );
    }

    fn trip_for_loss(&self) {
        *self.state.write() = CircuitBreakerState::Tripped;
        *self.last_trip_time.write() = Some(std::time::Instant::now());
        tracing::warn!(
            daily_pnl = *self.daily_pnl_cents.read(),
            max_loss = self.config.max_daily_loss_cents,
            "Circuit breaker tripped: daily loss limit exceeded"
        );
    }

    /// Returns the current state.
    #[must_use]
    pub fn state(&self) -> CircuitBreakerState {
        *self.state.read()
    }

    /// Returns the daily P&L in cents.
    #[must_use]
    pub fn daily_pnl_cents(&self) -> i64 {
        *self.daily_pnl_cents.read()
    }
}

// =============================================================================
// Daily Volume Tracker
// =============================================================================

/// Tracks daily trading volume for limit enforcement.
#[derive(Debug)]
struct DailyVolumeTracker {
    volume_cents: i64,
    last_reset_day: u64,
}

impl DailyVolumeTracker {
    fn new() -> Self {
        Self {
            volume_cents: 0,
            last_reset_day: Self::current_day(),
        }
    }

    fn current_day() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / 86400)
            .unwrap_or(0)
    }

    fn maybe_reset(&mut self) {
        let current = Self::current_day();
        if current != self.last_reset_day {
            self.volume_cents = 0;
            self.last_reset_day = current;
        }
    }

    fn get(&mut self) -> i64 {
        self.maybe_reset();
        self.volume_cents
    }

    fn add(&mut self, amount: i64) {
        self.maybe_reset();
        self.volume_cents += amount;
    }
}

// =============================================================================
// Executor Configuration
// =============================================================================

/// Configuration for the Kalshi executor.
#[derive(Debug, Clone, Default)]
pub struct KalshiExecutorConfig {
    /// Client configuration.
    pub client_config: KalshiClientConfig,

    /// Hard limits for order validation.
    pub hard_limits: HardLimits,

    /// Circuit breaker configuration.
    pub circuit_breaker_config: CircuitBreakerConfig,
}

impl KalshiExecutorConfig {
    /// Creates a demo configuration.
    #[must_use]
    pub fn demo() -> Self {
        Self {
            client_config: KalshiClientConfig::demo(),
            hard_limits: HardLimits::conservative(),
            circuit_breaker_config: CircuitBreakerConfig::default(),
        }
    }

    /// Creates a micro testing configuration.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            client_config: KalshiClientConfig::demo(),
            hard_limits: HardLimits::micro_testing(),
            circuit_breaker_config: CircuitBreakerConfig::micro_testing(),
        }
    }

    /// Sets the client configuration.
    #[must_use]
    pub fn with_client_config(mut self, config: KalshiClientConfig) -> Self {
        self.client_config = config;
        self
    }

    /// Sets the hard limits.
    #[must_use]
    pub fn with_hard_limits(mut self, limits: HardLimits) -> Self {
        self.hard_limits = limits;
        self
    }

    /// Sets the circuit breaker configuration.
    #[must_use]
    pub fn with_circuit_breaker_config(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_config = config;
        self
    }
}

// =============================================================================
// Kalshi Executor
// =============================================================================

/// Safe order executor for Kalshi with hard limits.
///
/// Wraps `KalshiClient` with additional safety features:
/// - Hard limits on order size, price, and value
/// - Daily volume tracking
/// - Circuit breaker for failures and losses
/// - Balance reserve protection
pub struct KalshiExecutor {
    client: KalshiClient,
    config: KalshiExecutorConfig,
    circuit_breaker: CircuitBreaker,
    daily_volume: RwLock<DailyVolumeTracker>,
    cached_balance: RwLock<Option<i64>>,
}

impl std::fmt::Debug for KalshiExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KalshiExecutor")
            .field("base_url", &self.client.base_url())
            .field("hard_limits", &self.config.hard_limits)
            .field("daily_volume", &self.daily_volume.read().volume_cents)
            .finish_non_exhaustive()
    }
}

impl KalshiExecutor {
    /// Creates a new executor with the given configuration.
    ///
    /// # Errors
    /// Returns error if client initialization fails.
    pub fn new(config: KalshiExecutorConfig) -> Result<Self> {
        let client = KalshiClient::new(config.client_config.clone())?;
        let circuit_breaker = CircuitBreaker::new(config.circuit_breaker_config.clone());

        Ok(Self {
            client,
            config,
            circuit_breaker,
            daily_volume: RwLock::new(DailyVolumeTracker::new()),
            cached_balance: RwLock::new(None),
        })
    }

    /// Creates an executor for demo environment.
    ///
    /// # Errors
    /// Returns error if client initialization fails.
    pub fn demo() -> Result<Self> {
        Self::new(KalshiExecutorConfig::demo())
    }

    /// Creates an executor for micro testing.
    ///
    /// # Errors
    /// Returns error if client initialization fails.
    pub fn micro_testing() -> Result<Self> {
        Self::new(KalshiExecutorConfig::micro_testing())
    }

    /// Returns a reference to the underlying client.
    #[must_use]
    pub fn client(&self) -> &KalshiClient {
        &self.client
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &KalshiExecutorConfig {
        &self.config
    }

    /// Returns the circuit breaker.
    #[must_use]
    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    /// Returns the current daily volume in cents.
    #[must_use]
    pub fn daily_volume_cents(&self) -> i64 {
        self.daily_volume.write().get()
    }

    /// Validates an order against all safety checks.
    fn validate_order(&self, order: &OrderRequest) -> Result<()> {
        // Check circuit breaker first
        self.circuit_breaker.can_trade()?;

        // Validate against hard limits
        self.config.hard_limits.validate_order(order)?;

        // Validate daily volume
        let current_volume = self.daily_volume.write().get();
        self.config
            .hard_limits
            .validate_daily_volume(order, current_volume)?;

        // Validate balance reserve if we have cached balance
        if let Some(balance) = *self.cached_balance.read() {
            self.config
                .hard_limits
                .validate_balance_reserve(order, balance)?;
        }

        Ok(())
    }

    /// Executes an order with all safety checks.
    ///
    /// # Arguments
    /// * `order` - The order to execute
    ///
    /// # Errors
    /// Returns error if validation fails or order is rejected.
    pub async fn execute_order(&self, order: &OrderRequest) -> Result<Order> {
        // Validate order first
        self.validate_order(order)?;

        tracing::info!(
            ticker = %order.ticker,
            side = ?order.side,
            count = order.count,
            "Submitting order"
        );

        // Submit order
        match self.client.submit_order(order).await {
            Ok(result) => {
                // Update daily volume on any fills
                if result.status.has_fills() {
                    let fill_value = result.filled_value_cents();
                    self.daily_volume
                        .write()
                        .add(fill_value.try_into().unwrap_or(0));
                    self.circuit_breaker.record_success(0);
                }

                tracing::info!(
                    order_id = %result.order_id,
                    status = ?result.status,
                    filled = result.filled_count,
                    "Order submitted"
                );

                Ok(result)
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                Err(e)
            }
        }
    }

    /// Gets and caches the current balance.
    ///
    /// # Errors
    /// Returns error if balance fetch fails.
    pub async fn refresh_balance(&self) -> Result<Balance> {
        let balance = self.client.get_balance().await?;
        *self.cached_balance.write() = Some(balance.available_balance);
        Ok(balance)
    }

    /// Cancels an order.
    ///
    /// # Errors
    /// Returns error if cancellation fails.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        self.client.cancel_order(order_id).await
    }

    /// Gets order status.
    ///
    /// # Errors
    /// Returns error if order is not found.
    pub async fn get_order(&self, order_id: &str) -> Result<Order> {
        self.client.get_order(order_id).await
    }

    /// Manually trips the circuit breaker.
    pub fn emergency_stop(&self) {
        self.circuit_breaker.trip();
        tracing::warn!("Emergency stop triggered");
    }

    /// Resets the circuit breaker.
    pub fn resume_trading(&self) {
        self.circuit_breaker.reset();
        tracing::info!("Trading resumed");
    }

    /// Records P&L for circuit breaker tracking.
    pub fn record_pnl(&self, pnl_cents: i64) {
        self.circuit_breaker.record_success(pnl_cents);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== HardLimits Tests ====================

    #[test]
    fn test_hard_limits_default() {
        let limits = HardLimits::default();

        assert_eq!(limits.max_order_contracts, 1000);
        assert_eq!(limits.min_order_contracts, 1);
        assert_eq!(limits.max_price_cents, 95);
        assert_eq!(limits.min_price_cents, 5);
        assert_eq!(limits.max_order_value_cents, 50000);
        assert_eq!(limits.max_daily_volume_cents, 500000);
        assert_eq!(limits.min_balance_reserve_cents, 5000);
    }

    #[test]
    fn test_hard_limits_conservative() {
        let limits = HardLimits::conservative();

        assert_eq!(limits.max_order_contracts, 100);
        assert_eq!(limits.max_order_value_cents, 10000);
    }

    #[test]
    fn test_hard_limits_micro_testing() {
        let limits = HardLimits::micro_testing();

        assert_eq!(limits.max_order_contracts, 50);
        assert_eq!(limits.max_order_value_cents, 2500);
    }

    #[test]
    fn test_hard_limits_validate_order_valid() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);

        assert!(limits.validate_order(&order).is_ok());
    }

    #[test]
    fn test_hard_limits_validate_order_count_too_small() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 0);

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("below minimum"));
    }

    #[test]
    fn test_hard_limits_validate_order_count_too_large() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 5000);

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_order_price_too_low() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 2, 100);

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("below minimum"));
    }

    #[test]
    fn test_hard_limits_validate_order_price_too_high() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 98, 100);

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_order_value_too_high() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 50, 2000); // 2000 * 50 = 100000

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_daily_volume_ok() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 50, 100); // 5000 cents

        let result = limits.validate_daily_volume(&order, 100000); // 100000 + 5000 < 500000
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_validate_daily_volume_exceeded() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 50, 100); // 5000 cents

        let result = limits.validate_daily_volume(&order, 496000); // 496000 + 5000 > 500000
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("daily limit"));
    }

    #[test]
    fn test_hard_limits_validate_balance_reserve_ok() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 50, 100); // 5000 cents

        let result = limits.validate_balance_reserve(&order, 20000); // 20000 - 5000 > 5000
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_validate_balance_reserve_insufficient() {
        let limits = HardLimits::default();
        let order = OrderRequest::buy_yes("KXBTC-TEST", 50, 100); // 5000 cents

        let result = limits.validate_balance_reserve(&order, 8000); // 8000 - 5000 < 5000
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("insufficient"));
    }

    // ==================== CircuitBreaker Tests ====================

    #[test]
    fn test_circuit_breaker_initial_state() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert_eq!(breaker.state(), CircuitBreakerState::Closed);
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_circuit_breaker_trips_after_failures() {
        let config = CircuitBreakerConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.can_trade().is_ok());

        breaker.record_failure();
        assert!(breaker.can_trade().is_err());
        assert_eq!(breaker.state(), CircuitBreakerState::Open);
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let config = CircuitBreakerConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success(0);

        // Failures reset, so we need 3 more
        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_circuit_breaker_daily_loss_limit() {
        let config = CircuitBreakerConfig {
            max_daily_loss_cents: 5000,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        breaker.record_success(-4000);
        assert!(breaker.can_trade().is_ok());

        breaker.record_success(-2000); // Total: -6000 > -5000
        assert!(breaker.can_trade().is_err());
        assert_eq!(breaker.state(), CircuitBreakerState::Tripped);
    }

    #[test]
    fn test_circuit_breaker_manual_trip() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        breaker.trip();
        assert!(breaker.can_trade().is_err());
        assert_eq!(breaker.state(), CircuitBreakerState::Tripped);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        breaker.trip();
        assert!(breaker.can_trade().is_err());

        breaker.reset();
        assert!(breaker.can_trade().is_ok());
        assert_eq!(breaker.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_circuit_breaker_daily_pnl_tracking() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        breaker.record_success(1000);
        breaker.record_success(-500);
        assert_eq!(breaker.daily_pnl_cents(), 500);
    }

    #[test]
    fn test_circuit_breaker_daily_reset() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        breaker.record_success(1000);
        breaker.reset_daily();
        assert_eq!(breaker.daily_pnl_cents(), 0);
    }

    // ==================== DailyVolumeTracker Tests ====================

    #[test]
    fn test_daily_volume_tracker_new() {
        let tracker = DailyVolumeTracker::new();
        assert_eq!(tracker.volume_cents, 0);
    }

    #[test]
    fn test_daily_volume_tracker_add() {
        let mut tracker = DailyVolumeTracker::new();

        tracker.add(1000);
        assert_eq!(tracker.get(), 1000);

        tracker.add(500);
        assert_eq!(tracker.get(), 1500);
    }

    // ==================== Config Tests ====================

    #[test]
    fn test_executor_config_default() {
        let config = KalshiExecutorConfig::default();

        assert_eq!(config.hard_limits.max_order_contracts, 1000);
    }

    #[test]
    fn test_executor_config_demo() {
        let config = KalshiExecutorConfig::demo();

        assert_eq!(config.hard_limits.max_order_contracts, 100);
    }

    #[test]
    fn test_executor_config_micro_testing() {
        let config = KalshiExecutorConfig::micro_testing();

        assert_eq!(config.hard_limits.max_order_contracts, 50);
        assert_eq!(config.circuit_breaker_config.max_consecutive_failures, 3);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = KalshiExecutorConfig::default()
            .with_hard_limits(HardLimits::aggressive())
            .with_circuit_breaker_config(CircuitBreakerConfig::micro_testing());

        assert_eq!(config.hard_limits.max_order_contracts, 5000);
        assert_eq!(config.circuit_breaker_config.max_consecutive_failures, 3);
    }

    // ==================== Order Validation Integration Tests ====================

    #[test]
    fn test_order_value_calculation() {
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);
        assert_eq!(order.order_value_cents(), 4500);
    }

    #[test]
    fn test_order_validation_boundary_values() {
        let limits = HardLimits {
            max_order_contracts: 100,
            min_order_contracts: 10,
            max_price_cents: 90,
            min_price_cents: 10,
            max_order_value_cents: 5000,
            max_daily_volume_cents: 50000,
            min_balance_reserve_cents: 1000,
        };

        // At minimum count
        let order_min = OrderRequest::buy_yes("KXBTC-TEST", 50, 10);
        assert!(limits.validate_order(&order_min).is_ok());

        // At maximum count (but within value limit)
        let order_max = OrderRequest::buy_yes("KXBTC-TEST", 10, 100);
        assert!(limits.validate_order(&order_max).is_ok());

        // At minimum price
        let order_min_price = OrderRequest::buy_yes("KXBTC-TEST", 10, 50);
        assert!(limits.validate_order(&order_min_price).is_ok());

        // At maximum price
        let order_max_price = OrderRequest::buy_yes("KXBTC-TEST", 90, 50);
        assert!(limits.validate_order(&order_max_price).is_ok());
    }
}
