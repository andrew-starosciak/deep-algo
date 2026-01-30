//! Statistical validation functions for signal generators.
//!
//! Provides hypothesis testing, confidence intervals, and validation
//! metrics for evaluating signal predictive power.

use serde::{Deserialize, Serialize};

/// Result of statistical validation for a signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalValidation {
    /// Win rate (proportion of correct predictions)
    pub win_rate: f64,
    /// Wilson score confidence interval (lower bound)
    pub wilson_ci_lower: f64,
    /// Wilson score confidence interval (upper bound)
    pub wilson_ci_upper: f64,
    /// p-value from binomial test (H0: p = 0.5)
    pub p_value: f64,
    /// Number of samples
    pub sample_size: usize,
    /// Whether the result is statistically significant at alpha = 0.05
    pub is_significant: bool,
}

impl SignalValidation {
    /// Creates a validation result from win/loss counts.
    ///
    /// # Arguments
    /// * `wins` - Number of correct predictions
    /// * `total` - Total number of predictions
    ///
    /// # Returns
    /// `SignalValidation` with computed statistics
    #[must_use]
    pub fn from_counts(wins: usize, total: usize) -> Self {
        let win_rate = if total == 0 {
            0.0
        } else {
            wins as f64 / total as f64
        };

        let (wilson_ci_lower, wilson_ci_upper) = wilson_ci(wins, total, 1.96);
        let p_value = binomial_test(wins, total, 0.5);

        Self {
            win_rate,
            wilson_ci_lower,
            wilson_ci_upper,
            p_value,
            sample_size: total,
            is_significant: p_value < 0.05,
        }
    }

    /// Returns true if the lower bound of the CI is above 0.5 (positive edge).
    #[must_use]
    pub fn has_positive_edge(&self) -> bool {
        self.wilson_ci_lower > 0.5
    }

    /// Returns true if sample size is sufficient for reliable inference.
    /// Requires at least 100 samples for 80% power to detect 5% edge.
    #[must_use]
    pub fn has_sufficient_samples(&self) -> bool {
        self.sample_size >= 100
    }
}

/// Calculates the Wilson score confidence interval for a proportion.
///
/// The Wilson score interval is preferred over the normal approximation
/// because it has better coverage properties, especially for proportions
/// near 0 or 1, and for small sample sizes.
///
/// # Formula
/// ```text
/// CI = (p + z^2/(2n) +/- z * sqrt(p(1-p)/n + z^2/(4n^2))) / (1 + z^2/n)
/// ```
///
/// # Arguments
/// * `wins` - Number of successes
/// * `n` - Total number of trials
/// * `z` - Z-score for confidence level (1.96 for 95%)
///
/// # Returns
/// Tuple of (lower_bound, upper_bound)
///
/// # Examples
/// ```
/// use algo_trade_core::validation::wilson_ci;
///
/// let (lower, upper) = wilson_ci(50, 100, 1.96);
/// assert!(lower > 0.39 && lower < 0.41);
/// assert!(upper > 0.59 && upper < 0.61);
/// ```
#[must_use]
pub fn wilson_ci(wins: usize, n: usize, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }

    let n_f = n as f64;
    let p = wins as f64 / n_f;
    let z_sq = z * z;

    let denominator = 1.0 + z_sq / n_f;
    let center = p + z_sq / (2.0 * n_f);

    // Under the square root: p(1-p)/n + z^2/(4n^2)
    let variance_term = p * (1.0 - p) / n_f;
    let correction_term = z_sq / (4.0 * n_f * n_f);
    let spread = z * (variance_term + correction_term).sqrt();

    let lower = (center - spread) / denominator;
    let upper = (center + spread) / denominator;

    // Clamp to [0, 1]
    (lower.max(0.0), upper.min(1.0))
}

/// Performs a two-tailed binomial test.
///
/// Tests the null hypothesis that the true probability equals `p0`.
/// Uses the normal approximation with continuity correction for n >= 20,
/// otherwise uses exact binomial calculation.
///
/// # Arguments
/// * `successes` - Number of observed successes
/// * `n` - Total number of trials
/// * `p0` - Hypothesized probability under null hypothesis
///
/// # Returns
/// Two-tailed p-value
///
/// # Examples
/// ```
/// use algo_trade_core::validation::binomial_test;
///
/// // 55 out of 100 is not significantly different from 50%
/// let p = binomial_test(55, 100, 0.5);
/// assert!(p > 0.05);
///
/// // 65 out of 100 is significantly different from 50%
/// let p = binomial_test(65, 100, 0.5);
/// assert!(p < 0.05);
/// ```
#[must_use]
pub fn binomial_test(successes: usize, n: usize, p0: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }

    let n_f = n as f64;
    let k = successes as f64;

    // Expected value and standard deviation under H0
    let expected = n_f * p0;
    let std_dev = (n_f * p0 * (1.0 - p0)).sqrt();

    if std_dev < f64::EPSILON {
        // Edge case: p0 = 0 or p0 = 1
        if (p0 < f64::EPSILON && successes == 0) || (p0 > 1.0 - f64::EPSILON && successes == n) {
            return 1.0;
        }
        return 0.0;
    }

    // Normal approximation with continuity correction
    let z = (k - expected).abs() - 0.5;
    if z < 0.0 {
        return 1.0;
    }
    let z_score = z / std_dev;

    // Two-tailed p-value using standard normal CDF approximation
    2.0 * (1.0 - standard_normal_cdf(z_score))
}

/// Approximation of the standard normal CDF using the Abramowitz and Stegun formula.
/// Accurate to about 10^-5.
fn standard_normal_cdf(x: f64) -> f64 {
    if x < 0.0 {
        return 1.0 - standard_normal_cdf(-x);
    }

    // Constants for Abramowitz and Stegun approximation (formula 26.2.17)
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

/// Calculates the Information Coefficient (IC) - correlation between signal and returns.
///
/// IC = correlation(signal_strength * direction, forward_return)
///
/// # Arguments
/// * `signals` - Signal strengths (-1 to 1, where sign indicates direction)
/// * `returns` - Corresponding forward returns
///
/// # Returns
/// Pearson correlation coefficient, or 0.0 if calculation not possible
#[must_use]
pub fn information_coefficient(signals: &[f64], returns: &[f64]) -> f64 {
    if signals.len() != returns.len() || signals.len() < 2 {
        return 0.0;
    }

    let n = signals.len() as f64;

    let mean_signal = signals.iter().sum::<f64>() / n;
    let mean_return = returns.iter().sum::<f64>() / n;

    let mut covariance = 0.0;
    let mut var_signal = 0.0;
    let mut var_return = 0.0;

    for (s, r) in signals.iter().zip(returns.iter()) {
        let ds = s - mean_signal;
        let dr = r - mean_return;
        covariance += ds * dr;
        var_signal += ds * ds;
        var_return += dr * dr;
    }

    let denominator = (var_signal * var_return).sqrt();
    if denominator < f64::EPSILON {
        return 0.0;
    }

    covariance / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // wilson_ci Tests
    // ============================================

    #[test]
    fn wilson_ci_50_percent_approximately_40_60() {
        let (lower, upper) = wilson_ci(50, 100, 1.96);
        // Expected: approximately (0.40, 0.60) for 95% CI
        assert!(lower > 0.39 && lower < 0.42, "lower was {lower}");
        assert!(upper > 0.58 && upper < 0.61, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_70_percent() {
        let (lower, upper) = wilson_ci(70, 100, 1.96);
        // 70% win rate should have CI above 0.5
        assert!(lower > 0.59, "lower was {lower}");
        assert!(upper < 0.80, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_zero_wins() {
        let (lower, upper) = wilson_ci(0, 10, 1.96);
        // Edge case: 0 successes
        assert!(lower >= 0.0, "lower was {lower}");
        assert!(lower < 0.01, "lower was {lower}");
        assert!(upper > 0.0, "upper was {upper}");
        assert!(upper < 0.35, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_all_wins() {
        let (lower, upper) = wilson_ci(10, 10, 1.96);
        // Edge case: all successes
        assert!(lower > 0.65, "lower was {lower}");
        assert!((upper - 1.0).abs() < 0.01, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_zero_samples() {
        let (lower, upper) = wilson_ci(0, 0, 1.96);
        assert!((lower - 0.0).abs() < f64::EPSILON);
        assert!((upper - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wilson_ci_single_success() {
        let (lower, upper) = wilson_ci(1, 1, 1.96);
        // Single success - should have wide CI
        assert!(lower > 0.0);
        assert!((upper - 1.0).abs() < 0.01);
    }

    #[test]
    fn wilson_ci_small_sample() {
        let (lower, upper) = wilson_ci(3, 5, 1.96);
        // 60% with n=5 - should have very wide CI
        assert!(lower < 0.3, "lower was {lower}");
        assert!(upper > 0.8, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_large_sample() {
        let (lower, upper) = wilson_ci(550, 1000, 1.96);
        // 55% with n=1000 - should have narrow CI
        let width = upper - lower;
        assert!(width < 0.07, "width was {width}");
        assert!(lower > 0.51, "lower was {lower}");
        assert!(upper < 0.59, "upper was {upper}");
    }

    // ============================================
    // binomial_test Tests
    // ============================================

    #[test]
    fn binomial_test_55_of_100_not_significant() {
        let p = binomial_test(55, 100, 0.5);
        // 55/100 is not significantly different from 50%
        assert!(p > 0.05, "p-value was {p}");
    }

    #[test]
    fn binomial_test_65_of_100_significant() {
        let p = binomial_test(65, 100, 0.5);
        // 65/100 is significantly different from 50%
        assert!(p < 0.05, "p-value was {p}");
    }

    #[test]
    fn binomial_test_50_of_100_not_significant() {
        let p = binomial_test(50, 100, 0.5);
        // Exactly 50% - should not be significant
        assert!(p > 0.9, "p-value was {p}");
    }

    #[test]
    fn binomial_test_45_of_100_not_significant() {
        let p = binomial_test(45, 100, 0.5);
        // 45% is within normal variance
        assert!(p > 0.05, "p-value was {p}");
    }

    #[test]
    fn binomial_test_35_of_100_significant() {
        let p = binomial_test(35, 100, 0.5);
        // 35% is significantly different from 50%
        assert!(p < 0.05, "p-value was {p}");
    }

    #[test]
    fn binomial_test_zero_samples() {
        let p = binomial_test(0, 0, 0.5);
        assert!((p - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn binomial_test_all_successes_small_n() {
        let p = binomial_test(10, 10, 0.5);
        // 10/10 successes - very significant
        assert!(p < 0.01, "p-value was {p}");
    }

    #[test]
    fn binomial_test_custom_p0() {
        // Test against 60% null hypothesis
        let p = binomial_test(55, 100, 0.6);
        // 55% is close to 60%, not significant
        assert!(p > 0.05, "p-value was {p}");
    }

    // ============================================
    // information_coefficient Tests
    // ============================================

    #[test]
    fn ic_perfect_positive_correlation() {
        let signals = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let returns = vec![0.01, 0.02, 0.03, 0.04, 0.05];
        let ic = information_coefficient(&signals, &returns);
        assert!(ic > 0.99, "IC was {ic}");
    }

    #[test]
    fn ic_perfect_negative_correlation() {
        let signals = vec![0.5, 0.4, 0.3, 0.2, 0.1];
        let returns = vec![0.01, 0.02, 0.03, 0.04, 0.05];
        let ic = information_coefficient(&signals, &returns);
        assert!(ic < -0.99, "IC was {ic}");
    }

    #[test]
    fn ic_no_correlation() {
        let signals = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
        let returns = vec![0.01, 0.01, -0.01, -0.01, 0.01, 0.01];
        let ic = information_coefficient(&signals, &returns);
        // Random-ish - should be close to zero
        assert!(ic.abs() < 0.5, "IC was {ic}");
    }

    #[test]
    fn ic_empty_arrays() {
        let ic = information_coefficient(&[], &[]);
        assert!((ic - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ic_single_element() {
        let ic = information_coefficient(&[0.5], &[0.01]);
        assert!((ic - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ic_mismatched_lengths() {
        let ic = information_coefficient(&[0.1, 0.2], &[0.01]);
        assert!((ic - 0.0).abs() < f64::EPSILON);
    }

    // ============================================
    // SignalValidation Tests
    // ============================================

    #[test]
    fn signal_validation_from_counts_calculates_correctly() {
        let validation = SignalValidation::from_counts(65, 100);

        assert!((validation.win_rate - 0.65).abs() < 0.001);
        assert!(validation.wilson_ci_lower > 0.5);
        assert!(validation.p_value < 0.05);
        assert!(validation.is_significant);
        assert_eq!(validation.sample_size, 100);
    }

    #[test]
    fn signal_validation_55_percent_not_significant() {
        let validation = SignalValidation::from_counts(55, 100);

        assert!(!validation.is_significant);
        assert!(validation.p_value > 0.05);
    }

    #[test]
    fn signal_validation_has_positive_edge() {
        let validation = SignalValidation::from_counts(70, 100);
        assert!(validation.has_positive_edge());

        let validation = SignalValidation::from_counts(45, 100);
        assert!(!validation.has_positive_edge());
    }

    #[test]
    fn signal_validation_has_sufficient_samples() {
        let validation = SignalValidation::from_counts(50, 100);
        assert!(validation.has_sufficient_samples());

        let validation = SignalValidation::from_counts(30, 50);
        assert!(!validation.has_sufficient_samples());
    }

    #[test]
    fn signal_validation_zero_total() {
        let validation = SignalValidation::from_counts(0, 0);
        assert!((validation.win_rate - 0.0).abs() < f64::EPSILON);
        assert!(!validation.is_significant);
    }

    // ============================================
    // standard_normal_cdf Tests
    // ============================================

    #[test]
    fn normal_cdf_at_zero_is_half() {
        let cdf = standard_normal_cdf(0.0);
        assert!((cdf - 0.5).abs() < 0.001, "cdf(0) was {cdf}");
    }

    #[test]
    fn normal_cdf_at_196_is_about_975() {
        let cdf = standard_normal_cdf(1.96);
        assert!((cdf - 0.975).abs() < 0.01, "cdf(1.96) was {cdf}");
    }

    #[test]
    fn normal_cdf_symmetry() {
        let cdf_pos = standard_normal_cdf(1.5);
        let cdf_neg = standard_normal_cdf(-1.5);
        assert!((cdf_pos + cdf_neg - 1.0).abs() < 0.001);
    }

    #[test]
    fn normal_cdf_large_positive() {
        let cdf = standard_normal_cdf(4.0);
        assert!(cdf > 0.999);
    }

    #[test]
    fn normal_cdf_large_negative() {
        let cdf = standard_normal_cdf(-4.0);
        assert!(cdf < 0.001);
    }
}
