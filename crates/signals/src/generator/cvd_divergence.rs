//! CVD Divergence signal generator.
//!
//! Detects divergences between price action and Cumulative Volume Delta (CVD).
//! - Bearish divergence: Price makes new high, CVD makes lower high
//! - Bullish divergence: Price makes new low, CVD makes higher low
//! - Absorption: High CVD with flat price suggests institutional accumulation/distribution

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::VecDeque;

/// Configuration for CVD divergence detection.
#[derive(Debug, Clone)]
pub struct CvdDivergenceConfig {
    /// Number of periods to look back for divergence detection
    pub lookback_periods: usize,
    /// Minimum price change threshold as a ratio (e.g., 0.001 = 0.1%)
    pub min_price_change: f64,
    /// Threshold for "flat" price in absorption detection (e.g., 0.001 = 0.1%)
    pub flat_price_threshold: f64,
    /// Minimum CVD magnitude for absorption signal (in base currency)
    pub absorption_cvd_threshold: f64,
    /// Weight for this signal in composite
    pub weight: f64,
}

impl Default for CvdDivergenceConfig {
    fn default() -> Self {
        Self {
            lookback_periods: 10,
            min_price_change: 0.001,        // 0.1%
            flat_price_threshold: 0.001,    // 0.1%
            absorption_cvd_threshold: 10.0, // 10 BTC
            weight: 1.0,
        }
    }
}

/// Result of divergence detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceType {
    /// Price new high, CVD lower high -> bearish
    Bearish,
    /// Price new low, CVD higher low -> bullish
    Bullish,
    /// No divergence detected
    None,
}

/// Result of absorption detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsorptionType {
    /// High buy CVD with flat price -> institutional accumulation
    BuyAbsorption,
    /// High sell CVD with flat price -> institutional distribution
    SellAbsorption,
    /// No absorption detected
    None,
}

/// CVD divergence signal generator.
///
/// Detects divergences between price and cumulative volume delta,
/// which can signal potential reversals.
#[derive(Debug, Clone)]
pub struct CvdDivergenceSignal {
    /// Configuration
    config: CvdDivergenceConfig,
    /// Name of this signal
    name: String,
    /// Rolling window of CVD values
    cvd_history: VecDeque<f64>,
    /// Rolling window of price values
    price_history: VecDeque<f64>,
    /// Current cumulative CVD
    cumulative_cvd: f64,
}

impl Default for CvdDivergenceSignal {
    fn default() -> Self {
        Self::new(CvdDivergenceConfig::default())
    }
}

impl CvdDivergenceSignal {
    /// Creates a new CVD divergence signal with the given configuration.
    #[must_use]
    pub fn new(config: CvdDivergenceConfig) -> Self {
        Self {
            cvd_history: VecDeque::with_capacity(config.lookback_periods),
            price_history: VecDeque::with_capacity(config.lookback_periods),
            cumulative_cvd: 0.0,
            name: "cvd_divergence".to_string(),
            config,
        }
    }

    /// Adds a new observation to the history.
    pub fn add_observation(&mut self, cvd_delta: f64, price: f64) {
        self.cumulative_cvd += cvd_delta;

        if self.cvd_history.len() >= self.config.lookback_periods {
            self.cvd_history.pop_front();
            self.price_history.pop_front();
        }

        self.cvd_history.push_back(self.cumulative_cvd);
        self.price_history.push_back(price);
    }

    /// Resets the cumulative CVD and history.
    pub fn reset(&mut self) {
        self.cumulative_cvd = 0.0;
        self.cvd_history.clear();
        self.price_history.clear();
    }

    /// Returns the current cumulative CVD.
    #[must_use]
    pub fn cumulative_cvd(&self) -> f64 {
        self.cumulative_cvd
    }

    /// Returns the number of observations in history.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.cvd_history.len()
    }

    /// Detects divergence based on current history.
    #[must_use]
    pub fn detect_divergence(&self) -> DivergenceType {
        if self.price_history.len() < 3 {
            return DivergenceType::None;
        }

        let prices: Vec<f64> = self.price_history.iter().copied().collect();
        let cvds: Vec<f64> = self.cvd_history.iter().copied().collect();

        detect_divergence(&prices, &cvds, self.config.min_price_change)
    }

    /// Detects absorption based on current history.
    #[must_use]
    pub fn detect_absorption(&self) -> AbsorptionType {
        if self.price_history.len() < 2 {
            return AbsorptionType::None;
        }

        let prices: Vec<f64> = self.price_history.iter().copied().collect();

        // Get recent CVD change
        let cvd_change = if self.cvd_history.len() >= 2 {
            let recent: Vec<f64> = self.cvd_history.iter().copied().collect();
            recent.last().unwrap_or(&0.0) - recent.first().unwrap_or(&0.0)
        } else {
            0.0
        };

        detect_absorption(
            &prices,
            cvd_change,
            self.config.flat_price_threshold,
            self.config.absorption_cvd_threshold,
        )
    }
}

/// Pure function to detect bearish or bullish divergence.
///
/// # Arguments
/// * `prices` - Price series (chronological order, oldest first)
/// * `cvds` - Cumulative CVD series (same order as prices)
/// * `min_price_change` - Minimum price change threshold as ratio
///
/// # Returns
/// Divergence type detected
#[must_use]
pub fn detect_divergence(prices: &[f64], cvds: &[f64], min_price_change: f64) -> DivergenceType {
    if prices.len() < 3 || cvds.len() < 3 || prices.len() != cvds.len() {
        return DivergenceType::None;
    }

    // Find local extremes
    let bearish = detect_bearish_divergence(prices, cvds, min_price_change);
    let bullish = detect_bullish_divergence(prices, cvds, min_price_change);

    // Return the detected divergence (bearish takes precedence if both detected)
    if bearish {
        DivergenceType::Bearish
    } else if bullish {
        DivergenceType::Bullish
    } else {
        DivergenceType::None
    }
}

/// Detects bearish divergence: price new high, CVD lower high.
///
/// # Arguments
/// * `prices` - Price series (chronological order)
/// * `cvds` - Cumulative CVD series
/// * `min_price_change` - Minimum price change threshold
///
/// # Returns
/// True if bearish divergence detected
#[must_use]
pub fn detect_bearish_divergence(prices: &[f64], cvds: &[f64], min_price_change: f64) -> bool {
    if prices.len() < 3 || cvds.len() < 3 {
        return false;
    }

    // Filter out NaN/Inf values - return false if any non-finite values present
    if prices.iter().any(|p| !p.is_finite()) || cvds.iter().any(|c| !c.is_finite()) {
        return false;
    }

    // Find the highest price excluding the last point
    let (prev_high_idx, prev_high) = prices[..prices.len() - 1]
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, &v)| (i, v))
        .unwrap_or((0, prices[0]));

    let current_price = prices[prices.len() - 1];
    let current_cvd = cvds[cvds.len() - 1];
    let prev_cvd = cvds[prev_high_idx];

    // Check if price made new high (use minimum threshold to avoid division issues)
    const MIN_PRICE: f64 = 1e-10;
    let price_change_ratio = if prev_high.abs() > MIN_PRICE {
        (current_price - prev_high) / prev_high
    } else {
        0.0
    };

    let price_new_high = price_change_ratio > min_price_change;

    // Check if CVD made lower high
    let cvd_lower_high = current_cvd < prev_cvd;

    price_new_high && cvd_lower_high
}

/// Detects bullish divergence: price new low, CVD higher low.
///
/// # Arguments
/// * `prices` - Price series (chronological order)
/// * `cvds` - Cumulative CVD series
/// * `min_price_change` - Minimum price change threshold
///
/// # Returns
/// True if bullish divergence detected
#[must_use]
pub fn detect_bullish_divergence(prices: &[f64], cvds: &[f64], min_price_change: f64) -> bool {
    if prices.len() < 3 || cvds.len() < 3 {
        return false;
    }

    // Filter out NaN/Inf values - return false if any non-finite values present
    if prices.iter().any(|p| !p.is_finite()) || cvds.iter().any(|c| !c.is_finite()) {
        return false;
    }

    // Find the lowest price excluding the last point
    let (prev_low_idx, prev_low) = prices[..prices.len() - 1]
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, &v)| (i, v))
        .unwrap_or((0, prices[0]));

    let current_price = prices[prices.len() - 1];
    let current_cvd = cvds[cvds.len() - 1];
    let prev_cvd = cvds[prev_low_idx];

    // Check if price made new low (use minimum threshold to avoid division issues)
    const MIN_PRICE: f64 = 1e-10;
    let price_change_ratio = if prev_low.abs() > MIN_PRICE {
        (prev_low - current_price) / prev_low
    } else {
        0.0
    };

    let price_new_low = price_change_ratio > min_price_change;

    // Check if CVD made higher low
    let cvd_higher_low = current_cvd > prev_cvd;

    price_new_low && cvd_higher_low
}

/// Detects absorption: high CVD with flat price.
///
/// Absorption occurs when there's significant volume delta but price
/// barely moves, suggesting institutional activity absorbing the flow.
///
/// # Arguments
/// * `prices` - Price series (chronological order)
/// * `cvd_change` - CVD change over the period
/// * `flat_threshold` - Maximum price range for "flat" classification
/// * `cvd_threshold` - Minimum |CVD| for absorption signal
///
/// # Returns
/// Absorption type detected
#[must_use]
pub fn detect_absorption(
    prices: &[f64],
    cvd_change: f64,
    flat_threshold: f64,
    cvd_threshold: f64,
) -> AbsorptionType {
    if prices.len() < 2 {
        return AbsorptionType::None;
    }

    // Calculate price range
    let max_price = prices.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_price = prices.iter().copied().fold(f64::INFINITY, f64::min);
    let avg_price = (max_price + min_price) / 2.0;

    if avg_price <= 0.0 {
        return AbsorptionType::None;
    }

    let price_range_ratio = (max_price - min_price) / avg_price;

    // Check if price is flat
    let is_flat = price_range_ratio < flat_threshold;

    // Check if CVD is significant
    let cvd_magnitude = cvd_change.abs();
    let is_significant_cvd = cvd_magnitude > cvd_threshold;

    if is_flat && is_significant_cvd {
        if cvd_change > 0.0 {
            AbsorptionType::BuyAbsorption
        } else {
            AbsorptionType::SellAbsorption
        }
    } else {
        AbsorptionType::None
    }
}

/// Calculates divergence strength (0.0 to 1.0).
///
/// Strength is based on the magnitude of the divergence between
/// price and CVD movements.
#[must_use]
pub fn calculate_divergence_strength(prices: &[f64], cvds: &[f64]) -> f64 {
    if prices.len() < 2 || cvds.len() < 2 {
        return 0.0;
    }

    // Calculate normalized price change
    let price_change = prices.last().unwrap_or(&0.0) - prices.first().unwrap_or(&0.0);
    let price_range = prices.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b))
        - prices.iter().fold(f64::INFINITY, |a, &b| a.min(b));

    let price_change_norm = if price_range > 0.0 {
        price_change / price_range
    } else {
        0.0
    };

    // Calculate normalized CVD change
    let cvd_change = cvds.last().unwrap_or(&0.0) - cvds.first().unwrap_or(&0.0);
    let cvd_range = cvds.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b))
        - cvds.iter().fold(f64::INFINITY, |a, &b| a.min(b));

    let cvd_change_norm = if cvd_range > 0.0 {
        cvd_change / cvd_range
    } else {
        0.0
    };

    // Divergence strength is based on opposite movements
    // If price up and CVD down (or vice versa), we have divergence
    let divergence_magnitude = (price_change_norm - cvd_change_norm).abs();

    // Scale to 0-1 range (max divergence would be 2.0)
    (divergence_magnitude / 2.0).min(1.0)
}

#[async_trait]
impl SignalGenerator for CvdDivergenceSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Get mid price from context
        let price = match ctx.mid_price {
            Some(p) => {
                let p_f64: f64 = p.to_string().parse().unwrap_or(0.0);
                p_f64
            }
            None => {
                return Ok(SignalValue::neutral());
            }
        };

        // For now, we'll need CVD data passed in via context
        // In a full implementation, this would come from a CVD aggregate provider
        // For this signal, we expect the caller to provide CVD via metadata or a custom field

        // Get CVD from orderbook imbalance as a proxy
        // This is a simplification - in production, use actual trade tick data
        let cvd_delta = ctx
            .orderbook
            .as_ref()
            .map(|ob| ob.calculate_imbalance())
            .unwrap_or(0.0);

        // Add observation
        self.add_observation(cvd_delta, price);

        // Need sufficient history
        if self.observation_count() < 3 {
            return Ok(SignalValue::neutral());
        }

        // Detect divergence
        let divergence = self.detect_divergence();
        let absorption = self.detect_absorption();

        // Determine direction and strength
        let (direction, base_strength) = match divergence {
            DivergenceType::Bearish => (Direction::Down, 0.7),
            DivergenceType::Bullish => (Direction::Up, 0.7),
            DivergenceType::None => match absorption {
                AbsorptionType::BuyAbsorption => (Direction::Up, 0.5),
                AbsorptionType::SellAbsorption => (Direction::Down, 0.5),
                AbsorptionType::None => (Direction::Neutral, 0.0),
            },
        };

        // Calculate strength based on divergence magnitude
        let prices: Vec<f64> = self.price_history.iter().copied().collect();
        let cvds: Vec<f64> = self.cvd_history.iter().copied().collect();
        let strength_multiplier = calculate_divergence_strength(&prices, &cvds);
        let strength = (base_strength * (0.5 + strength_multiplier * 0.5)).min(1.0);

        // Build signal
        let mut signal = SignalValue::new(direction, strength, 0.0)?
            .with_metadata("cumulative_cvd", self.cumulative_cvd)
            .with_metadata("price", price);

        match divergence {
            DivergenceType::Bearish => {
                signal = signal.with_metadata("divergence_type", -1.0);
            }
            DivergenceType::Bullish => {
                signal = signal.with_metadata("divergence_type", 1.0);
            }
            DivergenceType::None => {}
        }

        match absorption {
            AbsorptionType::BuyAbsorption => {
                signal = signal.with_metadata("absorption_type", 1.0);
            }
            AbsorptionType::SellAbsorption => {
                signal = signal.with_metadata("absorption_type", -1.0);
            }
            AbsorptionType::None => {}
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
    use rust_decimal_macros::dec;

    // ============================================
    // Divergence Detection Tests (TDD RED -> GREEN)
    // ============================================

    #[test]
    fn test_bearish_divergence_price_high_cvd_low() {
        // Price: [100, 105, 110] - making new highs
        // CVD: [50, 45, 40] - making lower highs
        // Should detect bearish divergence
        let prices = vec![100.0, 105.0, 110.0];
        let cvds = vec![50.0, 45.0, 40.0];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::Bearish);

        let bearish = detect_bearish_divergence(&prices, &cvds, 0.001);
        assert!(bearish);
    }

    #[test]
    fn test_bullish_divergence_price_low_cvd_high() {
        // Price: [100, 95, 90] - making new lows
        // CVD: [-50, -45, -40] - making higher lows (less negative)
        // Should detect bullish divergence
        let prices = vec![100.0, 95.0, 90.0];
        let cvds = vec![-50.0, -45.0, -40.0];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::Bullish);

        let bullish = detect_bullish_divergence(&prices, &cvds, 0.001);
        assert!(bullish);
    }

    #[test]
    fn test_no_divergence_when_aligned() {
        // Price: [100, 105, 110] - rising
        // CVD: [50, 55, 60] - also rising
        // No divergence
        let prices = vec![100.0, 105.0, 110.0];
        let cvds = vec![50.0, 55.0, 60.0];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::None);
    }

    #[test]
    fn test_no_divergence_both_falling() {
        // Price: [100, 95, 90] - falling
        // CVD: [50, 45, 40] - also falling
        // No divergence (aligned)
        let prices = vec![100.0, 95.0, 90.0];
        let cvds = vec![50.0, 45.0, 40.0];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::None);
    }

    #[test]
    fn test_divergence_insufficient_data() {
        let prices = vec![100.0, 105.0];
        let cvds = vec![50.0, 45.0];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::None);
    }

    #[test]
    fn test_divergence_empty_data() {
        let prices: Vec<f64> = vec![];
        let cvds: Vec<f64> = vec![];

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::None);
    }

    #[test]
    fn test_divergence_mismatched_lengths() {
        let prices = vec![100.0, 105.0, 110.0];
        let cvds = vec![50.0, 45.0]; // Different length

        let result = detect_divergence(&prices, &cvds, 0.001);
        assert_eq!(result, DivergenceType::None);
    }

    // ============================================
    // Absorption Detection Tests
    // ============================================

    #[test]
    fn test_absorption_flat_price_high_buy_cvd() {
        // Price range < 0.1%, |CVD| > threshold -> buy absorption
        let prices = vec![100.0, 100.05, 100.08];
        let cvd_change = 15.0; // Positive, above threshold

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::BuyAbsorption);
    }

    #[test]
    fn test_absorption_flat_price_high_sell_cvd() {
        // Price range < 0.1%, |CVD| > threshold -> sell absorption
        let prices = vec![100.0, 99.95, 99.98];
        let cvd_change = -15.0; // Negative, above threshold

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::SellAbsorption);
    }

    #[test]
    fn test_no_absorption_volatile_price() {
        // Price range > threshold, so no absorption despite high CVD
        let prices = vec![100.0, 102.0, 98.0]; // 4% range
        let cvd_change = 15.0;

        let result = detect_absorption(&prices, cvd_change, 0.01, 10.0);
        assert_eq!(result, AbsorptionType::None);
    }

    #[test]
    fn test_no_absorption_low_cvd() {
        // Flat price but low CVD
        let prices = vec![100.0, 100.01, 100.02];
        let cvd_change = 2.0; // Below threshold

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::None);
    }

    #[test]
    fn test_absorption_empty_prices() {
        let prices: Vec<f64> = vec![];
        let cvd_change = 15.0;

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::None);
    }

    #[test]
    fn test_absorption_single_price() {
        let prices = vec![100.0];
        let cvd_change = 15.0;

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::None);
    }

    // ============================================
    // Divergence Strength Tests
    // ============================================

    #[test]
    fn test_divergence_strength_strong_divergence() {
        // Price up, CVD down - strong divergence
        let prices = vec![100.0, 105.0, 110.0];
        let cvds = vec![50.0, 40.0, 30.0];

        let strength = calculate_divergence_strength(&prices, &cvds);
        assert!(strength > 0.5, "Expected strong divergence, got {strength}");
    }

    #[test]
    fn test_divergence_strength_aligned() {
        // Price up, CVD up - no divergence
        let prices = vec![100.0, 105.0, 110.0];
        let cvds = vec![50.0, 55.0, 60.0];

        let strength = calculate_divergence_strength(&prices, &cvds);
        // Aligned movements should have lower strength
        assert!(strength < 0.5, "Expected weak divergence, got {strength}");
    }

    #[test]
    fn test_divergence_strength_empty() {
        let prices: Vec<f64> = vec![];
        let cvds: Vec<f64> = vec![];

        let strength = calculate_divergence_strength(&prices, &cvds);
        assert_eq!(strength, 0.0);
    }

    #[test]
    fn test_divergence_strength_single_point() {
        let prices = vec![100.0];
        let cvds = vec![50.0];

        let strength = calculate_divergence_strength(&prices, &cvds);
        assert_eq!(strength, 0.0);
    }

    // ============================================
    // CvdDivergenceSignal Integration Tests
    // ============================================

    #[test]
    fn test_signal_add_observation() {
        let mut signal = CvdDivergenceSignal::default();

        signal.add_observation(1.0, 100.0);
        assert_eq!(signal.observation_count(), 1);
        assert_eq!(signal.cumulative_cvd(), 1.0);

        signal.add_observation(2.0, 105.0);
        assert_eq!(signal.observation_count(), 2);
        assert_eq!(signal.cumulative_cvd(), 3.0);
    }

    #[test]
    fn test_signal_reset() {
        let mut signal = CvdDivergenceSignal::default();

        signal.add_observation(1.0, 100.0);
        signal.add_observation(2.0, 105.0);

        signal.reset();

        assert_eq!(signal.observation_count(), 0);
        assert_eq!(signal.cumulative_cvd(), 0.0);
    }

    #[test]
    fn test_signal_respects_lookback() {
        let config = CvdDivergenceConfig {
            lookback_periods: 3,
            ..Default::default()
        };
        let mut signal = CvdDivergenceSignal::new(config);

        // Add 5 observations, should only keep last 3
        for i in 1..=5 {
            signal.add_observation(i as f64, 100.0 + i as f64);
        }

        assert_eq!(signal.observation_count(), 3);
        // Cumulative CVD should be sum of all: 1+2+3+4+5 = 15
        assert_eq!(signal.cumulative_cvd(), 15.0);
    }

    #[test]
    fn test_signal_detect_bearish() {
        let config = CvdDivergenceConfig {
            lookback_periods: 10,
            min_price_change: 0.001,
            ..Default::default()
        };
        let mut signal = CvdDivergenceSignal::new(config);

        // Create bearish divergence: price up, CVD (cumulative) down
        signal.add_observation(10.0, 100.0); // CVD: 10
        signal.add_observation(-5.0, 105.0); // CVD: 5 (going down)
        signal.add_observation(-10.0, 110.0); // CVD: -5 (still going down while price up)

        let divergence = signal.detect_divergence();
        assert_eq!(divergence, DivergenceType::Bearish);
    }

    #[test]
    fn test_signal_detect_bullish() {
        let config = CvdDivergenceConfig {
            lookback_periods: 10,
            min_price_change: 0.001,
            ..Default::default()
        };
        let mut signal = CvdDivergenceSignal::new(config);

        // Create bullish divergence: price down, CVD (cumulative) up
        signal.add_observation(-10.0, 100.0); // CVD: -10
        signal.add_observation(5.0, 95.0); // CVD: -5 (going up)
        signal.add_observation(10.0, 90.0); // CVD: 5 (still going up while price down)

        let divergence = signal.detect_divergence();
        assert_eq!(divergence, DivergenceType::Bullish);
    }

    #[test]
    fn test_signal_config() {
        let config = CvdDivergenceConfig {
            lookback_periods: 20,
            min_price_change: 0.005,
            flat_price_threshold: 0.002,
            absorption_cvd_threshold: 20.0,
            weight: 1.5,
        };

        let signal = CvdDivergenceSignal::new(config);

        assert_eq!(signal.weight(), 1.5);
        assert_eq!(signal.name(), "cvd_divergence");
    }

    #[tokio::test]
    async fn test_signal_compute_insufficient_data() {
        let mut signal = CvdDivergenceSignal::default();
        let ctx = algo_trade_core::SignalContext::new(chrono::Utc::now(), "BTCUSD")
            .with_mid_price(dec!(50000));

        // First computation - not enough data
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn test_signal_compute_no_price() {
        let mut signal = CvdDivergenceSignal::default();
        let ctx = algo_trade_core::SignalContext::new(chrono::Utc::now(), "BTCUSD");
        // No mid_price set

        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Neutral);
    }

    // ============================================
    // Edge Cases
    // ============================================

    #[test]
    fn test_bearish_divergence_minimal_price_change() {
        // Price barely changes - below threshold
        let prices = vec![100.0, 100.001, 100.002];
        let cvds = vec![50.0, 45.0, 40.0];

        let result = detect_bearish_divergence(&prices, &cvds, 0.01); // 1% threshold
        assert!(!result); // Price change too small
    }

    #[test]
    fn test_bullish_divergence_minimal_price_change() {
        // Price barely changes - below threshold
        let prices = vec![100.0, 99.999, 99.998];
        let cvds = vec![-50.0, -45.0, -40.0];

        let result = detect_bullish_divergence(&prices, &cvds, 0.01); // 1% threshold
        assert!(!result); // Price change too small
    }

    #[test]
    fn test_absorption_zero_price() {
        // Edge case: zero or negative prices
        let prices = vec![0.0, 0.0, 0.0];
        let cvd_change = 15.0;

        let result = detect_absorption(&prices, cvd_change, 0.001, 10.0);
        assert_eq!(result, AbsorptionType::None);
    }

    #[test]
    fn test_divergence_with_nan() {
        // NaN handling
        let prices = vec![100.0, f64::NAN, 110.0];
        let cvds = vec![50.0, 45.0, 40.0];

        // Should not panic, just return None or handle gracefully
        let _ = detect_divergence(&prices, &cvds, 0.001);
    }

    #[test]
    fn test_strength_capped_at_one() {
        // Extreme divergence should still cap at 1.0
        let prices = vec![100.0, 200.0];
        let cvds = vec![100.0, -100.0];

        let strength = calculate_divergence_strength(&prices, &cvds);
        assert!(strength <= 1.0);
    }
}
