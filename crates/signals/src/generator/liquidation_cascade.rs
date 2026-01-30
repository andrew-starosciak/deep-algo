//! Liquidation cascade signal generator.
//!
//! Generates trading signals based on liquidation volume and cascades.
//! Large liquidations can trigger further liquidations, creating momentum.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::VecDeque;

/// Signal generator based on liquidation cascade detection.
///
/// Large liquidations often trigger cascading liquidations as price moves
/// hit other leveraged positions' liquidation prices. This signal detects
/// unusually large liquidation volumes as potential indicators of continued
/// momentum in the direction of the liquidation.
#[derive(Debug, Clone)]
pub struct LiquidationCascadeSignal {
    /// Name of this signal
    name: String,
    /// Minimum USD value to consider a significant liquidation
    min_usd_threshold: Decimal,
    /// Multiple of average to consider as cascade trigger
    cascade_multiple: f64,
    /// Weight for composite signal aggregation
    weight: f64,
    /// Rolling window of liquidation values
    history: VecDeque<Decimal>,
    /// Maximum size of rolling window
    window_size: usize,
}

impl Default for LiquidationCascadeSignal {
    fn default() -> Self {
        Self::new(
            Decimal::new(5000, 0), // $5,000 minimum
            3.0,                   // 3x average
            1.0,
            100,
        )
    }
}

impl LiquidationCascadeSignal {
    /// Creates a new LiquidationCascadeSignal.
    ///
    /// # Arguments
    /// * `min_usd_threshold` - Minimum USD value for a significant liquidation
    /// * `cascade_multiple` - Multiple of average that triggers cascade signal
    /// * `weight` - Weight for composite signal aggregation
    /// * `window_size` - Number of observations to track
    #[must_use]
    pub fn new(
        min_usd_threshold: Decimal,
        cascade_multiple: f64,
        weight: f64,
        window_size: usize,
    ) -> Self {
        Self {
            name: "liquidation_cascade".to_string(),
            min_usd_threshold,
            cascade_multiple: cascade_multiple.max(1.0),
            weight,
            history: VecDeque::with_capacity(window_size),
            window_size: window_size.max(1),
        }
    }

    /// Returns the average liquidation value in the window.
    #[must_use]
    pub fn average_liquidation(&self) -> Decimal {
        if self.history.is_empty() {
            return Decimal::ZERO;
        }
        let sum: Decimal = self.history.iter().copied().sum();
        sum / Decimal::from(self.history.len())
    }

    /// Checks if a liquidation value triggers cascade detection.
    #[must_use]
    pub fn is_cascade(&self, value: Decimal) -> bool {
        if value < self.min_usd_threshold {
            return false;
        }

        let avg = self.average_liquidation();
        if avg.is_zero() {
            return value >= self.min_usd_threshold;
        }

        // Convert to f64 for comparison (safe for this use case)
        let value_f64: f64 = value.to_string().parse().unwrap_or(0.0);
        let avg_f64: f64 = avg.to_string().parse().unwrap_or(0.0);

        value_f64 >= avg_f64 * self.cascade_multiple
    }

    /// Adds a new liquidation observation.
    fn add_observation(&mut self, value: Decimal) {
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back(value);
    }

    /// Returns the number of observations in history.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.history.len()
    }
}

#[async_trait]
impl SignalGenerator for LiquidationCascadeSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get liquidation value from context
        let liq_value = match ctx.liquidation_usd {
            Some(val) => val,
            None => {
                tracing::debug!("No liquidation data in context, returning neutral signal");
                return Ok(SignalValue::neutral());
            }
        };

        // Check if this is a cascade event before adding to history
        let is_cascade = self.is_cascade(liq_value);
        let avg_before = self.average_liquidation();

        // Add to history
        self.add_observation(liq_value);

        // Determine signal based on cascade detection
        // Liquidations create momentum in the opposite direction
        // (longs liquidated = price dropped = bearish continuation likely)
        // For now, we don't have direction info, so we just detect unusual volume
        let (direction, strength) = if is_cascade {
            // Calculate strength based on how much above average
            let value_f64: f64 = liq_value.to_string().parse().unwrap_or(0.0);
            let avg_f64: f64 = avg_before.to_string().parse().unwrap_or(1.0);

            let ratio = if avg_f64 > 0.0 {
                value_f64 / avg_f64
            } else {
                self.cascade_multiple
            };

            let strength = ((ratio - self.cascade_multiple) / self.cascade_multiple)
                .clamp(0.0, 1.0);

            // Without direction info, we return neutral direction but positive strength
            // This allows composite signals to use the strength as a volatility indicator
            (Direction::Neutral, strength)
        } else {
            (Direction::Neutral, 0.0)
        };

        // Create signal
        let liq_f64: f64 = liq_value.to_string().parse().unwrap_or(0.0);
        let avg_f64: f64 = self.average_liquidation().to_string().parse().unwrap_or(0.0);

        let signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("liquidation_usd", liq_f64)
            .with_metadata("average_liquidation", avg_f64)
            .with_metadata("is_cascade", if is_cascade { 1.0 } else { 0.0 })
            .with_metadata("cascade_multiple", self.cascade_multiple);

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
    use rust_decimal_macros::dec;

    #[tokio::test]
    async fn signal_returns_neutral_without_liquidation_data() {
        let mut signal = LiquidationCascadeSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
        assert!((result.strength - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn signal_below_threshold_is_not_cascade() {
        let mut signal = LiquidationCascadeSignal::new(dec!(5000), 3.0, 1.0, 10);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(1000));
        let result = signal.compute(&ctx).await.unwrap();

        // Below $5000 threshold, should not be cascade
        assert!((result.strength - 0.0).abs() < f64::EPSILON);
        assert!(*result.metadata.get("is_cascade").unwrap() < 0.5);
    }

    #[tokio::test]
    async fn signal_above_threshold_with_no_history_is_cascade() {
        let mut signal = LiquidationCascadeSignal::new(dec!(5000), 3.0, 1.0, 10);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(10000));
        let result = signal.compute(&ctx).await.unwrap();

        // Above threshold, first observation, should be cascade
        assert!(*result.metadata.get("is_cascade").unwrap() > 0.5);
    }

    #[tokio::test]
    async fn signal_cascade_when_multiple_times_average() {
        let mut signal = LiquidationCascadeSignal::new(dec!(1000), 3.0, 1.0, 5);

        // Build history with consistent values
        for _ in 0..4 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(2000));
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Add value 4x the average
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(8000));
        let result = signal.compute(&ctx).await.unwrap();

        // 8000 / 2000 = 4x > 3x threshold = cascade
        assert!(*result.metadata.get("is_cascade").unwrap() > 0.5);
        assert!(result.strength > 0.0);
    }

    #[tokio::test]
    async fn signal_no_cascade_when_below_multiple() {
        let mut signal = LiquidationCascadeSignal::new(dec!(1000), 3.0, 1.0, 5);

        // Build history
        for _ in 0..4 {
            let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(2000));
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Add value 2x the average (below 3x threshold)
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(4000));
        let result = signal.compute(&ctx).await.unwrap();

        // 4000 / 2000 = 2x < 3x threshold = not cascade
        assert!(*result.metadata.get("is_cascade").unwrap() < 0.5);
    }

    #[test]
    fn signal_name_is_correct() {
        let signal = LiquidationCascadeSignal::default();
        assert_eq!(signal.name(), "liquidation_cascade");
    }

    #[test]
    fn signal_weight_is_configurable() {
        let signal = LiquidationCascadeSignal::new(dec!(5000), 3.0, 2.0, 100);
        assert!((signal.weight() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cascade_multiple_minimum_is_one() {
        let signal = LiquidationCascadeSignal::new(dec!(5000), 0.5, 1.0, 100);
        assert!((signal.cascade_multiple - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn window_size_minimum_is_one() {
        let signal = LiquidationCascadeSignal::new(dec!(5000), 3.0, 1.0, 0);
        assert_eq!(signal.window_size, 1);
    }

    #[test]
    fn average_liquidation_empty_is_zero() {
        let signal = LiquidationCascadeSignal::default();
        assert_eq!(signal.average_liquidation(), Decimal::ZERO);
    }

    #[test]
    fn average_liquidation_calculates_correctly() {
        let mut signal = LiquidationCascadeSignal::new(dec!(1000), 3.0, 1.0, 10);
        signal.add_observation(dec!(1000));
        signal.add_observation(dec!(2000));
        signal.add_observation(dec!(3000));

        // Average of 1000, 2000, 3000 = 2000
        assert_eq!(signal.average_liquidation(), dec!(2000));
    }

    #[tokio::test]
    async fn signal_metadata_contains_values() {
        let mut signal = LiquidationCascadeSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_usd(dec!(10000));

        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("liquidation_usd"));
        assert!(result.metadata.contains_key("average_liquidation"));
        assert!(result.metadata.contains_key("is_cascade"));
        assert!(result.metadata.contains_key("cascade_multiple"));
    }
}
