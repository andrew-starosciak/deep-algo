//! Latency arbitrage detection for Polymarket binary markets.
//!
//! This module detects opportunities where Polymarket prices lag behind
//! spot BTC movements on Binance. The strategy:
//!
//! 1. Monitor BTC spot price on Binance in real-time
//! 2. Track the "price to beat" at each 15-minute window open
//! 3. Compare current spot to window reference price
//! 4. Signal entry when:
//!    - One side is cheap (< $0.45)
//!    - Current spot confirms direction vs window reference
//!
//! # The Edge
//!
//! Polymarket windows compare FINAL price to OPENING price ("price to beat").
//! If BTC is currently above the reference and YES is cheap, buy YES.
//! If BTC is currently below the reference and NO is cheap, buy NO.
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
//! if let Some(signal) = detector.check(&tracker, yes_ask, no_ask, timestamp_ms) {
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

/// 15 minutes in milliseconds.
const WINDOW_DURATION_MS: i64 = 15 * 60 * 1000;

/// Spot price update with timestamp.
#[derive(Debug, Clone, Copy)]
pub struct SpotPrice {
    /// BTC price in USD.
    pub price: f64,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: i64,
}

/// Tracks BTC spot price with 15-minute window reference tracking.
///
/// The key insight is that Polymarket windows compare:
/// - FINAL price at window close
/// - vs OPENING price ("price to beat") at window start
///
/// So we track the reference price at each window boundary.
#[derive(Debug)]
pub struct SpotPriceTracker {
    /// Recent price history (newest first).
    prices: VecDeque<SpotPrice>,
    /// Current price (most recent).
    current: Option<SpotPrice>,
    /// Reference price for the current 15-minute window ("price to beat").
    window_reference: Option<f64>,
    /// Start timestamp of the current window (aligned to 15-min boundary).
    window_start_ms: Option<i64>,
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
            window_reference: None,
            window_start_ms: None,
        }
    }

    /// Calculates the 15-minute window start for a given timestamp.
    ///
    /// Windows are aligned to :00, :15, :30, :45 minute marks.
    #[must_use]
    pub fn window_start_for(timestamp_ms: i64) -> i64 {
        (timestamp_ms / WINDOW_DURATION_MS) * WINDOW_DURATION_MS
    }

    /// Updates with a new spot price.
    ///
    /// Automatically detects window boundaries and captures reference price.
    pub fn update(&mut self, price: f64, timestamp_ms: i64) {
        let spot = SpotPrice {
            price,
            timestamp_ms,
        };

        // Check if we've entered a new 15-minute window
        let new_window_start = Self::window_start_for(timestamp_ms);

        if self.window_start_ms != Some(new_window_start) {
            // New window! Capture the reference price
            self.window_start_ms = Some(new_window_start);
            self.window_reference = Some(price);
            tracing::info!(
                window_start = new_window_start,
                reference_price = price,
                "New 15-minute window started - captured reference price"
            );
        }

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

    /// Returns the reference price ("price to beat") for the current window.
    #[must_use]
    pub fn window_reference_price(&self) -> Option<f64> {
        self.window_reference
    }

    /// Returns the start timestamp of the current window.
    #[must_use]
    pub fn window_start(&self) -> Option<i64> {
        self.window_start_ms
    }

    /// Returns time remaining in the current window in milliseconds.
    #[must_use]
    pub fn time_remaining_ms(&self) -> Option<i64> {
        let current_ts = self.current?.timestamp_ms;
        let window_start = self.window_start_ms?;
        let window_end = window_start + WINDOW_DURATION_MS;
        Some((window_end - current_ts).max(0))
    }

    /// Returns time remaining in seconds.
    #[must_use]
    pub fn time_remaining_secs(&self) -> Option<i64> {
        self.time_remaining_ms().map(|ms| ms / 1000)
    }

    /// Calculates price change vs the window reference price.
    ///
    /// Returns (absolute_change, percent_change) or None if no reference.
    /// This is the KEY metric for Polymarket - comparing to "price to beat".
    #[must_use]
    pub fn change_vs_reference(&self) -> Option<(f64, f64)> {
        let current = self.current_price()?;
        let reference = self.window_reference?;

        let abs_change = current - reference;
        let pct_change = abs_change / reference;

        Some((abs_change, pct_change))
    }

    /// Returns true if current price is ABOVE the window reference.
    /// If true, YES is likely to win.
    #[must_use]
    pub fn is_above_reference(&self) -> Option<bool> {
        self.change_vs_reference().map(|(abs, _)| abs > 0.0)
    }

    /// Calculates price change over the specified duration (legacy method).
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

    /// Clears all price history and window state.
    pub fn clear(&mut self) {
        self.prices.clear();
        self.current = None;
        self.window_reference = None;
        self.window_start_ms = None;
    }

    /// Manually sets the window reference price.
    /// Useful when you know the exact "price to beat" from Polymarket.
    pub fn set_reference_price(&mut self, price: f64, window_start_ms: i64) {
        self.window_reference = Some(price);
        self.window_start_ms = Some(window_start_ms);
    }
}

/// Configuration for the latency detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyConfig {
    /// Minimum delta vs window reference (as decimal, e.g., 0.0005 = 0.05% = $39 on $78k BTC).
    /// This filters out noise - only signal when there's meaningful movement from reference.
    pub min_reference_delta: f64,
    /// Maximum price for entry (e.g., $0.45).
    pub max_entry_price: Decimal,
    /// Minimum time into window before signaling (milliseconds).
    /// Avoids signaling right at window open when reference is just being set.
    pub min_window_elapsed_ms: i64,
    /// Maximum time remaining to still enter (milliseconds).
    /// Don't enter if window is about to close.
    pub min_time_remaining_ms: i64,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            min_reference_delta: 0.0005, // 0.05% (~$39 on $78k BTC)
            max_entry_price: dec!(0.45), // $0.45 (gabagool enters at $0.41)
            min_window_elapsed_ms: 30_000, // Wait 30 seconds into window
            min_time_remaining_ms: 60_000, // Need at least 1 minute left
        }
    }
}

impl LatencyConfig {
    /// Creates a more aggressive config (lower thresholds, earlier entry).
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            min_reference_delta: 0.0002, // 0.02% (~$16 on $78k BTC)
            max_entry_price: dec!(0.48), // Almost any mispricing
            min_window_elapsed_ms: 15_000, // Enter after 15 seconds
            min_time_remaining_ms: 30_000, // Can enter with 30 sec left
        }
    }

    /// Creates a conservative config (higher thresholds, safer timing).
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            min_reference_delta: 0.001, // 0.1% (~$78 on $78k BTC)
            max_entry_price: dec!(0.40),
            min_window_elapsed_ms: 60_000, // Wait 1 minute into window
            min_time_remaining_ms: 120_000, // Need at least 2 minutes left
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
    /// Spot price change vs window reference (percent, e.g., 0.005 = 0.5%).
    pub spot_change_pct: f64,
    /// Current BTC spot price.
    pub spot_price: f64,
    /// Window reference price ("price to beat").
    pub reference_price: f64,
    /// Time remaining in window (seconds).
    pub time_remaining_secs: i64,
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
    /// * `tracker` - Spot price tracker with recent BTC prices and window reference
    /// * `yes_ask` - Best ask price for YES outcome
    /// * `no_ask` - Best ask price for NO outcome
    /// * `current_time_ms` - Current timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// Returns `Some(LatencySignal)` if an opportunity is detected, `None` otherwise.
    ///
    /// # Logic
    ///
    /// Compares CURRENT spot price to WINDOW REFERENCE ("price to beat"):
    /// - If current > reference by enough margin AND YES is cheap → BUY YES
    /// - If current < reference by enough margin AND NO is cheap → BUY NO
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

        // Check timing constraints
        let window_start = tracker.window_start()?;
        let elapsed_ms = current_time_ms - window_start;
        if elapsed_ms < self.config.min_window_elapsed_ms {
            return None; // Too early in window
        }

        let time_remaining_ms = tracker.time_remaining_ms()?;
        if time_remaining_ms < self.config.min_time_remaining_ms {
            return None; // Too late in window
        }

        // Get spot price change vs window reference
        let (_, spot_change_pct) = tracker.change_vs_reference()?;
        let spot_price = tracker.current_price()?;
        let reference_price = tracker.window_reference_price()?;
        let time_remaining_secs = time_remaining_ms / 1000;

        // Check for opportunity
        let signal = self.detect_opportunity(
            spot_change_pct,
            spot_price,
            reference_price,
            time_remaining_secs,
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
    ///
    /// Compares current spot to window reference ("price to beat"):
    /// - If BTC is ABOVE reference → YES wins → buy YES if cheap
    /// - If BTC is BELOW reference → NO wins → buy NO if cheap
    fn detect_opportunity(
        &self,
        spot_change_pct: f64,
        spot_price: f64,
        reference_price: f64,
        time_remaining_secs: i64,
        yes_ask: Decimal,
        no_ask: Decimal,
        current_time_ms: i64,
    ) -> Option<LatencySignal> {
        let timestamp = DateTime::from_timestamp_millis(current_time_ms)?;
        let abs_change = spot_change_pct.abs();

        // Need sufficient delta from reference to signal
        if abs_change < self.config.min_reference_delta {
            return None;
        }

        // Scenario 1: BTC is ABOVE reference → YES should win → buy YES if cheap
        if spot_change_pct > 0.0 && yes_ask <= self.config.max_entry_price {
            let yes_price_f64 = yes_ask.to_string().parse::<f64>().unwrap_or(0.5);
            let estimated_edge = abs_change - (1.0 - yes_price_f64);
            let strength = (abs_change / self.config.min_reference_delta).min(1.0);

            return Some(LatencySignal {
                direction: LatencyDirection::BuyYes,
                entry_price: yes_ask,
                spot_change_pct,
                spot_price,
                reference_price,
                time_remaining_secs,
                timestamp,
                estimated_edge,
                strength,
            });
        }

        // Scenario 2: BTC is BELOW reference → NO should win → buy NO if cheap
        if spot_change_pct < 0.0 && no_ask <= self.config.max_entry_price {
            let no_price_f64 = no_ask.to_string().parse::<f64>().unwrap_or(0.5);
            let estimated_edge = abs_change - (1.0 - no_price_f64);
            let strength = (abs_change / self.config.min_reference_delta).min(1.0);

            return Some(LatencySignal {
                direction: LatencyDirection::BuyNo,
                entry_price: no_ask,
                spot_change_pct,
                spot_price,
                reference_price,
                time_remaining_secs,
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
    fn test_tracker_captures_window_reference() {
        let mut tracker = SpotPriceTracker::new();

        // First update captures reference
        tracker.update(78_000.0, 0);
        assert_eq!(tracker.window_reference_price(), Some(78_000.0));
        assert_eq!(tracker.window_start(), Some(0));

        // Subsequent updates in same window don't change reference
        tracker.update(78_100.0, 1000);
        tracker.update(78_200.0, 2000);
        assert_eq!(tracker.window_reference_price(), Some(78_000.0)); // Still original
    }

    #[test]
    fn test_tracker_new_window_resets_reference() {
        let mut tracker = SpotPriceTracker::new();

        // Window 1 (starts at 0)
        tracker.update(78_000.0, 0);
        assert_eq!(tracker.window_reference_price(), Some(78_000.0));

        // Window 2 (starts at 15 min = 900_000 ms)
        tracker.update(78_500.0, WINDOW_DURATION_MS);
        assert_eq!(tracker.window_reference_price(), Some(78_500.0));
        assert_eq!(tracker.window_start(), Some(WINDOW_DURATION_MS));
    }

    #[test]
    fn test_tracker_change_vs_reference_positive() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0); // Reference
        tracker.update(78_078.0, 60_000); // +0.1% after 1 min

        let (abs, pct) = tracker.change_vs_reference().unwrap();
        assert!((abs - 78.0).abs() < 0.01);
        assert!((pct - 0.001).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_change_vs_reference_negative() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0); // Reference
        tracker.update(77_922.0, 60_000); // -0.1% after 1 min

        let (abs, pct) = tracker.change_vs_reference().unwrap();
        assert!((abs - (-78.0)).abs() < 0.01);
        assert!((pct - (-0.001)).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_is_above_reference() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 1000);

        assert_eq!(tracker.is_above_reference(), Some(true));

        tracker.update(77_900.0, 2000);
        assert_eq!(tracker.is_above_reference(), Some(false));
    }

    #[test]
    fn test_tracker_time_remaining() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0); // Window start

        // 1 minute in, 14 minutes remaining
        tracker.update(78_100.0, 60_000);
        assert_eq!(tracker.time_remaining_ms(), Some(WINDOW_DURATION_MS - 60_000));
        assert_eq!(tracker.time_remaining_secs(), Some(14 * 60)); // 840 seconds
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
    fn test_tracker_price_change_legacy() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(100_300.0, 60_000);

        let (abs, pct) = tracker.price_change(120_000).unwrap();
        assert!((abs - 300.0).abs() < 0.01);
        assert!((pct - 0.003).abs() < 0.0001);
    }

    #[test]
    fn test_tracker_clear() {
        let mut tracker = SpotPriceTracker::new();
        tracker.update(100_000.0, 0);
        tracker.update(101_000.0, 1000);

        tracker.clear();

        assert!(tracker.is_empty());
        assert!(tracker.current_price().is_none());
        assert!(tracker.window_reference_price().is_none());
        assert!(tracker.window_start().is_none());
    }

    #[test]
    fn test_tracker_set_reference_price() {
        let mut tracker = SpotPriceTracker::new();
        tracker.set_reference_price(78_458.86, 0);

        assert_eq!(tracker.window_reference_price(), Some(78_458.86));
        assert_eq!(tracker.window_start(), Some(0));

        // Now update with current price
        tracker.update(78_400.0, 60_000);

        // Should calculate change vs manually set reference
        let (abs, _) = tracker.change_vs_reference().unwrap();
        assert!((abs - (-58.86)).abs() < 0.01);
    }

    // =========================================================================
    // LatencyConfig Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = LatencyConfig::default();
        assert!((config.min_reference_delta - 0.0005).abs() < 0.0001); // 0.05%
        assert_eq!(config.max_entry_price, dec!(0.45));
        assert_eq!(config.min_window_elapsed_ms, 30_000);
        assert_eq!(config.min_time_remaining_ms, 60_000);
    }

    #[test]
    fn test_config_aggressive() {
        let config = LatencyConfig::aggressive();
        assert!((config.min_reference_delta - 0.0002).abs() < 0.0001); // 0.02%
        assert_eq!(config.max_entry_price, dec!(0.48));
        assert_eq!(config.min_window_elapsed_ms, 15_000);
        assert_eq!(config.min_time_remaining_ms, 30_000);
    }

    #[test]
    fn test_config_conservative() {
        let config = LatencyConfig::conservative();
        assert!((config.min_reference_delta - 0.001).abs() < 0.0001); // 0.1%
        assert_eq!(config.max_entry_price, dec!(0.40));
        assert_eq!(config.min_window_elapsed_ms, 60_000);
        assert_eq!(config.min_time_remaining_ms, 120_000);
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
            reference_price: 100_000.0,
            time_remaining_secs: 600,
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
    fn test_detector_no_signal_no_reference() {
        let mut detector = LatencyDetector::new(LatencyConfig::default());
        let tracker = SpotPriceTracker::new(); // Empty tracker, no reference

        let signal = detector.check(&tracker, dec!(0.35), dec!(0.35), 60_000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_detector_no_signal_too_early_in_window() {
        let config = LatencyConfig {
            min_window_elapsed_ms: 30_000, // Need 30 sec
            ..LatencyConfig::default()
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        tracker.update(78_000.0, 0); // Reference at window start
        tracker.update(78_100.0, 10_000); // Only 10 sec in, +0.13%

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 10_000);
        assert!(signal.is_none()); // Too early
    }

    #[test]
    fn test_detector_no_signal_too_late_in_window() {
        let config = LatencyConfig {
            min_window_elapsed_ms: 1000,
            min_time_remaining_ms: 60_000, // Need 1 min left
            ..LatencyConfig::default()
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        tracker.update(78_000.0, 0); // Reference at window start
        // 14.5 minutes in, only 30 sec left
        tracker.update(78_100.0, WINDOW_DURATION_MS - 30_000);

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), WINDOW_DURATION_MS - 30_000);
        assert!(signal.is_none()); // Too late
    }

    #[test]
    fn test_detector_no_signal_prices_too_high() {
        let config = LatencyConfig {
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
            ..LatencyConfig::default()
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 60_000); // +0.13%

        // Both YES and NO at $0.50 - too expensive
        let signal = detector.check(&tracker, dec!(0.50), dec!(0.50), 60_000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_detector_no_signal_delta_too_small() {
        let config = LatencyConfig {
            min_reference_delta: 0.001, // Need 0.1%
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
            ..LatencyConfig::default()
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        tracker.update(78_000.0, 0);
        tracker.update(78_039.0, 60_000); // Only +0.05%

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal.is_none()); // Delta too small
    }

    #[test]
    fn test_detector_buy_yes_signal_btc_above_reference() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005, // 0.05%
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // BTC above reference by 0.1%
        tracker.update(78_000.0, 0); // Reference
        tracker.update(78_078.0, 60_000); // +0.1%

        // YES is cheap at $0.30 - should signal BUY YES
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.direction, LatencyDirection::BuyYes);
        assert_eq!(signal.entry_price, dec!(0.30));
        assert!((signal.spot_change_pct - 0.001).abs() < 0.0001);
        assert_eq!(signal.reference_price, 78_000.0);
    }

    #[test]
    fn test_detector_buy_no_signal_btc_below_reference() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005, // 0.05%
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // BTC below reference by 0.1% - NO should win
        tracker.update(78_000.0, 0); // Reference
        tracker.update(77_922.0, 60_000); // -0.1%

        // NO is cheap at $0.30 - should signal BUY NO
        let signal = detector.check(&tracker, dec!(0.70), dec!(0.30), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.direction, LatencyDirection::BuyNo);
        assert_eq!(signal.entry_price, dec!(0.30));
        assert!((signal.spot_change_pct - (-0.001)).abs() < 0.0001);
        assert_eq!(signal.reference_price, 78_000.0);
    }

    #[test]
    fn test_detector_real_scenario_btc_down_buy_no() {
        // Recreate the actual failure scenario:
        // Reference: $78,458.86, Final: $78,427.91 = DOWN = NO wins
        let config = LatencyConfig {
            min_reference_delta: 0.0002, // Very sensitive
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // Simulate the real scenario
        tracker.set_reference_price(78_458.86, 0);
        tracker.update(78_427.91, 300_000); // 5 minutes in, BTC DOWN

        // NO should be cheap since BTC is below reference
        let signal = detector.check(&tracker, dec!(0.60), dec!(0.30), 300_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.direction, LatencyDirection::BuyNo);
        assert!(signal.spot_change_pct < 0.0); // Negative = below reference
        assert_eq!(signal.reference_price, 78_458.86);
    }

    #[test]
    fn test_detector_prefers_direction_matching_reference() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005,
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // BTC went UP vs reference
        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 60_000);

        // Both YES and NO are cheap - should pick YES because BTC is ABOVE reference
        let signal = detector.check(&tracker, dec!(0.30), dec!(0.30), 60_000);

        assert!(signal.is_some());
        assert_eq!(signal.unwrap().direction, LatencyDirection::BuyYes);
    }

    #[test]
    fn test_detector_cooldown() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005,
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config).with_cooldown(10_000);

        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 60_000);

        // First signal should work
        let signal1 = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal1.is_some());

        // Second signal within cooldown should be blocked
        let signal2 = detector.check(&tracker, dec!(0.30), dec!(0.70), 65_000);
        assert!(signal2.is_none());

        // Third signal after cooldown should work
        tracker.update(78_150.0, 75_000);
        let signal3 = detector.check(&tracker, dec!(0.30), dec!(0.70), 75_000);
        assert!(signal3.is_some());
    }

    #[test]
    fn test_detector_reset_cooldown() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005,
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config).with_cooldown(10_000);

        let mut tracker = SpotPriceTracker::new();
        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 60_000);

        let _ = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        detector.reset_cooldown();

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 61_000);
        assert!(signal.is_some());
    }

    #[test]
    fn test_detector_signal_strength() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005, // 0.05%
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // BTC moved UP 0.15% (3x the threshold)
        tracker.update(78_000.0, 0);
        tracker.update(78_117.0, 60_000); // +0.15%

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);

        assert!(signal.is_some());
        let signal = signal.unwrap();
        // Strength should be capped at 1.0
        assert!((signal.strength - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_detector_includes_time_remaining() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005,
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        tracker.update(78_000.0, 0);
        tracker.update(78_100.0, 300_000); // 5 minutes in

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 300_000).unwrap();

        // 5 min in = 10 min remaining = 600 sec
        assert_eq!(signal.time_remaining_secs, 600);
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_tracker_handles_rapid_updates() {
        let mut tracker = SpotPriceTracker::new();

        for i in 0..100 {
            tracker.update(100_000.0 + (i as f64), i as i64);
        }

        assert_eq!(tracker.len(), 100);
        assert_eq!(tracker.current_price(), Some(100_099.0));
        // Reference should be the first update
        assert_eq!(tracker.window_reference_price(), Some(100_000.0));
    }

    #[test]
    fn test_tracker_trims_old_entries() {
        let mut tracker = SpotPriceTracker::new();

        for i in 0..(MAX_PRICE_HISTORY + 100) {
            tracker.update(100_000.0, i as i64);
        }

        assert_eq!(tracker.len(), MAX_PRICE_HISTORY);
    }

    #[test]
    fn test_detector_handles_zero_delta() {
        let config = LatencyConfig {
            min_reference_delta: 0.0005,
            max_entry_price: dec!(0.45),
            min_window_elapsed_ms: 0,
            min_time_remaining_ms: 0,
        };
        let mut detector = LatencyDetector::new(config);
        let mut tracker = SpotPriceTracker::new();

        // No price change from reference
        tracker.update(78_000.0, 0);
        tracker.update(78_000.0, 60_000);

        let signal = detector.check(&tracker, dec!(0.30), dec!(0.70), 60_000);
        assert!(signal.is_none());
    }

    #[test]
    fn test_window_start_calculation() {
        // Window at :00
        assert_eq!(SpotPriceTracker::window_start_for(0), 0);
        assert_eq!(SpotPriceTracker::window_start_for(60_000), 0);
        assert_eq!(SpotPriceTracker::window_start_for(899_999), 0);

        // Window at :15
        assert_eq!(
            SpotPriceTracker::window_start_for(WINDOW_DURATION_MS),
            WINDOW_DURATION_MS
        );
        assert_eq!(
            SpotPriceTracker::window_start_for(WINDOW_DURATION_MS + 100_000),
            WINDOW_DURATION_MS
        );
    }
}
