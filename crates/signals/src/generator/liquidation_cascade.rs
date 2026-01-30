//! Liquidation cascade signal generator.
//!
//! Generates trading signals based on liquidation volume and cascades.
//! Large liquidations can trigger further liquidations, creating momentum.
//!
//! ## Signal Modes
//!
//! - **Cascade**: Follow liquidation direction (momentum trading)
//! - **Exhaustion**: Bet on reversal after exhaustion
//! - **Combined**: Weight both factors
//!
//! ## Key Concepts
//!
//! - **Net Delta**: Measures directional bias from liquidation imbalance
//! - **Cascade Detection**: Identifies high-volume, high-imbalance events
//! - **Exhaustion Detection**: Identifies post-spike volume decline for reversal

use algo_trade_core::{
    Direction, LiquidationAggregate, SignalContext, SignalGenerator, SignalValue,
};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::VecDeque;

// ============================================
// Net Delta Calculation
// ============================================

/// Calculates the net delta from liquidation aggregates.
///
/// Returns a value in [-1.0, 1.0] where:
/// - Positive = more long liquidations (bearish momentum - longs being stopped out)
/// - Negative = more short liquidations (bullish momentum - shorts being squeezed)
/// - Zero = balanced or no liquidations
///
/// # Arguments
/// * `agg` - Liquidation aggregate data
///
/// # Example
/// ```
/// use algo_trade_core::LiquidationAggregate;
/// use chrono::Utc;
/// use rust_decimal_macros::dec;
/// use algo_trade_signals::generator::liquidation_cascade::calculate_net_delta;
///
/// let agg = LiquidationAggregate {
///     timestamp: Utc::now(),
///     window_minutes: 5,
///     long_volume_usd: dec!(100000),
///     short_volume_usd: dec!(50000),
///     net_delta_usd: dec!(50000),
///     count_long: 10,
///     count_short: 5,
/// };
///
/// let delta = calculate_net_delta(&agg);
/// assert!(delta > 0.0); // More longs liquidated = bearish
/// ```
pub fn calculate_net_delta(agg: &LiquidationAggregate) -> f64 {
    let total = agg.total_volume();
    if total.is_zero() {
        return 0.0;
    }

    // Calculate imbalance: (long - short) / total
    let imbalance = agg.net_delta_usd / total;

    // Convert to f64, clamped to [-1, 1]
    imbalance
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0)
        .clamp(-1.0, 1.0)
}

// ============================================
// Cascade Detection
// ============================================

/// Configuration for cascade detection.
#[derive(Debug, Clone)]
pub struct CascadeConfig {
    /// Minimum total volume in USD to consider (e.g., $100,000)
    pub min_volume_usd: Decimal,
    /// Minimum imbalance ratio to trigger cascade (e.g., 0.6 = 60% imbalance)
    pub imbalance_threshold: f64,
}

impl Default for CascadeConfig {
    fn default() -> Self {
        Self {
            min_volume_usd: Decimal::new(100_000, 0), // $100,000
            imbalance_threshold: 0.6,
        }
    }
}

impl CascadeConfig {
    /// Creates a new cascade config with custom thresholds.
    #[must_use]
    pub fn new(min_volume_usd: Decimal, imbalance_threshold: f64) -> Self {
        Self {
            min_volume_usd,
            imbalance_threshold: imbalance_threshold.clamp(0.0, 1.0),
        }
    }
}

/// Detects if current liquidation data represents a cascade event.
///
/// A cascade is detected when:
/// 1. Total volume exceeds the minimum threshold
/// 2. Imbalance ratio exceeds the threshold
///
/// # Arguments
/// * `agg` - Liquidation aggregate data
/// * `config` - Cascade detection configuration
///
/// # Returns
/// `true` if a cascade is detected, `false` otherwise
pub fn is_cascade(agg: &LiquidationAggregate, config: &CascadeConfig) -> bool {
    let total = agg.total_volume();

    // Check minimum volume
    if total < config.min_volume_usd {
        return false;
    }

    // Check imbalance
    let imbalance = agg.imbalance_ratio().unwrap_or(0.0).abs();
    imbalance >= config.imbalance_threshold
}

// ============================================
// Exhaustion Detection
// ============================================

/// Configuration for exhaustion detection.
#[derive(Debug, Clone)]
pub struct ExhaustionConfig {
    /// Multiple of average volume to consider a spike (e.g., 3.0 = 3x average)
    pub spike_threshold_multiple: f64,
    /// Volume decline ratio to detect exhaustion (e.g., 0.3 = 70% decline from spike)
    pub volume_decline_ratio: f64,
}

impl Default for ExhaustionConfig {
    fn default() -> Self {
        Self {
            spike_threshold_multiple: 3.0,
            volume_decline_ratio: 0.3,
        }
    }
}

impl ExhaustionConfig {
    /// Creates a new exhaustion config with custom thresholds.
    #[must_use]
    pub fn new(spike_threshold_multiple: f64, volume_decline_ratio: f64) -> Self {
        Self {
            spike_threshold_multiple: spike_threshold_multiple.max(1.0),
            volume_decline_ratio: volume_decline_ratio.clamp(0.0, 1.0),
        }
    }
}

/// Represents a detected exhaustion signal.
#[derive(Debug, Clone)]
pub struct ExhaustionSignal {
    /// Direction of expected reversal
    pub direction: Direction,
    /// Volume during the spike period
    pub spike_volume: Decimal,
    /// Current volume (post-decline)
    pub current_volume: Decimal,
    /// Decline percentage (0.0 to 1.0)
    pub decline_ratio: f64,
}

/// Detects exhaustion after a liquidation spike.
///
/// Exhaustion is detected when:
/// 1. Previous period had volume >= spike_threshold_multiple * average
/// 2. Current period volume has declined to <= volume_decline_ratio * previous
///
/// The reversal direction is opposite to the spike's dominant liquidation side:
/// - If spike was mostly longs liquidated, expect price to reverse Up
/// - If spike was mostly shorts liquidated, expect price to reverse Down
///
/// # Arguments
/// * `current` - Current liquidation aggregate
/// * `previous` - Previous period liquidation aggregate (the potential spike)
/// * `average_volume` - Historical average volume for comparison
/// * `config` - Exhaustion detection configuration
///
/// # Returns
/// `Some(ExhaustionSignal)` if exhaustion detected, `None` otherwise
pub fn detect_exhaustion(
    current: &LiquidationAggregate,
    previous: &LiquidationAggregate,
    average_volume: Decimal,
    config: &ExhaustionConfig,
) -> Option<ExhaustionSignal> {
    // Check if previous period was a spike
    let prev_volume = previous.total_volume();

    if average_volume.is_zero() {
        return None;
    }

    // Calculate spike multiple
    let spike_multiple: f64 = (prev_volume / average_volume)
        .to_string()
        .parse()
        .unwrap_or(0.0);

    if spike_multiple < config.spike_threshold_multiple {
        return None;
    }

    // Check if current volume has declined
    let current_volume = current.total_volume();

    if prev_volume.is_zero() {
        return None;
    }

    let current_ratio: f64 = (current_volume / prev_volume)
        .to_string()
        .parse()
        .unwrap_or(1.0);

    if current_ratio > config.volume_decline_ratio {
        return None;
    }

    // Determine reversal direction based on spike's dominant side
    // If mostly longs were liquidated (positive net delta), price was falling
    // Exhaustion suggests reversal -> Up
    let direction = if previous.net_delta_usd > Decimal::ZERO {
        Direction::Up // Reversal from long liquidations (price was falling)
    } else if previous.net_delta_usd < Decimal::ZERO {
        Direction::Down // Reversal from short liquidations (price was rising)
    } else {
        Direction::Neutral
    };

    Some(ExhaustionSignal {
        direction,
        spike_volume: prev_volume,
        current_volume,
        decline_ratio: 1.0 - current_ratio,
    })
}

// ============================================
// Signal Mode
// ============================================

/// Mode of operation for the liquidation signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiquidationSignalMode {
    /// Follow liquidation direction (momentum trading)
    /// Long liquidations = bearish, short liquidations = bullish
    #[default]
    Cascade,
    /// Bet on reversal after exhaustion
    Exhaustion,
    /// Weight both cascade and exhaustion factors
    Combined,
}

// ============================================
// Main Signal Generator
// ============================================

/// Signal generator based on liquidation cascade detection.
///
/// Large liquidations often trigger cascading liquidations as price moves
/// hit other leveraged positions' liquidation prices. This signal can operate
/// in multiple modes:
///
/// - **Cascade**: Follow the momentum of liquidations
/// - **Exhaustion**: Detect potential reversals after liquidation spikes
/// - **Combined**: Weight both factors
#[derive(Debug, Clone)]
pub struct LiquidationCascadeSignal {
    /// Name of this signal
    name: String,
    /// Minimum USD value to consider a significant liquidation (legacy mode)
    min_usd_threshold: Decimal,
    /// Multiple of average to consider as cascade trigger (legacy mode)
    cascade_multiple: f64,
    /// Weight for composite signal aggregation
    weight: f64,
    /// Rolling window of liquidation values
    history: VecDeque<Decimal>,
    /// Maximum size of rolling window
    window_size: usize,
    /// Cascade detection configuration
    pub cascade_config: CascadeConfig,
    /// Exhaustion detection configuration (if enabled)
    pub exhaustion_config: Option<ExhaustionConfig>,
    /// Signal mode
    pub signal_mode: LiquidationSignalMode,
    /// Minimum volume threshold for generating signals
    pub min_volume_threshold: Decimal,
    /// Previous liquidation aggregate for exhaustion detection
    previous_aggregate: Option<LiquidationAggregate>,
    /// Rolling average of total volumes
    volume_history: VecDeque<Decimal>,
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
            cascade_config: CascadeConfig::default(),
            exhaustion_config: None,
            signal_mode: LiquidationSignalMode::default(),
            min_volume_threshold: Decimal::new(10_000, 0), // $10,000 default
            previous_aggregate: None,
            volume_history: VecDeque::with_capacity(window_size),
        }
    }

    /// Sets the signal mode.
    #[must_use]
    pub fn with_mode(mut self, mode: LiquidationSignalMode) -> Self {
        self.signal_mode = mode;
        self
    }

    /// Sets the cascade configuration.
    #[must_use]
    pub fn with_cascade_config(mut self, config: CascadeConfig) -> Self {
        self.cascade_config = config;
        self
    }

    /// Enables exhaustion detection with the given configuration.
    #[must_use]
    pub fn with_exhaustion_config(mut self, config: ExhaustionConfig) -> Self {
        self.exhaustion_config = Some(config);
        self
    }

    /// Sets the minimum volume threshold.
    #[must_use]
    pub fn with_min_volume(mut self, min_volume: Decimal) -> Self {
        self.min_volume_threshold = min_volume;
        self
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

    /// Returns the average volume from history.
    #[must_use]
    pub fn average_volume(&self) -> Decimal {
        if self.volume_history.is_empty() {
            return Decimal::ZERO;
        }
        let sum: Decimal = self.volume_history.iter().copied().sum();
        sum / Decimal::from(self.volume_history.len())
    }

    /// Checks if a liquidation value triggers cascade detection (legacy mode).
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

    /// Adds a volume observation to history.
    fn add_volume_observation(&mut self, volume: Decimal) {
        if self.volume_history.len() >= self.window_size {
            self.volume_history.pop_front();
        }
        self.volume_history.push_back(volume);
    }

    /// Returns the number of observations in history.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.history.len()
    }

    /// Computes signal using the new aggregate-based mode.
    fn compute_aggregate_signal(&mut self, agg: &LiquidationAggregate) -> Result<SignalValue> {
        let total_volume = agg.total_volume();
        let net_delta = calculate_net_delta(agg);

        // Add to volume history
        self.add_volume_observation(total_volume);
        let avg_volume = self.average_volume();

        // Check minimum volume threshold
        if total_volume < self.min_volume_threshold {
            self.previous_aggregate = Some(agg.clone());
            return Ok(SignalValue::neutral()
                .with_metadata(
                    "total_volume",
                    total_volume.to_string().parse().unwrap_or(0.0),
                )
                .with_metadata("net_delta", net_delta)
                .with_metadata("below_threshold", 1.0));
        }

        let (direction, strength, signal_type) = match self.signal_mode {
            LiquidationSignalMode::Cascade => self.compute_cascade_signal(agg, net_delta),
            LiquidationSignalMode::Exhaustion => self.compute_exhaustion_signal(agg, avg_volume),
            LiquidationSignalMode::Combined => {
                self.compute_combined_signal(agg, net_delta, avg_volume)
            }
        };

        // Update previous aggregate for exhaustion detection
        self.previous_aggregate = Some(agg.clone());

        let vol_f64: f64 = total_volume.to_string().parse().unwrap_or(0.0);
        let avg_vol_f64: f64 = avg_volume.to_string().parse().unwrap_or(0.0);

        Ok(SignalValue::new(direction, strength, 0.0)?
            .with_metadata("total_volume", vol_f64)
            .with_metadata("average_volume", avg_vol_f64)
            .with_metadata("net_delta", net_delta)
            .with_metadata(
                "long_volume",
                agg.long_volume_usd.to_string().parse().unwrap_or(0.0),
            )
            .with_metadata(
                "short_volume",
                agg.short_volume_usd.to_string().parse().unwrap_or(0.0),
            )
            .with_metadata("signal_type", signal_type))
    }

    /// Computes cascade-based signal (momentum following).
    fn compute_cascade_signal(
        &self,
        agg: &LiquidationAggregate,
        net_delta: f64,
    ) -> (Direction, f64, f64) {
        let is_cascade = is_cascade(agg, &self.cascade_config);

        if !is_cascade {
            return (Direction::Neutral, 0.0, 0.0);
        }

        // Direction based on net delta
        // More longs liquidated (positive delta) = bearish (price falling)
        // More shorts liquidated (negative delta) = bullish (price rising)
        let direction = if net_delta > 0.0 {
            Direction::Down // Follow bearish momentum
        } else if net_delta < 0.0 {
            Direction::Up // Follow bullish momentum
        } else {
            Direction::Neutral
        };

        // Strength based on imbalance magnitude
        let strength = net_delta.abs().clamp(0.0, 1.0);

        (direction, strength, 1.0) // signal_type = 1.0 for cascade
    }

    /// Computes exhaustion-based signal (reversal).
    fn compute_exhaustion_signal(
        &self,
        current: &LiquidationAggregate,
        avg_volume: Decimal,
    ) -> (Direction, f64, f64) {
        let Some(ref config) = self.exhaustion_config else {
            return (Direction::Neutral, 0.0, 0.0);
        };

        let Some(ref previous) = self.previous_aggregate else {
            return (Direction::Neutral, 0.0, 0.0);
        };

        match detect_exhaustion(current, previous, avg_volume, config) {
            Some(exhaustion) => {
                let strength = exhaustion.decline_ratio.clamp(0.0, 1.0);
                (exhaustion.direction, strength, 2.0) // signal_type = 2.0 for exhaustion
            }
            None => (Direction::Neutral, 0.0, 0.0),
        }
    }

    /// Computes combined signal (weighted cascade + exhaustion).
    fn compute_combined_signal(
        &self,
        agg: &LiquidationAggregate,
        net_delta: f64,
        avg_volume: Decimal,
    ) -> (Direction, f64, f64) {
        let (cascade_dir, cascade_strength, _) = self.compute_cascade_signal(agg, net_delta);
        let (exhaust_dir, exhaust_strength, _) = self.compute_exhaustion_signal(agg, avg_volume);

        // If both agree on direction
        if cascade_dir == exhaust_dir && cascade_dir != Direction::Neutral {
            let combined_strength = (cascade_strength + exhaust_strength) / 2.0;
            return (cascade_dir, combined_strength.clamp(0.0, 1.0), 3.0);
        }

        // If exhaustion signal is present, it takes priority (reversal)
        if exhaust_strength > 0.0 {
            return (exhaust_dir, exhaust_strength, 2.0);
        }

        // Otherwise use cascade
        if cascade_strength > 0.0 {
            return (cascade_dir, cascade_strength, 1.0);
        }

        (Direction::Neutral, 0.0, 0.0)
    }
}

#[async_trait]
impl SignalGenerator for LiquidationCascadeSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // First try to use aggregate data (new mode)
        if let Some(ref agg) = ctx.liquidation_aggregates {
            return self.compute_aggregate_signal(agg);
        }

        // Fall back to legacy mode using liquidation_usd
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

            let strength =
                ((ratio - self.cascade_multiple) / self.cascade_multiple).clamp(0.0, 1.0);

            // Without direction info, we return neutral direction but positive strength
            // This allows composite signals to use the strength as a volatility indicator
            (Direction::Neutral, strength)
        } else {
            (Direction::Neutral, 0.0)
        };

        // Create signal
        let liq_f64: f64 = liq_value.to_string().parse().unwrap_or(0.0);
        let avg_f64: f64 = self
            .average_liquidation()
            .to_string()
            .parse()
            .unwrap_or(0.0);

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

    // ============================================
    // Helper Functions
    // ============================================

    fn make_aggregate(long_volume: Decimal, short_volume: Decimal) -> LiquidationAggregate {
        LiquidationAggregate {
            timestamp: Utc::now(),
            window_minutes: 5,
            long_volume_usd: long_volume,
            short_volume_usd: short_volume,
            net_delta_usd: long_volume - short_volume,
            count_long: 10,
            count_short: 5,
        }
    }

    // ============================================
    // Net Delta Tests
    // ============================================

    #[test]
    fn net_delta_positive_for_long_cascade() {
        // More longs liquidated = positive net delta
        let agg = make_aggregate(dec!(100000), dec!(20000));

        let delta = calculate_net_delta(&agg);

        // (100000 - 20000) / 120000 = 80000/120000 = 0.666...
        assert!(delta > 0.6, "Expected delta > 0.6, got {}", delta);
        assert!(delta < 0.7, "Expected delta < 0.7, got {}", delta);
    }

    #[test]
    fn net_delta_negative_for_short_cascade() {
        // More shorts liquidated = negative net delta
        let agg = make_aggregate(dec!(20000), dec!(100000));

        let delta = calculate_net_delta(&agg);

        // (20000 - 100000) / 120000 = -80000/120000 = -0.666...
        assert!(delta < -0.6, "Expected delta < -0.6, got {}", delta);
        assert!(delta > -0.7, "Expected delta > -0.7, got {}", delta);
    }

    #[test]
    fn net_delta_zero_for_balanced() {
        let agg = make_aggregate(dec!(50000), dec!(50000));

        let delta = calculate_net_delta(&agg);

        assert!(delta.abs() < 0.001, "Expected delta ~= 0, got {}", delta);
    }

    #[test]
    fn net_delta_zero_for_no_liquidations() {
        let agg = make_aggregate(Decimal::ZERO, Decimal::ZERO);

        let delta = calculate_net_delta(&agg);

        assert!(delta.abs() < 0.001, "Expected delta = 0, got {}", delta);
    }

    // ============================================
    // Cascade Detection Tests
    // ============================================

    #[test]
    fn cascade_detected_with_high_volume_and_imbalance() {
        let agg = make_aggregate(dec!(150000), dec!(10000));
        let config = CascadeConfig::new(dec!(100000), 0.6);

        let result = is_cascade(&agg, &config);

        assert!(result, "Expected cascade to be detected");
    }

    #[test]
    fn no_cascade_with_low_volume() {
        // High imbalance but low volume
        let agg = make_aggregate(dec!(90000), dec!(5000));
        let config = CascadeConfig::new(dec!(100000), 0.6);

        let result = is_cascade(&agg, &config);

        assert!(!result, "Expected no cascade due to low volume");
    }

    #[test]
    fn no_cascade_with_low_imbalance() {
        // High volume but low imbalance
        let agg = make_aggregate(dec!(80000), dec!(70000));
        let config = CascadeConfig::new(dec!(100000), 0.6);

        let result = is_cascade(&agg, &config);

        assert!(!result, "Expected no cascade due to low imbalance");
    }

    #[test]
    fn cascade_config_default_values() {
        let config = CascadeConfig::default();

        assert_eq!(config.min_volume_usd, dec!(100000));
        assert!((config.imbalance_threshold - 0.6).abs() < 0.001);
    }

    #[test]
    fn cascade_config_clamps_threshold() {
        let config = CascadeConfig::new(dec!(1000), 1.5);

        assert!((config.imbalance_threshold - 1.0).abs() < 0.001);
    }

    // ============================================
    // Exhaustion Detection Tests
    // ============================================

    #[test]
    fn exhaustion_detected_after_long_spike_decline() {
        let config = ExhaustionConfig::new(3.0, 0.3);
        let average_volume = dec!(50000);

        // Previous period was a spike (4x average with long bias)
        let previous = make_aggregate(dec!(180000), dec!(20000));

        // Current period has declined significantly
        let current = make_aggregate(dec!(30000), dec!(10000));

        let result = detect_exhaustion(&current, &previous, average_volume, &config);

        assert!(result.is_some(), "Expected exhaustion to be detected");
        let exhaustion = result.unwrap();
        assert_eq!(
            exhaustion.direction,
            Direction::Up,
            "Expected reversal Up after long liquidations"
        );
        assert!(
            exhaustion.decline_ratio > 0.7,
            "Expected decline ratio > 0.7"
        );
    }

    #[test]
    fn exhaustion_detected_after_short_spike_decline() {
        let config = ExhaustionConfig::new(3.0, 0.3);
        let average_volume = dec!(50000);

        // Previous period was a spike (4x average with short bias)
        let previous = make_aggregate(dec!(20000), dec!(180000));

        // Current period has declined significantly
        let current = make_aggregate(dec!(10000), dec!(30000));

        let result = detect_exhaustion(&current, &previous, average_volume, &config);

        assert!(result.is_some(), "Expected exhaustion to be detected");
        let exhaustion = result.unwrap();
        assert_eq!(
            exhaustion.direction,
            Direction::Down,
            "Expected reversal Down after short liquidations"
        );
    }

    #[test]
    fn no_exhaustion_when_volume_continues() {
        let config = ExhaustionConfig::new(3.0, 0.3);
        let average_volume = dec!(50000);

        // Previous period was a spike
        let previous = make_aggregate(dec!(180000), dec!(20000));

        // Current period volume is still high (no decline)
        let current = make_aggregate(dec!(150000), dec!(50000));

        let result = detect_exhaustion(&current, &previous, average_volume, &config);

        assert!(
            result.is_none(),
            "Expected no exhaustion when volume continues"
        );
    }

    #[test]
    fn no_exhaustion_without_spike() {
        let config = ExhaustionConfig::new(3.0, 0.3);
        let average_volume = dec!(50000);

        // Previous period was not a spike (only 2x average)
        let previous = make_aggregate(dec!(80000), dec!(20000));

        // Current period has declined
        let current = make_aggregate(dec!(10000), dec!(5000));

        let result = detect_exhaustion(&current, &previous, average_volume, &config);

        assert!(
            result.is_none(),
            "Expected no exhaustion without initial spike"
        );
    }

    #[test]
    fn no_exhaustion_with_zero_average() {
        let config = ExhaustionConfig::default();
        let average_volume = Decimal::ZERO;
        let previous = make_aggregate(dec!(100000), dec!(50000));
        let current = make_aggregate(dec!(10000), dec!(5000));

        let result = detect_exhaustion(&current, &previous, average_volume, &config);

        assert!(result.is_none(), "Expected no exhaustion with zero average");
    }

    #[test]
    fn exhaustion_config_default_values() {
        let config = ExhaustionConfig::default();

        assert!((config.spike_threshold_multiple - 3.0).abs() < 0.001);
        assert!((config.volume_decline_ratio - 0.3).abs() < 0.001);
    }

    #[test]
    fn exhaustion_config_clamps_values() {
        let config = ExhaustionConfig::new(0.5, 1.5);

        assert!((config.spike_threshold_multiple - 1.0).abs() < 0.001);
        assert!((config.volume_decline_ratio - 1.0).abs() < 0.001);
    }

    // ============================================
    // Signal Generator Tests (Aggregate Mode)
    // ============================================

    #[tokio::test]
    async fn signal_cascade_mode_bearish_on_long_liquidations() {
        let mut signal = LiquidationCascadeSignal::default()
            .with_mode(LiquidationSignalMode::Cascade)
            .with_cascade_config(CascadeConfig::new(dec!(50000), 0.5))
            .with_min_volume(dec!(50000));

        let agg = make_aggregate(dec!(150000), dec!(10000));
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(agg);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(
            result.direction,
            Direction::Down,
            "Expected bearish on long liquidations"
        );
        assert!(result.strength > 0.5, "Expected significant strength");
    }

    #[tokio::test]
    async fn signal_cascade_mode_bullish_on_short_liquidations() {
        let mut signal = LiquidationCascadeSignal::default()
            .with_mode(LiquidationSignalMode::Cascade)
            .with_cascade_config(CascadeConfig::new(dec!(50000), 0.5))
            .with_min_volume(dec!(50000));

        let agg = make_aggregate(dec!(10000), dec!(150000));
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(agg);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(
            result.direction,
            Direction::Up,
            "Expected bullish on short liquidations"
        );
        assert!(result.strength > 0.5, "Expected significant strength");
    }

    #[tokio::test]
    async fn signal_returns_neutral_below_threshold() {
        let mut signal = LiquidationCascadeSignal::default()
            .with_mode(LiquidationSignalMode::Cascade)
            .with_min_volume(dec!(100000));

        let agg = make_aggregate(dec!(40000), dec!(10000)); // Total 50k < 100k
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(agg);

        let result = signal.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
        assert!(result.metadata.get("below_threshold").is_some());
    }

    #[tokio::test]
    async fn signal_exhaustion_mode_detects_reversal() {
        let mut signal = LiquidationCascadeSignal::default()
            .with_mode(LiquidationSignalMode::Exhaustion)
            .with_exhaustion_config(ExhaustionConfig::new(2.0, 0.3))
            .with_min_volume(dec!(10000));

        // Build up some volume history first to establish average (around 50k)
        for _ in 0..5 {
            let normal_agg = make_aggregate(dec!(30000), dec!(20000)); // 50k total
            let ctx =
                SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(normal_agg);
            let _ = signal.compute(&ctx).await.unwrap();
        }

        // Then add spike (4x the average = 200k, well above 2x threshold)
        let spike_agg = make_aggregate(dec!(180000), dec!(20000)); // 200k total with long bias
        let ctx1 = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(spike_agg);
        let _ = signal.compute(&ctx1).await.unwrap();

        // Finally, volume declines (40k is 20% of spike 200k, below 30% threshold)
        let decline_agg = make_aggregate(dec!(30000), dec!(10000)); // 40k total
        let ctx2 =
            SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(decline_agg);
        let result = signal.compute(&ctx2).await.unwrap();

        assert_eq!(
            result.direction,
            Direction::Up,
            "Expected reversal after long liquidation spike"
        );
    }

    #[tokio::test]
    async fn signal_metadata_contains_volumes() {
        let mut signal = LiquidationCascadeSignal::default()
            .with_mode(LiquidationSignalMode::Cascade)
            .with_min_volume(dec!(1000));

        let agg = make_aggregate(dec!(100000), dec!(50000));
        let ctx = SignalContext::new(Utc::now(), "BTCUSD").with_liquidation_aggregates(agg);

        let result = signal.compute(&ctx).await.unwrap();

        assert!(result.metadata.contains_key("total_volume"));
        assert!(result.metadata.contains_key("net_delta"));
        assert!(result.metadata.contains_key("long_volume"));
        assert!(result.metadata.contains_key("short_volume"));
    }

    // ============================================
    // Legacy Mode Tests (backward compatibility)
    // ============================================

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

    // ============================================
    // Builder Pattern Tests
    // ============================================

    #[test]
    fn builder_with_mode() {
        let signal =
            LiquidationCascadeSignal::default().with_mode(LiquidationSignalMode::Exhaustion);

        assert_eq!(signal.signal_mode, LiquidationSignalMode::Exhaustion);
    }

    #[test]
    fn builder_with_cascade_config() {
        let config = CascadeConfig::new(dec!(200000), 0.8);
        let signal = LiquidationCascadeSignal::default().with_cascade_config(config);

        assert_eq!(signal.cascade_config.min_volume_usd, dec!(200000));
        assert!((signal.cascade_config.imbalance_threshold - 0.8).abs() < 0.001);
    }

    #[test]
    fn builder_with_exhaustion_config() {
        let config = ExhaustionConfig::new(4.0, 0.2);
        let signal = LiquidationCascadeSignal::default().with_exhaustion_config(config);

        assert!(signal.exhaustion_config.is_some());
        let cfg = signal.exhaustion_config.unwrap();
        assert!((cfg.spike_threshold_multiple - 4.0).abs() < 0.001);
    }

    #[test]
    fn builder_with_min_volume() {
        let signal = LiquidationCascadeSignal::default().with_min_volume(dec!(50000));

        assert_eq!(signal.min_volume_threshold, dec!(50000));
    }
}
