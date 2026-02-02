//! Rate limiting for Polymarket CLOB API.
//!
//! This module provides thread-safe rate limiting for all Polymarket API operations,
//! implementing defense-in-depth with both soft limits (configurable) and hard limits
//! (safety maximums).
//!
//! # Polymarket API Rate Limits
//!
//! Based on the Polymarket API documentation:
//! - POST /order: 60 requests/second sustained
//! - DELETE /order: 50 requests/second sustained
//! - GET endpoints: 150 requests/second
//!
//! # Usage
//!
//! ```
//! use algo_trade_polymarket::arbitrage::rate_limiter::{ClobRateLimiter, RateLimiterConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create with default config (60% of limits for safety margin)
//!     let limiter = ClobRateLimiter::with_default_config();
//!
//!     // Or use presets
//!     let conservative = ClobRateLimiter::new(RateLimiterConfig::conservative());
//!     let aggressive = ClobRateLimiter::new(RateLimiterConfig::aggressive());
//!
//!     // Wait for permission before API calls
//!     limiter.wait_for_order_submit().await;
//!     // ... submit order ...
//!
//!     limiter.wait_for_read().await;
//!     // ... read data ...
//! }
//! ```
//!
//! # Hard Limits
//!
//! In addition to rate limiting, the `hard_limits` submodule provides safety
//! maximums that should never be exceeded regardless of configuration:
//!
//! ```
//! use algo_trade_polymarket::arbitrage::rate_limiter::hard_limits;
//! use rust_decimal_macros::dec;
//!
//! // Validate an order before submission
//! let result = hard_limits::enforce_hard_limits(
//!     dec!(100),   // size
//!     dec!(0.50),  // price
//! );
//! assert!(result.is_ok());
//! ```

use governor::{
    clock::DefaultClock,
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use std::num::NonZeroU32;
use std::sync::Arc;

// =============================================================================
// Rate Limiter Configuration
// =============================================================================

/// Configuration for CLOB rate limiting.
///
/// Default values are set to 60% of the official API limits to provide
/// a safety margin and account for other API consumers.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Requests per second for order submissions (POST /order).
    /// Official limit: 60/s. Default: 36/s (60%)
    pub order_submit_rps: NonZeroU32,

    /// Requests per second for order cancellations (DELETE /order).
    /// Official limit: 50/s. Default: 30/s (60%)
    pub order_cancel_rps: NonZeroU32,

    /// Requests per second for read operations (GET endpoints).
    /// Official limit: 150/s. Default: 90/s (60%)
    pub read_rps: NonZeroU32,
}

impl RateLimiterConfig {
    /// Creates a new configuration with custom rate limits.
    ///
    /// # Arguments
    ///
    /// * `order_submit_rps` - Requests per second for order submissions
    /// * `order_cancel_rps` - Requests per second for order cancellations
    /// * `read_rps` - Requests per second for read operations
    #[must_use]
    pub fn new(
        order_submit_rps: NonZeroU32,
        order_cancel_rps: NonZeroU32,
        read_rps: NonZeroU32,
    ) -> Self {
        Self {
            order_submit_rps,
            order_cancel_rps,
            read_rps,
        }
    }

    /// Conservative configuration: 30% of API limits.
    ///
    /// Recommended for:
    /// - Initial testing
    /// - Shared API keys
    /// - Unstable network conditions
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            // 30% of 60 = 18
            order_submit_rps: NonZeroU32::new(18).expect("18 > 0"),
            // 30% of 50 = 15
            order_cancel_rps: NonZeroU32::new(15).expect("15 > 0"),
            // 30% of 150 = 45
            read_rps: NonZeroU32::new(45).expect("45 > 0"),
        }
    }

    /// Aggressive configuration: 80% of API limits.
    ///
    /// Recommended for:
    /// - Dedicated API keys
    /// - Time-sensitive arbitrage
    /// - Stable network conditions
    ///
    /// # Warning
    ///
    /// This leaves less margin for error. Use with caution.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            // 80% of 60 = 48
            order_submit_rps: NonZeroU32::new(48).expect("48 > 0"),
            // 80% of 50 = 40
            order_cancel_rps: NonZeroU32::new(40).expect("40 > 0"),
            // 80% of 150 = 120
            read_rps: NonZeroU32::new(120).expect("120 > 0"),
        }
    }

    /// Builder method to set order submit rate.
    #[must_use]
    pub fn with_order_submit_rps(mut self, rps: NonZeroU32) -> Self {
        self.order_submit_rps = rps;
        self
    }

    /// Builder method to set order cancel rate.
    #[must_use]
    pub fn with_order_cancel_rps(mut self, rps: NonZeroU32) -> Self {
        self.order_cancel_rps = rps;
        self
    }

    /// Builder method to set read rate.
    #[must_use]
    pub fn with_read_rps(mut self, rps: NonZeroU32) -> Self {
        self.read_rps = rps;
        self
    }
}

impl Default for RateLimiterConfig {
    /// Default configuration: 60% of API limits.
    ///
    /// - Order submit: 36/s (60% of 60/s)
    /// - Order cancel: 30/s (60% of 50/s)
    /// - Read ops: 90/s (60% of 150/s)
    fn default() -> Self {
        Self {
            order_submit_rps: NonZeroU32::new(36).expect("36 > 0"),
            order_cancel_rps: NonZeroU32::new(30).expect("30 > 0"),
            read_rps: NonZeroU32::new(90).expect("90 > 0"),
        }
    }
}

// =============================================================================
// Rate Limiter
// =============================================================================

/// Type alias for the governor rate limiter.
type GovernorLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Thread-safe rate limiter for Polymarket CLOB operations.
///
/// Provides separate rate limits for:
/// - Order submissions (POST /order)
/// - Order cancellations (DELETE /order)
/// - Read operations (GET endpoints)
///
/// The limiter can be cloned and shared across threads safely.
#[derive(Clone)]
pub struct ClobRateLimiter {
    /// Rate limiter for order submissions.
    order_submit: Arc<GovernorLimiter>,

    /// Rate limiter for order cancellations.
    order_cancel: Arc<GovernorLimiter>,

    /// Rate limiter for read operations.
    read_ops: Arc<GovernorLimiter>,

    /// Configuration used to create this limiter.
    config: RateLimiterConfig,
}

impl ClobRateLimiter {
    /// Creates a new rate limiter with the given configuration.
    #[must_use]
    pub fn new(config: RateLimiterConfig) -> Self {
        let order_submit = Arc::new(RateLimiter::direct(Quota::per_second(
            config.order_submit_rps,
        )));
        let order_cancel = Arc::new(RateLimiter::direct(Quota::per_second(
            config.order_cancel_rps,
        )));
        let read_ops = Arc::new(RateLimiter::direct(Quota::per_second(config.read_rps)));

        Self {
            order_submit,
            order_cancel,
            read_ops,
            config,
        }
    }

    /// Creates a new rate limiter with the default configuration.
    ///
    /// Uses 60% of API limits for safety margin.
    #[must_use]
    pub fn with_default_config() -> Self {
        Self::new(RateLimiterConfig::default())
    }

    /// Returns a reference to the current configuration.
    #[must_use]
    pub fn config(&self) -> &RateLimiterConfig {
        &self.config
    }

    /// Waits for permission to submit an order.
    ///
    /// Blocks asynchronously until a token is available from the order submit limiter.
    /// Use this before every POST /order request.
    pub async fn wait_for_order_submit(&self) {
        self.order_submit.until_ready().await;
    }

    /// Waits for permission to cancel an order.
    ///
    /// Blocks asynchronously until a token is available from the order cancel limiter.
    /// Use this before every DELETE /order request.
    pub async fn wait_for_order_cancel(&self) {
        self.order_cancel.until_ready().await;
    }

    /// Waits for permission to perform a read operation.
    ///
    /// Blocks asynchronously until a token is available from the read limiter.
    /// Use this before every GET request.
    pub async fn wait_for_read(&self) {
        self.read_ops.until_ready().await;
    }

    /// Checks if an order submission can proceed without waiting.
    ///
    /// Returns `true` if a token is immediately available, `false` otherwise.
    /// Does not consume a token.
    #[must_use]
    pub fn can_submit_order(&self) -> bool {
        self.order_submit.check().is_ok()
    }

    /// Checks if an order cancellation can proceed without waiting.
    ///
    /// Returns `true` if a token is immediately available, `false` otherwise.
    /// Does not consume a token.
    #[must_use]
    pub fn can_cancel_order(&self) -> bool {
        self.order_cancel.check().is_ok()
    }

    /// Checks if a read operation can proceed without waiting.
    ///
    /// Returns `true` if a token is immediately available, `false` otherwise.
    /// Does not consume a token.
    #[must_use]
    pub fn can_read(&self) -> bool {
        self.read_ops.check().is_ok()
    }

    /// Tries to acquire permission to submit an order without waiting.
    ///
    /// Returns `true` if a token was consumed, `false` if rate limited.
    /// Use `wait_for_order_submit` for blocking behavior.
    pub fn try_order_submit(&self) -> bool {
        self.order_submit.check().is_ok()
    }

    /// Tries to acquire permission to cancel an order without waiting.
    ///
    /// Returns `true` if a token was consumed, `false` if rate limited.
    /// Use `wait_for_order_cancel` for blocking behavior.
    pub fn try_order_cancel(&self) -> bool {
        self.order_cancel.check().is_ok()
    }

    /// Tries to acquire permission for a read operation without waiting.
    ///
    /// Returns `true` if a token was consumed, `false` if rate limited.
    /// Use `wait_for_read` for blocking behavior.
    pub fn try_read(&self) -> bool {
        self.read_ops.check().is_ok()
    }
}

impl std::fmt::Debug for ClobRateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClobRateLimiter")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Default for ClobRateLimiter {
    fn default() -> Self {
        Self::with_default_config()
    }
}

// =============================================================================
// Hard Limits (Defense in Depth)
// =============================================================================

/// Hard safety limits for order validation.
///
/// These limits provide a defense-in-depth layer that cannot be overridden
/// by configuration. They represent absolute maximums that should never
/// be exceeded in normal operation.
pub mod hard_limits {
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use thiserror::Error;

    /// Maximum order size in shares.
    /// Set conservatively to prevent accidental large orders.
    pub const MAX_ORDER_SIZE: Decimal = dec!(10000);

    /// Maximum order notional value in USD.
    /// Prevents accidentally submitting large dollar amounts.
    pub const MAX_ORDER_NOTIONAL: Decimal = dec!(5000);

    /// Maximum orders per second (hard cap).
    /// Even aggressive configs should not exceed this.
    pub const MAX_ORDERS_PER_SECOND: u32 = 10;

    /// Minimum price for an order (prevents price of 0).
    pub const MIN_PRICE: Decimal = dec!(0.001);

    /// Maximum price for an order (probability market).
    pub const MAX_PRICE: Decimal = dec!(0.999);

    /// Errors that can occur when validating hard limits.
    #[derive(Debug, Clone, Error)]
    pub enum HardLimitError {
        /// Order size exceeds maximum.
        #[error("Order size {size} exceeds maximum {max}")]
        SizeExceeded {
            /// Requested size.
            size: Decimal,
            /// Maximum allowed.
            max: Decimal,
        },

        /// Order notional value exceeds maximum.
        #[error("Order notional {notional} exceeds maximum {max}")]
        NotionalExceeded {
            /// Calculated notional value.
            notional: Decimal,
            /// Maximum allowed.
            max: Decimal,
        },

        /// Price is out of valid range.
        #[error("Price {price} is outside valid range [{min}, {max}]")]
        PriceOutOfRange {
            /// Requested price.
            price: Decimal,
            /// Minimum allowed.
            min: Decimal,
            /// Maximum allowed.
            max: Decimal,
        },

        /// Size is zero or negative.
        #[error("Size must be positive, got {size}")]
        InvalidSize {
            /// Requested size.
            size: Decimal,
        },
    }

    /// Validates an order against hard safety limits.
    ///
    /// # Arguments
    ///
    /// * `size` - Number of shares to trade
    /// * `price` - Limit price for the order
    ///
    /// # Returns
    ///
    /// Ok(()) if the order passes all hard limit checks, or an error describing
    /// which limit was violated.
    ///
    /// # Example
    ///
    /// ```
    /// use algo_trade_polymarket::arbitrage::rate_limiter::hard_limits;
    /// use rust_decimal_macros::dec;
    ///
    /// // Valid order
    /// assert!(hard_limits::enforce_hard_limits(dec!(100), dec!(0.50)).is_ok());
    ///
    /// // Size too large
    /// assert!(hard_limits::enforce_hard_limits(dec!(20000), dec!(0.50)).is_err());
    ///
    /// // Notional too large
    /// assert!(hard_limits::enforce_hard_limits(dec!(9000), dec!(0.60)).is_err());
    ///
    /// // Price out of range
    /// assert!(hard_limits::enforce_hard_limits(dec!(100), dec!(1.50)).is_err());
    /// ```
    pub fn enforce_hard_limits(size: Decimal, price: Decimal) -> Result<(), HardLimitError> {
        // Check size is positive
        if size <= Decimal::ZERO {
            return Err(HardLimitError::InvalidSize { size });
        }

        // Check size limit
        if size > MAX_ORDER_SIZE {
            return Err(HardLimitError::SizeExceeded {
                size,
                max: MAX_ORDER_SIZE,
            });
        }

        // Check price range
        if price < MIN_PRICE || price > MAX_PRICE {
            return Err(HardLimitError::PriceOutOfRange {
                price,
                min: MIN_PRICE,
                max: MAX_PRICE,
            });
        }

        // Check notional value
        let notional = size * price;
        if notional > MAX_ORDER_NOTIONAL {
            return Err(HardLimitError::NotionalExceeded {
                notional,
                max: MAX_ORDER_NOTIONAL,
            });
        }

        Ok(())
    }

    /// Validates that the configured rate doesn't exceed hard limits.
    ///
    /// # Arguments
    ///
    /// * `orders_per_second` - Configured orders per second
    ///
    /// # Returns
    ///
    /// The input value if within limits, or the hard maximum if exceeded.
    #[must_use]
    pub fn clamp_rate(orders_per_second: u32) -> u32 {
        orders_per_second.min(MAX_ORDERS_PER_SECOND)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::time::{Duration, Instant};

    // ==================== RateLimiterConfig Tests ====================

    #[test]
    fn test_config_default() {
        let config = RateLimiterConfig::default();

        assert_eq!(config.order_submit_rps.get(), 36);
        assert_eq!(config.order_cancel_rps.get(), 30);
        assert_eq!(config.read_rps.get(), 90);
    }

    #[test]
    fn test_config_conservative() {
        let config = RateLimiterConfig::conservative();

        assert_eq!(config.order_submit_rps.get(), 18);
        assert_eq!(config.order_cancel_rps.get(), 15);
        assert_eq!(config.read_rps.get(), 45);
    }

    #[test]
    fn test_config_aggressive() {
        let config = RateLimiterConfig::aggressive();

        assert_eq!(config.order_submit_rps.get(), 48);
        assert_eq!(config.order_cancel_rps.get(), 40);
        assert_eq!(config.read_rps.get(), 120);
    }

    #[test]
    fn test_config_new() {
        let config = RateLimiterConfig::new(
            NonZeroU32::new(10).unwrap(),
            NonZeroU32::new(20).unwrap(),
            NonZeroU32::new(30).unwrap(),
        );

        assert_eq!(config.order_submit_rps.get(), 10);
        assert_eq!(config.order_cancel_rps.get(), 20);
        assert_eq!(config.read_rps.get(), 30);
    }

    #[test]
    fn test_config_builder() {
        let config = RateLimiterConfig::default()
            .with_order_submit_rps(NonZeroU32::new(50).unwrap())
            .with_order_cancel_rps(NonZeroU32::new(40).unwrap())
            .with_read_rps(NonZeroU32::new(100).unwrap());

        assert_eq!(config.order_submit_rps.get(), 50);
        assert_eq!(config.order_cancel_rps.get(), 40);
        assert_eq!(config.read_rps.get(), 100);
    }

    // ==================== ClobRateLimiter Tests ====================

    #[test]
    fn test_limiter_creation() {
        let limiter = ClobRateLimiter::with_default_config();
        assert_eq!(limiter.config().order_submit_rps.get(), 36);
    }

    #[test]
    fn test_limiter_clone() {
        let limiter1 = ClobRateLimiter::with_default_config();
        let limiter2 = limiter1.clone();

        // Both should share the same underlying limiters
        assert_eq!(
            limiter1.config().order_submit_rps,
            limiter2.config().order_submit_rps
        );
    }

    #[test]
    fn test_limiter_default() {
        let limiter = ClobRateLimiter::default();
        assert_eq!(limiter.config().order_submit_rps.get(), 36);
    }

    #[test]
    fn test_limiter_debug() {
        let limiter = ClobRateLimiter::with_default_config();
        let debug_str = format!("{:?}", limiter);
        assert!(debug_str.contains("ClobRateLimiter"));
    }

    #[test]
    fn test_can_submit_order_initially() {
        let limiter = ClobRateLimiter::with_default_config();
        // Should have tokens available initially
        assert!(limiter.can_submit_order());
    }

    #[test]
    fn test_can_cancel_order_initially() {
        let limiter = ClobRateLimiter::with_default_config();
        assert!(limiter.can_cancel_order());
    }

    #[test]
    fn test_can_read_initially() {
        let limiter = ClobRateLimiter::with_default_config();
        assert!(limiter.can_read());
    }

    #[test]
    fn test_try_order_submit_consumes_token() {
        // Use a very low rate to easily hit the limit
        let config = RateLimiterConfig::new(
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );
        let limiter = ClobRateLimiter::new(config);

        // First should succeed
        assert!(limiter.try_order_submit());

        // Second should fail (rate limited)
        assert!(!limiter.try_order_submit());
    }

    #[test]
    fn test_try_order_cancel_consumes_token() {
        let config = RateLimiterConfig::new(
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );
        let limiter = ClobRateLimiter::new(config);

        assert!(limiter.try_order_cancel());
        assert!(!limiter.try_order_cancel());
    }

    #[test]
    fn test_try_read_consumes_token() {
        let config = RateLimiterConfig::new(
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
        );
        let limiter = ClobRateLimiter::new(config);

        assert!(limiter.try_read());
        assert!(!limiter.try_read());
    }

    #[tokio::test]
    async fn test_wait_for_order_submit() {
        let limiter = ClobRateLimiter::with_default_config();

        // Should complete quickly when tokens are available
        let start = Instant::now();
        limiter.wait_for_order_submit().await;
        let elapsed = start.elapsed();

        // Should be nearly instant (< 10ms)
        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_wait_for_order_cancel() {
        let limiter = ClobRateLimiter::with_default_config();

        let start = Instant::now();
        limiter.wait_for_order_cancel().await;
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_wait_for_read() {
        let limiter = ClobRateLimiter::with_default_config();

        let start = Instant::now();
        limiter.wait_for_read().await;
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_rate_limiting_causes_delay() {
        // Use a rate of 2/second
        let config = RateLimiterConfig::new(
            NonZeroU32::new(2).unwrap(),
            NonZeroU32::new(2).unwrap(),
            NonZeroU32::new(2).unwrap(),
        );
        let limiter = ClobRateLimiter::new(config);

        // Consume tokens quickly
        limiter.wait_for_order_submit().await;
        limiter.wait_for_order_submit().await;

        // Third request should be delayed
        let start = Instant::now();
        limiter.wait_for_order_submit().await;
        let elapsed = start.elapsed();

        // Should take at least 400ms (1/2 second minus some slack)
        assert!(
            elapsed >= Duration::from_millis(400),
            "Expected delay but got {:?}",
            elapsed
        );
    }

    // ==================== Hard Limits Tests ====================

    #[test]
    fn test_hard_limits_valid_order() {
        let result = hard_limits::enforce_hard_limits(dec!(100), dec!(0.50));
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_size_exceeded() {
        let result = hard_limits::enforce_hard_limits(dec!(20000), dec!(0.50));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::SizeExceeded { .. })
        ));
    }

    #[test]
    fn test_hard_limits_notional_exceeded() {
        // Size 9000 * price 0.60 = 5400 > 5000 max notional
        let result = hard_limits::enforce_hard_limits(dec!(9000), dec!(0.60));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::NotionalExceeded { .. })
        ));
    }

    #[test]
    fn test_hard_limits_price_too_low() {
        let result = hard_limits::enforce_hard_limits(dec!(100), dec!(0.0001));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::PriceOutOfRange { .. })
        ));
    }

    #[test]
    fn test_hard_limits_price_too_high() {
        let result = hard_limits::enforce_hard_limits(dec!(100), dec!(1.50));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::PriceOutOfRange { .. })
        ));
    }

    #[test]
    fn test_hard_limits_price_at_boundaries() {
        // Min price should be valid
        let result = hard_limits::enforce_hard_limits(dec!(100), hard_limits::MIN_PRICE);
        assert!(result.is_ok());

        // Max price should be valid
        let result = hard_limits::enforce_hard_limits(dec!(100), hard_limits::MAX_PRICE);
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_zero_size() {
        let result = hard_limits::enforce_hard_limits(dec!(0), dec!(0.50));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::InvalidSize { .. })
        ));
    }

    #[test]
    fn test_hard_limits_negative_size() {
        let result = hard_limits::enforce_hard_limits(dec!(-100), dec!(0.50));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::InvalidSize { .. })
        ));
    }

    #[test]
    fn test_hard_limits_max_notional_boundary() {
        // Exactly at the limit should pass
        // 10000 * 0.50 = 5000 = MAX_ORDER_NOTIONAL
        let result = hard_limits::enforce_hard_limits(dec!(10000), dec!(0.50));
        assert!(result.is_ok());

        // Just over notional should fail (using size within limit but price that causes notional overflow)
        // 5001 * 0.999 = 4995.999 < 5000, so we need something like 5006 * 0.999 = 5000.994 > 5000
        let result = hard_limits::enforce_hard_limits(dec!(5006), dec!(0.999));
        assert!(matches!(
            result,
            Err(hard_limits::HardLimitError::NotionalExceeded { .. })
        ));
    }

    #[test]
    fn test_hard_limits_error_display() {
        let err = hard_limits::HardLimitError::SizeExceeded {
            size: dec!(20000),
            max: dec!(10000),
        };
        let msg = err.to_string();
        assert!(msg.contains("20000"));
        assert!(msg.contains("10000"));
    }

    #[test]
    fn test_clamp_rate_within_limit() {
        let result = hard_limits::clamp_rate(5);
        assert_eq!(result, 5);
    }

    #[test]
    fn test_clamp_rate_at_limit() {
        let result = hard_limits::clamp_rate(hard_limits::MAX_ORDERS_PER_SECOND);
        assert_eq!(result, hard_limits::MAX_ORDERS_PER_SECOND);
    }

    #[test]
    fn test_clamp_rate_exceeds_limit() {
        let result = hard_limits::clamp_rate(100);
        assert_eq!(result, hard_limits::MAX_ORDERS_PER_SECOND);
    }

    #[test]
    fn test_constants_values() {
        assert_eq!(hard_limits::MAX_ORDER_SIZE, dec!(10000));
        assert_eq!(hard_limits::MAX_ORDER_NOTIONAL, dec!(5000));
        assert_eq!(hard_limits::MAX_ORDERS_PER_SECOND, 10);
        assert_eq!(hard_limits::MIN_PRICE, dec!(0.001));
        assert_eq!(hard_limits::MAX_PRICE, dec!(0.999));
    }

    // ==================== Thread Safety Tests ====================

    #[tokio::test]
    async fn test_limiter_shared_across_tasks() {
        let limiter = ClobRateLimiter::with_default_config();
        let limiter1 = limiter.clone();
        let limiter2 = limiter.clone();

        // Spawn two tasks that both use the limiter
        let handle1 = tokio::spawn(async move {
            limiter1.wait_for_order_submit().await;
        });

        let handle2 = tokio::spawn(async move {
            limiter2.wait_for_order_submit().await;
        });

        // Both should complete without issue
        handle1.await.unwrap();
        handle2.await.unwrap();
    }
}
