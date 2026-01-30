//! Hypothesis testing for signal validation.
//!
//! Provides statistical tests for evaluating signal edge, including:
//! - Binomial test for directional accuracy
//! - T-test for return significance

use algo_trade_core::validation::{binomial_test, wilson_ci};
use algo_trade_data::models::SignalSnapshotRecord;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Type of statistical test performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestType {
    /// Binomial test: H0: P(correct) = 0.5
    Binomial,
    /// T-test: H0: mean return = 0
    TTest,
}

/// Result of a significance test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificanceTest {
    /// Name of the signal tested
    pub signal_name: String,
    /// Type of test performed
    pub test_type: TestType,
    /// Test statistic (z-score for binomial, t-score for t-test)
    pub statistic: f64,
    /// P-value (two-tailed)
    pub p_value: f64,
    /// 95% confidence interval
    pub confidence_interval: (f64, f64),
    /// Whether significant at alpha = 0.05
    pub is_significant_05: bool,
    /// Whether significant at alpha = 0.10
    pub is_significant_10: bool,
}

impl SignificanceTest {
    /// Returns true if the test shows a significant positive edge.
    ///
    /// For binomial test: lower CI bound > 0.5
    /// For t-test: lower CI bound > 0
    #[must_use]
    pub fn has_positive_edge(&self) -> bool {
        match self.test_type {
            TestType::Binomial => self.confidence_interval.0 > 0.5,
            TestType::TTest => self.confidence_interval.0 > 0.0,
        }
    }
}

/// Tests if signal directional accuracy is significantly better than random.
///
/// Uses binomial test with H0: P(correct prediction) = 0.5
///
/// # Arguments
/// * `snapshots` - Signal snapshot records with forward returns
///
/// # Returns
/// A `SignificanceTest` result
///
/// # Errors
/// Returns error if insufficient validated data
pub fn test_directional_accuracy(snapshots: &[SignalSnapshotRecord]) -> Result<SignificanceTest> {
    // Filter to validated snapshots with directional predictions
    let validated: Vec<_> = snapshots
        .iter()
        .filter(|s| {
            s.forward_return_15m.is_some()
                && s.parsed_direction()
                    .map(|d| d != algo_trade_data::models::SignalDirection::Neutral)
                    .unwrap_or(false)
        })
        .collect();

    if validated.is_empty() {
        return Err(anyhow!("No validated directional predictions"));
    }

    let signal_name = validated
        .first()
        .map(|s| s.signal_name.clone())
        .unwrap_or_default();

    // Count correct predictions
    let mut wins = 0;
    let mut total = 0;

    for snapshot in &validated {
        if let Some(is_correct) = snapshot.is_correct_prediction() {
            total += 1;
            if is_correct {
                wins += 1;
            }
        }
    }

    if total < 3 {
        return Err(anyhow!(
            "Insufficient predictions: need at least 3, got {}",
            total
        ));
    }

    // Calculate p-value using binomial test
    let p_value = binomial_test(wins, total, 0.5);

    // Calculate Wilson score CI
    let (ci_lower, ci_upper) = wilson_ci(wins, total, 1.96);

    // Calculate z-score for the statistic
    let expected = total as f64 * 0.5;
    let std_dev = (total as f64 * 0.5 * 0.5).sqrt();
    let z_score = if std_dev > f64::EPSILON {
        (wins as f64 - expected) / std_dev
    } else {
        0.0
    };

    Ok(SignificanceTest {
        signal_name,
        test_type: TestType::Binomial,
        statistic: z_score,
        p_value,
        confidence_interval: (ci_lower, ci_upper),
        is_significant_05: p_value < 0.05,
        is_significant_10: p_value < 0.10,
    })
}

/// Tests if mean return when signal is active is significantly different from zero.
///
/// Uses one-sample t-test with H0: mean return = 0
///
/// # Arguments
/// * `snapshots` - Signal snapshot records with forward returns
///
/// # Returns
/// A `SignificanceTest` result
///
/// # Errors
/// Returns error if insufficient validated data
pub fn test_return_significance(snapshots: &[SignalSnapshotRecord]) -> Result<SignificanceTest> {
    // Filter to validated snapshots with directional predictions
    let validated: Vec<_> = snapshots
        .iter()
        .filter(|s| {
            s.forward_return_15m.is_some()
                && s.parsed_direction()
                    .map(|d| d != algo_trade_data::models::SignalDirection::Neutral)
                    .unwrap_or(false)
        })
        .collect();

    if validated.is_empty() {
        return Err(anyhow!("No validated directional predictions"));
    }

    let signal_name = validated
        .first()
        .map(|s| s.signal_name.clone())
        .unwrap_or_default();

    // Calculate signed returns: positive if signal correct, negative if wrong
    let signed_returns: Vec<f64> = validated
        .iter()
        .filter_map(|s| {
            let direction = s.parsed_direction()?;
            let forward_return: f64 = s
                .forward_return_15m
                .as_ref()
                .and_then(|r| r.to_string().parse().ok())?;

            // Return is positive if signal direction matches return direction
            let sign = match direction {
                algo_trade_data::models::SignalDirection::Up => 1.0,
                algo_trade_data::models::SignalDirection::Down => -1.0,
                algo_trade_data::models::SignalDirection::Neutral => return None,
            };

            Some(sign * forward_return)
        })
        .collect();

    if signed_returns.len() < 3 {
        return Err(anyhow!(
            "Insufficient returns: need at least 3, got {}",
            signed_returns.len()
        ));
    }

    // Calculate mean and standard error
    let n = signed_returns.len() as f64;
    let mean = signed_returns.iter().sum::<f64>() / n;
    let variance = signed_returns
        .iter()
        .map(|r| (r - mean).powi(2))
        .sum::<f64>()
        / (n - 1.0);
    let std_dev = variance.sqrt();
    let std_error = std_dev / n.sqrt();

    // Calculate t-statistic
    let t_stat = if std_error > f64::EPSILON {
        mean / std_error
    } else {
        0.0
    };

    // Calculate p-value (two-tailed) using normal approximation
    let p_value = 2.0 * (1.0 - standard_normal_cdf(t_stat.abs()));
    let p_value = p_value.clamp(0.0, 1.0);

    // Calculate 95% CI: mean +/- t_critical * std_error
    // Using z = 1.96 for large samples
    let z_critical = 1.96;
    let ci_lower = mean - z_critical * std_error;
    let ci_upper = mean + z_critical * std_error;

    Ok(SignificanceTest {
        signal_name,
        test_type: TestType::TTest,
        statistic: t_stat,
        p_value,
        confidence_interval: (ci_lower, ci_upper),
        is_significant_05: p_value < 0.05,
        is_significant_10: p_value < 0.10,
    })
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

    // ============================================
    // Directional Accuracy Tests
    // ============================================

    #[test]
    fn directional_accuracy_significant_edge() {
        // 70% accuracy - should be significant
        let mut snapshots = Vec::new();
        for _ in 0..70 {
            // Correct predictions
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, 0.01));
        }
        for _ in 0..30 {
            // Incorrect predictions
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, -0.01));
        }

        let result = test_directional_accuracy(&snapshots).unwrap();

        assert!(result.is_significant_05, "p-value was {}", result.p_value);
        assert!(result.has_positive_edge());
        assert!(
            result.confidence_interval.0 > 0.5,
            "CI lower was {}",
            result.confidence_interval.0
        );
    }

    #[test]
    fn directional_accuracy_no_edge() {
        // 50% accuracy - no edge
        let mut snapshots = Vec::new();
        for _ in 0..50 {
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, 0.01));
        }
        for _ in 0..50 {
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, -0.01));
        }

        let result = test_directional_accuracy(&snapshots).unwrap();

        assert!(
            !result.is_significant_05 || result.confidence_interval.0 <= 0.5,
            "Should not show significant positive edge"
        );
    }

    #[test]
    fn directional_accuracy_uses_wilson_ci() {
        // Small sample - Wilson CI should be different from raw proportion
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Up, 0.8, -0.01),
        ];

        let result = test_directional_accuracy(&snapshots).unwrap();

        // Raw proportion is 75%, but Wilson CI should be wider
        assert!(
            result.confidence_interval.0 < 0.75,
            "Wilson CI lower should be below raw proportion"
        );
        assert!(
            result.confidence_interval.1 > 0.75,
            "Wilson CI upper should be above raw proportion"
        );
    }

    #[test]
    fn directional_accuracy_filters_neutral() {
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Neutral, 0.0, 0.005), // Should be filtered
            create_snapshot(SignalDirection::Down, 0.6, -0.01),
            create_snapshot(SignalDirection::Neutral, 0.0, -0.002), // Should be filtered
            create_snapshot(SignalDirection::Up, 0.7, 0.008),
        ];

        let result = test_directional_accuracy(&snapshots).unwrap();

        // Should only use 3 directional predictions
        assert_eq!(result.test_type, TestType::Binomial);
    }

    // ============================================
    // Return Significance Tests
    // ============================================

    #[test]
    fn return_significance_positive_returns() {
        // Consistently positive returns when following signal
        let mut snapshots = Vec::new();
        for i in 0..50 {
            let return_val = 0.005 + (i as f64 * 0.0001); // Small positive returns
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, return_val));
        }

        let result = test_return_significance(&snapshots).unwrap();

        assert!(result.is_significant_05, "p-value was {}", result.p_value);
        assert!(
            result.confidence_interval.0 > 0.0,
            "CI lower should be positive: {}",
            result.confidence_interval.0
        );
    }

    #[test]
    fn return_significance_negative_returns() {
        // Consistently negative returns (contrarian signal)
        let mut snapshots = Vec::new();
        for i in 0..50 {
            let return_val = 0.005 + (i as f64 * 0.0001);
            // Signal says Up but price goes down
            snapshots.push(create_snapshot(SignalDirection::Up, 0.8, -return_val));
        }

        let result = test_return_significance(&snapshots).unwrap();

        assert!(result.is_significant_05, "p-value was {}", result.p_value);
        assert!(
            result.confidence_interval.1 < 0.0,
            "CI upper should be negative: {}",
            result.confidence_interval.1
        );
    }

    #[test]
    fn return_significance_zero_mean() {
        // Returns around zero - no edge
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Up, 0.8, -0.01),
            create_snapshot(SignalDirection::Up, 0.8, 0.005),
            create_snapshot(SignalDirection::Up, 0.8, -0.005),
            create_snapshot(SignalDirection::Down, 0.8, -0.01),
            create_snapshot(SignalDirection::Down, 0.8, 0.01),
        ];

        let result = test_return_significance(&snapshots).unwrap();

        // CI should include zero
        assert!(
            result.confidence_interval.0 < 0.0 && result.confidence_interval.1 > 0.0,
            "CI should include zero: ({}, {})",
            result.confidence_interval.0,
            result.confidence_interval.1
        );
    }

    #[test]
    fn return_significance_insufficient_data() {
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.01),
            create_snapshot(SignalDirection::Down, 0.6, -0.01),
        ];

        let result = test_return_significance(&snapshots);

        assert!(result.is_err());
    }

    // ============================================
    // SignificanceTest Tests
    // ============================================

    #[test]
    fn significance_test_has_positive_edge_binomial() {
        let test = SignificanceTest {
            signal_name: "test".to_string(),
            test_type: TestType::Binomial,
            statistic: 2.5,
            p_value: 0.01,
            confidence_interval: (0.55, 0.75),
            is_significant_05: true,
            is_significant_10: true,
        };

        assert!(test.has_positive_edge());
    }

    #[test]
    fn significance_test_no_positive_edge_binomial() {
        let test = SignificanceTest {
            signal_name: "test".to_string(),
            test_type: TestType::Binomial,
            statistic: 0.5,
            p_value: 0.6,
            confidence_interval: (0.45, 0.55),
            is_significant_05: false,
            is_significant_10: false,
        };

        assert!(!test.has_positive_edge());
    }

    #[test]
    fn significance_test_has_positive_edge_ttest() {
        let test = SignificanceTest {
            signal_name: "test".to_string(),
            test_type: TestType::TTest,
            statistic: 3.0,
            p_value: 0.005,
            confidence_interval: (0.001, 0.005),
            is_significant_05: true,
            is_significant_10: true,
        };

        assert!(test.has_positive_edge());
    }
}
