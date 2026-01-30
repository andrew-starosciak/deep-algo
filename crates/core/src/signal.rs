//! Signal generation traits and types for statistical trading.
//!
//! This module provides the core abstraction for signal generators that produce
//! trading signals with statistical confidence measures.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Direction of a trading signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// Bullish signal - expect price to go up
    Up,
    /// Bearish signal - expect price to go down
    Down,
    /// No directional bias
    Neutral,
}

impl Direction {
    /// Returns the opposite direction.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Neutral => Self::Neutral,
        }
    }

    /// Returns true if this direction has a directional bias.
    #[must_use]
    pub const fn is_directional(self) -> bool {
        !matches!(self, Self::Neutral)
    }
}

/// Output from a signal generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalValue {
    /// The predicted direction
    pub direction: Direction,
    /// Signal strength from 0.0 (weakest) to 1.0 (strongest)
    pub strength: f64,
    /// Statistical confidence from 0.0 to 1.0
    pub confidence: f64,
    /// Optional metadata for debugging and analysis
    #[serde(default)]
    pub metadata: HashMap<String, f64>,
}

impl SignalValue {
    /// Creates a new SignalValue with validation.
    ///
    /// # Errors
    /// Returns error if strength or confidence are outside [0.0, 1.0].
    pub fn new(direction: Direction, strength: f64, confidence: f64) -> Result<Self> {
        if !(0.0..=1.0).contains(&strength) {
            anyhow::bail!("strength must be in [0.0, 1.0], got {strength}");
        }
        if !(0.0..=1.0).contains(&confidence) {
            anyhow::bail!("confidence must be in [0.0, 1.0], got {confidence}");
        }
        Ok(Self {
            direction,
            strength,
            confidence,
            metadata: HashMap::new(),
        })
    }

    /// Creates a neutral signal with zero strength.
    #[must_use]
    pub fn neutral() -> Self {
        Self {
            direction: Direction::Neutral,
            strength: 0.0,
            confidence: 0.0,
            metadata: HashMap::new(),
        }
    }

    /// Adds metadata to this signal.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: f64) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Context provided to signal generators for computation.
#[derive(Debug, Clone)]
pub struct SignalContext {
    /// Current timestamp
    pub timestamp: DateTime<Utc>,
    /// Symbol being analyzed
    pub symbol: String,
    /// Current mid price (if available)
    pub mid_price: Option<Decimal>,
    /// Order book snapshot (bid levels, ask levels)
    pub orderbook: Option<OrderBookSnapshot>,
    /// Recent funding rate
    pub funding_rate: Option<f64>,
    /// Recent liquidation data
    pub liquidation_usd: Option<Decimal>,
}

impl SignalContext {
    /// Creates a new SignalContext with minimal required fields.
    #[must_use]
    pub fn new(timestamp: DateTime<Utc>, symbol: impl Into<String>) -> Self {
        Self {
            timestamp,
            symbol: symbol.into(),
            mid_price: None,
            orderbook: None,
            funding_rate: None,
            liquidation_usd: None,
        }
    }

    /// Sets the mid price.
    #[must_use]
    pub fn with_mid_price(mut self, price: Decimal) -> Self {
        self.mid_price = Some(price);
        self
    }

    /// Sets the order book snapshot.
    #[must_use]
    pub fn with_orderbook(mut self, orderbook: OrderBookSnapshot) -> Self {
        self.orderbook = Some(orderbook);
        self
    }

    /// Sets the funding rate.
    #[must_use]
    pub fn with_funding_rate(mut self, rate: f64) -> Self {
        self.funding_rate = Some(rate);
        self
    }

    /// Sets liquidation USD value.
    #[must_use]
    pub fn with_liquidation_usd(mut self, usd: Decimal) -> Self {
        self.liquidation_usd = Some(usd);
        self
    }
}

/// Order book price level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    /// Price at this level
    pub price: Decimal,
    /// Quantity at this level
    pub quantity: Decimal,
}

/// Snapshot of an order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookSnapshot {
    /// Bid levels (highest price first)
    pub bids: Vec<PriceLevel>,
    /// Ask levels (lowest price first)
    pub asks: Vec<PriceLevel>,
    /// Timestamp of snapshot
    pub timestamp: DateTime<Utc>,
}

impl OrderBookSnapshot {
    /// Calculates order book imbalance: (bid_vol - ask_vol) / (bid_vol + ask_vol).
    /// Returns value in [-1.0, 1.0] where positive means more bid volume.
    #[must_use]
    pub fn calculate_imbalance(&self) -> f64 {
        let bid_vol: Decimal = self.bids.iter().map(|l| l.quantity).sum();
        let ask_vol: Decimal = self.asks.iter().map(|l| l.quantity).sum();
        let total = bid_vol + ask_vol;

        if total.is_zero() {
            return 0.0;
        }

        let imbalance = (bid_vol - ask_vol) / total;
        // Convert to f64 - safe for ratio calculations
        imbalance.to_string().parse::<f64>().unwrap_or(0.0)
    }

    /// Returns the best bid price (highest bid).
    #[must_use]
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.first().map(|l| l.price)
    }

    /// Returns the best ask price (lowest ask).
    #[must_use]
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.first().map(|l| l.price)
    }

    /// Calculates the mid price.
    #[must_use]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }
}

/// Trait for signal generators that produce trading signals.
///
/// All signal generators must implement this trait to be composable
/// within the trading system.
#[async_trait]
pub trait SignalGenerator: Send + Sync {
    /// Computes a signal based on the provided context.
    ///
    /// # Errors
    /// Returns error if signal computation fails.
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue>;

    /// Returns the name of this signal generator.
    fn name(&self) -> &str;

    /// Returns the weight of this signal for composite calculations.
    /// Default is 1.0.
    fn weight(&self) -> f64 {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================
    // Direction Tests
    // ============================================

    #[test]
    fn direction_opposite_up_is_down() {
        assert_eq!(Direction::Up.opposite(), Direction::Down);
    }

    #[test]
    fn direction_opposite_down_is_up() {
        assert_eq!(Direction::Down.opposite(), Direction::Up);
    }

    #[test]
    fn direction_opposite_neutral_is_neutral() {
        assert_eq!(Direction::Neutral.opposite(), Direction::Neutral);
    }

    #[test]
    fn direction_is_directional_up() {
        assert!(Direction::Up.is_directional());
    }

    #[test]
    fn direction_is_directional_down() {
        assert!(Direction::Down.is_directional());
    }

    #[test]
    fn direction_is_not_directional_neutral() {
        assert!(!Direction::Neutral.is_directional());
    }

    #[test]
    fn direction_serializes_to_json() {
        let json = serde_json::to_string(&Direction::Up).unwrap();
        assert_eq!(json, "\"Up\"");

        let json = serde_json::to_string(&Direction::Down).unwrap();
        assert_eq!(json, "\"Down\"");

        let json = serde_json::to_string(&Direction::Neutral).unwrap();
        assert_eq!(json, "\"Neutral\"");
    }

    #[test]
    fn direction_deserializes_from_json() {
        let dir: Direction = serde_json::from_str("\"Up\"").unwrap();
        assert_eq!(dir, Direction::Up);

        let dir: Direction = serde_json::from_str("\"Down\"").unwrap();
        assert_eq!(dir, Direction::Down);
    }

    // ============================================
    // SignalValue Tests
    // ============================================

    #[test]
    fn signal_value_valid_bounds_accepted() {
        let signal = SignalValue::new(Direction::Up, 0.5, 0.8).unwrap();
        assert_eq!(signal.direction, Direction::Up);
        assert!((signal.strength - 0.5).abs() < f64::EPSILON);
        assert!((signal.confidence - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_value_strength_at_zero_accepted() {
        let signal = SignalValue::new(Direction::Neutral, 0.0, 0.0).unwrap();
        assert!((signal.strength - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_value_strength_at_one_accepted() {
        let signal = SignalValue::new(Direction::Up, 1.0, 1.0).unwrap();
        assert!((signal.strength - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_value_strength_above_one_rejected() {
        let result = SignalValue::new(Direction::Up, 1.1, 0.5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("strength"));
    }

    #[test]
    fn signal_value_strength_below_zero_rejected() {
        let result = SignalValue::new(Direction::Up, -0.1, 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn signal_value_confidence_above_one_rejected() {
        let result = SignalValue::new(Direction::Up, 0.5, 1.5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("confidence"));
    }

    #[test]
    fn signal_value_confidence_below_zero_rejected() {
        let result = SignalValue::new(Direction::Up, 0.5, -0.1);
        assert!(result.is_err());
    }

    #[test]
    fn signal_value_neutral_has_zero_strength() {
        let signal = SignalValue::neutral();
        assert_eq!(signal.direction, Direction::Neutral);
        assert!((signal.strength - 0.0).abs() < f64::EPSILON);
        assert!((signal.confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_value_with_metadata_adds_key() {
        let signal = SignalValue::neutral()
            .with_metadata("imbalance", 0.3)
            .with_metadata("volume_ratio", 1.5);

        assert!((signal.metadata.get("imbalance").unwrap() - 0.3).abs() < f64::EPSILON);
        assert!((signal.metadata.get("volume_ratio").unwrap() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_value_serializes_to_json() {
        let signal = SignalValue::new(Direction::Up, 0.7, 0.9).unwrap();
        let json = serde_json::to_string(&signal).unwrap();

        assert!(json.contains("\"direction\":\"Up\""));
        assert!(json.contains("\"strength\":0.7"));
        assert!(json.contains("\"confidence\":0.9"));
    }

    // ============================================
    // SignalContext Tests
    // ============================================

    #[test]
    fn signal_context_new_creates_minimal_context() {
        let now = Utc::now();
        let ctx = SignalContext::new(now, "BTCUSD");

        assert_eq!(ctx.timestamp, now);
        assert_eq!(ctx.symbol, "BTCUSD");
        assert!(ctx.mid_price.is_none());
        assert!(ctx.orderbook.is_none());
        assert!(ctx.funding_rate.is_none());
    }

    #[test]
    fn signal_context_builder_methods_chain() {
        let now = Utc::now();
        let orderbook = OrderBookSnapshot {
            bids: vec![PriceLevel { price: dec!(100), quantity: dec!(10) }],
            asks: vec![PriceLevel { price: dec!(101), quantity: dec!(10) }],
            timestamp: now,
        };

        let ctx = SignalContext::new(now, "BTCUSD")
            .with_mid_price(dec!(42000))
            .with_orderbook(orderbook)
            .with_funding_rate(0.001)
            .with_liquidation_usd(dec!(50000));

        assert_eq!(ctx.mid_price, Some(dec!(42000)));
        assert!(ctx.orderbook.is_some());
        assert!((ctx.funding_rate.unwrap() - 0.001).abs() < f64::EPSILON);
        assert_eq!(ctx.liquidation_usd, Some(dec!(50000)));
    }

    // ============================================
    // OrderBookSnapshot Tests
    // ============================================

    #[test]
    fn orderbook_imbalance_balanced_is_zero() {
        let ob = OrderBookSnapshot {
            bids: vec![PriceLevel { price: dec!(100), quantity: dec!(10) }],
            asks: vec![PriceLevel { price: dec!(101), quantity: dec!(10) }],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        assert!(imbalance.abs() < 0.001);
    }

    #[test]
    fn orderbook_imbalance_more_bids_is_positive() {
        let ob = OrderBookSnapshot {
            bids: vec![
                PriceLevel { price: dec!(100), quantity: dec!(20) },
            ],
            asks: vec![
                PriceLevel { price: dec!(101), quantity: dec!(10) },
            ],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        // (20 - 10) / (20 + 10) = 10/30 = 0.333...
        assert!(imbalance > 0.3 && imbalance < 0.4);
    }

    #[test]
    fn orderbook_imbalance_more_asks_is_negative() {
        let ob = OrderBookSnapshot {
            bids: vec![
                PriceLevel { price: dec!(100), quantity: dec!(10) },
            ],
            asks: vec![
                PriceLevel { price: dec!(101), quantity: dec!(20) },
            ],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        // (10 - 20) / (10 + 20) = -10/30 = -0.333...
        assert!(imbalance < -0.3 && imbalance > -0.4);
    }

    #[test]
    fn orderbook_imbalance_empty_is_zero() {
        let ob = OrderBookSnapshot {
            bids: vec![],
            asks: vec![],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        assert!(imbalance.abs() < 0.001);
    }

    #[test]
    fn orderbook_best_bid_returns_highest() {
        let ob = OrderBookSnapshot {
            bids: vec![
                PriceLevel { price: dec!(100), quantity: dec!(10) },
                PriceLevel { price: dec!(99), quantity: dec!(20) },
            ],
            asks: vec![],
            timestamp: Utc::now(),
        };

        assert_eq!(ob.best_bid(), Some(dec!(100)));
    }

    #[test]
    fn orderbook_best_ask_returns_lowest() {
        let ob = OrderBookSnapshot {
            bids: vec![],
            asks: vec![
                PriceLevel { price: dec!(101), quantity: dec!(10) },
                PriceLevel { price: dec!(102), quantity: dec!(20) },
            ],
            timestamp: Utc::now(),
        };

        assert_eq!(ob.best_ask(), Some(dec!(101)));
    }

    #[test]
    fn orderbook_mid_price_calculates_correctly() {
        let ob = OrderBookSnapshot {
            bids: vec![PriceLevel { price: dec!(100), quantity: dec!(10) }],
            asks: vec![PriceLevel { price: dec!(102), quantity: dec!(10) }],
            timestamp: Utc::now(),
        };

        assert_eq!(ob.mid_price(), Some(dec!(101)));
    }

    #[test]
    fn orderbook_mid_price_none_when_empty_bids() {
        let ob = OrderBookSnapshot {
            bids: vec![],
            asks: vec![PriceLevel { price: dec!(101), quantity: dec!(10) }],
            timestamp: Utc::now(),
        };

        assert!(ob.mid_price().is_none());
    }

    // ============================================
    // Mock SignalGenerator for Testing
    // ============================================

    struct MockSignalGenerator {
        name: String,
        return_value: SignalValue,
    }

    #[async_trait]
    impl SignalGenerator for MockSignalGenerator {
        async fn compute(&mut self, _ctx: &SignalContext) -> Result<SignalValue> {
            Ok(self.return_value.clone())
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn weight(&self) -> f64 {
            2.0
        }
    }

    #[tokio::test]
    async fn signal_generator_mock_returns_expected() {
        let mut gen = MockSignalGenerator {
            name: "test_signal".to_string(),
            return_value: SignalValue::new(Direction::Up, 0.8, 0.9).unwrap(),
        };

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let signal = gen.compute(&ctx).await.unwrap();

        assert_eq!(signal.direction, Direction::Up);
        assert!((signal.strength - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn signal_generator_name_returns_correct() {
        let gen = MockSignalGenerator {
            name: "my_signal".to_string(),
            return_value: SignalValue::neutral(),
        };

        assert_eq!(gen.name(), "my_signal");
    }

    #[test]
    fn signal_generator_weight_can_be_overridden() {
        let gen = MockSignalGenerator {
            name: "test".to_string(),
            return_value: SignalValue::neutral(),
        };

        assert!((gen.weight() - 2.0).abs() < f64::EPSILON);
    }
}
