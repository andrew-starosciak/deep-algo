//! Liquidation ratio signal generator.
//!
//! Generates trading signals based on the ratio of long vs short liquidations
//! over a 24-hour period. This is a contrarian signal:
//!
//! - High long/short ratio (longs getting liquidated) = Bullish (reversal expected)
//! - Low long/short ratio (shorts getting liquidated) = Bearish (reversal expected)
//!
//! The hypothesis is that excessive liquidations on one side indicate
//! overleveraged positions being washed out, creating reversal opportunities.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

/// Configuration for the liquidation ratio signal.
#[derive(Debug, Clone)]
pub struct LiquidationRatioConfig {
    /// Ratio threshold above which to generate bullish signal.
    /// Default: 2.0 (2x more longs liquidated than shorts = bullish reversal)
    pub high_ratio_threshold: f64,

    /// Ratio threshold below which to generate bearish signal.
    /// Default: 0.5 (2x more shorts liquidated than longs = bearish reversal)
    pub low_ratio_threshold: f64,

    /// Minimum total volume in USD to generate a signal.
    /// Default: $100,000 (need significant volume for meaningful signal)
    pub min_volume_usd: Decimal,

    /// Weight for composite signal aggregation.
    pub weight: f64,
}

impl Default for LiquidationRatioConfig {
    fn default() -> Self {
        Self {
            high_ratio_threshold: 2.0,
            low_ratio_threshold: 0.5,
            min_volume_usd: Decimal::new(100_000, 0), // $100,000
            weight: 1.0,
        }
    }
}

impl LiquidationRatioConfig {
    /// Creates a new configuration with custom thresholds.
    #[must_use]
    pub fn new(
        high_ratio_threshold: f64,
        low_ratio_threshold: f64,
        min_volume_usd: Decimal,
    ) -> Self {
        Self {
            high_ratio_threshold: high_ratio_threshold.max(1.0),
            low_ratio_threshold: low_ratio_threshold.clamp(0.0, 1.0),
            min_volume_usd,
            weight: 1.0,
        }
    }

    /// Sets the weight for composite signal aggregation.
    #[must_use]
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }
}

/// 24-hour liquidation aggregate data for ratio calculation.
#[derive(Debug, Clone)]
pub struct LiquidationAggregate24h {
    /// Total long liquidation volume in USD over 24h.
    pub long_volume_usd: Decimal,
    /// Total short liquidation volume in USD over 24h.
    pub short_volume_usd: Decimal,
}

impl LiquidationAggregate24h {
    /// Creates a new 24h aggregate from volumes.
    #[must_use]
    pub fn new(long_volume_usd: Decimal, short_volume_usd: Decimal) -> Self {
        Self {
            long_volume_usd,
            short_volume_usd,
        }
    }

    /// Returns the total volume (long + short).
    #[must_use]
    pub fn total_volume(&self) -> Decimal {
        self.long_volume_usd + self.short_volume_usd
    }

    /// Returns the long/short ratio.
    ///
    /// Returns `None` if short volume is zero.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        if self.short_volume_usd.is_zero() {
            return None;
        }

        let ratio = self.long_volume_usd / self.short_volume_usd;
        ratio.to_string().parse().ok()
    }
}

/// Calculates signal direction and strength from liquidation ratio.
///
/// # Arguments
/// * `long_volume` - 24h long liquidation volume in USD
/// * `short_volume` - 24h short liquidation volume in USD
/// * `config` - Signal configuration
///
/// # Returns
/// `Some((Direction, strength))` if ratio is extreme, `None` otherwise
pub fn calculate_ratio_signal(
    long_volume: Decimal,
    short_volume: Decimal,
    config: &LiquidationRatioConfig,
) -> Option<(Direction, f64)> {
    // Check minimum volume
    let total = long_volume + short_volume;
    if total < config.min_volume_usd {
        return None;
    }

    // Calculate ratio (handle zero short volume)
    if short_volume.is_zero() {
        // All longs liquidated, extreme bullish reversal signal
        return Some((Direction::Up, 1.0));
    }

    let ratio: f64 = (long_volume / short_volume)
        .to_string()
        .parse()
        .unwrap_or(1.0);

    if ratio >= config.high_ratio_threshold {
        // High ratio = lots of longs liquidated = bullish reversal
        // Strength scales from 0 at threshold to 1 at 2x threshold
        let strength =
            ((ratio - config.high_ratio_threshold) / config.high_ratio_threshold).clamp(0.0, 1.0);
        Some((Direction::Up, strength))
    } else if ratio <= config.low_ratio_threshold {
        // Low ratio = lots of shorts liquidated = bearish reversal
        // Strength scales inversely
        let strength =
            ((config.low_ratio_threshold - ratio) / config.low_ratio_threshold).clamp(0.0, 1.0);
        Some((Direction::Down, strength))
    } else {
        None
    }
}

/// Signal generator based on 24-hour liquidation ratio.
///
/// This signal uses a contrarian approach:
/// - When longs are liquidated heavily (high L/S ratio), expect bullish reversal
/// - When shorts are liquidated heavily (low L/S ratio), expect bearish reversal
///
/// The hypothesis is that heavy liquidations on one side indicate
/// overleveraged positions being cleared, creating reversal opportunities.
#[derive(Debug, Clone)]
pub struct LiquidationRatioSignal {
    /// Signal name.
    name: String,
    /// Configuration.
    config: LiquidationRatioConfig,
    /// Cached 24h aggregate (set externally before compute).
    cached_aggregate: Option<LiquidationAggregate24h>,
}

impl Default for LiquidationRatioSignal {
    fn default() -> Self {
        Self::new(LiquidationRatioConfig::default())
    }
}

impl LiquidationRatioSignal {
    /// Creates a new `LiquidationRatioSignal` with the given configuration.
    #[must_use]
    pub fn new(config: LiquidationRatioConfig) -> Self {
        Self {
            name: "liquidation_ratio".to_string(),
            config,
            cached_aggregate: None,
        }
    }

    /// Sets the 24h aggregate data for signal computation.
    pub fn set_aggregate(&mut self, aggregate: LiquidationAggregate24h) {
        self.cached_aggregate = Some(aggregate);
    }

    /// Builder method to set the 24h aggregate.
    #[must_use]
    pub fn with_aggregate(mut self, aggregate: LiquidationAggregate24h) -> Self {
        self.cached_aggregate = Some(aggregate);
        self
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &LiquidationRatioConfig {
        &self.config
    }
}

#[async_trait]
impl SignalGenerator for LiquidationRatioSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Try to use liquidation_aggregates_24h from context first
        let aggregate = if let Some(ref agg) = ctx.liquidation_aggregates_24h {
            LiquidationAggregate24h::new(agg.long_volume_usd, agg.short_volume_usd)
        } else if let Some(ref cached) = self.cached_aggregate {
            cached.clone()
        } else {
            tracing::debug!("No 24h liquidation data available, returning neutral signal");
            return Ok(SignalValue::neutral());
        };

        let total_volume = aggregate.total_volume();
        let ratio = aggregate.ratio();

        // Calculate signal
        let result = calculate_ratio_signal(
            aggregate.long_volume_usd,
            aggregate.short_volume_usd,
            &self.config,
        );

        let (direction, strength) = result.unwrap_or((Direction::Neutral, 0.0));

        // Build metadata
        let long_vol_f64: f64 = aggregate.long_volume_usd.to_string().parse().unwrap_or(0.0);
        let short_vol_f64: f64 = aggregate
            .short_volume_usd
            .to_string()
            .parse()
            .unwrap_or(0.0);
        let total_vol_f64: f64 = total_volume.to_string().parse().unwrap_or(0.0);

        let mut signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("long_volume_24h", long_vol_f64)
            .with_metadata("short_volume_24h", short_vol_f64)
            .with_metadata("total_volume_24h", total_vol_f64);

        if let Some(r) = ratio {
            signal = signal.with_metadata("ratio", r);
        }

        Ok(signal)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn weight(&self) -> f64 {
        self.config.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    // ============================================
    // TDD: Tests written FIRST (RED phase)
    // ============================================

    // --------------------------------------------
    // LiquidationAggregate24h Tests
    // --------------------------------------------

    #[test]
    fn aggregate_24h_total_volume_is_sum() {
        let agg = LiquidationAggregate24h::new(dec!(100000), dec!(50000));

        assert_eq!(agg.total_volume(), dec!(150000));
    }

    #[test]
    fn aggregate_24h_ratio_calculation() {
        let agg = LiquidationAggregate24h::new(dec!(200000), dec!(100000));

        let ratio = agg.ratio().unwrap();
        assert!(
            (ratio - 2.0).abs() < 0.01,
            "Expected ratio ~2.0, got {ratio}"
        );
    }

    #[test]
    fn aggregate_24h_ratio_returns_none_for_zero_short() {
        let agg = LiquidationAggregate24h::new(dec!(100000), Decimal::ZERO);

        assert!(agg.ratio().is_none());
    }

    // --------------------------------------------
    // LiquidationRatioConfig Tests
    // --------------------------------------------

    #[test]
    fn config_default_values() {
        let config = LiquidationRatioConfig::default();

        assert!((config.high_ratio_threshold - 2.0).abs() < 0.01);
        assert!((config.low_ratio_threshold - 0.5).abs() < 0.01);
        assert_eq!(config.min_volume_usd, dec!(100000));
        assert!((config.weight - 1.0).abs() < 0.01);
    }

    #[test]
    fn config_custom_values() {
        let config = LiquidationRatioConfig::new(3.0, 0.3, dec!(200000));

        assert!((config.high_ratio_threshold - 3.0).abs() < 0.01);
        assert!((config.low_ratio_threshold - 0.3).abs() < 0.01);
        assert_eq!(config.min_volume_usd, dec!(200000));
    }

    #[test]
    fn config_clamps_high_threshold_minimum() {
        let config = LiquidationRatioConfig::new(0.5, 0.5, dec!(100000));

        // High threshold should be at least 1.0
        assert!(config.high_ratio_threshold >= 1.0);
    }

    #[test]
    fn config_clamps_low_threshold() {
        let config = LiquidationRatioConfig::new(2.0, 1.5, dec!(100000));

        // Low threshold should be clamped to [0, 1]
        assert!(config.low_ratio_threshold <= 1.0);
        assert!(config.low_ratio_threshold >= 0.0);
    }

    #[test]
    fn config_with_weight_builder() {
        let config = LiquidationRatioConfig::default().with_weight(2.0);

        assert!((config.weight - 2.0).abs() < 0.01);
    }

    // --------------------------------------------
    // calculate_ratio_signal Tests
    // --------------------------------------------

    #[test]
    fn ratio_signal_returns_none_below_min_volume() {
        let config = LiquidationRatioConfig::default();

        // Total = 50k, below 100k threshold
        let result = calculate_ratio_signal(dec!(30000), dec!(20000), &config);

        assert!(result.is_none());
    }

    #[test]
    fn ratio_signal_bullish_on_high_ratio() {
        let config = LiquidationRatioConfig::default();

        // Ratio = 200k/50k = 4.0 > 2.0 threshold
        let result = calculate_ratio_signal(dec!(200000), dec!(50000), &config);

        assert!(result.is_some());
        let (direction, strength) = result.unwrap();
        assert_eq!(direction, Direction::Up);
        assert!(strength > 0.0);
    }

    #[test]
    fn ratio_signal_bearish_on_low_ratio() {
        let config = LiquidationRatioConfig::default();

        // Ratio = 50k/200k = 0.25 < 0.5 threshold
        let result = calculate_ratio_signal(dec!(50000), dec!(200000), &config);

        assert!(result.is_some());
        let (direction, strength) = result.unwrap();
        assert_eq!(direction, Direction::Down);
        assert!(strength > 0.0);
    }

    #[test]
    fn ratio_signal_neutral_in_normal_range() {
        let config = LiquidationRatioConfig::default();

        // Ratio = 100k/100k = 1.0, between 0.5 and 2.0
        let result = calculate_ratio_signal(dec!(100000), dec!(100000), &config);

        assert!(result.is_none());
    }

    #[test]
    fn ratio_signal_strength_scales_with_extremity() {
        let config = LiquidationRatioConfig::default();

        // Mild: ratio = 2.5 (just above 2.0)
        let mild = calculate_ratio_signal(dec!(250000), dec!(100000), &config);

        // Extreme: ratio = 4.0 (2x the threshold)
        let extreme = calculate_ratio_signal(dec!(400000), dec!(100000), &config);

        let (_, mild_strength) = mild.unwrap();
        let (_, extreme_strength) = extreme.unwrap();

        assert!(
            extreme_strength > mild_strength,
            "Extreme should have higher strength: {} > {}",
            extreme_strength,
            mild_strength
        );
    }

    #[test]
    fn ratio_signal_max_bullish_on_zero_short() {
        let config = LiquidationRatioConfig {
            min_volume_usd: dec!(50000),
            ..Default::default()
        };

        // All longs liquidated, no shorts
        let result = calculate_ratio_signal(dec!(100000), Decimal::ZERO, &config);

        assert!(result.is_some());
        let (direction, strength) = result.unwrap();
        assert_eq!(direction, Direction::Up);
        assert!((strength - 1.0).abs() < 0.01, "Expected max strength 1.0");
    }

    // --------------------------------------------
    // LiquidationRatioSignal Tests
    // --------------------------------------------

    #[test]
    fn signal_name_is_correct() {
        let signal = LiquidationRatioSignal::default();

        assert_eq!(signal.name(), "liquidation_ratio");
    }

    #[test]
    fn signal_weight_from_config() {
        let config = LiquidationRatioConfig::default().with_weight(1.5);
        let signal = LiquidationRatioSignal::new(config);

        assert!((signal.weight() - 1.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn signal_returns_neutral_without_data() {
        let mut signal = LiquidationRatioSignal::default();
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn signal_uses_cached_aggregate() {
        let mut signal = LiquidationRatioSignal::new(LiquidationRatioConfig {
            min_volume_usd: dec!(50000),
            ..Default::default()
        });

        // Set cached aggregate with high L/S ratio
        signal.set_aggregate(LiquidationAggregate24h::new(dec!(200000), dec!(50000)));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = signal.compute(&ctx).await.unwrap();

        // Should be bullish due to high L/S ratio (4.0 > 2.0)
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn signal_metadata_contains_volumes() {
        let mut signal = LiquidationRatioSignal::new(LiquidationRatioConfig {
            min_volume_usd: dec!(50000),
            ..Default::default()
        });

        signal.set_aggregate(LiquidationAggregate24h::new(dec!(100000), dec!(100000)));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("long_volume_24h"));
        assert!(result.metadata.contains_key("short_volume_24h"));
        assert!(result.metadata.contains_key("total_volume_24h"));
        assert!(result.metadata.contains_key("ratio"));
    }

    #[tokio::test]
    async fn signal_with_aggregate_builder() {
        let signal = LiquidationRatioSignal::new(LiquidationRatioConfig {
            min_volume_usd: dec!(50000),
            ..Default::default()
        })
        .with_aggregate(LiquidationAggregate24h::new(dec!(50000), dec!(200000)));

        let mut signal = signal;
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = signal.compute(&ctx).await.unwrap();

        // Should be bearish due to low L/S ratio (0.25 < 0.5)
        assert_eq!(result.direction, Direction::Down);
    }
}
