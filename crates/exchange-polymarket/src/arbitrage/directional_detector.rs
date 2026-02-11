//! Single-leg directional trading detector.
//!
//! Per-coin direction detector that uses spot price confirmation against the
//! window reference price to generate directional trading signals.
//!
//! # Strategy
//!
//! Instead of correlation arbitrage (buy both legs), this detects single-leg
//! directional bets with favorable risk/reward:
//!
//! 1. Track spot price vs window reference ("price to beat")
//! 2. If spot is above reference → buy YES (BTC going up)
//! 3. If spot is below reference → buy NO (BTC going down)
//! 4. Only enter when delta is sufficient and entry price gives positive edge
//!
//! At $0.45/share with 55% win rate → +$0.089 EV/share.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Direction of a directional trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Buy YES — spot is above reference, expecting UP resolution.
    Up,
    /// Buy NO — spot is below reference, expecting DOWN resolution.
    Down,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Up => write!(f, "UP"),
            Direction::Down => write!(f, "DOWN"),
        }
    }
}

/// Configuration for the directional detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalConfig {
    /// Minimum spot-vs-reference delta to consider (e.g., 0.0005 = 0.05%).
    pub min_delta_pct: f64,

    /// Delta cap for confidence scaling (e.g., 0.03 = 3%).
    pub max_delta_pct: f64,

    /// Maximum price to pay for entry (e.g., 0.55).
    pub max_entry_price: Decimal,

    /// Minimum estimated edge to signal (e.g., 0.03 = 3%).
    pub min_edge: f64,

    /// Earliest entry: seconds before window close to START trading.
    pub entry_window_start_secs: i64,

    /// Latest entry: seconds before window close to STOP trading.
    pub entry_window_end_secs: i64,

    /// Per-coin signal cooldown in milliseconds.
    pub signal_cooldown_ms: i64,
}

impl Default for DirectionalConfig {
    fn default() -> Self {
        Self {
            min_delta_pct: 0.0005,       // 0.05%
            max_delta_pct: 0.03,         // 3%
            max_entry_price: dec!(0.55),
            min_edge: 0.03,              // 3%
            entry_window_start_secs: 600, // 10 min before close
            entry_window_end_secs: 120,   // 2 min before close
            signal_cooldown_ms: 30_000,   // 30s cooldown
        }
    }
}

/// A directional trading signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalSignal {
    /// Coin this signal is for (e.g., "btc").
    pub coin: String,

    /// Direction of the trade.
    pub direction: Direction,

    /// Token ID to buy (YES token if Up, NO token if Down).
    pub entry_token_id: String,

    /// Best ask price for the relevant side.
    pub entry_price: Decimal,

    /// Current spot price.
    pub spot_price: f64,

    /// Window reference price ("price to beat").
    pub reference_price: f64,

    /// Spot delta vs reference (signed percentage).
    pub delta_pct: f64,

    /// Confidence level (0.0 to 1.0).
    pub confidence: f64,

    /// Estimated win probability (0.50 to 0.80).
    pub win_probability: f64,

    /// Estimated edge (win_prob - entry_price).
    pub estimated_edge: f64,

    /// Time remaining in window (seconds).
    pub time_remaining_secs: i64,

    /// Signal generation timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Per-coin directional detector.
#[derive(Debug)]
pub struct DirectionalDetector {
    config: DirectionalConfig,
    /// Last signal timestamp per coin (for cooldown).
    last_signal_ms: Option<i64>,
}

impl DirectionalDetector {
    /// Creates a new detector with the given config.
    #[must_use]
    pub fn new(config: DirectionalConfig) -> Self {
        Self {
            config,
            last_signal_ms: None,
        }
    }

    /// Creates a detector with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DirectionalConfig::default())
    }

    /// Returns the config.
    #[must_use]
    pub fn config(&self) -> &DirectionalConfig {
        &self.config
    }

    /// Resets the cooldown.
    pub fn reset_cooldown(&mut self) {
        self.last_signal_ms = None;
    }

    /// Checks for a directional trading signal.
    ///
    /// # Arguments
    /// * `coin` - Coin name (e.g., "btc")
    /// * `spot_price` - Current spot price from Binance
    /// * `reference_price` - Window opening reference price
    /// * `yes_ask` - Best ask for YES token
    /// * `no_ask` - Best ask for NO token
    /// * `yes_token_id` - YES token ID
    /// * `no_token_id` - NO token ID
    /// * `time_remaining_secs` - Seconds until window closes
    /// * `timestamp_ms` - Current timestamp in milliseconds
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &mut self,
        coin: &str,
        spot_price: f64,
        reference_price: f64,
        yes_ask: Decimal,
        no_ask: Decimal,
        yes_token_id: &str,
        no_token_id: &str,
        time_remaining_secs: i64,
        timestamp_ms: i64,
    ) -> Option<DirectionalSignal> {
        // Check cooldown
        if let Some(last) = self.last_signal_ms {
            if timestamp_ms - last < self.config.signal_cooldown_ms {
                return None;
            }
        }

        // Check entry window timing
        if time_remaining_secs > self.config.entry_window_start_secs {
            return None; // Too early
        }
        if time_remaining_secs < self.config.entry_window_end_secs {
            return None; // Too late
        }

        // Calculate delta
        if reference_price <= 0.0 {
            return None;
        }
        let delta_pct = (spot_price - reference_price) / reference_price;

        // Check minimum delta
        if delta_pct.abs() < self.config.min_delta_pct {
            return None;
        }

        // Determine direction and entry price
        let (direction, entry_price, entry_token_id) = if delta_pct > 0.0 {
            (Direction::Up, yes_ask, yes_token_id.to_string())
        } else {
            (Direction::Down, no_ask, no_token_id.to_string())
        };

        // Check max entry price
        if entry_price > self.config.max_entry_price {
            return None;
        }

        // Calculate confidence: scales linearly with delta up to max_delta_pct
        let confidence = (delta_pct.abs() / self.config.max_delta_pct).min(1.0);

        // Win probability: 50% + (confidence * 30%) → range [50%, 80%]
        let win_probability = 0.50 + (confidence * 0.30);

        // Edge: win_prob - entry_price
        let entry_price_f64 = entry_price.to_string().parse::<f64>().unwrap_or(0.5);
        let estimated_edge = win_probability - entry_price_f64;

        // Check minimum edge
        if estimated_edge < self.config.min_edge {
            return None;
        }

        let timestamp = DateTime::from_timestamp_millis(timestamp_ms)?;

        // Update cooldown
        self.last_signal_ms = Some(timestamp_ms);

        Some(DirectionalSignal {
            coin: coin.to_string(),
            direction,
            entry_token_id,
            entry_price,
            spot_price,
            reference_price,
            delta_pct,
            confidence,
            win_probability,
            estimated_edge,
            time_remaining_secs,
            timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_time(minute: i64, second: i64) -> i64 {
        minute * 60 * 1000 + second * 1000
    }

    // =========================================================================
    // Direction Tests
    // =========================================================================

    #[test]
    fn test_direction_display() {
        assert_eq!(Direction::Up.to_string(), "UP");
        assert_eq!(Direction::Down.to_string(), "DOWN");
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = DirectionalConfig::default();
        assert!((config.min_delta_pct - 0.0005).abs() < 0.0001);
        assert!((config.max_delta_pct - 0.03).abs() < 0.001);
        assert_eq!(config.max_entry_price, dec!(0.55));
        assert!((config.min_edge - 0.03).abs() < 0.001);
        assert_eq!(config.entry_window_start_secs, 600);
        assert_eq!(config.entry_window_end_secs, 120);
        assert_eq!(config.signal_cooldown_ms, 30_000);
    }

    // =========================================================================
    // Up Signal Tests
    // =========================================================================

    #[test]
    fn test_up_signal_btc_above_reference() {
        let mut detector = DirectionalDetector::with_defaults();

        // BTC at $79k, reference $78.5k → +0.64% delta → Up
        let signal = detector.check(
            "btc",
            79_000.0,
            78_500.0,
            dec!(0.45), // YES ask
            dec!(0.55), // NO ask
            "yes-token",
            "no-token",
            300, // 5 min remaining (within entry window)
            make_time(10, 0),
        );

        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.coin, "btc");
        assert_eq!(s.direction, Direction::Up);
        assert_eq!(s.entry_token_id, "yes-token");
        assert_eq!(s.entry_price, dec!(0.45));
        assert!(s.delta_pct > 0.0);
        assert!(s.confidence > 0.0);
        assert!(s.win_probability > 0.50);
        assert!(s.estimated_edge > 0.0);
    }

    // =========================================================================
    // Down Signal Tests
    // =========================================================================

    #[test]
    fn test_down_signal_btc_below_reference() {
        let mut detector = DirectionalDetector::with_defaults();

        // BTC at $78k, reference $78.5k → -0.64% delta → Down
        let signal = detector.check(
            "btc",
            78_000.0,
            78_500.0,
            dec!(0.55), // YES ask
            dec!(0.45), // NO ask
            "yes-token",
            "no-token",
            300,
            make_time(10, 0),
        );

        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.direction, Direction::Down);
        assert_eq!(s.entry_token_id, "no-token");
        assert_eq!(s.entry_price, dec!(0.45));
        assert!(s.delta_pct < 0.0);
    }

    // =========================================================================
    // Filter Tests
    // =========================================================================

    #[test]
    fn test_no_signal_delta_below_min() {
        let mut detector = DirectionalDetector::with_defaults();

        // Delta ~0.013% < 0.05% min
        let signal = detector.check(
            "btc",
            78_510.0,
            78_500.0,
            dec!(0.45),
            dec!(0.55),
            "yes-token",
            "no-token",
            300,
            make_time(10, 0),
        );

        assert!(signal.is_none());
    }

    #[test]
    fn test_no_signal_entry_price_too_high() {
        let config = DirectionalConfig {
            max_entry_price: dec!(0.40),
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        let signal = detector.check(
            "btc",
            79_000.0,
            78_500.0,
            dec!(0.45), // Above 0.40 max
            dec!(0.55),
            "yes-token",
            "no-token",
            300,
            make_time(10, 0),
        );

        assert!(signal.is_none());
    }

    #[test]
    fn test_no_signal_edge_too_low() {
        let config = DirectionalConfig {
            min_edge: 0.20, // Require 20% edge (very high)
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        let signal = detector.check(
            "btc",
            79_000.0,
            78_500.0,
            dec!(0.50), // 50c entry — edge would be ~0.06, below 0.20
            dec!(0.50),
            "yes-token",
            "no-token",
            300,
            make_time(10, 0),
        );

        assert!(signal.is_none());
    }

    #[test]
    fn test_no_signal_too_early() {
        let mut detector = DirectionalDetector::with_defaults();

        // 800 seconds remaining > 600 entry_window_start_secs
        let signal = detector.check(
            "btc",
            79_000.0,
            78_500.0,
            dec!(0.45),
            dec!(0.55),
            "yes-token",
            "no-token",
            800, // Too early
            make_time(3, 20),
        );

        assert!(signal.is_none());
    }

    #[test]
    fn test_no_signal_too_late() {
        let mut detector = DirectionalDetector::with_defaults();

        // 60 seconds remaining < 120 entry_window_end_secs
        let signal = detector.check(
            "btc",
            79_000.0,
            78_500.0,
            dec!(0.45),
            dec!(0.55),
            "yes-token",
            "no-token",
            60, // Too late
            make_time(14, 0),
        );

        assert!(signal.is_none());
    }

    // =========================================================================
    // Cooldown Tests
    // =========================================================================

    #[test]
    fn test_signal_cooldown() {
        let config = DirectionalConfig {
            signal_cooldown_ms: 10_000,
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        let signal1 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            300, make_time(10, 0),
        );
        assert!(signal1.is_some());

        // 5 seconds later — blocked by cooldown
        let signal2 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            295, make_time(10, 5),
        );
        assert!(signal2.is_none());
    }

    #[test]
    fn test_cooldown_expires() {
        let config = DirectionalConfig {
            signal_cooldown_ms: 5_000,
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        let signal1 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            300, make_time(10, 0),
        );
        assert!(signal1.is_some());

        // 10 seconds later — cooldown expired
        let signal2 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            290, make_time(10, 10),
        );
        assert!(signal2.is_some());
    }

    #[test]
    fn test_reset_cooldown() {
        let config = DirectionalConfig {
            signal_cooldown_ms: 60_000,
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        let signal1 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            300, make_time(10, 0),
        );
        assert!(signal1.is_some());

        detector.reset_cooldown();

        // Immediate signal after reset
        let signal2 = detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "yes-token", "no-token",
            299, make_time(10, 1),
        );
        assert!(signal2.is_some());
    }

    // =========================================================================
    // Confidence / Probability Tests
    // =========================================================================

    #[test]
    fn test_confidence_scales_with_delta() {
        let mut detector = DirectionalDetector::with_defaults();

        // Small delta: 0.1% on 3% max → confidence ~0.033
        let signal = detector.check(
            "btc", 78_578.5, 78_500.0, // +0.1%
            dec!(0.40), dec!(0.60),
            "yes-token", "no-token",
            300, make_time(10, 0),
        );
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert!(s.confidence < 0.1);
        assert!(s.win_probability > 0.50);
        assert!(s.win_probability < 0.55);
    }

    #[test]
    fn test_confidence_caps_at_one() {
        let config = DirectionalConfig {
            signal_cooldown_ms: 0,
            ..DirectionalConfig::default()
        };
        let mut detector = DirectionalDetector::new(config);

        // Huge delta: 5% on 3% max → confidence capped at 1.0
        let signal = detector.check(
            "btc", 82_425.0, 78_500.0, // +5%
            dec!(0.30), dec!(0.70),
            "yes-token", "no-token",
            300, make_time(10, 0),
        );
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert!((s.confidence - 1.0).abs() < 0.001);
        assert!((s.win_probability - 0.80).abs() < 0.001);
    }

    // =========================================================================
    // Multi-Coin Test
    // =========================================================================

    #[test]
    fn test_multi_coin_signals() {
        let config = DirectionalConfig {
            signal_cooldown_ms: 0, // No cooldown for this test
            ..DirectionalConfig::default()
        };
        let mut btc_detector = DirectionalDetector::new(config.clone());
        let mut eth_detector = DirectionalDetector::new(config);

        let btc_signal = btc_detector.check(
            "btc", 79_000.0, 78_500.0,
            dec!(0.45), dec!(0.55),
            "btc-yes", "btc-no",
            300, make_time(10, 0),
        );

        let eth_signal = eth_detector.check(
            "eth", 2_050.0, 2_000.0,
            dec!(0.42), dec!(0.58),
            "eth-yes", "eth-no",
            300, make_time(10, 0),
        );

        assert!(btc_signal.is_some());
        assert!(eth_signal.is_some());
        assert_eq!(btc_signal.unwrap().coin, "btc");
        assert_eq!(eth_signal.unwrap().coin, "eth");
    }
}
