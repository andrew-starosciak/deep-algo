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

/// Historical funding rate record for signal context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalFundingRate {
    /// Timestamp of the funding rate
    pub timestamp: DateTime<Utc>,
    /// The 8-hour funding rate
    pub funding_rate: f64,
    /// Z-score relative to historical mean (if computed)
    pub zscore: Option<f64>,
    /// Percentile rank in historical distribution
    pub percentile: Option<f64>,
}

/// Aggregated liquidation data for signal context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationAggregate {
    /// End timestamp of the aggregation window
    pub timestamp: DateTime<Utc>,
    /// Window size in minutes
    pub window_minutes: i32,
    /// Total USD volume of long liquidations
    pub long_volume_usd: Decimal,
    /// Total USD volume of short liquidations
    pub short_volume_usd: Decimal,
    /// Net delta: long_volume - short_volume
    pub net_delta_usd: Decimal,
    /// Count of long liquidation events
    pub count_long: i32,
    /// Count of short liquidation events
    pub count_short: i32,
}

impl LiquidationAggregate {
    /// Returns the total liquidation volume.
    #[must_use]
    pub fn total_volume(&self) -> Decimal {
        self.long_volume_usd + self.short_volume_usd
    }

    /// Returns the imbalance ratio: (long - short) / (long + short).
    #[must_use]
    pub fn imbalance_ratio(&self) -> Option<f64> {
        let total = self.total_volume();
        if total > Decimal::ZERO {
            let ratio = self.net_delta_usd / total;
            ratio.to_string().parse::<f64>().ok()
        } else {
            None
        }
    }
}

/// News event for signal context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsEvent {
    /// Timestamp when news was published
    pub timestamp: DateTime<Utc>,
    /// News source identifier
    pub source: String,
    /// News headline/title
    pub title: String,
    /// Sentiment: "positive", "negative", "neutral"
    pub sentiment: Option<String>,
    /// Urgency score from 0.0 to 1.0
    pub urgency_score: Option<f64>,
    /// Cryptocurrency tickers mentioned
    pub currencies: Option<Vec<String>>,
}

impl NewsEvent {
    /// Returns true if this news mentions a specific currency.
    #[must_use]
    pub fn mentions_currency(&self, currency: &str) -> bool {
        self.currencies
            .as_ref()
            .map(|c| c.iter().any(|curr| curr.eq_ignore_ascii_case(currency)))
            .unwrap_or(false)
    }
}

/// Context provided to signal generators for computation.
#[derive(Debug, Clone)]
pub struct SignalContext {
    /// Current timestamp
    pub timestamp: DateTime<Utc>,
    /// Symbol being analyzed
    pub symbol: String,
    /// Exchange name
    pub exchange: String,
    /// Current mid price (if available)
    pub mid_price: Option<Decimal>,
    /// Order book snapshot (bid levels, ask levels)
    pub orderbook: Option<OrderBookSnapshot>,
    /// Recent funding rate
    pub funding_rate: Option<f64>,
    /// Recent liquidation data
    pub liquidation_usd: Option<Decimal>,
    /// Historical order book imbalances for z-score calculation
    pub historical_imbalances: Option<Vec<f64>>,
    /// Historical funding rates for percentile/z-score calculations
    pub historical_funding_rates: Option<Vec<HistoricalFundingRate>>,
    /// Recent liquidation aggregates
    pub liquidation_aggregates: Option<LiquidationAggregate>,
    /// Recent news events
    pub news_events: Option<Vec<NewsEvent>>,
}

impl SignalContext {
    /// Creates a new SignalContext with minimal required fields.
    #[must_use]
    pub fn new(timestamp: DateTime<Utc>, symbol: impl Into<String>) -> Self {
        Self {
            timestamp,
            symbol: symbol.into(),
            exchange: String::new(),
            mid_price: None,
            orderbook: None,
            funding_rate: None,
            liquidation_usd: None,
            historical_imbalances: None,
            historical_funding_rates: None,
            liquidation_aggregates: None,
            news_events: None,
        }
    }

    /// Sets the exchange name.
    #[must_use]
    pub fn with_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.exchange = exchange.into();
        self
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

    /// Sets historical order book imbalances for z-score calculation.
    #[must_use]
    pub fn with_historical_imbalances(mut self, imbalances: Vec<f64>) -> Self {
        self.historical_imbalances = Some(imbalances);
        self
    }

    /// Sets historical funding rates.
    #[must_use]
    pub fn with_historical_funding_rates(mut self, rates: Vec<HistoricalFundingRate>) -> Self {
        self.historical_funding_rates = Some(rates);
        self
    }

    /// Sets liquidation aggregates.
    #[must_use]
    pub fn with_liquidation_aggregates(mut self, aggregates: LiquidationAggregate) -> Self {
        self.liquidation_aggregates = Some(aggregates);
        self
    }

    /// Sets news events.
    #[must_use]
    pub fn with_news_events(mut self, events: Vec<NewsEvent>) -> Self {
        self.news_events = Some(events);
        self
    }

    /// Calculates the z-score of a value given the historical data.
    ///
    /// Returns None if insufficient historical data.
    #[must_use]
    pub fn calculate_zscore(values: &[f64], current: f64) -> Option<f64> {
        if values.len() < 2 {
            return None;
        }

        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let stddev = variance.sqrt();

        if stddev > f64::EPSILON {
            Some((current - mean) / stddev)
        } else {
            None
        }
    }

    /// Calculates the percentile of a value given the historical data.
    ///
    /// Returns the fraction of values less than or equal to current.
    #[must_use]
    pub fn calculate_percentile(values: &[f64], current: f64) -> Option<f64> {
        if values.is_empty() {
            return None;
        }

        let count_below = values.iter().filter(|&&v| v <= current).count();
        Some(count_below as f64 / values.len() as f64)
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
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: dec!(10),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(10),
            }],
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
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: dec!(10),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(10),
            }],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        assert!(imbalance.abs() < 0.001);
    }

    #[test]
    fn orderbook_imbalance_more_bids_is_positive() {
        let ob = OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: dec!(20),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(10),
            }],
            timestamp: Utc::now(),
        };

        let imbalance = ob.calculate_imbalance();
        // (20 - 10) / (20 + 10) = 10/30 = 0.333...
        assert!(imbalance > 0.3 && imbalance < 0.4);
    }

    #[test]
    fn orderbook_imbalance_more_asks_is_negative() {
        let ob = OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: dec!(10),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(20),
            }],
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
                PriceLevel {
                    price: dec!(100),
                    quantity: dec!(10),
                },
                PriceLevel {
                    price: dec!(99),
                    quantity: dec!(20),
                },
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
                PriceLevel {
                    price: dec!(101),
                    quantity: dec!(10),
                },
                PriceLevel {
                    price: dec!(102),
                    quantity: dec!(20),
                },
            ],
            timestamp: Utc::now(),
        };

        assert_eq!(ob.best_ask(), Some(dec!(101)));
    }

    #[test]
    fn orderbook_mid_price_calculates_correctly() {
        let ob = OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: dec!(10),
            }],
            asks: vec![PriceLevel {
                price: dec!(102),
                quantity: dec!(10),
            }],
            timestamp: Utc::now(),
        };

        assert_eq!(ob.mid_price(), Some(dec!(101)));
    }

    #[test]
    fn orderbook_mid_price_none_when_empty_bids() {
        let ob = OrderBookSnapshot {
            bids: vec![],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(10),
            }],
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

    // ============================================
    // SignalContext Historical Data Tests
    // ============================================

    #[test]
    fn signal_context_with_historical_data_builds_correctly() {
        let now = Utc::now();
        let historical_funding = vec![
            HistoricalFundingRate {
                timestamp: now - chrono::Duration::hours(8),
                funding_rate: 0.0001,
                zscore: Some(0.5),
                percentile: Some(0.6),
            },
            HistoricalFundingRate {
                timestamp: now - chrono::Duration::hours(16),
                funding_rate: 0.00015,
                zscore: Some(1.2),
                percentile: Some(0.8),
            },
        ];

        let liquidation_agg = LiquidationAggregate {
            timestamp: now,
            window_minutes: 5,
            long_volume_usd: dec!(100000),
            short_volume_usd: dec!(50000),
            net_delta_usd: dec!(50000),
            count_long: 10,
            count_short: 5,
        };

        let news = vec![NewsEvent {
            timestamp: now - chrono::Duration::minutes(5),
            source: "cryptopanic".to_string(),
            title: "Bitcoin hits new ATH".to_string(),
            sentiment: Some("positive".to_string()),
            urgency_score: Some(0.85),
            currencies: Some(vec!["BTC".to_string()]),
        }];

        let ctx = SignalContext::new(now, "BTCUSD")
            .with_exchange("binance")
            .with_historical_imbalances(vec![0.1, 0.2, 0.15, 0.05, -0.1])
            .with_historical_funding_rates(historical_funding.clone())
            .with_liquidation_aggregates(liquidation_agg)
            .with_news_events(news.clone());

        // Verify all fields are set
        assert_eq!(ctx.exchange, "binance");
        assert!(ctx.historical_imbalances.is_some());
        assert_eq!(ctx.historical_imbalances.as_ref().unwrap().len(), 5);
        assert!(ctx.historical_funding_rates.is_some());
        assert_eq!(ctx.historical_funding_rates.as_ref().unwrap().len(), 2);
        assert!(ctx.liquidation_aggregates.is_some());
        assert_eq!(
            ctx.liquidation_aggregates.as_ref().unwrap().long_volume_usd,
            dec!(100000)
        );
        assert!(ctx.news_events.is_some());
        assert_eq!(ctx.news_events.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn signal_context_historical_fields_default_to_none() {
        let now = Utc::now();
        let ctx = SignalContext::new(now, "BTCUSD");

        assert!(ctx.historical_imbalances.is_none());
        assert!(ctx.historical_funding_rates.is_none());
        assert!(ctx.liquidation_aggregates.is_none());
        assert!(ctx.news_events.is_none());
        assert!(ctx.exchange.is_empty());
    }

    #[test]
    fn signal_context_calculate_zscore_correct() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        // mean = 3.0, stddev = sqrt(2.5) = 1.58...

        let zscore = SignalContext::calculate_zscore(&values, 5.0).unwrap();
        // (5.0 - 3.0) / 1.58... = 1.26...
        assert!(zscore > 1.2 && zscore < 1.3);

        let zscore_mean = SignalContext::calculate_zscore(&values, 3.0).unwrap();
        assert!(zscore_mean.abs() < 0.001); // At mean, zscore = 0
    }

    #[test]
    fn signal_context_calculate_zscore_insufficient_data() {
        let values = vec![1.0]; // Need at least 2 values
        assert!(SignalContext::calculate_zscore(&values, 1.0).is_none());

        let empty: Vec<f64> = vec![];
        assert!(SignalContext::calculate_zscore(&empty, 1.0).is_none());
    }

    #[test]
    fn signal_context_calculate_zscore_zero_variance() {
        let values = vec![5.0, 5.0, 5.0, 5.0]; // All same
        assert!(SignalContext::calculate_zscore(&values, 5.0).is_none());
    }

    #[test]
    fn signal_context_calculate_percentile_correct() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let p = SignalContext::calculate_percentile(&values, 3.0).unwrap();
        assert!((p - 0.6).abs() < 0.001); // 3 values <= 3.0 out of 5 = 0.6

        let p_min = SignalContext::calculate_percentile(&values, 1.0).unwrap();
        assert!((p_min - 0.2).abs() < 0.001); // 1 value <= 1.0 out of 5 = 0.2

        let p_max = SignalContext::calculate_percentile(&values, 5.0).unwrap();
        assert!((p_max - 1.0).abs() < 0.001); // All values <= 5.0
    }

    #[test]
    fn signal_context_calculate_percentile_empty() {
        let empty: Vec<f64> = vec![];
        assert!(SignalContext::calculate_percentile(&empty, 1.0).is_none());
    }

    // ============================================
    // LiquidationAggregate Tests
    // ============================================

    #[test]
    fn liquidation_aggregate_total_volume_correct() {
        let agg = LiquidationAggregate {
            timestamp: Utc::now(),
            window_minutes: 5,
            long_volume_usd: dec!(100000),
            short_volume_usd: dec!(50000),
            net_delta_usd: dec!(50000),
            count_long: 10,
            count_short: 5,
        };

        assert_eq!(agg.total_volume(), dec!(150000));
    }

    #[test]
    fn liquidation_aggregate_imbalance_ratio_correct() {
        let agg = LiquidationAggregate {
            timestamp: Utc::now(),
            window_minutes: 5,
            long_volume_usd: dec!(75000),
            short_volume_usd: dec!(25000),
            net_delta_usd: dec!(50000),
            count_long: 10,
            count_short: 5,
        };

        // (75000 - 25000) / (75000 + 25000) = 0.5
        let ratio = agg.imbalance_ratio().unwrap();
        assert!((ratio - 0.5).abs() < 0.001);
    }

    #[test]
    fn liquidation_aggregate_imbalance_ratio_zero_volume() {
        let agg = LiquidationAggregate {
            timestamp: Utc::now(),
            window_minutes: 5,
            long_volume_usd: Decimal::ZERO,
            short_volume_usd: Decimal::ZERO,
            net_delta_usd: Decimal::ZERO,
            count_long: 0,
            count_short: 0,
        };

        assert!(agg.imbalance_ratio().is_none());
    }

    // ============================================
    // NewsEvent Tests
    // ============================================

    #[test]
    fn news_event_mentions_currency_case_insensitive() {
        let news = NewsEvent {
            timestamp: Utc::now(),
            source: "test".to_string(),
            title: "Test".to_string(),
            sentiment: None,
            urgency_score: None,
            currencies: Some(vec!["BTC".to_string(), "ETH".to_string()]),
        };

        assert!(news.mentions_currency("BTC"));
        assert!(news.mentions_currency("btc"));
        assert!(news.mentions_currency("Btc"));
        assert!(news.mentions_currency("ETH"));
        assert!(!news.mentions_currency("SOL"));
    }

    #[test]
    fn news_event_mentions_currency_none() {
        let news = NewsEvent {
            timestamp: Utc::now(),
            source: "test".to_string(),
            title: "Test".to_string(),
            sentiment: None,
            urgency_score: None,
            currencies: None,
        };

        assert!(!news.mentions_currency("BTC"));
    }

    #[test]
    fn historical_funding_rate_serializes_correctly() {
        let rate = HistoricalFundingRate {
            timestamp: Utc::now(),
            funding_rate: 0.0001,
            zscore: Some(1.5),
            percentile: Some(0.85),
        };

        let json = serde_json::to_string(&rate).unwrap();
        let deserialized: HistoricalFundingRate = serde_json::from_str(&json).unwrap();

        assert!((deserialized.funding_rate - 0.0001).abs() < f64::EPSILON);
        assert!((deserialized.zscore.unwrap() - 1.5).abs() < f64::EPSILON);
    }
}
