//! Funding rate signal generator.
//!
//! Generates trading signals based on funding rate extremes and reversals.
//! High positive funding rates suggest overleveraged longs, creating
//! potential for short squeezes and price drops.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::VecDeque;

/// Signal generator based on funding rate analysis.
///
/// Funding rates indicate the cost of holding perpetual futures positions.
/// Extreme positive rates suggest overleveraged longs (bearish signal),
/// while extreme negative rates suggest overleveraged shorts (bullish signal).
///
/// This signal uses z-score normalization to identify extreme readings.
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
        }
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
            return Some(0.0);
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

        let variance = self
            .history
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>()
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

        // Calculate z-score
        let zscore = self.current_zscore().unwrap_or(0.0);

        // Determine direction based on z-score
        // High positive funding = overleveraged longs = expect DOWN
        // High negative funding = overleveraged shorts = expect UP
        let (direction, strength) = if zscore > self.zscore_threshold {
            // High positive funding rate -> bearish (contrarian)
            let strength = ((zscore - self.zscore_threshold) / self.zscore_threshold).min(1.0);
            (Direction::Down, strength)
        } else if zscore < -self.zscore_threshold {
            // High negative funding rate -> bullish (contrarian)
            let strength = ((-zscore - self.zscore_threshold) / self.zscore_threshold).min(1.0);
            (Direction::Up, strength)
        } else {
            (Direction::Neutral, 0.0)
        };

        // Create signal
        let signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("funding_rate", funding_rate)
            .with_metadata("zscore", zscore)
            .with_metadata("threshold", self.zscore_threshold);

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
    use chrono::Utc;

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
        let mut signal = FundingRateSignal::new(2.0, 1.0, 10);

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
        let mut signal = FundingRateSignal::new(2.0, 1.0, 10);

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
        let mut signal = FundingRateSignal::new(2.0, 1.0, 5);

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
}
