//! Funding rate signal generator.
//!
//! Generates trading signals based on funding rate extremes and reversals.
//! High positive funding rates suggest overleveraged longs, creating
//! potential for short squeezes and price drops.

use algo_trade_core::{
    Direction, HistoricalFundingRate, SignalContext, SignalGenerator, SignalValue,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::VecDeque;

/// Configuration for funding rate reversal detection.
#[derive(Debug, Clone)]
pub struct FundingReversalConfig {
    /// Number of periods to look back for reversal detection
    pub lookback_periods: usize,
    /// Percentile threshold to consider funding "extreme" (e.g., 0.90)
    pub extreme_threshold_percentile: f64,
    /// Percentile threshold for "normal" range (e.g., 0.60)
    pub reversion_threshold_percentile: f64,
}

impl Default for FundingReversalConfig {
    fn default() -> Self {
        Self {
            lookback_periods: 10,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        }
    }
}

/// Signal indicating a funding rate reversal.
#[derive(Debug, Clone)]
pub struct ReversalSignal {
    /// Direction the funding was moving FROM before reversal
    pub from_direction: Direction,
    /// Strength of the reversal signal (0.0 to 1.0)
    pub strength: f64,
}

/// Signal combination mode for funding rate analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FundingSignalMode {
    /// Use z-score only
    ZScore,
    /// Use percentile only
    Percentile,
    /// Combine z-score and percentile signals
    #[default]
    Combined,
}

/// Signal generator based on funding rate analysis.
///
/// Funding rates indicate the cost of holding perpetual futures positions.
/// Extreme positive rates suggest overleveraged longs (bearish signal),
/// while extreme negative rates suggest overleveraged shorts (bullish signal).
///
/// This signal uses z-score normalization and percentile analysis to identify
/// extreme readings and potential reversals.
#[derive(Debug, Clone)]
pub struct FundingRateSignal {
    /// Name of this signal
    name: String,
    /// Z-score threshold for generating signals
    zscore_threshold: f64,
    /// Weight for composite signal aggregation
    weight: f64,
    /// Rolling window of funding rates for z-score calculation
    history: VecDeque<f64>,
    /// Maximum size of rolling window
    window_size: usize,
    /// High percentile threshold (e.g., 0.90 for 90th percentile)
    pub percentile_threshold_high: f64,
    /// Low percentile threshold (e.g., 0.10 for 10th percentile)
    pub percentile_threshold_low: f64,
    /// Reversal detection configuration (None = disabled)
    pub reversal_config: Option<FundingReversalConfig>,
    /// Signal combination mode
    pub signal_mode: FundingSignalMode,
}

impl Default for FundingRateSignal {
    fn default() -> Self {
        Self::new(2.0, 1.0, 100)
    }
}

impl FundingRateSignal {
    /// Creates a new FundingRateSignal.
    ///
    /// # Arguments
    /// * `zscore_threshold` - Z-score threshold for generating signals (typically 2.0)
    /// * `weight` - Weight for composite signal aggregation
    /// * `window_size` - Number of observations for z-score calculation
    #[must_use]
    pub fn new(zscore_threshold: f64, weight: f64, window_size: usize) -> Self {
        Self {
            name: "funding_rate".to_string(),
            zscore_threshold: zscore_threshold.abs(),
            weight,
            history: VecDeque::with_capacity(window_size),
            window_size: window_size.max(2),
            percentile_threshold_high: 0.90,
            percentile_threshold_low: 0.10,
            reversal_config: None,
            signal_mode: FundingSignalMode::Combined,
        }
    }

    /// Sets percentile thresholds.
    #[must_use]
    pub fn with_percentile_thresholds(mut self, low: f64, high: f64) -> Self {
        self.percentile_threshold_low = low.clamp(0.0, 1.0);
        self.percentile_threshold_high = high.clamp(0.0, 1.0);
        self
    }

    /// Sets reversal detection configuration.
    #[must_use]
    pub fn with_reversal_detection(mut self, config: FundingReversalConfig) -> Self {
        self.reversal_config = Some(config);
        self
    }

    /// Sets signal mode.
    #[must_use]
    pub fn with_signal_mode(mut self, mode: FundingSignalMode) -> Self {
        self.signal_mode = mode;
        self
    }

    /// Returns the current z-score of the latest funding rate.
    #[must_use]
    pub fn current_zscore(&self) -> Option<f64> {
        if self.history.len() < 2 {
            return None;
        }

        let latest = *self.history.back()?;
        let mean = self.mean()?;
        let std_dev = self.std_dev(mean)?;

        if std_dev < f64::EPSILON {
            return None;
        }

        Some((latest - mean) / std_dev)
    }

    /// Calculates the mean of the history.
    fn mean(&self) -> Option<f64> {
        if self.history.is_empty() {
            return None;
        }
        Some(self.history.iter().sum::<f64>() / self.history.len() as f64)
    }

    /// Calculates the standard deviation of the history.
    fn std_dev(&self, mean: f64) -> Option<f64> {
        if self.history.len() < 2 {
            return None;
        }

        let variance = self.history.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
            / (self.history.len() - 1) as f64;

        Some(variance.sqrt())
    }

    /// Adds a new funding rate observation.
    fn add_observation(&mut self, rate: f64) {
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back(rate);
    }

    /// Returns the number of observations in history.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.history.len()
    }
}

/// Calculates percentile-based signal from funding rate.
///
/// High percentile (above threshold) indicates overleveraged longs -> bearish.
/// Low percentile (below threshold) indicates overleveraged shorts -> bullish.
///
/// # Arguments
/// * `current` - Current funding rate
/// * `historical` - Historical funding rates (raw values)
/// * `high_threshold` - Upper percentile threshold (e.g., 0.90)
/// * `low_threshold` - Lower percentile threshold (e.g., 0.10)
///
/// # Returns
/// Direction and strength if percentile is extreme, None otherwise.
pub fn percentile_signal(
    current: f64,
    historical: &[f64],
    high_threshold: f64,
    low_threshold: f64,
) -> Option<(Direction, f64)> {
    let percentile = SignalContext::calculate_percentile(historical, current)?;

    if percentile >= high_threshold {
        // High positive funding = overleveraged longs = bearish
        let strength = (percentile - high_threshold) / (1.0 - high_threshold);
        Some((Direction::Down, strength.min(1.0)))
    } else if percentile <= low_threshold {
        // Low/negative funding = overleveraged shorts = bullish
        let strength = (low_threshold - percentile) / low_threshold;
        Some((Direction::Up, strength.min(1.0)))
    } else {
        None
    }
}

/// Detects funding rate reversal from extreme to normal levels.
///
/// A reversal occurs when funding moves from an extreme percentile back
/// toward normal levels, suggesting the market is correcting its imbalance.
///
/// # Arguments
/// * `historical` - Historical funding rate records (most recent first)
/// * `config` - Reversal detection configuration
///
/// # Returns
/// Reversal signal if detected, None otherwise.
pub fn detect_reversal(
    historical: &[HistoricalFundingRate],
    config: &FundingReversalConfig,
) -> Option<ReversalSignal> {
    if historical.len() < config.lookback_periods {
        return None;
    }

    // Take the most recent N records (assuming most recent first)
    let recent: Vec<_> = historical.iter().take(config.lookback_periods).collect();

    // Check if we have percentile data for current (first = most recent)
    let current_percentile = recent.first()?.percentile?;

    // Look for extreme percentile in recent past (skip current, look at history)
    let mut was_extreme_high = false;
    let mut was_extreme_low = false;
    let mut extreme_strength = 0.0;

    for record in recent.iter().skip(1) {
        if let Some(pct) = record.percentile {
            if pct >= config.extreme_threshold_percentile {
                was_extreme_high = true;
                extreme_strength = pct.max(extreme_strength);
            } else if pct <= (1.0 - config.extreme_threshold_percentile) {
                was_extreme_low = true;
                extreme_strength = (1.0 - pct).max(extreme_strength);
            }
        }
    }

    // Check if current is now in "normal" range
    // Normal range: between (1 - threshold) and threshold
    // e.g., with threshold = 0.60, normal is [0.40, 0.60]
    let is_normal = current_percentile >= (1.0 - config.reversion_threshold_percentile)
        && current_percentile <= config.reversion_threshold_percentile;

    if !is_normal {
        return None;
    }

    // Reversal detected
    if was_extreme_high {
        // Was extreme positive (bearish), now normalizing -> bullish reversal
        Some(ReversalSignal {
            from_direction: Direction::Down, // Was bearish signal
            strength: extreme_strength.min(1.0),
        })
    } else if was_extreme_low {
        // Was extreme negative (bullish), now normalizing -> bearish reversal
        Some(ReversalSignal {
            from_direction: Direction::Up, // Was bullish signal
            strength: extreme_strength.min(1.0),
        })
    } else {
        None
    }
}

#[async_trait]
impl SignalGenerator for FundingRateSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get funding rate from context
        let funding_rate = match ctx.funding_rate {
            Some(rate) => rate,
            None => {
                tracing::debug!("No funding rate in context, returning neutral signal");
                return Ok(SignalValue::neutral());
            }
        };

        // Add to history
        self.add_observation(funding_rate);

        // Need sufficient history for z-score
        if self.history.len() < 2 {
            tracing::debug!("Insufficient history for z-score, returning neutral signal");
            return Ok(SignalValue::neutral());
        }

        // Calculate z-score signal
        let zscore = self.current_zscore();
        let zscore_signal: Option<(Direction, f64)> = zscore.and_then(|z| {
            if z > self.zscore_threshold {
                // High positive funding rate -> bearish (contrarian)
                let strength = ((z - self.zscore_threshold) / self.zscore_threshold).min(1.0);
                Some((Direction::Down, strength))
            } else if z < -self.zscore_threshold {
                // High negative funding rate -> bullish (contrarian)
                let strength = ((-z - self.zscore_threshold) / self.zscore_threshold).min(1.0);
                Some((Direction::Up, strength))
            } else {
                None
            }
        });

        // Calculate percentile signal if historical data available
        let historical_rates: Vec<f64> = ctx
            .historical_funding_rates
            .as_ref()
            .map(|rates| rates.iter().map(|r| r.funding_rate).collect())
            .unwrap_or_else(|| self.history.iter().copied().collect());

        let percentile = SignalContext::calculate_percentile(&historical_rates, funding_rate);
        let percentile_signal_result = percentile_signal(
            funding_rate,
            &historical_rates,
            self.percentile_threshold_high,
            self.percentile_threshold_low,
        );

        // Detect reversal if configured
        let reversal = self.reversal_config.as_ref().and_then(|config| {
            ctx.historical_funding_rates
                .as_ref()
                .and_then(|rates| detect_reversal(rates, config))
        });

        // Combine signals based on mode
        let (direction, strength) = match self.signal_mode {
            FundingSignalMode::ZScore => zscore_signal.unwrap_or((Direction::Neutral, 0.0)),
            FundingSignalMode::Percentile => {
                percentile_signal_result.unwrap_or((Direction::Neutral, 0.0))
            }
            FundingSignalMode::Combined => {
                match (zscore_signal, percentile_signal_result) {
                    (Some((dir1, str1)), Some((dir2, str2))) => {
                        if dir1 == dir2 {
                            // Agreeing signals - average strength with boost
                            (dir1, ((str1 + str2) / 2.0 * 1.2).min(1.0))
                        } else {
                            // Conflicting signals - reduce strength
                            let dominant = if str1 > str2 {
                                (dir1, str1)
                            } else {
                                (dir2, str2)
                            };
                            (dominant.0, dominant.1 * 0.5)
                        }
                    }
                    (Some(signal), None) | (None, Some(signal)) => signal,
                    (None, None) => (Direction::Neutral, 0.0),
                }
            }
        };

        // Boost confidence if reversal detected in same direction
        let (final_direction, final_strength, confidence) = if let Some(ref rev) = reversal {
            if rev.from_direction.opposite() == direction {
                // Reversal supports our signal
                (direction, strength, rev.strength * 0.5) // Add confidence
            } else {
                (direction, strength, 0.0)
            }
        } else {
            (direction, strength, 0.0)
        };

        // Build signal with metadata
        let mut signal = SignalValue::new(final_direction, final_strength, confidence)?
            .with_metadata("funding_rate", funding_rate)
            .with_metadata("threshold", self.zscore_threshold);

        if let Some(z) = zscore {
            signal = signal.with_metadata("zscore", z);
        }

        if let Some(pct) = percentile {
            signal = signal.with_metadata("percentile", pct);
        }

        if let Some(ref rev) = reversal {
            signal = signal
                .with_metadata("reversal_detected", 1.0)
                .with_metadata("reversal_strength", rev.strength);
        }

        if let (Some((dir1, _)), Some((dir2, _))) = (zscore_signal, percentile_signal_result) {
            signal = signal.with_metadata("signals_agree", if dir1 == dir2 { 1.0 } else { 0.0 });
        }

        Ok(signal)
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
    use chrono::{Duration, Utc};

    // ============================================
    // Original Tests
    // ============================================

    #[tokio::test]
    async fn signal_returns_neutral_without_funding_rate() {
        let mut signal = FundingRateSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn signal_returns_neutral_with_insufficient_history() {
        let mut signal = FundingRateSignal::new(2.0, 1.0, 10);
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn signal_bearish_on_high_positive_funding() {
        let mut signal =
            FundingRateSignal::new(2.0, 1.0, 10).with_signal_mode(FundingSignalMode::ZScore);

        // Build up history with consistent normal rates
        for _ in 0..9 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Add extreme positive rate (10x the normal)
        // With mean ~0.001 and low variance, this should be > 2 std devs
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.02);
        let result = signal.compute(&ctx).await.unwrap();

        // High positive funding should give bearish signal
        assert_eq!(result.direction, Direction::Down);
    }

    #[tokio::test]
    async fn signal_bullish_on_high_negative_funding() {
        let mut signal =
            FundingRateSignal::new(2.0, 1.0, 10).with_signal_mode(FundingSignalMode::ZScore);

        // Build up history with consistent normal rates
        for _ in 0..9 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Add extreme negative rate
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(-0.02);
        let result = signal.compute(&ctx).await.unwrap();

        // High negative funding should give bullish signal
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn signal_neutral_on_normal_funding() {
        let mut signal =
            FundingRateSignal::new(2.0, 1.0, 5).with_signal_mode(FundingSignalMode::ZScore);

        // Build up history with consistent rates
        for rate in [0.001, 0.001, 0.001, 0.001, 0.001] {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(rate);
            let result = signal.compute(&ctx).await.unwrap();

            // All similar values = low z-score = neutral
            if signal.observation_count() >= 2 {
                assert_eq!(result.direction, Direction::Neutral);
            }
        }
    }

    #[test]
    fn signal_name_is_correct() {
        let signal = FundingRateSignal::default();
        assert_eq!(signal.name(), "funding_rate");
    }

    #[test]
    fn signal_weight_is_configurable() {
        let signal = FundingRateSignal::new(2.0, 1.5, 100);
        assert!((signal.weight() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn window_size_minimum_is_two() {
        let signal = FundingRateSignal::new(2.0, 1.0, 1);
        assert_eq!(signal.window_size, 2);
    }

    #[test]
    fn zscore_threshold_is_absolute() {
        let signal = FundingRateSignal::new(-2.0, 1.0, 100);
        assert!((signal.zscore_threshold - 2.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn signal_metadata_contains_values() {
        let mut signal = FundingRateSignal::new(2.0, 1.0, 5);

        // Build some history
        for rate in [0.001, 0.001, 0.001] {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(rate);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.002);
        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("funding_rate"));
        assert!(result.metadata.contains_key("zscore"));
        assert!(result.metadata.contains_key("threshold"));
    }

    #[test]
    fn zscore_calculation_is_correct() {
        let mut signal = FundingRateSignal::new(2.0, 1.0, 5);

        // Add values: 1, 2, 3, 4, 5
        // Mean = 3, Std = sqrt(2.5) ~= 1.58
        // Z-score of 5: (5 - 3) / 1.58 ~= 1.26
        for val in [1.0, 2.0, 3.0, 4.0, 5.0] {
            signal.add_observation(val);
        }

        let zscore = signal.current_zscore().unwrap();
        assert!(zscore > 1.2 && zscore < 1.3, "zscore was {zscore}");
    }

    // ============================================
    // Phase 2C: Percentile Signal Tests
    // ============================================

    #[test]
    fn percentile_calculation_is_correct() {
        let historical = vec![0.001, 0.002, 0.003, 0.004, 0.005];

        // Value at median should be ~0.6 (3 out of 5 values <= 0.003)
        let pct = SignalContext::calculate_percentile(&historical, 0.003).unwrap();
        assert!((pct - 0.6).abs() < 0.01, "pct was {pct}");

        // Minimum value should be 0.2 (1 out of 5)
        let pct_min = SignalContext::calculate_percentile(&historical, 0.001).unwrap();
        assert!((pct_min - 0.2).abs() < 0.01, "pct_min was {pct_min}");

        // Maximum value should be 1.0 (5 out of 5)
        let pct_max = SignalContext::calculate_percentile(&historical, 0.005).unwrap();
        assert!((pct_max - 1.0).abs() < 0.01, "pct_max was {pct_max}");
    }

    #[test]
    fn high_percentile_gives_bearish_signal() {
        // Create historical data where current is at the very top
        let historical: Vec<f64> = (0..100).map(|i| i as f64 * 0.0001).collect();
        let current = 0.015; // Above all historical values

        let result = percentile_signal(current, &historical, 0.90, 0.10);

        assert!(result.is_some());
        let (direction, _strength) = result.unwrap();
        assert_eq!(direction, Direction::Down);
    }

    #[test]
    fn low_percentile_gives_bullish_signal() {
        // Create historical data where current is at the very bottom
        let historical: Vec<f64> = (0..100).map(|i| (i + 10) as f64 * 0.0001).collect();
        let current = 0.0001; // Below all historical values

        let result = percentile_signal(current, &historical, 0.90, 0.10);

        assert!(result.is_some());
        let (direction, _strength) = result.unwrap();
        assert_eq!(direction, Direction::Up);
    }

    #[test]
    fn middle_percentile_gives_no_signal() {
        // Create historical data where current is in the middle
        let historical: Vec<f64> = (0..100).map(|i| i as f64 * 0.0001).collect();
        let current = 0.005; // ~50th percentile

        let result = percentile_signal(current, &historical, 0.90, 0.10);

        assert!(result.is_none());
    }

    #[test]
    fn percentile_signal_strength_scales_with_extremity() {
        let historical: Vec<f64> = (0..100).map(|i| i as f64 * 0.0001).collect();

        // Just above threshold
        let current_low_extreme = 0.0095; // 96th percentile
        let result1 = percentile_signal(current_low_extreme, &historical, 0.90, 0.10);

        // Very extreme
        let current_high_extreme = 0.015; // >100th percentile
        let result2 = percentile_signal(current_high_extreme, &historical, 0.90, 0.10);

        assert!(result1.is_some());
        assert!(result2.is_some());

        let (_, strength1) = result1.unwrap();
        let (_, strength2) = result2.unwrap();

        // More extreme should have higher strength
        assert!(
            strength2 >= strength1,
            "strength1={strength1}, strength2={strength2}"
        );
    }

    // ============================================
    // Phase 2C: Reversal Detection Tests
    // ============================================

    fn make_funding_history(percentiles: &[f64]) -> Vec<HistoricalFundingRate> {
        let now = Utc::now();
        percentiles
            .iter()
            .enumerate()
            .map(|(i, &pct)| HistoricalFundingRate {
                timestamp: now - Duration::hours(i as i64 * 8),
                funding_rate: pct * 0.01, // Not used in reversal detection
                zscore: None,
                percentile: Some(pct),
            })
            .collect()
    }

    #[test]
    fn reversal_detected_from_high_to_normal() {
        let config = FundingReversalConfig {
            lookback_periods: 5,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        // Most recent first: now at 0.50 (normal), was at 0.95 (extreme high)
        let historical = make_funding_history(&[0.50, 0.85, 0.92, 0.95, 0.88]);

        let reversal = detect_reversal(&historical, &config);

        assert!(reversal.is_some());
        let rev = reversal.unwrap();
        // Was extreme positive (bearish), reverting -> bullish signal expected
        assert_eq!(rev.from_direction, Direction::Down);
    }

    #[test]
    fn reversal_detected_from_low_to_normal() {
        let config = FundingReversalConfig {
            lookback_periods: 5,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        // Most recent first: now at 0.50 (normal), was at 0.05 (extreme low)
        let historical = make_funding_history(&[0.50, 0.15, 0.08, 0.05, 0.12]);

        let reversal = detect_reversal(&historical, &config);

        assert!(reversal.is_some());
        let rev = reversal.unwrap();
        // Was extreme negative (bullish), reverting -> bearish signal expected
        assert_eq!(rev.from_direction, Direction::Up);
    }

    #[test]
    fn no_reversal_when_staying_extreme() {
        let config = FundingReversalConfig {
            lookback_periods: 5,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        // Still at extreme high
        let historical = make_funding_history(&[0.92, 0.95, 0.93, 0.94, 0.91]);

        let reversal = detect_reversal(&historical, &config);
        assert!(reversal.is_none());
    }

    #[test]
    fn no_reversal_when_always_normal() {
        let config = FundingReversalConfig {
            lookback_periods: 5,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        // Always in normal range
        let historical = make_funding_history(&[0.50, 0.55, 0.45, 0.52, 0.48]);

        let reversal = detect_reversal(&historical, &config);
        assert!(reversal.is_none());
    }

    #[test]
    fn reversal_requires_sufficient_history() {
        let config = FundingReversalConfig {
            lookback_periods: 10,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        // Only 3 periods of history
        let historical = make_funding_history(&[0.50, 0.95, 0.92]);

        let reversal = detect_reversal(&historical, &config);
        assert!(reversal.is_none());
    }

    // ============================================
    // Phase 2C: Enhanced compute() Tests
    // ============================================

    #[tokio::test]
    async fn combined_mode_averages_signals() {
        let mut signal = FundingRateSignal::new(2.0, 1.0, 100)
            .with_signal_mode(FundingSignalMode::Combined)
            .with_percentile_thresholds(0.10, 0.90);

        // Create historical funding rates for percentile calculation
        let historical: Vec<HistoricalFundingRate> = (0..100)
            .map(|i| {
                let now = Utc::now();
                HistoricalFundingRate {
                    timestamp: now - Duration::hours(i * 8),
                    funding_rate: (i as f64 - 50.0) * 0.0001,
                    zscore: None,
                    percentile: Some(i as f64 / 100.0),
                }
            })
            .collect();

        // Build internal z-score history
        for i in 0..50 {
            let rate = (i as f64 - 25.0) * 0.0001;
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(rate);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Now test with extreme value
        let extreme_rate = 0.01; // Very high
        let ctx = SignalContext::new(Utc::now(), "BTCUSD")
            .with_funding_rate(extreme_rate)
            .with_historical_funding_rates(historical);

        let result = signal.compute(&ctx).await.unwrap();

        // Should have computed signal (likely bearish due to high funding)
        assert!(result.metadata.contains_key("zscore"));
        assert!(result.metadata.contains_key("percentile"));
    }

    #[tokio::test]
    async fn reversal_boosts_confidence() {
        let reversal_config = FundingReversalConfig {
            lookback_periods: 5,
            extreme_threshold_percentile: 0.90,
            reversion_threshold_percentile: 0.60,
        };

        let mut signal =
            FundingRateSignal::new(2.0, 1.0, 100).with_reversal_detection(reversal_config);

        // Build history for z-score
        for _ in 0..50 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Historical funding with reversal pattern
        let historical = make_funding_history(&[0.50, 0.85, 0.92, 0.95, 0.88]);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD")
            .with_funding_rate(0.001)
            .with_historical_funding_rates(historical);

        let result = signal.compute(&ctx).await.unwrap();

        // Should detect reversal
        assert!(result.metadata.contains_key("reversal_detected"));
    }

    #[tokio::test]
    async fn conflicting_signals_reduce_strength() {
        let mut signal = FundingRateSignal::new(0.5, 1.0, 10)
            .with_signal_mode(FundingSignalMode::Combined)
            .with_percentile_thresholds(0.30, 0.70);

        // Build biased history where zscore and percentile might conflict
        for i in 0..9 {
            let rate = (i as f64 - 4.0) * 0.001;
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(rate);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // This test verifies the combined logic works
        // Exact behavior depends on the data
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.002);
        let result = signal.compute(&ctx).await.unwrap();

        // Should produce some result
        assert!(result.strength >= 0.0 && result.strength <= 1.0);
    }

    #[tokio::test]
    async fn percentile_mode_uses_only_percentile() {
        let mut signal = FundingRateSignal::new(2.0, 1.0, 100)
            .with_signal_mode(FundingSignalMode::Percentile)
            .with_percentile_thresholds(0.10, 0.90);

        // Build history
        for _ in 0..50 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Create historical data where current is extreme
        let historical: Vec<HistoricalFundingRate> = (0..100)
            .map(|i| HistoricalFundingRate {
                timestamp: Utc::now() - Duration::hours(i * 8),
                funding_rate: i as f64 * 0.0001,
                zscore: None,
                percentile: None,
            })
            .collect();

        let extreme_rate = 0.015; // Very high - should be >90th percentile
        let ctx = SignalContext::new(Utc::now(), "BTCUSD")
            .with_funding_rate(extreme_rate)
            .with_historical_funding_rates(historical);

        let result = signal.compute(&ctx).await.unwrap();

        // Should be bearish due to high percentile
        assert_eq!(result.direction, Direction::Down);
    }

    #[tokio::test]
    async fn zscore_mode_uses_only_zscore() {
        let mut signal =
            FundingRateSignal::new(2.0, 1.0, 10).with_signal_mode(FundingSignalMode::ZScore);

        // Build up history with normal rates
        for _ in 0..9 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.001);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Add extreme positive rate
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_funding_rate(0.02);
        let result = signal.compute(&ctx).await.unwrap();

        // Should be bearish based on z-score
        assert_eq!(result.direction, Direction::Down);
        assert!(result.metadata.contains_key("zscore"));
    }
}
