//! Circuit breaker for arbitrage trading safety.
//!
//! This module provides a circuit breaker mechanism to halt trading when:
//! - Daily losses exceed a threshold
//! - Consecutive failures reach a limit
//! - Balance drops below warning level
//!
//! The circuit breaker is essential for Phase 1 validation to prevent
//! runaway losses during testing.
//!
//! # Example
//!
//! ```
//! use algo_trade_polymarket::arbitrage::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
//! use rust_decimal_macros::dec;
//!
//! // Create with default config ($50 max loss, 3 consecutive failures)
//! let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
//!
//! // Check if trading is allowed
//! assert!(breaker.can_trade().is_ok());
//!
//! // Record a successful trade with a loss
//! breaker.record_success(dec!(-10));
//!
//! // Still allowed - haven't hit limit
//! assert!(breaker.can_trade().is_ok());
//! ```

use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the circuit breaker.
///
/// Defines thresholds that trigger trading halts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum daily loss before halting (USDC).
    /// Default: $50
    pub max_daily_loss: Decimal,

    /// Maximum consecutive failures before pausing.
    /// Default: 3
    pub max_consecutive_failures: u32,

    /// Duration to pause after hitting failure threshold.
    /// Default: 5 minutes
    #[serde(with = "humantime_serde")]
    pub pause_duration: Duration,

    /// Balance level that triggers a warning.
    /// Default: $50
    pub min_balance_warning: Decimal,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_loss: dec!(50),
            max_consecutive_failures: 3,
            pause_duration: Duration::from_secs(5 * 60), // 5 minutes
            min_balance_warning: dec!(50),
        }
    }
}

impl CircuitBreakerConfig {
    /// Creates a configuration for micro testing with tighter limits.
    ///
    /// - Max daily loss: $10
    /// - Max consecutive failures: 5
    /// - Pause duration: 2 minutes
    /// - Min balance warning: $10
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            max_daily_loss: dec!(10),
            max_consecutive_failures: 5,
            pause_duration: Duration::from_secs(2 * 60), // 2 minutes
            min_balance_warning: dec!(10),
        }
    }

    /// Builder method to set max daily loss.
    #[must_use]
    pub fn with_max_daily_loss(mut self, loss: Decimal) -> Self {
        self.max_daily_loss = loss;
        self
    }

    /// Builder method to set max consecutive failures.
    #[must_use]
    pub fn with_max_consecutive_failures(mut self, failures: u32) -> Self {
        self.max_consecutive_failures = failures;
        self
    }

    /// Builder method to set pause duration.
    #[must_use]
    pub fn with_pause_duration(mut self, duration: Duration) -> Self {
        self.pause_duration = duration;
        self
    }

    /// Builder method to set min balance warning.
    #[must_use]
    pub fn with_min_balance_warning(mut self, balance: Decimal) -> Self {
        self.min_balance_warning = balance;
        self
    }
}

// =============================================================================
// Circuit Breaker State
// =============================================================================

/// Internal state tracked by the circuit breaker.
#[derive(Debug)]
struct CircuitBreakerState {
    /// Cumulative daily P&L (can be negative).
    daily_pnl: Decimal,

    /// Count of consecutive execution failures.
    consecutive_failures: u32,

    /// When the current pause started (if paused).
    pause_started: Option<Instant>,

    /// Total number of successful trades today.
    successful_trades: u32,

    /// Total number of failed trades today.
    failed_trades: u32,

    /// Whether circuit breaker has been manually tripped.
    manually_tripped: bool,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            daily_pnl: Decimal::ZERO,
            consecutive_failures: 0,
            pause_started: None,
            successful_trades: 0,
            failed_trades: 0,
            manually_tripped: false,
        }
    }
}

// =============================================================================
// Circuit Breaker Errors
// =============================================================================

/// Errors returned when trading is not allowed.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum CircuitBreakerError {
    /// Daily loss limit exceeded.
    #[error("Daily loss limit exceeded: {current_loss} >= {max_loss}")]
    DailyLossExceeded {
        /// Current cumulative loss (positive value).
        current_loss: Decimal,
        /// Maximum allowed loss.
        max_loss: Decimal,
    },

    /// Too many consecutive failures.
    #[error("Max consecutive failures reached: {failures} >= {max_failures}")]
    ConsecutiveFailuresExceeded {
        /// Current failure count.
        failures: u32,
        /// Maximum allowed.
        max_failures: u32,
    },

    /// Currently in pause period.
    #[error("Circuit breaker paused, {remaining_secs} seconds remaining")]
    Paused {
        /// Seconds until pause expires.
        remaining_secs: u64,
    },

    /// Manually tripped by operator.
    #[error("Circuit breaker manually tripped")]
    ManuallyTripped,
}

// =============================================================================
// Circuit Breaker
// =============================================================================

/// Circuit breaker for halting trading under adverse conditions.
///
/// Thread-safe implementation using `parking_lot::RwLock` for state management.
///
/// # Safety Guarantees
///
/// - Trading halts when daily losses exceed threshold
/// - Trading pauses after consecutive failures
/// - Manual trip available for emergency stops
/// - All state changes are atomic
pub struct CircuitBreaker {
    /// Configuration (immutable after creation).
    config: CircuitBreakerConfig,

    /// Protected internal state.
    state: RwLock<CircuitBreakerState>,
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.read();
        f.debug_struct("CircuitBreaker")
            .field("config", &self.config)
            .field("daily_pnl", &state.daily_pnl)
            .field("consecutive_failures", &state.consecutive_failures)
            .field("is_paused", &state.pause_started.is_some())
            .finish()
    }
}

impl CircuitBreaker {
    /// Creates a new circuit breaker with the given configuration.
    #[must_use]
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: RwLock::new(CircuitBreakerState::new()),
        }
    }

    /// Creates a circuit breaker with default configuration.
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }

    /// Checks if trading is currently allowed.
    ///
    /// Returns `Ok(())` if trading can proceed, or an error explaining why not.
    ///
    /// # Errors
    ///
    /// - `CircuitBreakerError::DailyLossExceeded` - Daily loss threshold reached
    /// - `CircuitBreakerError::ConsecutiveFailuresExceeded` - Too many failures in a row
    /// - `CircuitBreakerError::Paused` - Currently in a pause period
    /// - `CircuitBreakerError::ManuallyTripped` - Operator has tripped the breaker
    pub fn can_trade(&self) -> Result<(), CircuitBreakerError> {
        let state = self.state.read();

        // Check manual trip first
        if state.manually_tripped {
            return Err(CircuitBreakerError::ManuallyTripped);
        }

        // Check daily loss
        // Note: daily_pnl is negative for losses, so we check if -daily_pnl >= max_daily_loss
        let current_loss = -state.daily_pnl;
        if current_loss >= self.config.max_daily_loss {
            return Err(CircuitBreakerError::DailyLossExceeded {
                current_loss,
                max_loss: self.config.max_daily_loss,
            });
        }

        // Check if we're in a pause period first
        if let Some(pause_start) = state.pause_started {
            let elapsed = pause_start.elapsed();
            if elapsed < self.config.pause_duration {
                let remaining = self.config.pause_duration - elapsed;
                return Err(CircuitBreakerError::Paused {
                    remaining_secs: remaining.as_secs(),
                });
            }
            // Pause has expired, continue to check failures
        }

        // Check consecutive failures
        // After pause expires, still blocked until a success resets the counter
        if state.consecutive_failures >= self.config.max_consecutive_failures {
            return Err(CircuitBreakerError::ConsecutiveFailuresExceeded {
                failures: state.consecutive_failures,
                max_failures: self.config.max_consecutive_failures,
            });
        }

        Ok(())
    }

    /// Records a successful trade with its P&L.
    ///
    /// A successful trade:
    /// - Resets the consecutive failure counter
    /// - Clears any pause state
    /// - Adds the P&L to daily total
    ///
    /// Note: P&L can be negative (loss on a successful execution).
    pub fn record_success(&self, pnl: Decimal) {
        let mut state = self.state.write();
        state.consecutive_failures = 0;
        state.pause_started = None;
        state.daily_pnl += pnl;
        state.successful_trades += 1;
    }

    /// Records an execution failure.
    ///
    /// A failure:
    /// - Increments the consecutive failure counter
    /// - If threshold reached, starts a pause period
    pub fn record_failure(&self) {
        let mut state = self.state.write();
        state.consecutive_failures += 1;
        state.failed_trades += 1;

        // Start pause if threshold reached
        if state.consecutive_failures >= self.config.max_consecutive_failures {
            state.pause_started = Some(Instant::now());
        }
    }

    /// Resets the circuit breaker to initial state.
    ///
    /// Clears:
    /// - Daily P&L
    /// - Consecutive failures
    /// - Pause state
    /// - Trade counters
    /// - Manual trip flag
    pub fn reset(&self) {
        let mut state = self.state.write();
        *state = CircuitBreakerState::new();
    }

    /// Manually trips the circuit breaker.
    ///
    /// Use for emergency stops. Must call `reset()` to resume trading.
    pub fn trip(&self) {
        let mut state = self.state.write();
        state.manually_tripped = true;
    }

    /// Returns the current daily P&L.
    #[must_use]
    pub fn daily_pnl(&self) -> Decimal {
        self.state.read().daily_pnl
    }

    /// Returns the current consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.state.read().consecutive_failures
    }

    /// Returns true if currently in a pause period.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        let state = self.state.read();
        if let Some(pause_start) = state.pause_started {
            pause_start.elapsed() < self.config.pause_duration
        } else {
            false
        }
    }

    /// Returns the number of successful trades today.
    #[must_use]
    pub fn successful_trades(&self) -> u32 {
        self.state.read().successful_trades
    }

    /// Returns the number of failed trades today.
    #[must_use]
    pub fn failed_trades(&self) -> u32 {
        self.state.read().failed_trades
    }

    /// Returns true if manually tripped.
    #[must_use]
    pub fn is_manually_tripped(&self) -> bool {
        self.state.read().manually_tripped
    }

    /// Checks if the given balance is below the warning threshold.
    #[must_use]
    pub fn is_balance_warning(&self, balance: Decimal) -> bool {
        balance < self.config.min_balance_warning
    }

    /// Returns remaining pause time, if any.
    #[must_use]
    pub fn remaining_pause(&self) -> Option<Duration> {
        let state = self.state.read();
        if let Some(pause_start) = state.pause_started {
            let elapsed = pause_start.elapsed();
            if elapsed < self.config.pause_duration {
                return Some(self.config.pause_duration - elapsed);
            }
        }
        None
    }
}

// =============================================================================
// Serde support for Duration
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // ==================== Configuration Tests ====================

    #[test]
    fn test_config_default_values() {
        let config = CircuitBreakerConfig::default();

        assert_eq!(config.max_daily_loss, dec!(50));
        assert_eq!(config.max_consecutive_failures, 3);
        assert_eq!(config.pause_duration, Duration::from_secs(5 * 60));
        assert_eq!(config.min_balance_warning, dec!(50));
    }

    #[test]
    fn test_config_micro_testing_values() {
        let config = CircuitBreakerConfig::micro_testing();

        assert_eq!(config.max_daily_loss, dec!(10));
        assert_eq!(config.max_consecutive_failures, 5);
        assert_eq!(config.pause_duration, Duration::from_secs(2 * 60));
        assert_eq!(config.min_balance_warning, dec!(10));
    }

    #[test]
    fn test_config_builder_methods() {
        let config = CircuitBreakerConfig::default()
            .with_max_daily_loss(dec!(100))
            .with_max_consecutive_failures(5)
            .with_pause_duration(Duration::from_secs(120))
            .with_min_balance_warning(dec!(75));

        assert_eq!(config.max_daily_loss, dec!(100));
        assert_eq!(config.max_consecutive_failures, 5);
        assert_eq!(config.pause_duration, Duration::from_secs(120));
        assert_eq!(config.min_balance_warning, dec!(75));
    }

    // ==================== Can Trade Tests ====================

    #[test]
    fn test_can_trade_initially_returns_ok() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_can_trade_after_max_failures_returns_error() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(3)
            .with_pause_duration(Duration::from_secs(300)); // 5 minutes
        let breaker = CircuitBreaker::new(config);

        // Record 3 failures (the threshold)
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_failure();

        let result = breaker.can_trade();
        assert!(result.is_err());

        // When max failures is reached, a pause is triggered.
        // During pause, we get Paused error.
        match result {
            Err(CircuitBreakerError::Paused { remaining_secs }) => {
                // Pause was triggered by consecutive failures
                assert!(remaining_secs > 0);
                assert!(remaining_secs <= 300);
            }
            _ => panic!("Expected Paused error when failure threshold triggers pause"),
        }
    }

    #[test]
    fn test_can_trade_after_max_loss_returns_error() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CircuitBreaker::new(config);

        // Record a loss that equals the threshold
        breaker.record_success(dec!(-50));

        let result = breaker.can_trade();
        assert!(result.is_err());

        match result {
            Err(CircuitBreakerError::DailyLossExceeded {
                current_loss,
                max_loss,
            }) => {
                assert_eq!(current_loss, dec!(50));
                assert_eq!(max_loss, dec!(50));
            }
            _ => panic!("Expected DailyLossExceeded error"),
        }
    }

    #[test]
    fn test_can_trade_during_pause_returns_error() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(2)
            .with_pause_duration(Duration::from_secs(60));
        let breaker = CircuitBreaker::new(config);

        // Trigger pause by reaching failure threshold
        breaker.record_failure();
        breaker.record_failure();

        let result = breaker.can_trade();
        assert!(result.is_err());

        // Should report as paused (with remaining time) or consecutive failures
        match result {
            Err(CircuitBreakerError::ConsecutiveFailuresExceeded { .. }) => {
                // Also valid - failures triggered the pause
            }
            Err(CircuitBreakerError::Paused { remaining_secs }) => {
                assert!(remaining_secs <= 60);
            }
            _ => panic!("Expected Paused or ConsecutiveFailuresExceeded error"),
        }
    }

    #[test]
    fn test_can_trade_after_pause_expires_still_blocked_until_success() {
        // Use a very short pause duration for testing
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(2)
            .with_pause_duration(Duration::from_millis(10));
        let breaker = CircuitBreaker::new(config);

        // Trigger pause
        breaker.record_failure();
        breaker.record_failure();

        // Wait for pause to expire
        thread::sleep(Duration::from_millis(20));

        // Still blocked because consecutive failures haven't reset
        let result = breaker.can_trade();
        assert!(result.is_err());

        // After pause expires, we get ConsecutiveFailuresExceeded
        match result {
            Err(CircuitBreakerError::ConsecutiveFailuresExceeded {
                failures,
                max_failures,
            }) => {
                assert_eq!(failures, 2);
                assert_eq!(max_failures, 2);
            }
            _ => panic!("Expected ConsecutiveFailuresExceeded after pause expires"),
        }

        // Record a success to reset
        breaker.record_success(Decimal::ZERO);

        // Now should be allowed
        assert!(breaker.can_trade().is_ok());
    }

    // ==================== Recording Tests ====================

    #[test]
    fn test_record_success_resets_failure_count() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        // Record some failures
        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.consecutive_failures(), 2);

        // Record success
        breaker.record_success(dec!(5));

        // Failures should be reset
        assert_eq!(breaker.consecutive_failures(), 0);
    }

    #[test]
    fn test_record_success_tracks_daily_pnl() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        breaker.record_success(dec!(10));
        assert_eq!(breaker.daily_pnl(), dec!(10));

        breaker.record_success(dec!(5));
        assert_eq!(breaker.daily_pnl(), dec!(15));

        breaker.record_success(dec!(-8));
        assert_eq!(breaker.daily_pnl(), dec!(7));
    }

    #[test]
    fn test_record_failure_increments_count() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert_eq!(breaker.consecutive_failures(), 0);

        breaker.record_failure();
        assert_eq!(breaker.consecutive_failures(), 1);

        breaker.record_failure();
        assert_eq!(breaker.consecutive_failures(), 2);

        breaker.record_failure();
        assert_eq!(breaker.consecutive_failures(), 3);
    }

    #[test]
    fn test_record_failure_triggers_pause_at_threshold() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(2)
            .with_pause_duration(Duration::from_secs(300));
        let breaker = CircuitBreaker::new(config);

        // One failure - no pause yet
        breaker.record_failure();
        assert!(!breaker.is_paused());

        // Second failure - triggers pause
        breaker.record_failure();
        assert!(breaker.is_paused());
    }

    // ==================== Reset Tests ====================

    #[test]
    fn test_reset_clears_state() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(2)
            .with_pause_duration(Duration::from_secs(300));
        let breaker = CircuitBreaker::new(config);

        // Build up some state
        breaker.record_success(dec!(-30));
        breaker.record_failure();
        breaker.record_failure();
        breaker.trip();

        assert_eq!(breaker.daily_pnl(), dec!(-30));
        assert_eq!(breaker.consecutive_failures(), 2);
        assert!(breaker.is_paused());
        assert!(breaker.is_manually_tripped());

        // Reset
        breaker.reset();

        // All state should be cleared
        assert_eq!(breaker.daily_pnl(), Decimal::ZERO);
        assert_eq!(breaker.consecutive_failures(), 0);
        assert!(!breaker.is_paused());
        assert!(!breaker.is_manually_tripped());
        assert_eq!(breaker.successful_trades(), 0);
        assert_eq!(breaker.failed_trades(), 0);
    }

    #[test]
    fn test_reset_allows_trading_again() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(20));
        let breaker = CircuitBreaker::new(config);

        // Exceed daily loss
        breaker.record_success(dec!(-25));
        assert!(breaker.can_trade().is_err());

        // Reset
        breaker.reset();

        // Should be allowed again
        assert!(breaker.can_trade().is_ok());
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_negative_pnl_accumulates() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(100));
        let breaker = CircuitBreaker::new(config);

        // Accumulate losses
        breaker.record_success(dec!(-20));
        breaker.record_success(dec!(-30));
        breaker.record_success(dec!(-40));

        assert_eq!(breaker.daily_pnl(), dec!(-90));

        // Still allowed (90 < 100)
        assert!(breaker.can_trade().is_ok());

        // One more loss pushes over
        breaker.record_success(dec!(-10));
        assert_eq!(breaker.daily_pnl(), dec!(-100));

        // Now blocked
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_pause_duration_respected() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(1)
            .with_pause_duration(Duration::from_millis(50));
        let breaker = CircuitBreaker::new(config);

        // Trigger pause
        breaker.record_failure();

        // Should be paused
        assert!(breaker.is_paused());

        // Wait less than pause duration
        thread::sleep(Duration::from_millis(20));
        assert!(breaker.is_paused());

        // Wait for pause to expire
        thread::sleep(Duration::from_millis(40));
        assert!(!breaker.is_paused());
    }

    #[test]
    fn test_manual_trip_blocks_trading() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert!(breaker.can_trade().is_ok());

        breaker.trip();

        let result = breaker.can_trade();
        assert!(result.is_err());
        assert!(matches!(result, Err(CircuitBreakerError::ManuallyTripped)));
    }

    #[test]
    fn test_balance_warning_threshold() {
        let config = CircuitBreakerConfig::default().with_min_balance_warning(dec!(50));
        let breaker = CircuitBreaker::new(config);

        assert!(!breaker.is_balance_warning(dec!(100)));
        assert!(!breaker.is_balance_warning(dec!(50)));
        assert!(breaker.is_balance_warning(dec!(49.99)));
        assert!(breaker.is_balance_warning(dec!(0)));
    }

    #[test]
    fn test_trade_counters() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert_eq!(breaker.successful_trades(), 0);
        assert_eq!(breaker.failed_trades(), 0);

        breaker.record_success(dec!(10));
        breaker.record_success(dec!(5));
        breaker.record_failure();

        assert_eq!(breaker.successful_trades(), 2);
        assert_eq!(breaker.failed_trades(), 1);
    }

    #[test]
    fn test_remaining_pause_duration() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(1)
            .with_pause_duration(Duration::from_millis(100));
        let breaker = CircuitBreaker::new(config);

        // No pause initially
        assert!(breaker.remaining_pause().is_none());

        // Trigger pause
        breaker.record_failure();

        // Should have remaining time
        let remaining = breaker.remaining_pause();
        assert!(remaining.is_some());
        assert!(remaining.unwrap() <= Duration::from_millis(100));

        // Wait for pause to expire
        thread::sleep(Duration::from_millis(110));

        // No remaining time
        assert!(breaker.remaining_pause().is_none());
    }

    #[test]
    fn test_mixed_positive_and_negative_pnl() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CircuitBreaker::new(config);

        // Mix of wins and losses
        breaker.record_success(dec!(100)); // +100
        breaker.record_success(dec!(-80)); // +20
        breaker.record_success(dec!(30)); // +50
        breaker.record_success(dec!(-90)); // -40

        assert_eq!(breaker.daily_pnl(), dec!(-40));

        // Still allowed
        assert!(breaker.can_trade().is_ok());

        // Push over limit
        breaker.record_success(dec!(-15)); // -55

        // Now blocked
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_success_clears_pause() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(2)
            .with_pause_duration(Duration::from_secs(300));
        let breaker = CircuitBreaker::new(config);

        // Trigger pause
        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.is_paused());

        // Record success
        breaker.record_success(Decimal::ZERO);

        // Pause should be cleared
        assert!(!breaker.is_paused());
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_exactly_at_loss_threshold() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CircuitBreaker::new(config);

        // Exactly at threshold
        breaker.record_success(dec!(-50));

        // Should be blocked (>= threshold)
        let result = breaker.can_trade();
        assert!(result.is_err());
    }

    #[test]
    fn test_just_below_loss_threshold() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CircuitBreaker::new(config);

        // Just below threshold
        breaker.record_success(dec!(-49.99));

        // Should still be allowed
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_exactly_at_failure_threshold() {
        let config = CircuitBreakerConfig::default().with_max_consecutive_failures(3);
        let breaker = CircuitBreaker::new(config);

        // Record exactly threshold failures
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_failure();

        // Should be blocked
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_one_below_failure_threshold() {
        let config = CircuitBreakerConfig::default().with_max_consecutive_failures(3);
        let breaker = CircuitBreaker::new(config);

        // One below threshold
        breaker.record_failure();
        breaker.record_failure();

        // Should still be allowed
        assert!(breaker.can_trade().is_ok());
    }

    // ==================== Thread Safety Tests ====================

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;

        let breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default()));
        let mut handles = vec![];

        // Spawn multiple threads recording success/failure
        for i in 0..10 {
            let b = Arc::clone(&breaker);
            let handle = thread::spawn(move || {
                if i % 2 == 0 {
                    b.record_success(dec!(1));
                } else {
                    b.record_failure();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have recorded all operations
        let total = breaker.successful_trades() + breaker.failed_trades();
        assert_eq!(total, 10);
    }

    // ==================== Error Display Tests ====================

    #[test]
    fn test_error_display() {
        let err1 = CircuitBreakerError::DailyLossExceeded {
            current_loss: dec!(60),
            max_loss: dec!(50),
        };
        assert!(err1.to_string().contains("60"));
        assert!(err1.to_string().contains("50"));

        let err2 = CircuitBreakerError::ConsecutiveFailuresExceeded {
            failures: 5,
            max_failures: 3,
        };
        assert!(err2.to_string().contains("5"));
        assert!(err2.to_string().contains("3"));

        let err3 = CircuitBreakerError::Paused {
            remaining_secs: 120,
        };
        assert!(err3.to_string().contains("120"));

        let err4 = CircuitBreakerError::ManuallyTripped;
        assert!(err4.to_string().contains("manually"));
    }

    // ==================== Debug Trait Tests ====================

    #[test]
    fn test_debug_output() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let debug_str = format!("{:?}", breaker);

        assert!(debug_str.contains("CircuitBreaker"));
        assert!(debug_str.contains("daily_pnl"));
        assert!(debug_str.contains("consecutive_failures"));
    }

    #[test]
    fn test_config_debug_output() {
        let config = CircuitBreakerConfig::default();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("max_daily_loss"));
        assert!(debug_str.contains("max_consecutive_failures"));
    }
}
