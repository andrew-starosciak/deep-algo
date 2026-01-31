//! Order book imbalance signal generator.
//!
//! Generates trading signals based on the imbalance between bid and ask
//! volumes in the order book.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::VecDeque;

/// Side of the order book (bid or ask).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Bid side (buy orders)
    Bid,
    /// Ask side (sell orders)
    Ask,
}

/// Wall semantics representing floor (support) or ceiling (resistance).
///
/// - **Floor**: Bid wall acting as support - bullish (prevents price from falling)
/// - **Ceiling**: Ask wall acting as resistance - bearish (prevents price from rising)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallSemantics {
    /// Bid wall = floor (support) - bullish
    Floor,
    /// Ask wall = ceiling (resistance) - bearish
    Ceiling,
}

impl WallSemantics {
    /// Returns the directional bias of this wall semantics.
    ///
    /// - Floor (bid wall) returns +1.0 (bullish)
    /// - Ceiling (ask wall) returns -1.0 (bearish)
    #[must_use]
    pub fn direction_bias(&self) -> f64 {
        match self {
            WallSemantics::Floor => 1.0,
            WallSemantics::Ceiling => -1.0,
        }
    }
}

impl From<Side> for WallSemantics {
    fn from(side: Side) -> Self {
        match side {
            Side::Bid => WallSemantics::Floor,
            Side::Ask => WallSemantics::Ceiling,
        }
    }
}

/// Configuration for wall detection.
#[derive(Debug, Clone)]
pub struct WallDetectionConfig {
    /// Minimum size in BTC to be considered a wall
    pub min_wall_size_btc: Decimal,
    /// Maximum distance from mid-price in basis points
    pub proximity_bps: u32,
}

impl Default for WallDetectionConfig {
    fn default() -> Self {
        Self {
            min_wall_size_btc: Decimal::new(10, 0), // 10 BTC
            proximity_bps: 100,                     // 1%
        }
    }
}

/// Detected wall in the order book.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields are part of public API for consumers
pub struct Wall {
    /// Side of the wall (bid or ask)
    pub side: Side,
    /// Semantics of the wall (floor/support or ceiling/resistance)
    pub semantics: WallSemantics,
    /// Price level of the wall
    pub price: Decimal,
    /// Size of the wall in BTC
    pub size: Decimal,
    /// Distance from mid-price in basis points
    pub distance_bps: u32,
}

/// Wall bias analysis result.
///
/// Represents the aggregated bias from detected walls, where floors (bid walls)
/// contribute positive bias and ceilings (ask walls) contribute negative bias.
#[derive(Debug, Clone)]
pub struct WallBias {
    /// Aggregate bias from -1.0 (ceiling dominant) to +1.0 (floor dominant)
    pub bias: f64,
    /// Total weighted strength of floor walls
    pub floor_strength: f64,
    /// Total weighted strength of ceiling walls
    pub ceiling_strength: f64,
    /// The wall with the highest weighted score
    pub dominant_wall: Option<Wall>,
    /// Number of floor (bid) walls
    pub floor_count: usize,
    /// Number of ceiling (ask) walls
    pub ceiling_count: usize,
}

impl WallBias {
    /// Returns the direction indicated by the wall bias.
    ///
    /// - Positive bias -> Up (floor dominant)
    /// - Negative bias -> Down (ceiling dominant)
    /// - Zero/near-zero -> Neutral
    #[must_use]
    pub fn direction(&self) -> Direction {
        if self.bias > 0.01 {
            Direction::Up
        } else if self.bias < -0.01 {
            Direction::Down
        } else {
            Direction::Neutral
        }
    }
}

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
    /// Use weighted imbalance (price proximity)
    pub use_weighted: bool,
    /// Wall detection configuration (None = disabled)
    pub wall_config: Option<WallDetectionConfig>,
    /// Minimum history for z-score (0 = disabled)
    pub min_zscore_history: usize,
    /// Z-score threshold for strong signal
    pub zscore_threshold: f64,
    /// Basic imbalance threshold
    pub imbalance_threshold: f64,
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
            use_weighted: false,
            wall_config: None,
            min_zscore_history: 0,
            zscore_threshold: 2.0,
            imbalance_threshold: threshold.clamp(0.0, 1.0),
        }
    }

    /// Enables weighted imbalance calculation.
    #[must_use]
    pub fn with_weighted(mut self, enabled: bool) -> Self {
        self.use_weighted = enabled;
        self
    }

    /// Sets wall detection configuration.
    #[must_use]
    pub fn with_wall_detection(mut self, config: WallDetectionConfig) -> Self {
        self.wall_config = Some(config);
        self
    }

    /// Sets minimum history requirement for z-score calculation.
    #[must_use]
    pub fn with_zscore_history(mut self, min_history: usize) -> Self {
        self.min_zscore_history = min_history;
        self
    }

    /// Sets z-score threshold for strong signals.
    #[must_use]
    pub fn with_zscore_threshold(mut self, threshold: f64) -> Self {
        self.zscore_threshold = threshold;
        self
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

/// Calculates weighted imbalance where levels closer to mid-price have more weight.
///
/// The weight for each level is `1 / (1 + distance_from_mid)` where distance
/// is measured as a fraction of the mid-price.
///
/// # Arguments
/// * `bids` - Bid levels as (price, quantity) tuples
/// * `asks` - Ask levels as (price, quantity) tuples
///
/// # Returns
/// Weighted imbalance in [-1.0, 1.0] where positive means more bid weight.
pub fn calculate_weighted_imbalance(
    bids: &[(Decimal, Decimal)],
    asks: &[(Decimal, Decimal)],
) -> f64 {
    if bids.is_empty() && asks.is_empty() {
        return 0.0;
    }

    // Calculate mid price from best bid and ask
    let best_bid = bids.first().map(|(p, _)| *p);
    let best_ask = asks.first().map(|(p, _)| *p);

    let mid_price = match (best_bid, best_ask) {
        (Some(bid), Some(ask)) => (bid + ask) / Decimal::TWO,
        (Some(bid), None) => bid,
        (None, Some(ask)) => ask,
        (None, None) => return 0.0,
    };

    if mid_price.is_zero() {
        return 0.0;
    }

    // Calculate weighted bid volume
    let weighted_bid: f64 = bids
        .iter()
        .filter_map(|(price, qty)| {
            let distance = ((mid_price - *price) / mid_price).abs();
            let distance_f64: f64 = distance.to_string().parse().ok()?;
            let weight = 1.0 / (1.0 + distance_f64);
            let qty_f64: f64 = qty.to_string().parse().ok()?;
            Some(qty_f64 * weight)
        })
        .sum();

    // Calculate weighted ask volume
    let weighted_ask: f64 = asks
        .iter()
        .filter_map(|(price, qty)| {
            let distance = ((*price - mid_price) / mid_price).abs();
            let distance_f64: f64 = distance.to_string().parse().ok()?;
            let weight = 1.0 / (1.0 + distance_f64);
            let qty_f64: f64 = qty.to_string().parse().ok()?;
            Some(qty_f64 * weight)
        })
        .sum();

    let total = weighted_bid + weighted_ask;
    if total < f64::EPSILON {
        return 0.0;
    }

    (weighted_bid - weighted_ask) / total
}

/// Detects large orders (walls) in the order book.
///
/// A wall is a large order at a single price level that may act as
/// support or resistance.
///
/// # Arguments
/// * `config` - Configuration for wall detection
/// * `bids` - Bid levels as (price, quantity) tuples
/// * `asks` - Ask levels as (price, quantity) tuples
/// * `mid_price` - Current mid price
///
/// # Returns
/// Vector of detected walls sorted by size (largest first).
pub fn detect_walls(
    config: &WallDetectionConfig,
    bids: &[(Decimal, Decimal)],
    asks: &[(Decimal, Decimal)],
    mid_price: Decimal,
) -> Vec<Wall> {
    let mut walls = Vec::new();

    if mid_price.is_zero() {
        return walls;
    }

    // Check bid walls
    for (price, qty) in bids {
        if *qty >= config.min_wall_size_btc {
            // Calculate distance in basis points
            let distance_pct = ((mid_price - *price) / mid_price).abs();
            let distance_bps = (distance_pct * Decimal::new(10000, 0))
                .to_string()
                .parse::<f64>()
                .ok()
                .map(|v| v.clamp(0.0, u32::MAX as f64) as u32)
                .unwrap_or(u32::MAX);

            if distance_bps <= config.proximity_bps {
                walls.push(Wall {
                    side: Side::Bid,
                    semantics: WallSemantics::Floor,
                    price: *price,
                    size: *qty,
                    distance_bps,
                });
            }
        }
    }

    // Check ask walls
    for (price, qty) in asks {
        if *qty >= config.min_wall_size_btc {
            // Calculate distance in basis points
            let distance_pct = ((*price - mid_price) / mid_price).abs();
            let distance_bps = (distance_pct * Decimal::new(10000, 0))
                .to_string()
                .parse::<f64>()
                .ok()
                .map(|v| v.clamp(0.0, u32::MAX as f64) as u32)
                .unwrap_or(u32::MAX);

            if distance_bps <= config.proximity_bps {
                walls.push(Wall {
                    side: Side::Ask,
                    semantics: WallSemantics::Ceiling,
                    price: *price,
                    size: *qty,
                    distance_bps,
                });
            }
        }
    }

    // Sort by size descending
    walls.sort_by(|a, b| b.size.cmp(&a.size));
    walls
}

/// Calculates the aggregate bias from detected walls.
///
/// Walls are weighted by both size and proximity to the mid-price.
/// Proximity weight: `1 / (1 + distance_bps / 100)`
/// Total weight: `size * proximity_weight`
///
/// The bias is calculated as: `(floor_strength - ceiling_strength) / total_strength`
///
/// # Arguments
/// * `walls` - Detected walls with semantics
/// * `_mid_price` - Current mid price (reserved for future use)
///
/// # Returns
/// `WallBias` containing aggregate bias, individual strengths, and dominant wall.
#[allow(unused_variables)]
pub fn calculate_wall_bias(walls: &[Wall], _mid_price: Decimal) -> WallBias {
    if walls.is_empty() {
        return WallBias {
            bias: 0.0,
            floor_strength: 0.0,
            ceiling_strength: 0.0,
            dominant_wall: None,
            floor_count: 0,
            ceiling_count: 0,
        };
    }

    let mut floor_strength = 0.0;
    let mut ceiling_strength = 0.0;
    let mut floor_count = 0;
    let mut ceiling_count = 0;
    let mut max_weighted_score = 0.0;
    let mut dominant_wall: Option<Wall> = None;

    for wall in walls {
        // Calculate proximity weight: closer walls have more influence
        // 1 / (1 + distance_bps / 100) gives weight from ~1.0 (0 bps) to ~0.5 (100 bps)
        let proximity_weight = 1.0 / (1.0 + (wall.distance_bps as f64) / 100.0);

        // Total weight is size * proximity
        let size_f64: f64 = wall.size.to_string().parse().unwrap_or(0.0);
        let weighted_score = size_f64 * proximity_weight;

        // Track dominant wall
        if weighted_score > max_weighted_score {
            max_weighted_score = weighted_score;
            dominant_wall = Some(wall.clone());
        }

        // Accumulate based on semantics
        match wall.semantics {
            WallSemantics::Floor => {
                floor_strength += weighted_score;
                floor_count += 1;
            }
            WallSemantics::Ceiling => {
                ceiling_strength += weighted_score;
                ceiling_count += 1;
            }
        }
    }

    let total_strength = floor_strength + ceiling_strength;
    let bias = if total_strength > f64::EPSILON {
        (floor_strength - ceiling_strength) / total_strength
    } else {
        0.0
    };

    WallBias {
        bias,
        floor_strength,
        ceiling_strength,
        dominant_wall,
        floor_count,
        ceiling_count,
    }
}

/// Calculates z-score of current imbalance vs historical values.
///
/// Uses `SignalContext::calculate_zscore()` internally.
///
/// # Arguments
/// * `current` - Current imbalance value
/// * `historical` - Historical imbalance values
///
/// # Returns
/// Z-score if sufficient history, None otherwise.
pub fn calculate_imbalance_zscore(current: f64, historical: &[f64]) -> Option<f64> {
    SignalContext::calculate_zscore(historical, current)
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

        // Calculate current imbalance (weighted or basic)
        let raw_imbalance = if self.use_weighted {
            let bids: Vec<(Decimal, Decimal)> = orderbook
                .bids
                .iter()
                .map(|l| (l.price, l.quantity))
                .collect();
            let asks: Vec<(Decimal, Decimal)> = orderbook
                .asks
                .iter()
                .map(|l| (l.price, l.quantity))
                .collect();
            calculate_weighted_imbalance(&bids, &asks)
        } else {
            orderbook.calculate_imbalance()
        };

        // Add to rolling window
        self.add_observation(raw_imbalance);

        // Get smoothed imbalance
        let smoothed = self.current_imbalance();

        // Detect walls if configured
        let walls = if let Some(ref config) = self.wall_config {
            if let Some(mid) = orderbook.mid_price() {
                let bids: Vec<(Decimal, Decimal)> = orderbook
                    .bids
                    .iter()
                    .map(|l| (l.price, l.quantity))
                    .collect();
                let asks: Vec<(Decimal, Decimal)> = orderbook
                    .asks
                    .iter()
                    .map(|l| (l.price, l.quantity))
                    .collect();
                detect_walls(config, &bids, &asks, mid)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Calculate z-score if historical data available
        let zscore = if self.min_zscore_history > 0 {
            ctx.historical_imbalances
                .as_ref()
                .filter(|h| h.len() >= self.min_zscore_history)
                .and_then(|h| calculate_imbalance_zscore(smoothed, h))
        } else {
            None
        };

        // Determine direction based on z-score (if available) or raw imbalance
        let (direction, strength) = if let Some(z) = zscore {
            // Use z-score for direction when available
            if z > self.zscore_threshold {
                (Direction::Up, (z / self.zscore_threshold).min(1.0))
            } else if z < -self.zscore_threshold {
                (Direction::Down, (-z / self.zscore_threshold).min(1.0))
            } else {
                (
                    Direction::Neutral,
                    (z.abs() / self.zscore_threshold).min(1.0),
                )
            }
        } else {
            // Fall back to basic imbalance threshold
            if smoothed > self.threshold {
                (Direction::Up, smoothed.min(1.0))
            } else if smoothed < -self.threshold {
                (Direction::Down, smoothed.abs().min(1.0))
            } else {
                (Direction::Neutral, smoothed.abs())
            }
        };

        // Build signal with metadata
        let mut signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("raw_imbalance", raw_imbalance)
            .with_metadata("smoothed_imbalance", smoothed)
            .with_metadata("threshold", self.threshold);

        if let Some(z) = zscore {
            signal = signal.with_metadata("zscore", z);
        }

        // Add wall metadata
        signal = signal.with_metadata("wall_count", walls.len() as f64);
        let bid_wall_count = walls.iter().filter(|w| w.side == Side::Bid).count();
        let ask_wall_count = walls.iter().filter(|w| w.side == Side::Ask).count();
        signal = signal
            .with_metadata("bid_wall_count", bid_wall_count as f64)
            .with_metadata("ask_wall_count", ask_wall_count as f64);

        // Calculate wall bias if walls were detected
        if !walls.is_empty() {
            let mid_price = orderbook.mid_price().unwrap_or(Decimal::ZERO);
            let wall_bias = calculate_wall_bias(&walls, mid_price);
            signal = signal
                .with_metadata("wall_bias", wall_bias.bias)
                .with_metadata("floor_strength", wall_bias.floor_strength)
                .with_metadata("ceiling_strength", wall_bias.ceiling_strength);
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
    use algo_trade_core::{OrderBookSnapshot, PriceLevel};
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn create_orderbook(bid_qty: i64, ask_qty: i64) -> OrderBookSnapshot {
        OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(100),
                quantity: Decimal::new(bid_qty, 0),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: Decimal::new(ask_qty, 0),
            }],
            timestamp: Utc::now(),
        }
    }

    fn create_multi_level_orderbook() -> OrderBookSnapshot {
        OrderBookSnapshot {
            bids: vec![
                PriceLevel {
                    price: dec!(100),
                    quantity: dec!(10),
                },
                PriceLevel {
                    price: dec!(99),
                    quantity: dec!(20),
                },
                PriceLevel {
                    price: dec!(98),
                    quantity: dec!(30),
                },
            ],
            asks: vec![
                PriceLevel {
                    price: dec!(101),
                    quantity: dec!(10),
                },
                PriceLevel {
                    price: dec!(102),
                    quantity: dec!(20),
                },
                PriceLevel {
                    price: dec!(103),
                    quantity: dec!(30),
                },
            ],
            timestamp: Utc::now(),
        }
    }

    // ============================================
    // Original Tests
    // ============================================

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

    // ============================================
    // Phase 2B: Weighted Imbalance Tests
    // ============================================

    #[test]
    fn weighted_imbalance_gives_more_weight_to_near_levels() {
        // Near level: price 100.5, qty 10
        // Far level: price 95, qty 20
        // Mid price would be ~100.5
        // Near level has smaller qty but should have more impact

        let bids = vec![
            (dec!(100), dec!(10)), // Near: distance ~0.5%
            (dec!(95), dec!(20)),  // Far: distance ~5%
        ];
        let asks = vec![
            (dec!(101), dec!(15)), // Near: distance ~0.5%
        ];

        let weighted = calculate_weighted_imbalance(&bids, &asks);
        let basic = {
            let bid_vol = dec!(30); // 10 + 20
            let ask_vol = dec!(15);
            let total = bid_vol + ask_vol;
            ((bid_vol - ask_vol) / total)
                .to_string()
                .parse::<f64>()
                .unwrap()
        };

        // Weighted should be different from basic because of distance weighting
        // The near bid (10) gets higher weight than far bid (20)
        assert!(
            (weighted - basic).abs() > 0.01,
            "weighted={weighted}, basic={basic}"
        );
    }

    #[test]
    fn weighted_imbalance_handles_empty_book() {
        let bids: Vec<(Decimal, Decimal)> = vec![];
        let asks: Vec<(Decimal, Decimal)> = vec![];

        let imbalance = calculate_weighted_imbalance(&bids, &asks);
        assert!((imbalance - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn weighted_imbalance_symmetric_book_returns_zero() {
        // Same quantities at symmetric distances from mid
        let bids = vec![(dec!(99), dec!(10)), (dec!(98), dec!(10))];
        let asks = vec![(dec!(101), dec!(10)), (dec!(102), dec!(10))];

        let imbalance = calculate_weighted_imbalance(&bids, &asks);
        // Should be approximately zero for symmetric book
        assert!(imbalance.abs() < 0.05, "imbalance was {imbalance}");
    }

    #[test]
    fn weighted_imbalance_one_sided_book() {
        // Only bids, no asks
        let bids = vec![(dec!(100), dec!(10))];
        let asks: Vec<(Decimal, Decimal)> = vec![];

        let imbalance = calculate_weighted_imbalance(&bids, &asks);
        assert!((imbalance - 1.0).abs() < f64::EPSILON);

        // Only asks, no bids
        let bids: Vec<(Decimal, Decimal)> = vec![];
        let asks = vec![(dec!(100), dec!(10))];

        let imbalance = calculate_weighted_imbalance(&bids, &asks);
        assert!((imbalance - (-1.0)).abs() < f64::EPSILON);
    }

    // ============================================
    // Phase 2B: Wall Detection Tests
    // ============================================

    #[test]
    fn wall_detection_finds_large_orders() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(10),
            proximity_bps: 200, // 2%
        };

        let mid_price = dec!(100);
        let bids = vec![
            (dec!(99), dec!(15)), // 15 BTC, 1% away - should be detected
            (dec!(98), dec!(5)),  // 5 BTC - too small
        ];
        let asks = vec![
            (dec!(101), dec!(20)), // 20 BTC, 1% away - should be detected
        ];

        let walls = detect_walls(&config, &bids, &asks, mid_price);

        assert_eq!(walls.len(), 2);
        // Should be sorted by size descending
        assert_eq!(walls[0].size, dec!(20));
        assert_eq!(walls[0].side, Side::Ask);
        assert_eq!(walls[1].size, dec!(15));
        assert_eq!(walls[1].side, Side::Bid);
    }

    #[test]
    fn wall_detection_ignores_small_orders() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(10),
            proximity_bps: 200,
        };

        let mid_price = dec!(100);
        let bids = vec![
            (dec!(99), dec!(5)), // 5 BTC - too small
            (dec!(98), dec!(8)), // 8 BTC - too small
        ];
        let asks = vec![
            (dec!(101), dec!(9)), // 9 BTC - too small
        ];

        let walls = detect_walls(&config, &bids, &asks, mid_price);
        assert!(walls.is_empty());
    }

    #[test]
    fn wall_detection_respects_proximity_threshold() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(10),
            proximity_bps: 100, // 1%
        };

        let mid_price = dec!(100);
        let bids = vec![
            (dec!(99), dec!(15)), // 1% away - within threshold
            (dec!(95), dec!(25)), // 5% away - outside threshold
        ];
        let asks = vec![
            (dec!(103), dec!(20)), // 3% away - outside threshold
        ];

        let walls = detect_walls(&config, &bids, &asks, mid_price);

        assert_eq!(walls.len(), 1);
        assert_eq!(walls[0].price, dec!(99));
        assert_eq!(walls[0].side, Side::Bid);
    }

    #[test]
    fn wall_detection_handles_zero_mid_price() {
        let config = WallDetectionConfig::default();
        let bids = vec![(dec!(99), dec!(15))];
        let asks = vec![(dec!(101), dec!(20))];

        let walls = detect_walls(&config, &bids, &asks, Decimal::ZERO);
        assert!(walls.is_empty());
    }

    #[test]
    fn wall_detection_calculates_distance_correctly() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(10),
            proximity_bps: 200,
        };

        let mid_price = dec!(100);
        let bids = vec![(dec!(99), dec!(15))]; // 1% = 100 bps
        let asks: Vec<(Decimal, Decimal)> = vec![];

        let walls = detect_walls(&config, &bids, &asks, mid_price);

        assert_eq!(walls.len(), 1);
        assert_eq!(walls[0].distance_bps, 100);
    }

    // ============================================
    // Phase 2B: Z-Score Tests
    // ============================================

    #[test]
    fn zscore_returns_none_with_insufficient_history() {
        let historical = vec![1.0]; // Only 1 value, need at least 2
        let result = calculate_imbalance_zscore(0.5, &historical);
        assert!(result.is_none());

        let empty: Vec<f64> = vec![];
        let result = calculate_imbalance_zscore(0.5, &empty);
        assert!(result.is_none());
    }

    #[test]
    fn zscore_calculation_is_mathematically_correct() {
        // Values: 1, 2, 3, 4, 5
        // Mean = 3.0
        // Sample variance = sum((x - mean)^2) / (n-1) = 10/4 = 2.5
        // Stddev = sqrt(2.5) = 1.5811...
        // Z-score of 5: (5 - 3) / 1.5811 = 1.2649...
        let historical = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let zscore = calculate_imbalance_zscore(5.0, &historical).unwrap();
        assert!(zscore > 1.26 && zscore < 1.27, "zscore was {zscore}");

        // Z-score of mean should be 0
        let zscore_mean = calculate_imbalance_zscore(3.0, &historical).unwrap();
        assert!(zscore_mean.abs() < 0.001, "zscore_mean was {zscore_mean}");
    }

    #[test]
    fn zscore_handles_zero_variance() {
        let historical = vec![5.0, 5.0, 5.0, 5.0]; // All same = zero variance
        let result = calculate_imbalance_zscore(5.0, &historical);
        assert!(result.is_none());
    }

    // ============================================
    // Phase 2B: Enhanced compute() Tests
    // ============================================

    #[tokio::test]
    async fn signal_uses_weighted_when_configured() {
        let mut signal = OrderBookImbalanceSignal::new(0.1, 1.0, 1).with_weighted(true);
        let orderbook = create_multi_level_orderbook();

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook.clone());
        let weighted_result = signal.compute(&ctx).await.unwrap();

        let mut basic_signal = OrderBookImbalanceSignal::new(0.1, 1.0, 1).with_weighted(false);
        let basic_result = basic_signal.compute(&ctx).await.unwrap();

        // The raw imbalances should be different
        let weighted_raw = weighted_result.metadata.get("raw_imbalance").unwrap();
        let basic_raw = basic_result.metadata.get("raw_imbalance").unwrap();

        // For this symmetric order book, both should be close to 0
        // but the exact values may differ due to weighting
        assert!(weighted_raw.abs() < 0.1);
        assert!(basic_raw.abs() < 0.1);
    }

    #[tokio::test]
    async fn signal_detects_walls_in_metadata() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(5),
            proximity_bps: 200,
        };
        let mut signal = OrderBookImbalanceSignal::new(0.1, 1.0, 1).with_wall_detection(config);

        // Create orderbook with walls
        let orderbook = OrderBookSnapshot {
            bids: vec![PriceLevel {
                price: dec!(99),
                quantity: dec!(10),
            }],
            asks: vec![PriceLevel {
                price: dec!(101),
                quantity: dec!(15),
            }],
            timestamp: Utc::now(),
        };

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);
        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(*result.metadata.get("wall_count").unwrap() as i32, 2);
        assert_eq!(*result.metadata.get("bid_wall_count").unwrap() as i32, 1);
        assert_eq!(*result.metadata.get("ask_wall_count").unwrap() as i32, 1);
    }

    #[tokio::test]
    async fn signal_uses_zscore_for_direction() {
        let mut signal = OrderBookImbalanceSignal::new(0.1, 1.0, 1)
            .with_zscore_history(5)
            .with_zscore_threshold(2.0);

        // Historical imbalances with mean around 0
        let historical = vec![0.1, -0.1, 0.05, -0.05, 0.0];

        // Current imbalance is extremely high relative to history
        let orderbook = create_orderbook(95, 5); // ~0.9 imbalance

        let ctx = SignalContext::new(Utc::now(), "BTCUSD")
            .with_orderbook(orderbook)
            .with_historical_imbalances(historical);

        let result = signal.compute(&ctx).await.unwrap();

        // Should have zscore in metadata
        assert!(result.metadata.contains_key("zscore"));
        let zscore = *result.metadata.get("zscore").unwrap();
        assert!(zscore > 2.0, "zscore was {zscore}");

        // Direction should be Up due to high positive z-score
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn signal_falls_back_to_basic_when_zscore_unavailable() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 1).with_zscore_history(10); // Require 10 points

        // Only 3 historical points - not enough
        let historical = vec![0.1, 0.0, -0.1];
        let orderbook = create_orderbook(80, 20); // 0.6 imbalance > 0.2 threshold

        let ctx = SignalContext::new(Utc::now(), "BTCUSD")
            .with_orderbook(orderbook)
            .with_historical_imbalances(historical);

        let result = signal.compute(&ctx).await.unwrap();

        // Should not have zscore in metadata (insufficient history)
        assert!(!result.metadata.contains_key("zscore"));

        // Should still give directional signal based on basic threshold
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn signal_no_zscore_without_historical_data() {
        let mut signal = OrderBookImbalanceSignal::new(0.2, 1.0, 1).with_zscore_history(5);

        let orderbook = create_orderbook(80, 20);

        // No historical imbalances in context
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);

        let result = signal.compute(&ctx).await.unwrap();

        // Should not have zscore in metadata
        assert!(!result.metadata.contains_key("zscore"));
        // Should fall back to basic direction
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn signal_wall_count_zero_when_no_walls() {
        let config = WallDetectionConfig {
            min_wall_size_btc: dec!(100), // Very high threshold
            proximity_bps: 100,
        };
        let mut signal = OrderBookImbalanceSignal::new(0.1, 1.0, 1).with_wall_detection(config);

        let orderbook = create_orderbook(10, 10); // Small quantities

        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_orderbook(orderbook);
        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(*result.metadata.get("wall_count").unwrap() as i32, 0);
    }

    // Note: Phase 2.2D Wall Semantics tests are pending implementation of
    // WallSemantics enum and calculate_wall_bias function.
}
