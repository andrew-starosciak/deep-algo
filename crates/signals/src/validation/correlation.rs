//! Correlation analysis for signal validation.
//!
//! Provides Pearson correlation between signal values and forward returns
//! to assess signal predictive power.

use algo_trade_data::models::SignalSnapshotRecord;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Result of correlation analysis between signal and forward returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationAnalysis {
    /// Name of the signal analyzed
    pub signal_name: String,
    /// Pearson correlation coefficient [-1, 1]
    pub correlation: f64,
    /// P-value for the correlation (two-tailed)
    pub p_value: f64,
    /// Number of samples used
    pub sample_size: usize,
}

impl CorrelationAnalysis {
    /// Returns true if the correlation is statistically significant at alpha = 0.05.
    #[must_use]
    pub fn is_significant(&self) -> bool {
        self.p_value < 0.05
    }

    /// Returns true if the correlation is marginally significant at alpha = 0.10.
    #[must_use]
    pub fn is_marginally_significant(&self) -> bool {
        self.p_value < 0.10
    }
}

/// Calculates the Pearson correlation coefficient between two series.
fn pearson_correlation(x: &[f64], y: &[f64]) -> f64 {
    if x.len() != y.len() || x.len() < 2 {
        return 0.0;
    }

    let n = x.len() as f64;
    let mean_x = x.iter().sum::<f64>() / n;
    let mean_y = y.iter().sum::<f64>() / n;

    let mut covariance = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;

    for (xi, yi) in x.iter().zip(y.iter()) {
        let dx = xi - mean_x;
        let dy = yi - mean_y;
        covariance += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }

    let denominator = (var_x * var_y).sqrt();
    if denominator < f64::EPSILON {
        return 0.0;
    }

    covariance / denominator
}

/// Calculates the p-value for a correlation using t-distribution approximation.
///
/// Uses the transformation: t = r * sqrt(n-2) / sqrt(1 - r^2)
/// which follows a t-distribution with n-2 degrees of freedom.
fn correlation_p_value(r: f64, n: usize) -> f64 {
    if n < 3 {
        return 1.0;
    }

    let r_clamped = r.clamp(-0.9999, 0.9999); // Avoid division by zero
    let df = n as f64 - 2.0;
    let t_stat = r_clamped * (df / (1.0 - r_clamped * r_clamped)).sqrt();

    // Two-tailed p-value using normal approximation for simplicity
    // For large samples this is accurate; for small samples it's conservative
    let p = 2.0 * (1.0 - standard_normal_cdf(t_stat.abs()));
    p.clamp(0.0, 1.0)
}

/// Standard normal CDF approximation.
fn standard_normal_cdf(x: f64) -> f64 {
    if x < 0.0 {
        return 1.0 - standard_normal_cdf(-x);
    }

    let b1 = 0.319_381_530;
    let b2 = -0.356_563_782;
    let b3 = 1.781_477_937;
    let b4 = -1.821_255_978;
    let b5 = 1.330_274_429;
    let p = 0.231_641_9;

    let t = 1.0 / (1.0 + p * x);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;

    let pdf = (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    1.0 - pdf * (b1 * t + b2 * t2 + b3 * t3 + b4 * t4 + b5 * t5)
}

/// Analyzes the correlation between signal values and forward returns.
///
/// # Arguments
/// * `snapshots` - Signal snapshot records with forward returns
///
/// # Returns
/// A `CorrelationAnalysis` result
///
/// # Errors
/// Returns error if insufficient validated data
pub fn analyze_signal_correlation(
    snapshots: &[SignalSnapshotRecord],
) -> Result<CorrelationAnalysis> {
    // Filter to validated snapshots (those with forward returns)
    let validated: Vec<_> = snapshots
        .iter()
        .filter(|s| s.forward_return_15m.is_some() && s.parsed_direction().is_some())
        .collect();

    if validated.len() < 3 {
        return Err(anyhow!(
            "Insufficient validated data: need at least 3 samples, got {}",
            validated.len()
        ));
    }

    let signal_name = validated
        .first()
        .map(|s| s.signal_name.clone())
        .unwrap_or_default();

    // Extract signal values: strength * direction_sign
    let signal_values: Vec<f64> = validated
        .iter()
        .filter_map(|s| {
            let direction = s.parsed_direction()?;
            let sign = match direction {
                algo_trade_data::models::SignalDirection::Up => 1.0,
                algo_trade_data::models::SignalDirection::Down => -1.0,
                algo_trade_data::models::SignalDirection::Neutral => 0.0,
            };
            let strength: f64 = s.strength.to_string().parse().ok().unwrap_or(0.0);
            Some(sign * strength)
        })
        .collect();

    // Extract forward returns
    let returns: Vec<f64> = validated
        .iter()
        .filter_map(|s| {
            s.forward_return_15m
                .as_ref()
                .and_then(|r| r.to_string().parse().ok())
        })
        .collect();

    if signal_values.len() != returns.len() || signal_values.len() < 3 {
        return Err(anyhow!("Mismatched or insufficient data for correlation"));
    }

    let correlation = pearson_correlation(&signal_values, &returns);
    let p_value = correlation_p_value(correlation, signal_values.len());

    Ok(CorrelationAnalysis {
        signal_name,
        correlation,
        p_value,
        sample_size: signal_values.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_data::models::SignalDirection;
    use chrono::{TimeZone, Utc};
    use rust_decimal_macros::dec;

    fn create_snapshot(
        direction: SignalDirection,
        strength: f64,
        forward_return: f64,
    ) -> SignalSnapshotRecord {
        let mut record = SignalSnapshotRecord::new(
            Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap(),
            "test_signal",
            "BTCUSDT",
            "binance",
            direction,
            rust_decimal::Decimal::try_from(strength).unwrap_or(dec!(0.5)),
            dec!(0.5),
        );
        record.set_forward_return(
            rust_decimal::Decimal::try_from(forward_return).unwrap_or(dec!(0.0)),
        );
        record
    }

    #[test]
    fn correlation_perfect_positive_returns_one() {
        // Signal perfectly predicts returns
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.02),
            create_snapshot(SignalDirection::Up, 0.6, 0.015),
            create_snapshot(SignalDirection::Up, 0.4, 0.01),
            create_snapshot(SignalDirection::Down, 0.5, -0.012),
            create_snapshot(SignalDirection::Down, 0.7, -0.017),
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        assert!(
            result.correlation > 0.95,
            "correlation was {}",
            result.correlation
        );
    }

    #[test]
    fn correlation_perfect_negative_returns_minus_one() {
        // Signal inversely predicts returns (contrarian indicator)
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, -0.02),
            create_snapshot(SignalDirection::Up, 0.6, -0.015),
            create_snapshot(SignalDirection::Up, 0.4, -0.01),
            create_snapshot(SignalDirection::Down, 0.5, 0.012),
            create_snapshot(SignalDirection::Down, 0.7, 0.017),
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        assert!(
            result.correlation < -0.95,
            "correlation was {}",
            result.correlation
        );
    }

    #[test]
    fn correlation_no_relationship_returns_near_zero() {
        // Random relationship
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, -0.01),
            create_snapshot(SignalDirection::Down, 0.6, -0.005),
            create_snapshot(SignalDirection::Up, 0.4, 0.02),
            create_snapshot(SignalDirection::Down, 0.5, 0.01),
            create_snapshot(SignalDirection::Up, 0.7, -0.008),
            create_snapshot(SignalDirection::Down, 0.3, 0.003),
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        // Random data should have weak correlation
        assert!(
            result.correlation.abs() < 0.8,
            "correlation was {}",
            result.correlation
        );
    }

    #[test]
    fn p_value_significant_for_strong_correlation() {
        // Strong positive correlation with decent sample size
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.9, 0.025),
            create_snapshot(SignalDirection::Up, 0.8, 0.02),
            create_snapshot(SignalDirection::Up, 0.7, 0.018),
            create_snapshot(SignalDirection::Up, 0.5, 0.012),
            create_snapshot(SignalDirection::Down, 0.4, -0.01),
            create_snapshot(SignalDirection::Down, 0.6, -0.015),
            create_snapshot(SignalDirection::Down, 0.8, -0.02),
            create_snapshot(SignalDirection::Down, 0.9, -0.023),
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        assert!(result.is_significant(), "p-value was {}", result.p_value);
    }

    #[test]
    fn p_value_not_significant_for_weak_correlation() {
        // Weak correlation - p-value should be high
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.5, 0.001),
            create_snapshot(SignalDirection::Up, 0.5, -0.001),
            create_snapshot(SignalDirection::Down, 0.5, 0.001),
            create_snapshot(SignalDirection::Down, 0.5, -0.001),
            create_snapshot(SignalDirection::Neutral, 0.0, 0.0),
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        // Weak correlation should not be significant
        assert!(
            !result.is_significant() || result.correlation.abs() < 0.3,
            "p-value={}, correlation={}",
            result.p_value,
            result.correlation
        );
    }

    #[test]
    fn analyze_returns_error_for_insufficient_data() {
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.02),
            create_snapshot(SignalDirection::Down, 0.6, -0.01),
        ];

        let result = analyze_signal_correlation(&snapshots);

        assert!(result.is_err());
    }

    #[test]
    fn analyze_filters_unvalidated_snapshots() {
        let unvalidated = SignalSnapshotRecord::new(
            Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.8),
            dec!(0.5),
        );
        // Don't set forward_return - it's unvalidated

        let validated = create_snapshot(SignalDirection::Up, 0.8, 0.02);

        let snapshots = vec![
            unvalidated.clone(),
            unvalidated,
            validated.clone(),
            validated.clone(),
            validated,
        ];

        let result = analyze_signal_correlation(&snapshots).unwrap();

        // Should only use the 3 validated snapshots
        assert_eq!(result.sample_size, 3);
    }

    #[test]
    fn correlation_analysis_is_marginally_significant() {
        let analysis = CorrelationAnalysis {
            signal_name: "test".to_string(),
            correlation: 0.3,
            p_value: 0.08,
            sample_size: 50,
        };

        assert!(!analysis.is_significant());
        assert!(analysis.is_marginally_significant());
    }
}
