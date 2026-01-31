//! Momentum exhaustion signal generator.
//!
//! Detects stalling after big price moves, providing a contrarian reversal signal.
//! When price makes a big move but then stalls (range compression), momentum
//! may be exhausted and a reversal is likely.

use algo_trade_core::{Direction, OhlcvCandle, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

/// Configuration for momentum exhaustion detection.
#[derive(Debug, Clone)]
pub struct MomentumExhaustionConfig {
    /// Minimum percentage move to be considered a "big move" (e.g., 0.02 = 2%)
    pub big_move_threshold: f64,
    /// Number of candles to look back for detecting big moves
    pub big_move_lookback: usize,
    /// Ratio of recent range to big move range to detect stall (e.g., 0.3 = 30%)
    pub stall_ratio: f64,
    /// Number of candles after big move to check for stall
    pub stall_lookback: usize,
    /// Minimum candles required for valid signal
    pub min_candles: usize,
}

impl Default for MomentumExhaustionConfig {
    fn default() -> Self {
        Self {
            big_move_threshold: 0.02, // 2% move
            big_move_lookback: 5,     // Look back 5 candles
            stall_ratio: 0.3,         // Stall if range < 30% of big move
            stall_lookback: 3,        // Check last 3 candles for stall
            min_candles: 8,           // Need at least 8 candles
        }
    }
}

/// Result of big move detection.
#[derive(Debug, Clone)]
pub struct BigMoveResult {
    /// Direction of the big move
    pub direction: Direction,
    /// Magnitude of the move as a percentage (e.g., 0.03 = 3%)
    pub magnitude: f64,
    /// Index of the candle where the big move occurred
    pub candle_index: usize,
}

/// Detects a big move in the candle data.
///
/// A big move is when the cumulative price change over the lookback period
/// exceeds the threshold.
///
/// # Arguments
/// * `candles` - OHLCV candles (most recent last)
/// * `threshold` - Minimum percentage move (e.g., 0.02 for 2%)
/// * `lookback` - Number of candles to analyze
///
/// # Returns
/// `Some(BigMoveResult)` if a big move is detected, `None` otherwise.
pub fn detect_big_move(
    candles: &[OhlcvCandle],
    threshold: f64,
    lookback: usize,
) -> Option<BigMoveResult> {
    if candles.len() < lookback + 1 {
        return None;
    }

    // Get the candles in the lookback window (excluding most recent stall window)
    let start_idx = candles.len().saturating_sub(lookback + 1);
    let end_idx = candles.len() - 1;

    if start_idx >= end_idx {
        return None;
    }

    let start_price = candles[start_idx].open;
    let end_price = candles[end_idx].close;

    if start_price.is_zero() {
        return None;
    }

    // Calculate percentage change
    let change = (end_price - start_price) / start_price;
    let change_f64: f64 = change.to_string().parse().ok()?;
    let magnitude = change_f64.abs();

    if magnitude >= threshold {
        let direction = if change_f64 > 0.0 {
            Direction::Up
        } else {
            Direction::Down
        };
        Some(BigMoveResult {
            direction,
            magnitude,
            candle_index: end_idx,
        })
    } else {
        None
    }
}

/// Detects a stall (range compression) after a big move.
///
/// A stall is when recent candles have significantly smaller ranges
/// compared to the big move candles.
///
/// # Arguments
/// * `candles` - OHLCV candles (most recent last)
/// * `stall_ratio` - Maximum ratio of recent range to big move range
/// * `stall_lookback` - Number of recent candles to check
/// * `big_move_lookback` - Number of candles in the big move period
///
/// # Returns
/// `true` if a stall is detected, `false` otherwise.
pub fn detect_stall(
    candles: &[OhlcvCandle],
    stall_ratio: f64,
    stall_lookback: usize,
    big_move_lookback: usize,
) -> bool {
    if candles.len() < stall_lookback + big_move_lookback {
        return false;
    }

    // Calculate average range of big move candles
    let big_move_start = candles
        .len()
        .saturating_sub(stall_lookback + big_move_lookback);
    let big_move_end = candles.len().saturating_sub(stall_lookback);

    if big_move_start >= big_move_end {
        return false;
    }

    let big_move_ranges: Vec<Decimal> = candles[big_move_start..big_move_end]
        .iter()
        .map(|c| c.range())
        .collect();

    let big_move_avg_range: Decimal =
        big_move_ranges.iter().copied().sum::<Decimal>() / Decimal::from(big_move_ranges.len());

    if big_move_avg_range.is_zero() {
        return false;
    }

    // Calculate average range of recent (stall) candles
    let stall_start = candles.len().saturating_sub(stall_lookback);
    let stall_ranges: Vec<Decimal> = candles[stall_start..].iter().map(|c| c.range()).collect();

    let stall_avg_range: Decimal =
        stall_ranges.iter().copied().sum::<Decimal>() / Decimal::from(stall_ranges.len());

    // Check if stall range is less than ratio of big move range
    let ratio = stall_avg_range / big_move_avg_range;
    let ratio_f64: f64 = ratio.to_string().parse().unwrap_or(1.0);

    ratio_f64 < stall_ratio
}

/// Detects momentum exhaustion (big move followed by stall).
///
/// When a big move is followed by range compression (stall), momentum
/// may be exhausted and a reversal is likely. Returns a contrarian signal.
///
/// # Arguments
/// * `candles` - OHLCV candles (most recent last)
/// * `config` - Configuration for detection thresholds
///
/// # Returns
/// `Some((direction, strength))` for the contrarian signal, `None` if no exhaustion.
pub fn detect_momentum_exhaustion(
    candles: &[OhlcvCandle],
    config: &MomentumExhaustionConfig,
) -> Option<(Direction, f64)> {
    if candles.len() < config.min_candles {
        return None;
    }

    // First, detect a big move
    let big_move = detect_big_move(candles, config.big_move_threshold, config.big_move_lookback)?;

    // Then, check for stall
    let is_stall = detect_stall(
        candles,
        config.stall_ratio,
        config.stall_lookback,
        config.big_move_lookback,
    );

    if is_stall {
        // Contrarian signal: opposite of the big move direction
        let signal_direction = big_move.direction.opposite();

        // Strength based on magnitude of the big move
        // Normalize: 2% move = 0.5 strength, 4% move = 1.0 strength
        let strength = (big_move.magnitude / (config.big_move_threshold * 2.0)).min(1.0);

        Some((signal_direction, strength))
    } else {
        None
    }
}

/// Signal generator based on momentum exhaustion detection.
///
/// Generates contrarian signals when a big price move is followed by
/// range compression (stall), indicating potential momentum exhaustion
/// and reversal.
#[derive(Debug, Clone)]
pub struct MomentumExhaustionSignal {
    /// Name of this signal
    name: String,
    /// Configuration for exhaustion detection
    config: MomentumExhaustionConfig,
    /// Weight for composite signal aggregation
    weight: f64,
}

impl Default for MomentumExhaustionSignal {
    fn default() -> Self {
        Self::new(MomentumExhaustionConfig::default(), 1.0)
    }
}

impl MomentumExhaustionSignal {
    /// Creates a new `MomentumExhaustionSignal`.
    ///
    /// # Arguments
    /// * `config` - Configuration for exhaustion detection
    /// * `weight` - Weight for composite signal aggregation
    #[must_use]
    pub fn new(config: MomentumExhaustionConfig, weight: f64) -> Self {
        Self {
            name: "momentum_exhaustion".to_string(),
            config,
            weight,
        }
    }

    /// Sets the big move threshold.
    #[must_use]
    pub fn with_big_move_threshold(mut self, threshold: f64) -> Self {
        self.config.big_move_threshold = threshold.abs();
        self
    }

    /// Sets the stall ratio.
    #[must_use]
    pub fn with_stall_ratio(mut self, ratio: f64) -> Self {
        self.config.stall_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// Sets lookback periods.
    #[must_use]
    pub fn with_lookbacks(mut self, big_move: usize, stall: usize) -> Self {
        self.config.big_move_lookback = big_move.max(1);
        self.config.stall_lookback = stall.max(1);
        self
    }

    /// Returns the current configuration.
    #[must_use]
    pub fn config(&self) -> &MomentumExhaustionConfig {
        &self.config
    }
}

#[async_trait]
impl SignalGenerator for MomentumExhaustionSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get OHLCV data from context
        let candles = match &ctx.historical_ohlcv {
            Some(c) if !c.is_empty() => c,
            _ => {
                tracing::debug!("No OHLCV data in context, returning neutral signal");
                return Ok(SignalValue::neutral());
            }
        };

        // Detect momentum exhaustion
        let result = detect_momentum_exhaustion(candles, &self.config);

        match result {
            Some((direction, strength)) => {
                let mut signal = SignalValue::new(direction, strength, 0.0)?
                    .with_metadata("big_move_threshold", self.config.big_move_threshold)
                    .with_metadata("stall_ratio", self.config.stall_ratio);

                // Add big move info if available
                if let Some(big_move) = detect_big_move(
                    candles,
                    self.config.big_move_threshold,
                    self.config.big_move_lookback,
                ) {
                    signal = signal
                        .with_metadata("big_move_magnitude", big_move.magnitude)
                        .with_metadata(
                            "big_move_direction",
                            match big_move.direction {
                                Direction::Up => 1.0,
                                Direction::Down => -1.0,
                                Direction::Neutral => 0.0,
                            },
                        );
                }

                Ok(signal)
            }
            None => Ok(SignalValue::neutral()),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn weight(&self) -> f64 {
        self.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    // ============================================
    // Test Helpers
    // ============================================

    fn make_candle(open: i64, high: i64, low: i64, close: i64) -> OhlcvCandle {
        OhlcvCandle {
            timestamp: Utc::now(),
            open: Decimal::new(open, 0),
            high: Decimal::new(high, 0),
            low: Decimal::new(low, 0),
            close: Decimal::new(close, 0),
            volume: dec!(1000),
        }
    }

    // ============================================
    // OhlcvCandle Tests
    // ============================================

    #[test]
    fn candle_range_calculates_correctly() {
        let candle = make_candle(100, 110, 95, 105);
        assert_eq!(candle.range(), dec!(15)); // 110 - 95
    }

    #[test]
    fn candle_body_calculates_correctly() {
        let candle = make_candle(100, 110, 95, 105);
        assert_eq!(candle.body(), dec!(5)); // |105 - 100|

        let bearish = make_candle(105, 110, 95, 100);
        assert_eq!(bearish.body(), dec!(5)); // |100 - 105|
    }

    #[test]
    fn candle_change_calculates_correctly() {
        let bullish = make_candle(100, 110, 95, 105);
        assert_eq!(bullish.change(), dec!(5)); // 105 - 100

        let bearish = make_candle(105, 110, 95, 100);
        assert_eq!(bearish.change(), dec!(-5)); // 100 - 105
    }

    #[test]
    fn candle_is_bullish_when_close_above_open() {
        let bullish = make_candle(100, 110, 95, 105);
        assert!(bullish.is_bullish());
        assert!(!bullish.is_bearish());
    }

    #[test]
    fn candle_is_bearish_when_close_below_open() {
        let bearish = make_candle(105, 110, 95, 100);
        assert!(bearish.is_bearish());
        assert!(!bearish.is_bullish());
    }

    // ============================================
    // Config Tests
    // ============================================

    #[test]
    fn config_default_values() {
        let config = MomentumExhaustionConfig::default();

        assert!((config.big_move_threshold - 0.02).abs() < f64::EPSILON);
        assert_eq!(config.big_move_lookback, 5);
        assert!((config.stall_ratio - 0.3).abs() < f64::EPSILON);
        assert_eq!(config.stall_lookback, 3);
        assert_eq!(config.min_candles, 8);
    }

    #[test]
    fn config_custom_values() {
        let config = MomentumExhaustionConfig {
            big_move_threshold: 0.03,
            big_move_lookback: 10,
            stall_ratio: 0.25,
            stall_lookback: 4,
            min_candles: 15,
        };

        assert!((config.big_move_threshold - 0.03).abs() < f64::EPSILON);
        assert_eq!(config.big_move_lookback, 10);
        assert!((config.stall_ratio - 0.25).abs() < f64::EPSILON);
        assert_eq!(config.stall_lookback, 4);
        assert_eq!(config.min_candles, 15);
    }

    // ============================================
    // Big Move Detection Tests
    // ============================================

    #[test]
    fn detect_big_move_up_above_threshold() {
        // Start at 100, end at 103 = 3% up move
        let candles = vec![
            make_candle(100, 101, 99, 100),
            make_candle(100, 102, 99, 101),
            make_candle(101, 103, 100, 102),
            make_candle(102, 104, 101, 103),
            make_candle(103, 105, 102, 103), // Final close at 103
        ];

        let result = detect_big_move(&candles, 0.02, 4); // 2% threshold

        assert!(result.is_some());
        let big_move = result.unwrap();
        assert_eq!(big_move.direction, Direction::Up);
        assert!(
            big_move.magnitude >= 0.02,
            "magnitude was {}",
            big_move.magnitude
        );
    }

    #[test]
    fn detect_big_move_down_above_threshold() {
        // Start at 100, end at 97 = 3% down move
        let candles = vec![
            make_candle(100, 101, 99, 100),
            make_candle(100, 101, 98, 99),
            make_candle(99, 100, 97, 98),
            make_candle(98, 99, 96, 97),
            make_candle(97, 98, 96, 97), // Final close at 97
        ];

        let result = detect_big_move(&candles, 0.02, 4);

        assert!(result.is_some());
        let big_move = result.unwrap();
        assert_eq!(big_move.direction, Direction::Down);
        assert!(big_move.magnitude >= 0.02);
    }

    #[test]
    fn no_big_move_below_threshold() {
        // Start at 100, end at 101 = 1% move (below 2% threshold)
        let candles = vec![
            make_candle(100, 101, 99, 100),
            make_candle(100, 101, 99, 100),
            make_candle(100, 102, 99, 101),
            make_candle(101, 102, 100, 101),
            make_candle(101, 102, 100, 101),
        ];

        let result = detect_big_move(&candles, 0.02, 4);

        assert!(result.is_none());
    }

    #[test]
    fn no_big_move_insufficient_candles() {
        let candles = vec![
            make_candle(100, 105, 95, 103),
            make_candle(103, 108, 100, 106),
        ];

        let result = detect_big_move(&candles, 0.02, 5);

        assert!(result.is_none());
    }

    // ============================================
    // Stall Detection Tests
    // ============================================

    #[test]
    fn detect_stall_after_big_move() {
        // Big move candles with large ranges (20 each)
        // Followed by stall candles with small ranges (5 each)
        let candles = vec![
            // Big move period (large ranges)
            make_candle(100, 120, 100, 115), // range = 20
            make_candle(115, 135, 115, 130), // range = 20
            make_candle(130, 150, 130, 145), // range = 20
            make_candle(145, 165, 145, 160), // range = 20
            make_candle(160, 180, 160, 175), // range = 20
            // Stall period (small ranges)
            make_candle(175, 178, 173, 176), // range = 5
            make_candle(176, 179, 174, 177), // range = 5
            make_candle(177, 180, 175, 178), // range = 5
        ];

        let is_stall = detect_stall(&candles, 0.3, 3, 5);

        // 5/20 = 0.25 which is < 0.3, so should be a stall
        assert!(is_stall);
    }

    #[test]
    fn no_stall_when_momentum_continues() {
        // All candles have similar large ranges
        let candles = vec![
            make_candle(100, 120, 100, 115), // range = 20
            make_candle(115, 135, 115, 130), // range = 20
            make_candle(130, 150, 130, 145), // range = 20
            make_candle(145, 165, 145, 160), // range = 20
            make_candle(160, 180, 160, 175), // range = 20
            // Recent candles also have large ranges
            make_candle(175, 195, 175, 190), // range = 20
            make_candle(190, 210, 190, 205), // range = 20
            make_candle(205, 225, 205, 220), // range = 20
        ];

        let is_stall = detect_stall(&candles, 0.3, 3, 5);

        // 20/20 = 1.0 which is > 0.3, so not a stall
        assert!(!is_stall);
    }

    #[test]
    fn no_stall_insufficient_candles() {
        let candles = vec![
            make_candle(100, 120, 100, 115),
            make_candle(115, 120, 113, 116),
        ];

        let is_stall = detect_stall(&candles, 0.3, 3, 5);

        assert!(!is_stall);
    }

    // ============================================
    // Momentum Exhaustion Tests
    // ============================================

    #[test]
    fn exhaustion_bearish_after_big_rise_and_stall() {
        // Big upward move followed by stall -> bearish reversal signal
        let candles = vec![
            // Big move up (large ranges, trending up)
            make_candle(100, 120, 100, 115), // range = 20
            make_candle(115, 135, 115, 130), // range = 20
            make_candle(130, 150, 130, 145), // range = 20
            make_candle(145, 165, 145, 160), // range = 20
            make_candle(160, 180, 160, 175), // range = 20, close at 175 (75% up from 100)
            // Stall period (small ranges)
            make_candle(175, 178, 173, 176), // range = 5
            make_candle(176, 179, 174, 177), // range = 5
            make_candle(177, 180, 175, 178), // range = 5
        ];

        let config = MomentumExhaustionConfig {
            big_move_threshold: 0.10, // 10% threshold (we moved 75%)
            big_move_lookback: 5,
            stall_ratio: 0.3,
            stall_lookback: 3,
            min_candles: 8,
        };

        let result = detect_momentum_exhaustion(&candles, &config);

        assert!(result.is_some(), "Expected exhaustion signal");
        let (direction, strength) = result.unwrap();
        // Big move was up, so contrarian signal should be down
        assert_eq!(direction, Direction::Down);
        assert!(strength > 0.0 && strength <= 1.0);
    }

    #[test]
    fn exhaustion_bullish_after_big_drop_and_stall() {
        // Big downward move followed by stall -> bullish reversal signal
        let candles = vec![
            // Big move down (large ranges, trending down)
            make_candle(175, 180, 160, 160), // range = 20
            make_candle(160, 165, 145, 145), // range = 20
            make_candle(145, 150, 130, 130), // range = 20
            make_candle(130, 135, 115, 115), // range = 20
            make_candle(115, 120, 100, 100), // range = 20, close at 100 (down from 175)
            // Stall period (small ranges)
            make_candle(100, 103, 98, 101),  // range = 5
            make_candle(101, 104, 99, 102),  // range = 5
            make_candle(102, 105, 100, 103), // range = 5
        ];

        let config = MomentumExhaustionConfig {
            big_move_threshold: 0.10, // 10% threshold
            big_move_lookback: 5,
            stall_ratio: 0.3,
            stall_lookback: 3,
            min_candles: 8,
        };

        let result = detect_momentum_exhaustion(&candles, &config);

        assert!(result.is_some(), "Expected exhaustion signal");
        let (direction, strength) = result.unwrap();
        // Big move was down, so contrarian signal should be up
        assert_eq!(direction, Direction::Up);
        assert!(strength > 0.0 && strength <= 1.0);
    }

    #[test]
    fn no_exhaustion_without_stall() {
        // Big move but momentum continues (no stall)
        let candles = vec![
            make_candle(100, 120, 100, 115),
            make_candle(115, 135, 115, 130),
            make_candle(130, 150, 130, 145),
            make_candle(145, 165, 145, 160),
            make_candle(160, 180, 160, 175),
            // Momentum continues (large ranges)
            make_candle(175, 195, 175, 190),
            make_candle(190, 210, 190, 205),
            make_candle(205, 225, 205, 220),
        ];

        let config = MomentumExhaustionConfig {
            big_move_threshold: 0.10,
            big_move_lookback: 5,
            stall_ratio: 0.3,
            stall_lookback: 3,
            min_candles: 8,
        };

        let result = detect_momentum_exhaustion(&candles, &config);

        assert!(
            result.is_none(),
            "Should not detect exhaustion without stall"
        );
    }

    #[test]
    fn no_exhaustion_without_big_move() {
        // Small moves only (no big move to exhaust)
        let candles = vec![
            make_candle(100, 102, 99, 101),
            make_candle(101, 103, 100, 102),
            make_candle(102, 104, 101, 103),
            make_candle(103, 105, 102, 104),
            make_candle(104, 106, 103, 105),
            make_candle(105, 107, 104, 106),
            make_candle(106, 108, 105, 107),
            make_candle(107, 109, 106, 108),
        ];

        let config = MomentumExhaustionConfig {
            big_move_threshold: 0.10, // 10% threshold (moves are only ~1%)
            big_move_lookback: 5,
            stall_ratio: 0.3,
            stall_lookback: 3,
            min_candles: 8,
        };

        let result = detect_momentum_exhaustion(&candles, &config);

        assert!(
            result.is_none(),
            "Should not detect exhaustion without big move"
        );
    }

    #[test]
    fn no_exhaustion_insufficient_candles() {
        let candles = vec![
            make_candle(100, 120, 100, 115),
            make_candle(115, 118, 113, 116),
        ];

        let config = MomentumExhaustionConfig::default();

        let result = detect_momentum_exhaustion(&candles, &config);

        assert!(result.is_none());
    }

    // ============================================
    // SignalGenerator Tests
    // ============================================

    #[tokio::test]
    async fn compute_returns_neutral_without_ohlcv() {
        let mut signal = MomentumExhaustionSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
        assert!((result.strength - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn compute_returns_bearish_on_up_exhaustion() {
        let mut signal = MomentumExhaustionSignal::new(
            MomentumExhaustionConfig {
                big_move_threshold: 0.10,
                big_move_lookback: 5,
                stall_ratio: 0.3,
                stall_lookback: 3,
                min_candles: 8,
            },
            1.0,
        );

        // Big upward move followed by stall
        let candles = vec![
            make_candle(100, 120, 100, 115),
            make_candle(115, 135, 115, 130),
            make_candle(130, 150, 130, 145),
            make_candle(145, 165, 145, 160),
            make_candle(160, 180, 160, 175),
            make_candle(175, 178, 173, 176),
            make_candle(176, 179, 174, 177),
            make_candle(177, 180, 175, 178),
        ];

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_historical_ohlcv(candles);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Down);
        assert!(result.strength > 0.0);
        assert!(result.metadata.contains_key("big_move_threshold"));
        assert!(result.metadata.contains_key("stall_ratio"));
    }

    #[tokio::test]
    async fn compute_returns_bullish_on_down_exhaustion() {
        let mut signal = MomentumExhaustionSignal::new(
            MomentumExhaustionConfig {
                big_move_threshold: 0.10,
                big_move_lookback: 5,
                stall_ratio: 0.3,
                stall_lookback: 3,
                min_candles: 8,
            },
            1.0,
        );

        // Big downward move followed by stall
        let candles = vec![
            make_candle(175, 180, 160, 160),
            make_candle(160, 165, 145, 145),
            make_candle(145, 150, 130, 130),
            make_candle(130, 135, 115, 115),
            make_candle(115, 120, 100, 100),
            make_candle(100, 103, 98, 101),
            make_candle(101, 104, 99, 102),
            make_candle(102, 105, 100, 103),
        ];

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_historical_ohlcv(candles);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
        assert!(result.strength > 0.0);
    }

    #[test]
    fn signal_name_is_correct() {
        let signal = MomentumExhaustionSignal::default();
        assert_eq!(signal.name(), "momentum_exhaustion");
    }

    #[test]
    fn signal_weight_is_configurable() {
        let signal = MomentumExhaustionSignal::new(MomentumExhaustionConfig::default(), 2.5);
        assert!((signal.weight() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_builder_methods_work() {
        let signal = MomentumExhaustionSignal::default()
            .with_big_move_threshold(0.05)
            .with_stall_ratio(0.25)
            .with_lookbacks(10, 4);

        let config = signal.config();
        assert!((config.big_move_threshold - 0.05).abs() < f64::EPSILON);
        assert!((config.stall_ratio - 0.25).abs() < f64::EPSILON);
        assert_eq!(config.big_move_lookback, 10);
        assert_eq!(config.stall_lookback, 4);
    }

    #[tokio::test]
    async fn compute_returns_neutral_on_empty_ohlcv() {
        let mut signal = MomentumExhaustionSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_historical_ohlcv(vec![]);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn compute_includes_big_move_metadata() {
        let mut signal = MomentumExhaustionSignal::new(
            MomentumExhaustionConfig {
                big_move_threshold: 0.10,
                big_move_lookback: 5,
                stall_ratio: 0.3,
                stall_lookback: 3,
                min_candles: 8,
            },
            1.0,
        );

        let candles = vec![
            make_candle(100, 120, 100, 115),
            make_candle(115, 135, 115, 130),
            make_candle(130, 150, 130, 145),
            make_candle(145, 165, 145, 160),
            make_candle(160, 180, 160, 175),
            make_candle(175, 178, 173, 176),
            make_candle(176, 179, 174, 177),
            make_candle(177, 180, 175, 178),
        ];

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_historical_ohlcv(candles);

        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("big_move_magnitude"));
        assert!(result.metadata.contains_key("big_move_direction"));
    }
}
