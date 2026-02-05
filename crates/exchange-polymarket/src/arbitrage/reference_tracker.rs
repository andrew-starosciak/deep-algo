//! Window reference price tracking for Polymarket binary options.
//!
//! This module provides accurate tracking of the "price to beat" for each
//! 15-minute BTC binary option window. The reference price is the BTC price
//! at the START of the window - if the final price is above it, YES wins.
//!
//! # The Reference Price Problem
//!
//! Polymarket 15-minute windows use an opening price as the reference:
//! - Window opens at 3:00:00 PM
//! - Reference = BTC price at exactly 3:00:00 PM
//! - If final price (at 3:15:00) > reference → YES wins
//! - If final price < reference → NO wins
//!
//! Getting this reference wrong means our direction signals are inverted!
//!
//! # Multiple Sources
//!
//! We use multiple sources with confidence scoring:
//! 1. **Polymarket API** - Most accurate if available
//! 2. **Binance first trade** - First trade after window boundary
//! 3. **Binance VWAP** - Volume-weighted average of first N seconds
//! 4. **Interpolated** - Estimated from surrounding data (low confidence)

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tracing::{debug, info, warn};

/// Duration of each trading window in milliseconds.
pub const WINDOW_DURATION_MS: i64 = 15 * 60 * 1000; // 15 minutes

/// How the reference price was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceSource {
    /// From Polymarket market data (most accurate).
    PolymarketApi,
    /// First Binance trade after window open.
    BinanceFirstTrade,
    /// VWAP of first N seconds from Binance.
    BinanceVwap,
    /// Interpolated from surrounding data.
    Interpolated,
    /// Manually provided for testing.
    Manual,
}

impl ReferenceSource {
    /// Returns the default confidence for this source.
    #[must_use]
    pub fn default_confidence(&self) -> ReferenceConfidence {
        match self {
            Self::PolymarketApi => ReferenceConfidence::High,
            Self::Manual => ReferenceConfidence::High,
            Self::BinanceFirstTrade => ReferenceConfidence::Medium,
            Self::BinanceVwap => ReferenceConfidence::Medium,
            Self::Interpolated => ReferenceConfidence::Low,
        }
    }
}

/// Confidence level for a reference price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReferenceConfidence {
    /// Low confidence - may be inaccurate.
    Low = 0,
    /// Medium confidence - likely accurate.
    Medium = 1,
    /// High confidence - validated or from authoritative source.
    High = 2,
}

/// A captured reference price for a trading window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowReference {
    /// Start time of the window in milliseconds since epoch.
    pub window_start_ms: i64,
    /// End time of the window in milliseconds since epoch.
    pub window_end_ms: i64,
    /// The reference price ("price to beat").
    pub reference_price: f64,
    /// How the reference was obtained.
    pub source: ReferenceSource,
    /// Confidence in this reference.
    pub confidence: ReferenceConfidence,
    /// When the reference was captured.
    pub captured_at_ms: i64,
    /// Delay from window start to capture (ms).
    pub capture_delay_ms: i64,
}

impl WindowReference {
    /// Creates a new window reference.
    #[must_use]
    pub fn new(
        window_start_ms: i64,
        reference_price: f64,
        source: ReferenceSource,
        captured_at_ms: i64,
    ) -> Self {
        let capture_delay_ms = captured_at_ms - window_start_ms;
        let confidence = if capture_delay_ms > 5000 {
            // Captured more than 5 seconds after window start
            ReferenceConfidence::Low
        } else if capture_delay_ms > 1000 {
            ReferenceConfidence::Medium
        } else {
            source.default_confidence()
        };

        Self {
            window_start_ms,
            window_end_ms: window_start_ms + WINDOW_DURATION_MS,
            reference_price,
            source,
            confidence,
            captured_at_ms,
            capture_delay_ms,
        }
    }

    /// Creates a reference with explicit confidence override.
    #[must_use]
    pub fn with_confidence(mut self, confidence: ReferenceConfidence) -> Self {
        self.confidence = confidence;
        self
    }

    /// Returns time remaining in this window (in milliseconds).
    #[must_use]
    pub fn time_remaining_ms(&self, current_time_ms: i64) -> i64 {
        (self.window_end_ms - current_time_ms).max(0)
    }

    /// Returns time elapsed since window start (in milliseconds).
    #[must_use]
    pub fn time_elapsed_ms(&self, current_time_ms: i64) -> i64 {
        (current_time_ms - self.window_start_ms).max(0)
    }

    /// Returns true if the window is still active.
    #[must_use]
    pub fn is_active(&self, current_time_ms: i64) -> bool {
        current_time_ms >= self.window_start_ms && current_time_ms < self.window_end_ms
    }

    /// Calculates the price change from reference (as a ratio).
    ///
    /// Positive = above reference (YES winning)
    /// Negative = below reference (NO winning)
    #[must_use]
    pub fn price_change_ratio(&self, current_price: f64) -> f64 {
        (current_price - self.reference_price) / self.reference_price
    }

    /// Returns true if current price is above reference (YES winning).
    #[must_use]
    pub fn is_above_reference(&self, current_price: f64) -> bool {
        current_price > self.reference_price
    }
}

/// Configuration for the reference tracker.
#[derive(Debug, Clone)]
pub struct ReferenceTrackerConfig {
    /// Maximum delay (ms) to accept for a reference capture.
    pub max_capture_delay_ms: i64,
    /// Number of prices to use for VWAP calculation.
    pub vwap_sample_count: usize,
    /// Time window (ms) for VWAP calculation.
    pub vwap_window_ms: i64,
}

impl Default for ReferenceTrackerConfig {
    fn default() -> Self {
        Self {
            max_capture_delay_ms: 10_000, // 10 seconds
            vwap_sample_count: 10,
            vwap_window_ms: 2000, // 2 seconds
        }
    }
}

/// Tracks reference prices across trading windows.
#[derive(Debug)]
pub struct ReferenceTracker {
    config: ReferenceTrackerConfig,
    /// Current active reference.
    current_reference: Option<WindowReference>,
    /// Recent prices for VWAP calculation.
    recent_prices: VecDeque<(i64, f64)>, // (timestamp_ms, price)
    /// Historical references for validation.
    history: VecDeque<WindowReference>,
    /// Maximum history to keep.
    max_history: usize,
}

impl ReferenceTracker {
    /// Creates a new reference tracker.
    #[must_use]
    pub fn new(config: ReferenceTrackerConfig) -> Self {
        Self {
            config,
            current_reference: None,
            recent_prices: VecDeque::with_capacity(100),
            history: VecDeque::with_capacity(100),
            max_history: 100,
        }
    }

    /// Creates a tracker with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ReferenceTrackerConfig::default())
    }

    /// Returns the current active reference.
    #[must_use]
    pub fn current_reference(&self) -> Option<&WindowReference> {
        self.current_reference.as_ref()
    }

    /// Calculates the window start time for a given timestamp.
    #[must_use]
    pub fn window_start_for_time(timestamp_ms: i64) -> i64 {
        // Windows start at 00, 15, 30, 45 minutes
        let dt = DateTime::from_timestamp_millis(timestamp_ms)
            .unwrap_or_else(Utc::now);

        let minute = dt.minute();
        let window_minute = (minute / 15) * 15;

        dt.with_minute(window_minute)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .map(|d| d.timestamp_millis())
            .unwrap_or(timestamp_ms)
    }

    /// Updates with a new price observation.
    ///
    /// This should be called with every price update from Binance.
    pub fn update_price(&mut self, timestamp_ms: i64, price: f64) {
        // Add to recent prices
        self.recent_prices.push_back((timestamp_ms, price));

        // Keep only recent prices (last 10 seconds)
        while let Some(&(ts, _)) = self.recent_prices.front() {
            if timestamp_ms - ts > 10_000 {
                self.recent_prices.pop_front();
            } else {
                break;
            }
        }

        // Check if we need to capture a new reference
        let window_start = Self::window_start_for_time(timestamp_ms);

        let need_new_reference = match &self.current_reference {
            Some(ref_) => ref_.window_start_ms != window_start,
            None => true,
        };

        if need_new_reference {
            self.capture_reference(window_start, timestamp_ms);
        }
    }

    /// Captures a reference for a new window.
    fn capture_reference(&mut self, window_start_ms: i64, current_time_ms: i64) {
        let capture_delay = current_time_ms - window_start_ms;

        // Try to find the best price for this window start
        let (price, source) = if capture_delay <= self.config.max_capture_delay_ms {
            // We're close enough to window start - use first available price
            if let Some(price) = self.find_price_at_or_after(window_start_ms) {
                (price, ReferenceSource::BinanceFirstTrade)
            } else if let Some(vwap) = self.calculate_vwap(window_start_ms) {
                (vwap, ReferenceSource::BinanceVwap)
            } else if let Some(&(_, price)) = self.recent_prices.back() {
                (price, ReferenceSource::Interpolated)
            } else {
                warn!(
                    "No price available for reference at window {}",
                    window_start_ms
                );
                return;
            }
        } else {
            // Too late - we missed the window start
            warn!(
                "Missed window start by {}ms, using interpolated reference",
                capture_delay
            );
            if let Some(&(_, price)) = self.recent_prices.back() {
                (price, ReferenceSource::Interpolated)
            } else {
                return;
            }
        };

        let reference = WindowReference::new(
            window_start_ms,
            price,
            source,
            current_time_ms,
        );

        info!(
            window_start = window_start_ms,
            reference_price = format!("${:.2}", reference.reference_price),
            source = ?reference.source,
            confidence = ?reference.confidence,
            delay_ms = reference.capture_delay_ms,
            "Captured window reference"
        );

        // Archive old reference
        if let Some(old) = self.current_reference.take() {
            self.history.push_back(old);
            while self.history.len() > self.max_history {
                self.history.pop_front();
            }
        }

        self.current_reference = Some(reference);
    }

    /// Finds the first price at or after a timestamp.
    fn find_price_at_or_after(&self, timestamp_ms: i64) -> Option<f64> {
        self.recent_prices
            .iter()
            .find(|(ts, _)| *ts >= timestamp_ms)
            .map(|(_, price)| *price)
    }

    /// Calculates VWAP for prices around a timestamp.
    fn calculate_vwap(&self, around_timestamp_ms: i64) -> Option<f64> {
        let window_start = around_timestamp_ms;
        let window_end = around_timestamp_ms + self.config.vwap_window_ms;

        let prices: Vec<f64> = self.recent_prices
            .iter()
            .filter(|(ts, _)| *ts >= window_start && *ts <= window_end)
            .map(|(_, price)| *price)
            .collect();

        if prices.is_empty() {
            return None;
        }

        // Simple average (could be volume-weighted if we had volume)
        Some(prices.iter().sum::<f64>() / prices.len() as f64)
    }

    /// Sets a reference manually (for testing or from external source).
    pub fn set_reference(&mut self, reference: WindowReference) {
        info!(
            window_start = reference.window_start_ms,
            reference_price = format!("${:.2}", reference.reference_price),
            source = ?reference.source,
            "Reference set manually"
        );

        if let Some(old) = self.current_reference.take() {
            self.history.push_back(old);
        }

        self.current_reference = Some(reference);
    }

    /// Validates a reference against the actual outcome.
    ///
    /// Returns true if our reference would have predicted correctly.
    #[must_use]
    pub fn validate_outcome(
        &self,
        window_start_ms: i64,
        final_price: f64,
        actual_outcome_is_yes: bool,
    ) -> Option<bool> {
        // Find the reference for this window
        let reference = if self.current_reference.as_ref()
            .map(|r| r.window_start_ms == window_start_ms)
            .unwrap_or(false)
        {
            self.current_reference.as_ref()
        } else {
            self.history.iter().find(|r| r.window_start_ms == window_start_ms)
        };

        reference.map(|r| {
            let our_prediction_is_yes = final_price > r.reference_price;
            let correct = our_prediction_is_yes == actual_outcome_is_yes;

            debug!(
                window_start = window_start_ms,
                our_reference = format!("${:.2}", r.reference_price),
                final_price = format!("${:.2}", final_price),
                our_prediction = if our_prediction_is_yes { "YES" } else { "NO" },
                actual = if actual_outcome_is_yes { "YES" } else { "NO" },
                correct = correct,
                "Reference validation"
            );

            correct
        })
    }

    /// Returns the history of references.
    #[must_use]
    pub fn history(&self) -> &VecDeque<WindowReference> {
        &self.history
    }

    /// Clears all state.
    pub fn clear(&mut self) {
        self.current_reference = None;
        self.recent_prices.clear();
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_time(hour: u32, minute: u32, second: u32) -> i64 {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 2, 2, hour, minute, second)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn test_window_start_calculation() {
        // 15:00:00 -> 15:00:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 0, 0)),
            make_time(15, 0, 0)
        );

        // 15:07:30 -> 15:00:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 7, 30)),
            make_time(15, 0, 0)
        );

        // 15:15:00 -> 15:15:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 15, 0)),
            make_time(15, 15, 0)
        );

        // 15:29:59 -> 15:15:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 29, 59)),
            make_time(15, 15, 0)
        );

        // 15:30:00 -> 15:30:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 30, 0)),
            make_time(15, 30, 0)
        );

        // 15:45:30 -> 15:45:00
        assert_eq!(
            ReferenceTracker::window_start_for_time(make_time(15, 45, 30)),
            make_time(15, 45, 0)
        );
    }

    #[test]
    fn test_reference_creation() {
        let window_start = make_time(15, 0, 0);
        let captured_at = make_time(15, 0, 1); // 1 second delay

        let reference = WindowReference::new(
            window_start,
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            captured_at,
        );

        assert_eq!(reference.window_start_ms, window_start);
        assert_eq!(reference.reference_price, 78500.0);
        assert_eq!(reference.capture_delay_ms, 1000);
        assert_eq!(reference.confidence, ReferenceConfidence::Medium);
    }

    #[test]
    fn test_reference_confidence_by_delay() {
        let window_start = make_time(15, 0, 0);

        // Fast capture (< 1 second) - keeps source confidence
        let fast = WindowReference::new(
            window_start,
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            window_start + 500,
        );
        assert_eq!(fast.confidence, ReferenceConfidence::Medium);

        // Medium delay (1-5 seconds) - Medium confidence
        let medium = WindowReference::new(
            window_start,
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            window_start + 3000,
        );
        assert_eq!(medium.confidence, ReferenceConfidence::Medium);

        // Slow capture (> 5 seconds) - Low confidence
        let slow = WindowReference::new(
            window_start,
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            window_start + 6000,
        );
        assert_eq!(slow.confidence, ReferenceConfidence::Low);
    }

    #[test]
    fn test_time_remaining() {
        let window_start = make_time(15, 0, 0);
        let reference = WindowReference::new(
            window_start,
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            window_start,
        );

        // At window start: 15 minutes remaining
        assert_eq!(reference.time_remaining_ms(window_start), WINDOW_DURATION_MS);

        // At 15:07:30: 7.5 minutes remaining
        let mid_point = make_time(15, 7, 30);
        assert_eq!(reference.time_remaining_ms(mid_point), 7 * 60 * 1000 + 30 * 1000);

        // At window end: 0 remaining
        let end_time = make_time(15, 15, 0);
        assert_eq!(reference.time_remaining_ms(end_time), 0);

        // After window: still 0 (clamped)
        let after = make_time(15, 20, 0);
        assert_eq!(reference.time_remaining_ms(after), 0);
    }

    #[test]
    fn test_price_change_ratio() {
        let reference = WindowReference::new(
            make_time(15, 0, 0),
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            make_time(15, 0, 0),
        );

        // 1% above reference
        let ratio = reference.price_change_ratio(79285.0);
        assert!((ratio - 0.01).abs() < 0.0001);

        // 1% below reference
        let ratio = reference.price_change_ratio(77715.0);
        assert!((ratio - (-0.01)).abs() < 0.0001);

        // Exactly at reference
        let ratio = reference.price_change_ratio(78500.0);
        assert!(ratio.abs() < 0.0001);
    }

    #[test]
    fn test_is_above_reference() {
        let reference = WindowReference::new(
            make_time(15, 0, 0),
            78500.0,
            ReferenceSource::BinanceFirstTrade,
            make_time(15, 0, 0),
        );

        assert!(reference.is_above_reference(78501.0));
        assert!(!reference.is_above_reference(78499.0));
        assert!(!reference.is_above_reference(78500.0)); // Equal is not above
    }

    #[test]
    fn test_tracker_captures_reference() {
        let mut tracker = ReferenceTracker::with_defaults();

        let window_start = make_time(15, 0, 0);

        // Update with a price at window start
        tracker.update_price(window_start, 78500.0);

        let reference = tracker.current_reference().expect("Should have reference");
        assert_eq!(reference.window_start_ms, window_start);
        assert_eq!(reference.reference_price, 78500.0);
    }

    #[test]
    fn test_tracker_transitions_windows() {
        let mut tracker = ReferenceTracker::with_defaults();

        let window1_start = make_time(15, 0, 0);
        let window2_start = make_time(15, 15, 0);

        // First window
        tracker.update_price(window1_start, 78500.0);
        assert_eq!(
            tracker.current_reference().unwrap().window_start_ms,
            window1_start
        );

        // Still in first window
        tracker.update_price(make_time(15, 7, 30), 78600.0);
        assert_eq!(
            tracker.current_reference().unwrap().window_start_ms,
            window1_start
        );
        assert_eq!(tracker.current_reference().unwrap().reference_price, 78500.0);

        // Transition to second window
        tracker.update_price(window2_start, 78700.0);
        assert_eq!(
            tracker.current_reference().unwrap().window_start_ms,
            window2_start
        );
        assert_eq!(tracker.current_reference().unwrap().reference_price, 78700.0);

        // First window should be in history
        assert_eq!(tracker.history().len(), 1);
        assert_eq!(tracker.history()[0].window_start_ms, window1_start);
    }

    #[test]
    fn test_validate_outcome() {
        let mut tracker = ReferenceTracker::with_defaults();

        let window_start = make_time(15, 0, 0);
        tracker.update_price(window_start, 78500.0);

        // Price went up, YES won
        let correct = tracker.validate_outcome(window_start, 78600.0, true);
        assert_eq!(correct, Some(true));

        // Price went up, but NO won (reference was wrong!)
        let correct = tracker.validate_outcome(window_start, 78600.0, false);
        assert_eq!(correct, Some(false));

        // Price went down, NO won
        let correct = tracker.validate_outcome(window_start, 78400.0, false);
        assert_eq!(correct, Some(true));

        // Unknown window
        let unknown = make_time(16, 0, 0);
        assert_eq!(tracker.validate_outcome(unknown, 78500.0, true), None);
    }

    #[test]
    fn test_manual_reference() {
        let mut tracker = ReferenceTracker::with_defaults();

        let reference = WindowReference::new(
            make_time(15, 0, 0),
            78484.41, // Exact "price to beat" from Polymarket
            ReferenceSource::Manual,
            make_time(15, 0, 0),
        ).with_confidence(ReferenceConfidence::High);

        tracker.set_reference(reference);

        let current = tracker.current_reference().unwrap();
        assert_eq!(current.reference_price, 78484.41);
        assert_eq!(current.confidence, ReferenceConfidence::High);
    }
}
