//! Order book imbalance signal generator.
//!
//! Generates trading signals based on the imbalance between bid and ask
//! volumes in the order book.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::VecDeque;

/// Signal generator based on order book bid/ask imbalance.
///
/// When bid volume significantly exceeds ask volume (positive imbalance),
/// this generates a bullish signal. When ask volume exceeds bid volume
/// (negative imbalance), this generates a bearish signal.
///
/// The signal uses a rolling window to smooth out noise and requires
/// the imbalance to exceed a configurable threshold before generating
/// a directional signal.
#[derive(Debug, Clone)]
pub struct OrderBookImbalanceSignal {
    /// Name of this signal
    name: String,
    /// Threshold for generating directional signal (0.0 to 1.0)
    threshold: f64,
    /// Weight for composite signal aggregation
    weight: f64,
    /// Rolling window of recent imbalances for smoothing
    history: VecDeque<f64>,
    /// Maximum size of rolling window
    window_size: usize,
}

impl Default for OrderBookImbalanceSignal {
    fn default() -> Self {
        Self::new(0.3, 1.0, 10)
    }
}

impl OrderBookImbalanceSignal {
    /// Creates a new OrderBookImbalanceSignal.
    ///
    /// # Arguments
    /// * `threshold` - Minimum imbalance to generate directional signal (0.0 to 1.0)
    /// * `weight` - Weight for composite signal aggregation
    /// * `window_size` - Number of observations to average for smoothing
    #[must_use]
    pub fn new(threshold: f64, weight: f64, window_size: usize) -> Self {
        Self {
            name: "orderbook_imbalance".to_string(),
            threshold: threshold.clamp(0.0, 1.0),
            weight,
            history: VecDeque::with_capacity(window_size),
            window_size: window_size.max(1),
        }
    }

    /// Returns the current smoothed imbalance value.
    #[must_use]
    pub fn current_imbalance(&self) -> f64 {
        if self.history.is_empty() {
            return 0.0;
        }
        self.history.iter().sum::<f64>() / self.history.len() as f64
    }

    /// Adds a new imbalance observation to the rolling window.
    fn add_observation(&mut self, imbalance: f64) {
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back(imbalance);
    }
}

#[async_trait]
impl SignalGenerator for OrderBookImbalanceSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get order book from context
        let orderbook = match &ctx.orderbook {
            Some(ob) => ob,
            None => {
                tracing::debug!("No orderbook in context, returning neutral signal");
                return Ok(SignalValue::neutral());
            }
        };

        // Calculate current imbalance
        let raw_imbalance = orderbook.calculate_imbalance();

        // Add to rolling window
        self.add_observation(raw_imbalance);

        // Get smoothed imbalance
        let smoothed = self.current_imbalance();

        // Determine direction based on threshold
        let (direction, strength) = if smoothed > self.threshold {
            (Direction::Up, smoothed.min(1.0))
        } else if smoothed < -self.threshold {
            (Direction::Down, smoothed.abs().min(1.0))
        } else {
            (Direction::Neutral, smoothed.abs())
        };

        // Create signal value
        let signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("raw_imbalance", raw_imbalance)
            .with_metadata("smoothed_imbalance", smoothed)
            .with_metadata("threshold", self.threshold);

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
    use algo_trade_core::{OrderBookSnapshot, PriceLevel};
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn create_orderbook(bid_qty: i64, ask_qty: i64) -> OrderBookSnapshot {
        OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: rust_decimal::Decimal::new(bid_qty, 0),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: rust_decimal::Decimal::new(ask_qty, 0),
            }],
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn signal_returns_neutral_without_orderbook() {
        let mut signal = OrderBookImbalanceSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
        assert!((result.strength - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn signal_bullish_when_more_bids() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 1);
        let orderbook = create_orderbook(80, 20); // 80 bids, 20 asks = 0.6 imbalance

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
        assert!(result.strength > 0.5);
    }

    #[tokio::test]
    async fn signal_bearish_when_more_asks() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 1);
        let orderbook = create_orderbook(20, 80); // 20 bids, 80 asks = -0.6 imbalance

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Down);
        assert!(result.strength > 0.5);
    }

    #[tokio::test]
    async fn signal_neutral_when_balanced() {
        let mut signal = OrderBookImbalanceSignal::new(0.3, 1.0, 1);
        let orderbook = create_orderbook(50, 50); // Equal = 0 imbalance

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn signal_neutral_below_threshold() {
        let mut signal = OrderBookImbalanceSignal::new(0.5, 1.0, 1);
        let orderbook = create_orderbook(60, 40); // 60 bids, 40 asks = 0.2 imbalance (< 0.5)

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn signal_smooths_over_window() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 3);

        // First observation: strong bullish
        let ob1 = create_orderbook(90, 10); // 0.8 imbalance
        let ctx1 = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(ob1);
        let _ = signal.compute(&ctx1).await.unwrap();

        // Second observation: neutral
        let ob2 = create_orderbook(50, 50); // 0.0 imbalance
        let ctx2 = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(ob2);
        let _ = signal.compute(&ctx2).await.unwrap();

        // Third observation: slight bearish
        let ob3 = create_orderbook(40, 60); // -0.2 imbalance
        let ctx3 = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(ob3);
        let _result = signal.compute(&ctx3).await.unwrap();

        // Average: (0.8 + 0.0 - 0.2) / 3 = 0.2
        let avg = signal.current_imbalance();
        assert!(avg > 0.15 && avg < 0.25, "avg was {avg}");
    }

    #[test]
    fn signal_name_is_correct() {
        let signal = OrderBookImbalanceSignal::default();
        assert_eq!(signal.name(), "orderbook_imbalance");
    }

    #[test]
    fn signal_weight_is_configurable() {
        let signal = OrderBookImbalanceSignal::new(0.3, 2.5, 10);
        assert!((signal.weight() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn threshold_is_clamped() {
        let signal = OrderBookImbalanceSignal::new(1.5, 1.0, 10);
        assert!((signal.threshold - 1.0).abs() < f64::EPSILON);

        let signal = OrderBookImbalanceSignal::new(-0.5, 1.0, 10);
        assert!((signal.threshold - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn window_size_minimum_is_one() {
        let signal = OrderBookImbalanceSignal::new(0.3, 1.0, 0);
        assert_eq!(signal.window_size, 1);
    }

    #[tokio::test]
    async fn signal_metadata_contains_values() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 1);
        let orderbook = create_orderbook(70, 30);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);
        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("raw_imbalance"));
        assert!(result.metadata.contains_key("smoothed_imbalance"));
        assert!(result.metadata.contains_key("threshold"));
    }
}
