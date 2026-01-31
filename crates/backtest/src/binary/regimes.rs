//! Market regime analysis for binary outcome backtesting.
//!
//! This module provides classification of market conditions into regimes
//! (volatility, trend, time period) and computes performance metrics
//! segmented by regime to identify conditional edges.

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::metrics::BinaryMetrics;
use super::outcome::SettlementResult;

/// Volatility regime classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VolatilityRegime {
    /// Low volatility (below 33rd percentile).
    Low,
    /// Medium volatility (33rd to 67th percentile).
    Medium,
    /// High volatility (above 67th percentile).
    High,
}

/// Trend regime classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrendRegime {
    /// Bullish trend (positive returns above threshold).
    Bullish,
    /// Bearish trend (negative returns below threshold).
    Bearish,
    /// Ranging/sideways market (returns within threshold).
    Ranging,
}

/// Time period classification based on UTC hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimePeriod {
    /// Asia Open: 00:00 - 04:00 UTC.
    AsiaOpen,
    /// Asia Session: 04:00 - 08:00 UTC.
    AsiaSession,
    /// Europe Open: 08:00 - 12:00 UTC.
    EuropeOpen,
    /// Europe Session: 12:00 - 14:00 UTC.
    EuropeSession,
    /// US Open: 14:00 - 18:00 UTC.
    USOpen,
    /// US Session: 18:00 - 22:00 UTC.
    USSession,
    /// US Close: 22:00 - 24:00 UTC.
    USClose,
}

/// Regime label for a single data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeLabel {
    /// Volatility classification.
    pub volatility: VolatilityRegime,
    /// Trend classification.
    pub trend: TrendRegime,
    /// Time period classification.
    pub time_period: TimePeriod,
}

/// Combined regime state for tracking transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegimeCombination {
    /// Volatility regime.
    pub volatility: VolatilityRegime,
    /// Trend regime.
    pub trend: TrendRegime,
}

/// Metrics grouped by regime.
#[derive(Debug, Clone)]
pub struct RegimeMetrics {
    /// Metrics by volatility regime.
    pub by_volatility: HashMap<VolatilityRegime, BinaryMetrics>,
    /// Metrics by trend regime.
    pub by_trend: HashMap<TrendRegime, BinaryMetrics>,
    /// Metrics by time period.
    pub by_time_period: HashMap<TimePeriod, BinaryMetrics>,
    /// Metrics by combined volatility-trend regime.
    pub by_combination: HashMap<RegimeCombination, BinaryMetrics>,
    /// Number of regime transitions observed.
    pub regime_transitions: u32,
}

impl RegimeMetrics {
    /// Returns metrics for a specific volatility regime.
    #[must_use]
    pub fn volatility_metrics(&self, regime: VolatilityRegime) -> Option<&BinaryMetrics> {
        self.by_volatility.get(&regime)
    }

    /// Returns metrics for a specific trend regime.
    #[must_use]
    pub fn trend_metrics(&self, regime: TrendRegime) -> Option<&BinaryMetrics> {
        self.by_trend.get(&regime)
    }

    /// Returns metrics for a specific time period.
    #[must_use]
    pub fn time_period_metrics(&self, period: TimePeriod) -> Option<&BinaryMetrics> {
        self.by_time_period.get(&period)
    }
}

/// Configuration for regime analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeConfig {
    /// Low volatility percentile boundary (default: 0.33).
    pub volatility_low_percentile: f64,
    /// High volatility percentile boundary (default: 0.67).
    pub volatility_high_percentile: f64,
    /// Trend threshold for bullish/bearish classification (default: 0.001).
    pub trend_threshold: f64,
}

impl Default for RegimeConfig {
    fn default() -> Self {
        Self {
            volatility_low_percentile: 0.33,
            volatility_high_percentile: 0.67,
            trend_threshold: 0.001,
        }
    }
}

/// Analyzer for classifying market regimes and computing regime-specific metrics.
#[derive(Debug, Clone)]
pub struct RegimeAnalyzer {
    config: RegimeConfig,
}

impl RegimeAnalyzer {
    /// Creates a new regime analyzer with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: RegimeConfig::default(),
        }
    }

    /// Creates a new regime analyzer with custom configuration.
    #[must_use]
    pub fn with_config(config: RegimeConfig) -> Self {
        Self { config }
    }

    /// Classifies volatility based on percentile boundaries.
    ///
    /// # Arguments
    /// * `volatility` - Current volatility value
    /// * `volatilities` - Historical volatility values for percentile calculation
    ///
    /// # Returns
    /// The volatility regime classification
    #[must_use]
    pub fn classify_volatility(&self, volatility: f64, volatilities: &[f64]) -> VolatilityRegime {
        if volatilities.is_empty() {
            return VolatilityRegime::Medium;
        }

        let low_threshold = percentile(volatilities, self.config.volatility_low_percentile);
        let high_threshold = percentile(volatilities, self.config.volatility_high_percentile);

        if volatility < low_threshold {
            VolatilityRegime::Low
        } else if volatility > high_threshold {
            VolatilityRegime::High
        } else {
            VolatilityRegime::Medium
        }
    }

    /// Classifies trend based on price return threshold.
    ///
    /// # Arguments
    /// * `price_return` - The price return (as decimal, e.g., 0.01 for 1%)
    ///
    /// # Returns
    /// The trend regime classification
    #[must_use]
    pub fn classify_trend(&self, price_return: f64) -> TrendRegime {
        if price_return > self.config.trend_threshold {
            TrendRegime::Bullish
        } else if price_return < -self.config.trend_threshold {
            TrendRegime::Bearish
        } else {
            TrendRegime::Ranging
        }
    }

    /// Classifies time period from UTC timestamp.
    ///
    /// # Arguments
    /// * `timestamp` - UTC timestamp
    ///
    /// # Returns
    /// The time period classification
    #[must_use]
    pub fn classify_time_period(&self, timestamp: DateTime<Utc>) -> TimePeriod {
        let hour = timestamp.hour();
        match hour {
            0..=3 => TimePeriod::AsiaOpen,
            4..=7 => TimePeriod::AsiaSession,
            8..=11 => TimePeriod::EuropeOpen,
            12..=13 => TimePeriod::EuropeSession,
            14..=17 => TimePeriod::USOpen,
            18..=21 => TimePeriod::USSession,
            22..=23 => TimePeriod::USClose,
            _ => TimePeriod::AsiaOpen, // Should never happen
        }
    }

    /// Classifies a single data point into all regime dimensions.
    ///
    /// # Arguments
    /// * `volatility` - Current volatility value
    /// * `volatilities` - Historical volatility values
    /// * `price_return` - Current price return
    /// * `timestamp` - UTC timestamp
    ///
    /// # Returns
    /// Complete regime label
    #[must_use]
    pub fn classify(
        &self,
        volatility: f64,
        volatilities: &[f64],
        price_return: f64,
        timestamp: DateTime<Utc>,
    ) -> RegimeLabel {
        RegimeLabel {
            volatility: self.classify_volatility(volatility, volatilities),
            trend: self.classify_trend(price_return),
            time_period: self.classify_time_period(timestamp),
        }
    }

    /// Analyzes settlement results and returns metrics grouped by regime.
    ///
    /// # Arguments
    /// * `settlements` - Slice of settlement results with regime metadata
    /// * `volatilities` - Volatility value for each settlement
    ///
    /// # Returns
    /// Metrics segmented by each regime dimension
    #[must_use]
    pub fn analyze(&self, settlements: &[SettlementResult], volatilities: &[f64]) -> RegimeMetrics {
        if settlements.is_empty() || volatilities.is_empty() {
            return RegimeMetrics {
                by_volatility: HashMap::new(),
                by_trend: HashMap::new(),
                by_time_period: HashMap::new(),
                by_combination: HashMap::new(),
                regime_transitions: 0,
            };
        }

        // Group settlements by regime
        let mut by_volatility: HashMap<VolatilityRegime, Vec<SettlementResult>> = HashMap::new();
        let mut by_trend: HashMap<TrendRegime, Vec<SettlementResult>> = HashMap::new();
        let mut by_time_period: HashMap<TimePeriod, Vec<SettlementResult>> = HashMap::new();
        let mut by_combination: HashMap<RegimeCombination, Vec<SettlementResult>> = HashMap::new();

        let mut prev_combination: Option<RegimeCombination> = None;
        let mut transitions = 0u32;

        for (i, settlement) in settlements.iter().enumerate() {
            let vol = volatilities.get(i).copied().unwrap_or(0.0);
            let price_return = f64::try_from(settlement.price_return).unwrap_or(0.0);

            let label = self.classify(vol, volatilities, price_return, settlement.bet.timestamp);

            // Group by volatility
            by_volatility
                .entry(label.volatility)
                .or_default()
                .push(settlement.clone());

            // Group by trend
            by_trend
                .entry(label.trend)
                .or_default()
                .push(settlement.clone());

            // Group by time period
            by_time_period
                .entry(label.time_period)
                .or_default()
                .push(settlement.clone());

            // Group by combination
            let combination = RegimeCombination {
                volatility: label.volatility,
                trend: label.trend,
            };
            by_combination
                .entry(combination)
                .or_default()
                .push(settlement.clone());

            // Count transitions
            if let Some(prev) = prev_combination {
                if prev != combination {
                    transitions += 1;
                }
            }
            prev_combination = Some(combination);
        }

        // Compute metrics for each group
        let metrics_by_volatility = by_volatility
            .into_iter()
            .map(|(k, v)| (k, BinaryMetrics::from_settlements(&v)))
            .collect();

        let metrics_by_trend = by_trend
            .into_iter()
            .map(|(k, v)| (k, BinaryMetrics::from_settlements(&v)))
            .collect();

        let metrics_by_time_period = by_time_period
            .into_iter()
            .map(|(k, v)| (k, BinaryMetrics::from_settlements(&v)))
            .collect();

        let metrics_by_combination = by_combination
            .into_iter()
            .map(|(k, v)| (k, BinaryMetrics::from_settlements(&v)))
            .collect();

        RegimeMetrics {
            by_volatility: metrics_by_volatility,
            by_trend: metrics_by_trend,
            by_time_period: metrics_by_time_period,
            by_combination: metrics_by_combination,
            regime_transitions: transitions,
        }
    }
}

impl Default for RegimeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculates the percentile value from a slice of values.
///
/// # Arguments
/// * `values` - Slice of values (need not be sorted)
/// * `p` - Percentile (0.0 to 1.0)
///
/// # Returns
/// The value at the given percentile
fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    let idx = idx.min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet, BinaryOutcome};
    use chrono::TimeZone;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    // ============================================================
    // VolatilityRegime Tests
    // ============================================================

    #[test]
    fn volatility_regime_variants_are_distinct() {
        let low = VolatilityRegime::Low;
        let medium = VolatilityRegime::Medium;
        let high = VolatilityRegime::High;

        assert_ne!(low, medium);
        assert_ne!(medium, high);
        assert_ne!(low, high);
    }

    #[test]
    fn volatility_regime_serializes_correctly() {
        let low = VolatilityRegime::Low;
        let medium = VolatilityRegime::Medium;
        let high = VolatilityRegime::High;

        assert_eq!(serde_json::to_string(&low).unwrap(), r#""Low""#);
        assert_eq!(serde_json::to_string(&medium).unwrap(), r#""Medium""#);
        assert_eq!(serde_json::to_string(&high).unwrap(), r#""High""#);
    }

    #[test]
    fn volatility_regime_is_hashable() {
        let mut map: HashMap<VolatilityRegime, u32> = HashMap::new();
        map.insert(VolatilityRegime::Low, 1);
        map.insert(VolatilityRegime::Medium, 2);
        map.insert(VolatilityRegime::High, 3);

        assert_eq!(map.get(&VolatilityRegime::Low), Some(&1));
        assert_eq!(map.get(&VolatilityRegime::Medium), Some(&2));
        assert_eq!(map.get(&VolatilityRegime::High), Some(&3));
    }

    // ============================================================
    // TrendRegime Tests
    // ============================================================

    #[test]
    fn trend_regime_variants_are_distinct() {
        let bullish = TrendRegime::Bullish;
        let bearish = TrendRegime::Bearish;
        let ranging = TrendRegime::Ranging;

        assert_ne!(bullish, bearish);
        assert_ne!(bearish, ranging);
        assert_ne!(bullish, ranging);
    }

    #[test]
    fn trend_regime_serializes_correctly() {
        let bullish = TrendRegime::Bullish;
        let bearish = TrendRegime::Bearish;
        let ranging = TrendRegime::Ranging;

        assert_eq!(serde_json::to_string(&bullish).unwrap(), r#""Bullish""#);
        assert_eq!(serde_json::to_string(&bearish).unwrap(), r#""Bearish""#);
        assert_eq!(serde_json::to_string(&ranging).unwrap(), r#""Ranging""#);
    }

    #[test]
    fn trend_regime_is_hashable() {
        let mut map: HashMap<TrendRegime, u32> = HashMap::new();
        map.insert(TrendRegime::Bullish, 1);
        map.insert(TrendRegime::Bearish, 2);
        map.insert(TrendRegime::Ranging, 3);

        assert_eq!(map.get(&TrendRegime::Bullish), Some(&1));
        assert_eq!(map.get(&TrendRegime::Bearish), Some(&2));
        assert_eq!(map.get(&TrendRegime::Ranging), Some(&3));
    }

    // ============================================================
    // TimePeriod Tests
    // ============================================================

    #[test]
    fn time_period_variants_are_distinct() {
        let periods = [
            TimePeriod::AsiaOpen,
            TimePeriod::AsiaSession,
            TimePeriod::EuropeOpen,
            TimePeriod::EuropeSession,
            TimePeriod::USOpen,
            TimePeriod::USSession,
            TimePeriod::USClose,
        ];

        for i in 0..periods.len() {
            for j in (i + 1)..periods.len() {
                assert_ne!(periods[i], periods[j]);
            }
        }
    }

    #[test]
    fn time_period_serializes_correctly() {
        assert_eq!(
            serde_json::to_string(&TimePeriod::AsiaOpen).unwrap(),
            r#""AsiaOpen""#
        );
        assert_eq!(
            serde_json::to_string(&TimePeriod::USClose).unwrap(),
            r#""USClose""#
        );
    }

    #[test]
    fn time_period_is_hashable() {
        let mut map: HashMap<TimePeriod, u32> = HashMap::new();
        map.insert(TimePeriod::AsiaOpen, 1);
        map.insert(TimePeriod::USOpen, 2);

        assert_eq!(map.get(&TimePeriod::AsiaOpen), Some(&1));
        assert_eq!(map.get(&TimePeriod::USOpen), Some(&2));
    }

    // ============================================================
    // RegimeConfig Tests
    // ============================================================

    #[test]
    fn regime_config_default_values() {
        let config = RegimeConfig::default();

        assert!((config.volatility_low_percentile - 0.33).abs() < f64::EPSILON);
        assert!((config.volatility_high_percentile - 0.67).abs() < f64::EPSILON);
        assert!((config.trend_threshold - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn regime_config_custom_values() {
        let config = RegimeConfig {
            volatility_low_percentile: 0.25,
            volatility_high_percentile: 0.75,
            trend_threshold: 0.002,
        };

        assert!((config.volatility_low_percentile - 0.25).abs() < f64::EPSILON);
        assert!((config.volatility_high_percentile - 0.75).abs() < f64::EPSILON);
        assert!((config.trend_threshold - 0.002).abs() < f64::EPSILON);
    }

    // ============================================================
    // classify_volatility Tests
    // ============================================================

    #[test]
    fn classify_volatility_empty_returns_medium() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_volatility(0.5, &[]);

        assert_eq!(result, VolatilityRegime::Medium);
    }

    #[test]
    fn classify_volatility_low_value() {
        let analyzer = RegimeAnalyzer::new();
        // Values: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
        // 33rd percentile ~= 3.3, 67th percentile ~= 6.7
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let result = analyzer.classify_volatility(1.0, &volatilities);
        assert_eq!(result, VolatilityRegime::Low);
    }

    #[test]
    fn classify_volatility_medium_value() {
        let analyzer = RegimeAnalyzer::new();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let result = analyzer.classify_volatility(5.0, &volatilities);
        assert_eq!(result, VolatilityRegime::Medium);
    }

    #[test]
    fn classify_volatility_high_value() {
        let analyzer = RegimeAnalyzer::new();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let result = analyzer.classify_volatility(9.0, &volatilities);
        assert_eq!(result, VolatilityRegime::High);
    }

    #[test]
    fn classify_volatility_at_low_boundary() {
        let analyzer = RegimeAnalyzer::new();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        let low_threshold = percentile(&volatilities, 0.33);

        // At boundary should be medium (not strictly less than)
        let result = analyzer.classify_volatility(low_threshold, &volatilities);
        assert_eq!(result, VolatilityRegime::Medium);
    }

    #[test]
    fn classify_volatility_at_high_boundary() {
        let analyzer = RegimeAnalyzer::new();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        let high_threshold = percentile(&volatilities, 0.67);

        // At boundary should be medium (not strictly greater than)
        let result = analyzer.classify_volatility(high_threshold, &volatilities);
        assert_eq!(result, VolatilityRegime::Medium);
    }

    // ============================================================
    // classify_trend Tests
    // ============================================================

    #[test]
    fn classify_trend_bullish() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(0.01); // 1% return

        assert_eq!(result, TrendRegime::Bullish);
    }

    #[test]
    fn classify_trend_bearish() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(-0.01); // -1% return

        assert_eq!(result, TrendRegime::Bearish);
    }

    #[test]
    fn classify_trend_ranging_positive() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(0.0005); // 0.05% return (below threshold)

        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn classify_trend_ranging_negative() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(-0.0005); // -0.05% return (above -threshold)

        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn classify_trend_at_positive_threshold() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(0.001); // exactly at threshold

        // At threshold should be ranging (not strictly greater than)
        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn classify_trend_at_negative_threshold() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(-0.001); // exactly at -threshold

        // At threshold should be ranging (not strictly less than)
        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn classify_trend_zero_is_ranging() {
        let analyzer = RegimeAnalyzer::new();
        let result = analyzer.classify_trend(0.0);

        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn classify_trend_custom_threshold() {
        let config = RegimeConfig {
            trend_threshold: 0.005, // 0.5% threshold
            ..Default::default()
        };
        let analyzer = RegimeAnalyzer::with_config(config);

        // 0.3% return should be ranging with 0.5% threshold
        assert_eq!(analyzer.classify_trend(0.003), TrendRegime::Ranging);
        // 0.6% return should be bullish with 0.5% threshold
        assert_eq!(analyzer.classify_trend(0.006), TrendRegime::Bullish);
    }

    // ============================================================
    // classify_time_period Tests
    // ============================================================

    #[test]
    fn classify_time_period_asia_open() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 0..=3 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::AsiaOpen,
                "hour {} should be AsiaOpen",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_asia_session() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 4..=7 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::AsiaSession,
                "hour {} should be AsiaSession",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_europe_open() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 8..=11 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::EuropeOpen,
                "hour {} should be EuropeOpen",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_europe_session() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 12..=13 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::EuropeSession,
                "hour {} should be EuropeSession",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_us_open() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 14..=17 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::USOpen,
                "hour {} should be USOpen",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_us_session() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 18..=21 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::USSession,
                "hour {} should be USSession",
                hour
            );
        }
    }

    #[test]
    fn classify_time_period_us_close() {
        let analyzer = RegimeAnalyzer::new();

        for hour in 22..=23 {
            let ts = Utc.with_ymd_and_hms(2025, 1, 15, hour, 30, 0).unwrap();
            assert_eq!(
                analyzer.classify_time_period(ts),
                TimePeriod::USClose,
                "hour {} should be USClose",
                hour
            );
        }
    }

    // ============================================================
    // classify (complete) Tests
    // ============================================================

    #[test]
    fn classify_returns_complete_label() {
        let analyzer = RegimeAnalyzer::new();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 15, 0, 0).unwrap();

        let label = analyzer.classify(9.0, &volatilities, 0.02, ts);

        assert_eq!(label.volatility, VolatilityRegime::High);
        assert_eq!(label.trend, TrendRegime::Bullish);
        assert_eq!(label.time_period, TimePeriod::USOpen);
    }

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_settlement(
        timestamp: DateTime<Utc>,
        price_return: Decimal,
        outcome: BinaryOutcome,
    ) -> SettlementResult {
        let bet = BinaryBet::new(
            timestamp,
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50),
            0.75,
        );
        // Calculate start and settlement prices to achieve desired price_return
        let start_price = dec!(43000);
        let settlement_price = start_price * (Decimal::ONE + price_return);
        SettlementResult::new(
            bet,
            timestamp + chrono::Duration::minutes(15),
            settlement_price,
            start_price,
            outcome,
            dec!(0),
        )
    }

    // ============================================================
    // analyze Tests
    // ============================================================

    #[test]
    fn analyze_empty_settlements_returns_empty_metrics() {
        let analyzer = RegimeAnalyzer::new();
        let settlements: Vec<SettlementResult> = vec![];
        let volatilities: Vec<f64> = vec![];

        let result = analyzer.analyze(&settlements, &volatilities);

        assert!(result.by_volatility.is_empty());
        assert!(result.by_trend.is_empty());
        assert!(result.by_time_period.is_empty());
        assert!(result.by_combination.is_empty());
        assert_eq!(result.regime_transitions, 0);
    }

    #[test]
    fn analyze_groups_by_volatility() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create 10 settlements to have meaningful percentile boundaries
        let settlements: Vec<SettlementResult> = (0..10)
            .map(|_| create_settlement(ts, dec!(0.01), BinaryOutcome::Win))
            .collect();

        // Volatilities from 1 to 10 - percentile boundaries at ~3.3 and ~6.7
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let result = analyzer.analyze(&settlements, &volatilities);

        // Should have all three regimes represented
        assert!(result.by_volatility.contains_key(&VolatilityRegime::Low));
        assert!(result.by_volatility.contains_key(&VolatilityRegime::Medium));
        assert!(result.by_volatility.contains_key(&VolatilityRegime::High));
    }

    #[test]
    fn analyze_groups_by_trend() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create settlements with different price returns
        // Default threshold is 0.001 (0.1%)
        let settlements = vec![
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // Bullish (2% > 0.1%)
            create_settlement(ts, dec!(-0.02), BinaryOutcome::Win), // Bearish (-2% < -0.1%)
            create_settlement(ts, dec!(0.00005), BinaryOutcome::Win), // Ranging (0.005% within threshold)
        ];

        let volatilities = vec![5.0, 5.0, 5.0];

        let result = analyzer.analyze(&settlements, &volatilities);

        assert!(
            result.by_trend.contains_key(&TrendRegime::Bullish),
            "Expected Bullish trend in results"
        );
        assert!(
            result.by_trend.contains_key(&TrendRegime::Bearish),
            "Expected Bearish trend in results"
        );
        assert!(
            result.by_trend.contains_key(&TrendRegime::Ranging),
            "Expected Ranging trend in results"
        );
    }

    #[test]
    fn analyze_groups_by_time_period() {
        let analyzer = RegimeAnalyzer::new();

        // Create settlements at different hours
        let settlements = vec![
            create_settlement(
                Utc.with_ymd_and_hms(2025, 1, 15, 2, 0, 0).unwrap(),
                dec!(0.01),
                BinaryOutcome::Win,
            ),
            create_settlement(
                Utc.with_ymd_and_hms(2025, 1, 15, 15, 0, 0).unwrap(),
                dec!(0.01),
                BinaryOutcome::Win,
            ),
        ];

        let volatilities = vec![5.0, 5.0];

        let result = analyzer.analyze(&settlements, &volatilities);

        assert!(result.by_time_period.contains_key(&TimePeriod::AsiaOpen));
        assert!(result.by_time_period.contains_key(&TimePeriod::USOpen));
    }

    #[test]
    fn analyze_counts_regime_transitions() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create settlements that will have different volatility/trend combos
        let settlements = vec![
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // High vol, Bullish
            create_settlement(ts, dec!(-0.02), BinaryOutcome::Win), // Low vol, Bearish
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // High vol, Bullish
        ];

        // Different volatilities to force transitions
        let volatilities = vec![9.0, 1.0, 9.0];

        let result = analyzer.analyze(&settlements, &volatilities);

        // Transitions: (High,Bullish) -> (Low,Bearish) -> (High,Bullish) = 2 transitions
        assert_eq!(result.regime_transitions, 2);
    }

    #[test]
    fn analyze_no_transitions_same_regime() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // All same regime
        let settlements = vec![
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
        ];

        // All same high volatility
        let volatilities = vec![9.0, 9.0, 9.0];

        let result = analyzer.analyze(&settlements, &volatilities);

        assert_eq!(result.regime_transitions, 0);
    }

    #[test]
    fn analyze_computes_metrics_per_group() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create 10 settlements to establish percentile boundaries
        // First 7 are wins with high volatility (8, 9, 10 are high)
        // Last 3 are losses with low volatility (1, 2, 3 are low)
        let settlements = vec![
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=10 (high)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=9 (high)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=8 (high)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=7 (medium)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=6 (medium)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=5 (medium)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win), // vol=4 (medium)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Loss), // vol=3 (low)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Loss), // vol=2 (low)
            create_settlement(ts, dec!(0.02), BinaryOutcome::Loss), // vol=1 (low)
        ];

        // Volatilities: 10 down to 1
        let volatilities: Vec<f64> = (1..=10).rev().map(|x| x as f64).collect();

        let result = analyzer.analyze(&settlements, &volatilities);

        // High vol (8,9,10) should have 3 wins
        let high_metrics = result.by_volatility.get(&VolatilityRegime::High).unwrap();
        assert_eq!(high_metrics.wins, 3);
        assert_eq!(high_metrics.losses, 0);

        // Low vol (1,2,3) should have 3 losses
        let low_metrics = result.by_volatility.get(&VolatilityRegime::Low).unwrap();
        assert_eq!(low_metrics.wins, 0);
        assert_eq!(low_metrics.losses, 3);
    }

    #[test]
    fn analyze_handles_mismatched_lengths() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        let settlements = vec![
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
            create_settlement(ts, dec!(0.02), BinaryOutcome::Win),
        ];

        // Only 1 volatility value (will use 0.0 for missing)
        let volatilities = vec![5.0];

        let result = analyzer.analyze(&settlements, &volatilities);

        // Should not panic, should handle gracefully
        assert!(!result.by_volatility.is_empty());
    }

    // ============================================================
    // RegimeCombination Tests
    // ============================================================

    #[test]
    fn regime_combination_equality() {
        let combo1 = RegimeCombination {
            volatility: VolatilityRegime::High,
            trend: TrendRegime::Bullish,
        };
        let combo2 = RegimeCombination {
            volatility: VolatilityRegime::High,
            trend: TrendRegime::Bullish,
        };
        let combo3 = RegimeCombination {
            volatility: VolatilityRegime::Low,
            trend: TrendRegime::Bullish,
        };

        assert_eq!(combo1, combo2);
        assert_ne!(combo1, combo3);
    }

    #[test]
    fn regime_combination_is_hashable() {
        let mut map: HashMap<RegimeCombination, u32> = HashMap::new();
        let combo = RegimeCombination {
            volatility: VolatilityRegime::High,
            trend: TrendRegime::Bullish,
        };
        map.insert(combo, 42);

        assert_eq!(map.get(&combo), Some(&42));
    }

    // ============================================================
    // percentile Helper Tests
    // ============================================================

    #[test]
    fn percentile_empty_returns_zero() {
        assert!((percentile(&[], 0.5) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_single_value() {
        assert!((percentile(&[5.0], 0.5) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_median() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let median = percentile(&values, 0.5);
        assert!((median - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_min() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let min = percentile(&values, 0.0);
        assert!((min - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_max() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let max = percentile(&values, 1.0);
        assert!((max - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_unsorted_input() {
        let values = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        let median = percentile(&values, 0.5);
        assert!((median - 3.0).abs() < f64::EPSILON);
    }

    // ============================================================
    // RegimeAnalyzer Constructor Tests
    // ============================================================

    #[test]
    fn regime_analyzer_default_uses_default_config() {
        let analyzer = RegimeAnalyzer::default();
        // Should classify using default thresholds
        let result = analyzer.classify_trend(0.0005);
        assert_eq!(result, TrendRegime::Ranging);
    }

    #[test]
    fn regime_analyzer_with_config_uses_custom_config() {
        let config = RegimeConfig {
            trend_threshold: 0.0001, // Very low threshold
            ..Default::default()
        };
        let analyzer = RegimeAnalyzer::with_config(config);

        // 0.0005 should now be bullish with lower threshold
        let result = analyzer.classify_trend(0.0005);
        assert_eq!(result, TrendRegime::Bullish);
    }

    // ============================================================
    // RegimeLabel Tests
    // ============================================================

    #[test]
    fn regime_label_serialization_roundtrip() {
        let label = RegimeLabel {
            volatility: VolatilityRegime::High,
            trend: TrendRegime::Bearish,
            time_period: TimePeriod::USOpen,
        };

        let json = serde_json::to_string(&label).unwrap();
        let deserialized: RegimeLabel = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.volatility, VolatilityRegime::High);
        assert_eq!(deserialized.trend, TrendRegime::Bearish);
        assert_eq!(deserialized.time_period, TimePeriod::USOpen);
    }

    // ============================================================
    // RegimeMetrics Tests
    // ============================================================

    #[test]
    fn regime_metrics_accessor_methods() {
        let analyzer = RegimeAnalyzer::new();
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create settlements spanning different regimes
        let settlements: Vec<SettlementResult> = (0..10)
            .map(|_| create_settlement(ts, dec!(0.02), BinaryOutcome::Win))
            .collect();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let metrics = analyzer.analyze(&settlements, &volatilities);

        // Test accessor methods
        assert!(metrics.volatility_metrics(VolatilityRegime::High).is_some());
        assert!(metrics.trend_metrics(TrendRegime::Bullish).is_some());
        assert!(metrics
            .time_period_metrics(TimePeriod::EuropeOpen)
            .is_some());
    }

    #[test]
    fn regime_metrics_accessor_returns_none_for_missing() {
        let metrics = RegimeMetrics {
            by_volatility: HashMap::new(),
            by_trend: HashMap::new(),
            by_time_period: HashMap::new(),
            by_combination: HashMap::new(),
            regime_transitions: 0,
        };

        assert!(metrics.volatility_metrics(VolatilityRegime::High).is_none());
        assert!(metrics.trend_metrics(TrendRegime::Bullish).is_none());
        assert!(metrics.time_period_metrics(TimePeriod::USOpen).is_none());
    }
}
