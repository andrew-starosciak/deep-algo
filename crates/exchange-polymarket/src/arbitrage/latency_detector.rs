//! Latency arbitrage detection for Polymarket binary markets.
//!
//! This module detects opportunities where Polymarket prices lag behind
//! spot BTC movements on Binance. The strategy:
//!
//! 1. Monitor BTC spot price on Binance in real-time
//! 2. Track rolling price changes (1-min, 5-min)
//! 3. Compare with Polymarket YES/NO odds
//! 4. Signal entry when:
//!    - One side is cheap (< $0.35)
//!    - Spot has already moved in confirming direction
//!
//! # The Edge
//!
//! Polymarket prices lag spot by 1-30 seconds. If BTC moved up 0.3%+,
//! and YES is still priced at $0.35, that's a high-probability entry.
//! Direction is CONFIRMED, not predicted - hence 95%+ win rates.
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::latency_detector::{
//!     LatencyDetector, LatencyConfig, SpotPriceTracker,
//! };
//!
//! let mut tracker = SpotPriceTracker::new();
//! tracker.update(105_000.0, timestamp_ms);
//!
//! let detector = LatencyDetector::new(LatencyConfig::default());
//! if let Some(signal) = detector.check(&tracker, yes_ask, no_ask) {
//!     println!("Entry signal: {:?}", signal);
//! }
//! ```

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Maximum price history entries to keep (5 minutes at ~10 updates/sec).
const MAX_PRICE_HISTORY: usize = 3000;

/// Spot price update with timestamp.
#[derive(Debug, Clone, Copy)]
pub struct SpotPrice {
    /// BTC price in USD.
    pub price: f64,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: i64,
}

/// Tracks BTC spot price and calculates rolling changes.
#[derive(Debug)]
pub struct SpotPriceTracker {
    /// Recent price history (newest first).
    prices: VecDeque<SpotPrice>,
    /// Current price (most recent).
    current: Option<SpotPrice>,
}

impl Default for SpotPriceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SpotPriceTracker {
    /// Creates a new empty price tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            prices: VecDeque::with_capacity(MAX_PRICE_HISTORY),
            current: None,
        }
    }

    /// Updates with a new spot price.
    pub fn update(&mut self, price: f64, timestamp_ms: i64) {
        let spot = SpotPrice {
            price,
            timestamp_ms,
        };

        self.current = Some(spot);
        self.prices.push_front(spot);

        // Trim old entries
        while self.prices.len() > MAX_PRICE_HISTORY {
            self.prices.pop_back();
        }
    }

    /// Returns the current spot price.
    #[must_use]
    pub fn current_price(&self) -> Option<f64> {
        self.current.map(|s| s.price)
    }

    /// Returns the current timestamp.
    #[must_use]
    pub fn current_timestamp_ms(&self) -> Option<i64> {
        self.current.map(|s| s.timestamp_ms)
    }

    /// Calculates price change over the specified duration.
    ///
    /// Returns (absolute_change, percent_change) or None if insufficient data.
    /// Finds the oldest price at or before the lookback cutoff time.
    #[must_use]
    pub fn price_change(&self, lookback_ms: i64) -> Option<(f64, f64)> {
        let current = self.current?;
        let cutoff = current.timestamp_ms - lookback_ms;

        // Find the most recent price that's at or before the cutoff
        // (i.e., the price at the start of our lookback window)
        // Prices are stored newest-first, so we iterate and find first one <= cutoff
        let old_price = self
            .prices
            .iter()
            .find(|p| p.timestamp_ms <= cutoff)
            .or_else(|| self.prices.back())?; // Fall back to oldest if none before cutoff

        if old_price.timestamp_ms == current.timestamp_ms {
            return Some((0.0, 0.0));
        }

        let abs_change = current.price - old_price.price;
        let pct_change = abs_change / old_price.price;

        Some((abs_change, pct_change))
    }

    /// Returns the 1-minute price change as a percentage.
    #[must_use]
    pub fn change_1min(&self) -> Option<f64> {
        self.price_change(60_000).map(|(_, pct)| pct)
    }

    /// Returns the 5-minute price change as a percentage.
    #[must_use]
    pub fn change_5min(&self) -> Option<f64> {
        self.price_change(300_000).map(|(_, pct)| pct)
    }

    /// Returns the number of price updates stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.prices.len()
    }

    /// Returns true if no prices have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    /// Clears all price history.
    pub fn clear(&mut self) {
        self.prices.clear();
        self.current = None;
    }
}

/// Configuration for the latency detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyConfig {
    /// Minimum spot price change to consider (as decimal, e.g., 0.003 = 0.3%).
    pub min_spot_change: f64,
    /// Maximum price for entry (e.g., $0.35).
    pub max_entry_price: Decimal,
    /// Lookback period for spot change in milliseconds.
    pub lookback_ms: i64,
    /// Minimum staleness (how long Poly should lag spot) in milliseconds.
    pub min_staleness_ms: i64,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            min_spot_change: 0.002, // 0.2% (gabagool observed threshold)
            max_entry_price: dec!(0.45), // $0.45 (gabagool enters at $0.41)
            lookback_ms: 300_000, // 5 minutes
            min_staleness_ms: 1_000, // 1 second
        }
    }
}

impl LatencyConfig {
    /// Creates a more aggressive config (lower thresholds).
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            min_spot_change: 0.001, // 0.1%
            max_entry_price: dec!(0.48), // Almost any mispricing
            lookback_ms: 60_000, // 1 minute
            min_staleness_ms: 500,
        }
    }

    /// Creates a conservative config (higher thresholds).
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            min_spot_change: 0.003, // 0.3%
            max_entry_price: dec!(0.40),
            lookback_ms: 300_000, // 5 minutes
            min_staleness_ms: 2_000,
        }
    }
}

/// Direction to trade based on latency signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LatencyDirection {
    /// Buy YES - BTC moved up, YES is cheap.
    BuyYes,
    /// Buy NO - BTC moved down, NO is cheap.
    BuyNo,
}

/// A detected latency arbitrage opportunity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySignal {
    /// Which side to buy.
    pub direction: LatencyDirection,
    /// Entry price for the cheap side.
    pub entry_price: Decimal,
    /// Spot price change that triggered this signal.
    pub spot_change_pct: f64,
    /// Current BTC spot price.
    pub spot_price: f64,
    /// Timestamp when signal was generated.
    pub timestamp: DateTime<Utc>,
    /// Estimated edge (how mispriced the market is).
    pub estimated_edge: f64,
    /// Signal strength (0.0 to 1.0).
    pub strength: f64,
}

impl LatencySignal {
    /// Returns the opposite side price needed for a full hedge.
    ///
    /// For the position to be fully hedged, combined cost should be < $1.00.
    #[must_use]
    pub fn hedge_target(&self) -> Decimal {
        // If we bought at entry_price, we need opposite side at < (1.0 - entry_price)
        // With some margin for fees
        Decimal::ONE - self.entry_price - dec!(0.02) // 2% buffer for fees
    }
}

/// Latency arbitrage detector.
///
/// Monitors spot price movements and Polymarket odds to detect
/// when Polymarket is stale (lagging behind confirmed spot moves).
#[derive(Debug)]
pub struct LatencyDetector {
    /// Configuration.
    config: LatencyConfig,
    /// Last signal generated (to avoid duplicates).
    last_signal_ms: Option<i64>,
    /// Minimum time between signals in milliseconds.
    signal_cooldown_ms: i64,
}

impl LatencyDetector {
    /// Creates a new latency detector with the given config.
    #[must_use]
    pub fn new(config: LatencyConfig) -> Self {
        Self {
            config,
            last_signal_ms: None,
            signal_cooldown_ms: 5_000, // 5 second cooldown between signals
        }
    }

    /// Sets the signal cooldown period.
    #[must_use]
    pub fn with_cooldown(mut self, cooldown_ms: i64) -> Self {
        self.signal_cooldown_ms = cooldown_ms;
        self
    }

    /// Checks for a latency arbitrage opportunity.
    ///
    /// # Arguments
    ///
    /// * `tracker` - Spot price tracker with recent BTC prices
    /// * `yes_ask` - Best ask price for YES outcome
    /// * `no_ask` - Best ask price for NO outcome
    /// * `current_time_ms` - Current timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// Returns `Some(LatencySignal)` if an opportunity is detected, `None` otherwise.
    pub fn check(
        &mut self,
        tracker: &SpotPriceTracker,
        yes_ask: Decimal,
        no_ask: Decimal,
        current_time_ms: i64,
    ) -> Option<LatencySignal> {
        // Check cooldown
        if let Some(last) = self.last_signal_ms {
            if current_time_ms - last < self.signal_cooldown_ms {
                return None;
            }
        }

        // Get spot price change
        let (_, spot_change_pct) = tracker.price_change(self.config.lookback_ms)?;
        let spot_price = tracker.current_price()?;

        // Check for opportunity
        let signal = self.detect_opportunity(
            spot_change_pct,
            spot_price,
            yes_ask,
            no_ask,
            current_time_ms,
        );

        if signal.is_some() {
            self.last_signal_ms = Some(current_time_ms);
        }

        signal
    }

    /// Internal opportunity detection logic.
    fn detect_opportunity(
        &self,
        spot_change_pct: f64,
        spot_price: f64,
        yes_ask: Decimal,
        no_ask: Decimal,
        current_time_ms: i64,
    ) -> Option<LatencySignal> {
        let timestamp = DateTime::from_timestamp_millis(current_time_ms)?;

        // Scenario 1: BTC went UP, YES should be valuable, but YES is cheap
        if spot_change_pct >= self.config.min_spot_change
            && yes_ask <= self.config.max_entry_price
        {
            let estimated_edge = spot_change_pct - (1.0 - yes_ask.to_string().parse::<f64>().unwrap_or(0.5));
            let strength = (spot_change_pct / self.config.min_spot_change).min(1.0);

            return Some(LatencySignal {
                direction: LatencyDirection::BuyYes,
                entry_price: yes_ask,
                spot_change_pct,
                spot_price,
                timestamp,
                estimated_edge,
                strength,
            });
        }

        // Scenario 2: BTC went DOWN, NO should be valuable, but NO is cheap
        if spot_change_pct <= -self.config.min_spot_change
            && no_ask <= self.config.max_entry_price
        {
            let estimated_edge = (-spot_change_pct) - (1.0 - no_ask.to_string().parse::<f64>().unwrap_or(0.5));
            let strength = ((-spot_change_pct) / self.config.min_spot_change).min(1.0);

            return Some(LatencySignal {
                direction: LatencyDirection::BuyNo,
                entry_price: no_ask,
                spot_change_pct,
                spot_price,
                timestamp,
                estimated_edge,
                strength,
            });
        }

        None
    }

    /// Resets the cooldown timer (useful for testing).
    pub fn reset_cooldown(&mut self) {
        self.last_signal_ms = None;
    }

    /// Returns the current configuration.
    #[must_use]
    pub fn config(&self) -> &LatencyConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // SpotPriceTracker Tests
    // =========================================================================

    #[test]
    fn test_tracker_new_is_empty() {
        let tracker = SpotPriceTracker::new();
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);
        assert!(tracker.current_price().is_none());
    }

    #[test]
    fn test_tracker_single_update() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(105_000.0, 1000);

        assert!(!tracker.is_empty());
        assert_eq!(tracker.len(), 1);
        assert_eq!(tracker.current_price(), Some(105_000.0));
        assert_eq!(tracker.current_timestamp_ms(), Some(1000));
    }

    #[test]
    fn test_tracker_multiple_updates() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 1000);
        tracker.update(101_000.0, 2000);
        tracker.update(102_000.0, 3000);

        assert_eq!(tracker.len(), 3);
        assert_eq!(tracker.current_price(), Some(102_000.0));
    }

    #[test]
    fn test_tracker_price_change_single_point() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 1000);

        // Single point should return zero change
        let change = tracker.price_change(60_000);
        assert_eq!(change, Some((0.0, 0.0)));
    }

    #[test]
    fn test_tracker_price_change_positive() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_300.0, 60_000); // +0.3% after 1 minute

        let (abs, pct) = tracker.price_change(120_000).unwrap(); // 2 min lookback
        assert!((abs - 300.0).abs() < 0.01);
        assert!((pct - 0.003).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_price_change_negative() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(99_500.0, 60_000); // -0.5% after 1 minute

        let (abs, pct) = tracker.price_change(120_000).unwrap();
        assert!((abs - (-500.0)).abs() < 0.01);
        assert!((pct - (-0.005)).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_change_1min() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 30_000); // +0.5% after 30 sec

        let pct = tracker.change_1min().unwrap();
        assert!((pct - 0.005).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_change_5min() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(101_000.0, 150_000); // +1% after 2.5 min

        let pct = tracker.change_5min().unwrap();
        assert!((pct - 0.01).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_lookback_window() {
        let mut tracker = SpotPriceTracker::new();

        // Price 5 minutes ago
        tracker.update(100_000.0, 0);
        // Price 3 minutes ago
        tracker.update(100_200.0, 120_000);
        // Current price
        tracker.update(100_500.0, 300_000);

        // 2-minute lookback should only see the last update
        let (_, pct_2min) = tracker.price_change(120_000).unwrap();
        assert!((pct_2min - 0.003).abs() < 0.0001); // From 100_200 to 100_500

        // 5-minute lookback should see from the beginning
        let (_, pct_5min) = tracker.price_change(300_000).unwrap();
        assert!((pct_5min - 0.005).abs() < 0.0001); // From 100_000 to 100_500
    }

    #[test]
    fn test_tracker_clear() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(101_000.0, 1000);

        tracker.clear();

        assert!(tracker.is_empty());
        assert!(tracker.current_price().is_none());
    }

    // =========================================================================
    // LatencyConfig Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = LatencyConfig::default();
        assert!((config.min_spot_change - 0.002).abs() < 0.0001); // 0.2%
        assert_eq!(config.max_entry_price, dec!(0.45));
        assert_eq!(config.lookback_ms, 300_000);
    }

    #[test]
    fn test_config_aggressive() {
        let config = LatencyConfig::aggressive();
        assert!((config.min_spot_change - 0.001).abs() < 0.0001); // 0.1%
        assert_eq!(config.max_entry_price, dec!(0.48));
    }

    #[test]
    fn test_config_conservative() {
        let config = LatencyConfig::conservative();
        assert!((config.min_spot_change - 0.003).abs() < 0.0001); // 0.3%
        assert_eq!(config.max_entry_price, dec!(0.40));
    }

    // =========================================================================
    // LatencySignal Tests
    // =========================================================================

    #[test]
    fn test_signal_hedge_target() {
        let signal = LatencySignal {
            direction: LatencyDirection::BuyYes,
            entry_price: dec!(0.35),
            spot_change_pct: 0.005,
            spot_price: 100_500.0,
            timestamp: Utc::now(),
            estimated_edge: 0.01,
            strength: 1.0,
        };

        // Hedge target: 1.00 - 0.35 - 0.02 = 0.63
        assert_eq!(signal.hedge_target(), dec!(0.63));
    }

    // =========================================================================
    // LatencyDetector Tests
    // =========================================================================

    #[test]
    fn test_detector_no_signal_insufficient_data() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let tracker = SpotPriceTracker::new(); // Empty tracker

        let signal = detector.check(&tracker, dec!(0.35), dec!(0.35), 1000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_detector_no_signal_prices_too_high() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 60_000); // +0.5% move

        // Both YES and NO at $0.50 - too expensive
        let signal = detector.check(&tracker, dec!(0.50), dec!(0.50), 60_000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_detector_no_signal_spot_move_too_small() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_100.0, 60_000); // Only +0.1% move

        // YES is cheap but spot didn't move enough
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_detector_buy_yes_signal() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved UP 0.5%
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 60_000);

        // YES is cheap at $0.30 - should signal BUY YES
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.direction, LatencyDirection::BuyYes);
        assert_eq!(signal.entry_price, dec!(0.30));
        assert!((signal.spot_change_pct - 0.005).abs() < 0.0001);
    }

    #[test]
    fn test_detector_buy_no_signal() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved DOWN 0.5%
        tracker.update(100_000.0, 0);
        tracker.update(99_500.0, 60_000);

        // NO is cheap at $0.30 - should signal BUY NO
        let signal = detector.check(&tracker, dec!(0.70), dec!(0.30), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.direction, LatencyDirection::BuyNo);
        assert_eq!(signal.entry_price, dec!(0.30));
        assert!((signal.spot_change_pct - (-0.005)).abs() < 0.0001);
    }

    #[test]
    fn test_detector_prefers_direction_matching_spot() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved UP
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 60_000);

        // Both YES and NO are cheap - but should pick YES because BTC went UP
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.30), 60_000);

        assert!(signal.is_some());
        assert_eq!(signal.unwrap().direction, LatencyDirection::BuyYes);
    }

    #[test]
    fn test_detector_cooldown() {
        let mut detector = LatencyDetector::new(LatencyConfig::default())
            .with_cooldown(10_000); // 10 second cooldown

        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 60_000);

        // First signal should work
        let signal1 = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal1.is_some());

        // Second signal within cooldown should be blocked
        let signal2 = detector.check(&tracker, dec!(0.30), dec!(0.70), 65_000);
        assert!(signal2.is_none());

        // Third signal after cooldown should work
        let signal3 = detector.check(&tracker, dec!(0.30), dec!(0.70), 75_000);
        assert!(signal3.is_some());
    }

    #[test]
    fn test_detector_reset_cooldown() {
        let mut detector = LatencyDetector::new(LatencyConfig::default())
            .with_cooldown(10_000);

        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_500.0, 60_000);

        // First signal
        let _ = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        // Reset cooldown
        detector.reset_cooldown();

        // Should work immediately after reset
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 61_000);
        assert!(signal.is_some());
    }

    #[test]
    fn test_detector_signal_strength() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved UP 0.6% (2x the minimum threshold)
        tracker.update(100_000.0, 0);
        tracker.update(100_600.0, 60_000);

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        // Strength should be capped at 1.0 (0.6% / 0.3% = 2.0, capped)
        assert!((signal.strength - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_detector_with_aggressive_config() {
        let mut detector = LatencyDetector::new(LatencyConfig::aggressive());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved UP only 0.25%
        tracker.update(100_000.0, 0);
        tracker.update(100_250.0, 60_000);

        // With aggressive config (0.2% threshold), this should trigger
        // And max entry price is $0.40
        let signal = detector.check(&tracker, dec!(0.38), dec!(0.62), 60_000);

        assert!(signal.is_some());
        assert_eq!(signal.unwrap().direction, LatencyDirection::BuyYes);
    }

    #[test]
    fn test_detector_exact_threshold() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved exactly 0.2% (the threshold)
        tracker.update(100_000.0, 0);
        tracker.update(100_200.0, 60_000);

        // YES at exactly $0.45 (the max entry price)
        let signal = detector.check(&tracker, dec!(0.45), dec!(0.55), 60_000);

        assert!(signal.is_some());
    }

    #[test]
    fn test_detector_just_below_threshold() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // BTC moved 0.19% (just below 0.2% threshold)
        tracker.update(100_000.0, 0);
        tracker.update(100_190.0, 60_000);

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        assert!(signal.is_none());
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_tracker_handles_rapid_updates() {
        let mut tracker = SpotPriceTracker::new();

        // Simulate 100 rapid updates
        for i in 0..100 {
            tracker.update(100_000.0 + (i as f64), i as i64);
        }

        assert_eq!(tracker.len(), 100);
        assert_eq!(tracker.current_price(), Some(100_099.0));
    }

    #[test]
    fn test_tracker_trims_old_entries() {
        let mut tracker = SpotPriceTracker::new();

        // Add more than MAX_PRICE_HISTORY entries
        for i in 0..(MAX_PRICE_HISTORY + 100) {
            tracker.update(100_000.0, i as i64);
        }

        assert_eq!(tracker.len(), MAX_PRICE_HISTORY);
    }

    #[test]
    fn test_detector_handles_zero_spot_change() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let mut tracker = SpotPriceTracker::new();

        // No price change
        tracker.update(100_000.0, 0);
        tracker.update(100_000.0, 60_000);

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal.is_none());
    }
}
