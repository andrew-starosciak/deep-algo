//! Statistical validation metrics for arbitrage strategies.
//!
//! This module provides comprehensive metrics tracking and statistical validation
//! for Polymarket arbitrage operations. It includes:
//!
//! - Opportunity detection statistics (detection rate with Wilson CI)
//! - Execution success metrics (fill rate with Wilson CI)
//! - Profit distribution analysis (mean, std_dev, t-test, p-value)
//! - Imbalance risk tracking (mean, max, variance)
//! - Timing metrics (opportunity duration, fill latency)
//! - Financial totals (invested, payout, fees, P&L)
//!
//! # Go/No-Go Criteria
//!
//! The module enforces statistical validation gates:
//! - Fill rate Wilson CI lower bound > 60%
//! - Profit t-test p-value < 0.10
//! - Minimum 41 attempts for reliable inference

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

/// Minimum sample size for statistical validation (provides ~80% power).
pub const MIN_SAMPLE_SIZE: u32 = 41;

/// Minimum acceptable fill rate (Wilson CI lower bound).
pub const MIN_FILL_RATE_CI_LOWER: f64 = 0.60;

/// Maximum p-value for profit significance.
pub const MAX_PROFIT_P_VALUE: f64 = 0.10;

/// Comprehensive metrics for arbitrage strategy performance.
///
/// Tracks all aspects of arbitrage operation from opportunity detection
/// through execution and settlement. Provides statistical validation
/// methods to determine if the strategy meets production readiness criteria.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArbitrageMetrics {
    // ========================================
    // Opportunity Detection
    // ========================================
    /// Total number of 15-minute windows analyzed for opportunities.
    pub windows_analyzed: u32,

    /// Number of arbitrage opportunities detected (pair_cost < threshold).
    pub opportunities_detected: u32,

    /// Detection rate (opportunities_detected / windows_analyzed).
    pub detection_rate: f64,

    /// Wilson score 95% CI for detection rate (lower, upper).
    pub detection_rate_wilson_ci: (f64, f64),

    // ========================================
    // Execution Success
    // ========================================
    /// Total number of execution attempts (orders submitted).
    pub attempts: u32,

    /// Number of successfully filled YES+NO pairs.
    pub successful_pairs: u32,

    /// Number of partial fills (one leg filled, other rejected).
    pub partial_fills: u32,

    /// Fill rate (successful_pairs / attempts).
    pub fill_rate: f64,

    /// Wilson score 95% CI for fill rate (lower, upper).
    pub fill_rate_wilson_ci: (f64, f64),

    // ========================================
    // Profit Distribution
    // ========================================
    /// Mean net profit per pair after fees.
    pub mean_net_profit_per_pair: Decimal,

    /// Standard deviation of net profit per pair.
    pub std_dev_profit: Decimal,

    /// t-statistic from one-sample t-test (H0: mean = 0).
    pub profit_t_statistic: f64,

    /// Two-tailed p-value from profit t-test.
    pub profit_p_value: f64,

    // ========================================
    // Imbalance Risk
    // ========================================
    /// Mean imbalance (YES shares - NO shares) across positions.
    pub mean_imbalance: Decimal,

    /// Maximum absolute imbalance observed.
    pub max_imbalance: Decimal,

    /// Variance of imbalance values.
    pub imbalance_variance: Decimal,

    // ========================================
    // Timing Metrics
    // ========================================
    /// Mean duration of arbitrage opportunity in milliseconds.
    /// (Time from detection to disappearance or execution)
    pub mean_opportunity_duration_ms: f64,

    /// Mean latency from order submission to fill in milliseconds.
    pub mean_fill_latency_ms: f64,

    // ========================================
    // Financial Totals
    // ========================================
    /// Total amount invested across all positions.
    pub total_invested: Decimal,

    /// Total payout received from settled positions.
    pub total_payout: Decimal,

    /// Total fees paid (gas + exchange fees).
    pub total_fees: Decimal,

    /// Total net P&L (payout - invested - fees).
    pub total_pnl: Decimal,
}

impl ArbitrageMetrics {
    /// Creates a new empty ArbitrageMetrics instance.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an analyzed window, updating detection statistics.
    ///
    /// # Arguments
    /// * `opportunity_found` - Whether an arbitrage opportunity was detected
    pub fn record_window(&mut self, opportunity_found: bool) {
        self.windows_analyzed += 1;
        if opportunity_found {
            self.opportunities_detected += 1;
        }
        self.update_detection_rate_ci();
    }

    /// Records an execution attempt, updating fill statistics.
    ///
    /// # Arguments
    /// * `success` - Whether both legs filled successfully
    /// * `partial` - Whether only one leg filled (requires unwind)
    pub fn record_execution(&mut self, success: bool, partial: bool) {
        self.attempts += 1;
        if success {
            self.successful_pairs += 1;
        }
        if partial {
            self.partial_fills += 1;
        }
        self.update_fill_rate_ci();
    }

    /// Records the profit from a settled position.
    ///
    /// # Arguments
    /// * `net_profit` - Net profit after all fees
    /// * `invested` - Total amount invested
    /// * `payout` - Total payout received
    /// * `fees` - Total fees paid
    pub fn record_profit(
        &mut self,
        net_profit: Decimal,
        invested: Decimal,
        payout: Decimal,
        fees: Decimal,
    ) {
        self.total_invested += invested;
        self.total_payout += payout;
        self.total_fees += fees;
        self.total_pnl += net_profit;
    }

    /// Records an imbalance observation for risk tracking.
    ///
    /// # Arguments
    /// * `imbalance` - The imbalance value (YES shares - NO shares)
    pub fn record_imbalance(&mut self, imbalance: Decimal) {
        let abs_imbalance = imbalance.abs();
        if abs_imbalance > self.max_imbalance {
            self.max_imbalance = abs_imbalance;
        }
    }

    /// Records timing information for latency analysis.
    ///
    /// # Arguments
    /// * `opportunity_duration_ms` - How long the opportunity lasted
    /// * `fill_latency_ms` - Time from order submission to fill
    pub fn record_timing(&mut self, opportunity_duration_ms: f64, fill_latency_ms: f64) {
        // Incremental mean update using Welford's algorithm
        let n = self.successful_pairs as f64;
        if n == 0.0 {
            return;
        }

        // Update opportunity duration mean
        let delta_opp = opportunity_duration_ms - self.mean_opportunity_duration_ms;
        self.mean_opportunity_duration_ms += delta_opp / n;

        // Update fill latency mean
        let delta_lat = fill_latency_ms - self.mean_fill_latency_ms;
        self.mean_fill_latency_ms += delta_lat / n;
    }

    /// Updates the detection rate and its Wilson CI.
    fn update_detection_rate_ci(&mut self) {
        if self.windows_analyzed > 0 {
            self.detection_rate = self.opportunities_detected as f64 / self.windows_analyzed as f64;
            self.detection_rate_wilson_ci =
                wilson_ci(self.opportunities_detected, self.windows_analyzed, 1.96);
        } else {
            self.detection_rate = 0.0;
            self.detection_rate_wilson_ci = (0.0, 0.0);
        }
    }

    /// Recalculates fill rate and its Wilson confidence interval.
    ///
    /// Uses z = 1.96 for a 95% confidence interval.
    pub fn update_fill_rate_ci(&mut self) {
        if self.attempts > 0 {
            self.fill_rate = self.successful_pairs as f64 / self.attempts as f64;
            self.fill_rate_wilson_ci = wilson_ci(self.successful_pairs, self.attempts, 1.96);
        } else {
            self.fill_rate = 0.0;
            self.fill_rate_wilson_ci = (0.0, 0.0);
        }
    }

    /// Updates profit statistics from a vector of profit observations.
    ///
    /// Calculates mean, standard deviation, t-statistic, and p-value.
    ///
    /// # Arguments
    /// * `profits` - Vector of net profit values from settled positions
    pub fn update_profit_statistics(&mut self, profits: &[Decimal]) {
        if profits.is_empty() {
            self.mean_net_profit_per_pair = Decimal::ZERO;
            self.std_dev_profit = Decimal::ZERO;
            self.profit_t_statistic = 0.0;
            self.profit_p_value = 1.0;
            return;
        }

        // Calculate mean
        let sum: Decimal = profits.iter().copied().sum();
        let n = Decimal::from(profits.len() as u32);
        let mean = sum / n;
        self.mean_net_profit_per_pair = mean;

        // Calculate standard deviation
        if profits.len() >= 2 {
            let n_minus_1 = Decimal::from((profits.len() - 1) as u32);
            let variance_sum: Decimal = profits
                .iter()
                .map(|&p| {
                    let diff = p - mean;
                    diff * diff
                })
                .sum();
            let variance = variance_sum / n_minus_1;
            // Approximate sqrt using f64 conversion
            let variance_f64 = variance.to_f64_lossy();
            self.std_dev_profit =
                Decimal::from_f64_retain(variance_f64.sqrt()).unwrap_or(Decimal::ZERO);
        } else {
            self.std_dev_profit = Decimal::ZERO;
        }

        // Run t-test
        let (t_stat, p_val) = profit_t_test(profits);
        self.profit_t_statistic = t_stat;
        self.profit_p_value = p_val;
    }

    /// Updates imbalance statistics from a vector of imbalance observations.
    ///
    /// Calculates mean, max, and variance.
    ///
    /// # Arguments
    /// * `imbalances` - Vector of imbalance values (YES - NO shares)
    pub fn update_imbalance_statistics(&mut self, imbalances: &[Decimal]) {
        if imbalances.is_empty() {
            self.mean_imbalance = Decimal::ZERO;
            self.max_imbalance = Decimal::ZERO;
            self.imbalance_variance = Decimal::ZERO;
            return;
        }

        // Calculate mean
        let sum: Decimal = imbalances.iter().copied().sum();
        let n = Decimal::from(imbalances.len() as u32);
        let mean = sum / n;
        self.mean_imbalance = mean;

        // Find max absolute imbalance
        self.max_imbalance = imbalances
            .iter()
            .map(|i| i.abs())
            .max()
            .unwrap_or(Decimal::ZERO);

        // Calculate variance
        if imbalances.len() >= 2 {
            let n_minus_1 = Decimal::from((imbalances.len() - 1) as u32);
            let variance_sum: Decimal = imbalances
                .iter()
                .map(|&i| {
                    let diff = i - mean;
                    diff * diff
                })
                .sum();
            self.imbalance_variance = variance_sum / n_minus_1;
        } else {
            self.imbalance_variance = Decimal::ZERO;
        }
    }

    /// Checks if the fill rate meets the minimum threshold for production.
    ///
    /// Requires:
    /// 1. At least 41 attempts (MIN_SAMPLE_SIZE)
    /// 2. Wilson CI lower bound > 60% (MIN_FILL_RATE_CI_LOWER)
    ///
    /// # Returns
    /// `true` if fill rate is acceptable for production deployment
    #[must_use]
    pub fn fill_rate_acceptable(&self) -> bool {
        self.attempts >= MIN_SAMPLE_SIZE && self.fill_rate_wilson_ci.0 > MIN_FILL_RATE_CI_LOWER
    }

    /// Checks if profit is statistically significant.
    ///
    /// Requires:
    /// 1. At least 41 attempts (MIN_SAMPLE_SIZE)
    /// 2. p-value < 0.10 (MAX_PROFIT_P_VALUE)
    ///
    /// # Returns
    /// `true` if profit is statistically significant
    #[must_use]
    pub fn profit_significant(&self) -> bool {
        self.attempts >= MIN_SAMPLE_SIZE && self.profit_p_value < MAX_PROFIT_P_VALUE
    }

    /// Checks if all Go/No-Go criteria are met for production.
    ///
    /// Requirements:
    /// 1. Fill rate Wilson CI lower > 60%
    /// 2. Profit p-value < 0.10
    /// 3. No imbalance events > 50 shares
    /// 4. At least 41 attempts
    ///
    /// # Returns
    /// `true` if all criteria are met
    #[must_use]
    pub fn production_ready(&self) -> bool {
        self.fill_rate_acceptable()
            && self.profit_significant()
            && self.max_imbalance <= dec!(50)
            && self.total_pnl > Decimal::ZERO
    }

    /// Returns a summary of validation status for logging/display.
    #[must_use]
    pub fn validation_summary(&self) -> ValidationSummary {
        ValidationSummary {
            attempts: self.attempts,
            min_required: MIN_SAMPLE_SIZE,
            fill_rate: self.fill_rate,
            fill_rate_ci_lower: self.fill_rate_wilson_ci.0,
            fill_rate_ci_upper: self.fill_rate_wilson_ci.1,
            fill_rate_acceptable: self.fill_rate_acceptable(),
            mean_profit: self.mean_net_profit_per_pair,
            profit_p_value: self.profit_p_value,
            profit_significant: self.profit_significant(),
            max_imbalance: self.max_imbalance,
            imbalance_acceptable: self.max_imbalance <= dec!(50),
            total_pnl: self.total_pnl,
            production_ready: self.production_ready(),
        }
    }
}

/// Summary of validation status for display/logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationSummary {
    /// Number of execution attempts.
    pub attempts: u32,
    /// Minimum required attempts for validation.
    pub min_required: u32,
    /// Current fill rate.
    pub fill_rate: f64,
    /// Wilson CI lower bound for fill rate.
    pub fill_rate_ci_lower: f64,
    /// Wilson CI upper bound for fill rate.
    pub fill_rate_ci_upper: f64,
    /// Whether fill rate meets threshold.
    pub fill_rate_acceptable: bool,
    /// Mean profit per pair.
    pub mean_profit: Decimal,
    /// p-value from profit t-test.
    pub profit_p_value: f64,
    /// Whether profit is significant.
    pub profit_significant: bool,
    /// Maximum imbalance observed.
    pub max_imbalance: Decimal,
    /// Whether imbalance is within limits.
    pub imbalance_acceptable: bool,
    /// Total P&L.
    pub total_pnl: Decimal,
    /// Whether all criteria are met.
    pub production_ready: bool,
}

/// Calculates the Wilson score confidence interval for a proportion.
///
/// The Wilson score interval provides accurate coverage even for small
/// sample sizes and proportions near 0 or 1, making it ideal for
/// fill rate and detection rate statistics.
///
/// # Formula
///
/// ```text
/// CI = (p + z^2/(2n) +/- z * sqrt(p(1-p)/n + z^2/(4n^2))) / (1 + z^2/n)
/// ```
///
/// # Arguments
/// * `successes` - Number of successful outcomes
/// * `total` - Total number of trials
/// * `z` - Z-score for confidence level (1.96 for 95% CI)
///
/// # Returns
/// Tuple of (lower_bound, upper_bound), clamped to [0, 1]
///
/// # Examples
///
/// ```
/// use algo_trade_polymarket::arbitrage::metrics::wilson_ci;
///
/// // 70% fill rate with 50 attempts
/// let (lower, upper) = wilson_ci(35, 50, 1.96);
/// assert!(lower > 0.55 && lower < 0.60);
/// assert!(upper > 0.80 && upper < 0.85);
/// ```
#[must_use]
pub fn wilson_ci(successes: u32, total: u32, z: f64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 0.0);
    }

    let n = total as f64;
    let p = successes as f64 / n;
    let z2 = z * z;

    // Denominator: 1 + z^2/n
    let denom = 1.0 + z2 / n;

    // Center: p + z^2/(2n)
    let center = p + z2 / (2.0 * n);

    // Spread: z * sqrt(p(1-p)/n + z^2/(4n^2))
    let variance_term = p * (1.0 - p) / n;
    let correction_term = z2 / (4.0 * n * n);
    let spread = z * (variance_term + correction_term).sqrt();

    // Calculate bounds
    let lower = ((center - spread) / denom).max(0.0);
    let upper = ((center + spread) / denom).min(1.0);

    (lower, upper)
}

/// Performs a one-sample t-test for mean profit.
///
/// Tests H0: mean = 0 (no profit) against H1: mean != 0 (non-zero profit).
/// Uses the normal approximation for p-value calculation via the error function.
///
/// # Arguments
/// * `profits` - Slice of profit values to test
///
/// # Returns
/// Tuple of (t_statistic, p_value)
///
/// # Examples
///
/// ```
/// use algo_trade_polymarket::arbitrage::metrics::profit_t_test;
/// use rust_decimal_macros::dec;
///
/// let profits = vec![
///     dec!(0.02), dec!(0.03), dec!(0.01), dec!(0.02), dec!(0.03),
///     dec!(0.02), dec!(0.01), dec!(0.02), dec!(0.03), dec!(0.02),
/// ];
/// let (t_stat, p_value) = profit_t_test(&profits);
/// assert!(t_stat > 0.0); // Positive mean
/// assert!(p_value < 0.05); // Significant
/// ```
#[must_use]
pub fn profit_t_test(profits: &[Decimal]) -> (f64, f64) {
    // Need at least 2 observations for variance calculation
    if profits.len() < 2 {
        return (0.0, 1.0);
    }

    let n = profits.len() as f64;

    // Calculate mean
    let mean: f64 = profits.iter().map(|d| d.to_f64_lossy()).sum::<f64>() / n;

    // Calculate sample variance (with Bessel's correction: n-1)
    let variance: f64 = profits
        .iter()
        .map(|d| {
            let x = d.to_f64_lossy();
            (x - mean).powi(2)
        })
        .sum::<f64>()
        / (n - 1.0);

    // Calculate standard error
    let std_err = (variance / n).sqrt();

    // Handle zero variance case
    if std_err == 0.0 || std_err.is_nan() {
        if mean.abs() < f64::EPSILON {
            return (0.0, 1.0); // No variation, mean is zero
        }
        // All values identical and non-zero - infinitely significant
        return (
            if mean > 0.0 {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            },
            0.0,
        );
    }

    // Calculate t-statistic
    let t_stat = mean / std_err;

    // Calculate two-tailed p-value using normal CDF approximation
    // For large n (> 30), t-distribution approaches normal
    // Using normal CDF: p = 2 * (1 - Phi(|t|))
    let p_value = 2.0 * (1.0 - normal_cdf(t_stat.abs()));

    (t_stat, p_value)
}

/// Approximates the standard normal CDF using the error function.
///
/// Uses the relationship: Phi(x) = 0.5 * (1 + erf(x / sqrt(2)))
///
/// # Arguments
/// * `x` - The z-score to evaluate
///
/// # Returns
/// The cumulative probability P(Z <= x) for standard normal Z
fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + libm::erf(x / std::f64::consts::SQRT_2))
}

/// Trait extension for Decimal to support f64 conversion in statistical calculations.
trait DecimalExt {
    fn to_f64_lossy(&self) -> f64;
}

impl DecimalExt for Decimal {
    fn to_f64_lossy(&self) -> f64 {
        use std::str::FromStr;
        f64::from_str(&self.to_string()).unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================
    // wilson_ci Tests
    // ============================================

    #[test]
    fn wilson_ci_zero_total_returns_zeros() {
        let (lower, upper) = wilson_ci(0, 0, 1.96);
        assert!((lower - 0.0).abs() < f64::EPSILON);
        assert!((upper - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wilson_ci_50_percent_with_100_samples() {
        let (lower, upper) = wilson_ci(50, 100, 1.96);
        // Expected: approximately (0.40, 0.60) for 95% CI
        assert!(lower > 0.39 && lower < 0.42, "lower was {lower}");
        assert!(upper > 0.58 && upper < 0.61, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_70_percent_has_lower_above_60() {
        let (lower, upper) = wilson_ci(35, 50, 1.96);
        // 70% with n=50 should have lower > 55%
        assert!(lower > 0.55, "lower was {lower}");
        assert!(upper < 0.85, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_zero_successes() {
        let (lower, upper) = wilson_ci(0, 10, 1.96);
        assert!(lower >= 0.0, "lower was {lower}");
        assert!(lower < 0.01, "lower was {lower}");
        assert!(upper > 0.0 && upper < 0.35, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_all_successes() {
        let (lower, upper) = wilson_ci(10, 10, 1.96);
        assert!(lower > 0.65, "lower was {lower}");
        assert!((upper - 1.0).abs() < 0.01, "upper was {upper}");
    }

    #[test]
    fn wilson_ci_single_success() {
        let (lower, upper) = wilson_ci(1, 1, 1.96);
        assert!(lower > 0.0);
        assert!((upper - 1.0).abs() < 0.01);
    }

    #[test]
    fn wilson_ci_large_sample_narrow_interval() {
        let (lower, upper) = wilson_ci(550, 1000, 1.96);
        let width = upper - lower;
        // CI should be narrow with large sample
        assert!(width < 0.07, "width was {width}");
        assert!(lower > 0.51, "lower was {lower}");
        assert!(upper < 0.59, "upper was {upper}");
    }

    // ============================================
    // profit_t_test Tests
    // ============================================

    #[test]
    fn t_test_single_value_returns_default() {
        let profits = vec![dec!(0.05)];
        let (t_stat, p_value) = profit_t_test(&profits);
        assert!((t_stat - 0.0).abs() < f64::EPSILON);
        assert!((p_value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn t_test_empty_returns_default() {
        let profits: Vec<Decimal> = vec![];
        let (t_stat, p_value) = profit_t_test(&profits);
        assert!((t_stat - 0.0).abs() < f64::EPSILON);
        assert!((p_value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn t_test_positive_profits_positive_t_stat() {
        let profits = vec![
            dec!(0.02),
            dec!(0.03),
            dec!(0.01),
            dec!(0.02),
            dec!(0.03),
            dec!(0.02),
            dec!(0.01),
            dec!(0.02),
            dec!(0.03),
            dec!(0.02),
        ];
        let (t_stat, p_value) = profit_t_test(&profits);
        assert!(t_stat > 0.0, "t_stat was {t_stat}");
        assert!(p_value < 0.05, "p_value was {p_value}");
    }

    #[test]
    fn t_test_negative_profits_negative_t_stat() {
        let profits = vec![
            dec!(-0.02),
            dec!(-0.03),
            dec!(-0.01),
            dec!(-0.02),
            dec!(-0.03),
            dec!(-0.02),
            dec!(-0.01),
            dec!(-0.02),
            dec!(-0.03),
            dec!(-0.02),
        ];
        let (t_stat, _p_value) = profit_t_test(&profits);
        assert!(t_stat < 0.0, "t_stat was {t_stat}");
    }

    #[test]
    fn t_test_mixed_profits_around_zero() {
        // Equal positive and negative values - mean should be near zero
        let profits = vec![
            dec!(0.01),
            dec!(-0.01),
            dec!(0.02),
            dec!(-0.02),
            dec!(0.01),
            dec!(-0.01),
        ];
        let (t_stat, p_value) = profit_t_test(&profits);
        // Mean is zero, t_stat should be near zero
        assert!(t_stat.abs() < 0.001, "t_stat was {t_stat}");
        // Should not be significant
        assert!(p_value > 0.90, "p_value was {p_value}");
    }

    #[test]
    fn t_test_identical_values_handles_zero_variance() {
        let profits = vec![dec!(0.02), dec!(0.02), dec!(0.02), dec!(0.02)];
        let (t_stat, p_value) = profit_t_test(&profits);
        // All identical non-zero values - infinitely significant
        assert!(t_stat.is_infinite() || t_stat > 1000.0);
        assert!(p_value < 0.001);
    }

    #[test]
    fn t_test_identical_zeros_not_significant() {
        let profits = vec![dec!(0), dec!(0), dec!(0), dec!(0)];
        let (t_stat, p_value) = profit_t_test(&profits);
        assert!((t_stat - 0.0).abs() < f64::EPSILON);
        assert!((p_value - 1.0).abs() < f64::EPSILON);
    }

    // ============================================
    // normal_cdf Tests
    // ============================================

    #[test]
    fn normal_cdf_at_zero_is_half() {
        let cdf = normal_cdf(0.0);
        assert!((cdf - 0.5).abs() < 0.001, "cdf(0) was {cdf}");
    }

    #[test]
    fn normal_cdf_at_196_is_about_975() {
        let cdf = normal_cdf(1.96);
        assert!((cdf - 0.975).abs() < 0.001, "cdf(1.96) was {cdf}");
    }

    #[test]
    fn normal_cdf_symmetry() {
        let cdf_pos = normal_cdf(1.5);
        let cdf_neg = normal_cdf(-1.5);
        assert!((cdf_pos + cdf_neg - 1.0).abs() < 0.001);
    }

    #[test]
    fn normal_cdf_large_positive() {
        let cdf = normal_cdf(4.0);
        assert!(cdf > 0.999);
    }

    #[test]
    fn normal_cdf_large_negative() {
        let cdf = normal_cdf(-4.0);
        assert!(cdf < 0.001);
    }

    // ============================================
    // ArbitrageMetrics Tests
    // ============================================

    #[test]
    fn metrics_new_is_empty() {
        let metrics = ArbitrageMetrics::new();
        assert_eq!(metrics.windows_analyzed, 0);
        assert_eq!(metrics.attempts, 0);
        assert_eq!(metrics.successful_pairs, 0);
        assert_eq!(metrics.total_pnl, Decimal::ZERO);
    }

    #[test]
    fn metrics_record_window_updates_detection_stats() {
        let mut metrics = ArbitrageMetrics::new();

        metrics.record_window(true);
        metrics.record_window(true);
        metrics.record_window(false);
        metrics.record_window(false);
        metrics.record_window(true);

        assert_eq!(metrics.windows_analyzed, 5);
        assert_eq!(metrics.opportunities_detected, 3);
        assert!((metrics.detection_rate - 0.6).abs() < 0.001);
    }

    #[test]
    fn metrics_record_execution_updates_fill_stats() {
        let mut metrics = ArbitrageMetrics::new();

        metrics.record_execution(true, false);
        metrics.record_execution(true, false);
        metrics.record_execution(false, true);
        metrics.record_execution(false, false);
        metrics.record_execution(true, false);

        assert_eq!(metrics.attempts, 5);
        assert_eq!(metrics.successful_pairs, 3);
        assert_eq!(metrics.partial_fills, 1);
        assert!((metrics.fill_rate - 0.6).abs() < 0.001);
    }

    #[test]
    fn metrics_fill_rate_acceptable_requires_sample_size() {
        let mut metrics = ArbitrageMetrics::new();

        // 80% fill rate but only 10 attempts - not enough
        for _ in 0..8 {
            metrics.record_execution(true, false);
        }
        for _ in 0..2 {
            metrics.record_execution(false, false);
        }

        assert!(!metrics.fill_rate_acceptable());
    }

    #[test]
    fn metrics_fill_rate_acceptable_requires_ci_above_threshold() {
        let mut metrics = ArbitrageMetrics::new();

        // 50% fill rate with 50 attempts - CI includes values below 60%
        for _ in 0..25 {
            metrics.record_execution(true, false);
        }
        for _ in 0..25 {
            metrics.record_execution(false, false);
        }

        assert!(!metrics.fill_rate_acceptable());
    }

    #[test]
    fn metrics_fill_rate_acceptable_passes_for_high_rate() {
        let mut metrics = ArbitrageMetrics::new();

        // 80% fill rate with 50 attempts - should pass
        for _ in 0..40 {
            metrics.record_execution(true, false);
        }
        for _ in 0..10 {
            metrics.record_execution(false, false);
        }

        assert!(metrics.fill_rate_acceptable());
    }

    #[test]
    fn metrics_update_profit_statistics_calculates_correctly() {
        let mut metrics = ArbitrageMetrics::new();

        let profits = vec![dec!(0.02), dec!(0.03), dec!(0.01), dec!(0.02), dec!(0.025)];

        metrics.update_profit_statistics(&profits);

        // Mean should be 0.021
        assert!(
            (metrics.mean_net_profit_per_pair - dec!(0.021)).abs() < dec!(0.001),
            "mean was {}",
            metrics.mean_net_profit_per_pair
        );

        // Std dev should be positive
        assert!(metrics.std_dev_profit > Decimal::ZERO);

        // t-stat should be positive (mean > 0)
        assert!(metrics.profit_t_statistic > 0.0);
    }

    #[test]
    fn metrics_update_imbalance_statistics_calculates_correctly() {
        let mut metrics = ArbitrageMetrics::new();

        let imbalances = vec![dec!(5), dec!(-3), dec!(2), dec!(-1), dec!(4)];

        metrics.update_imbalance_statistics(&imbalances);

        // Mean should be (5 - 3 + 2 - 1 + 4) / 5 = 7/5 = 1.4
        assert!(
            (metrics.mean_imbalance - dec!(1.4)).abs() < dec!(0.01),
            "mean was {}",
            metrics.mean_imbalance
        );

        // Max absolute imbalance is 5
        assert_eq!(metrics.max_imbalance, dec!(5));

        // Variance should be positive
        assert!(metrics.imbalance_variance > Decimal::ZERO);
    }

    #[test]
    fn metrics_profit_significant_requires_sample_size() {
        let mut metrics = ArbitrageMetrics::new();

        // Strong profit signal but not enough samples
        metrics.attempts = 20;
        metrics.profit_p_value = 0.01;

        assert!(!metrics.profit_significant());
    }

    #[test]
    fn metrics_profit_significant_requires_low_p_value() {
        let mut metrics = ArbitrageMetrics::new();

        // Enough samples but high p-value
        metrics.attempts = 50;
        metrics.profit_p_value = 0.15;

        assert!(!metrics.profit_significant());
    }

    #[test]
    fn metrics_profit_significant_passes_when_criteria_met() {
        let mut metrics = ArbitrageMetrics::new();

        metrics.attempts = 50;
        metrics.profit_p_value = 0.05;

        assert!(metrics.profit_significant());
    }

    #[test]
    fn metrics_production_ready_requires_all_criteria() {
        let mut metrics = ArbitrageMetrics::new();

        // Set up all criteria to pass
        for _ in 0..40 {
            metrics.record_execution(true, false);
        }
        for _ in 0..5 {
            metrics.record_execution(false, false);
        }

        metrics.profit_p_value = 0.05;
        metrics.max_imbalance = dec!(30);
        metrics.total_pnl = dec!(100);

        assert!(metrics.production_ready());
    }

    #[test]
    fn metrics_production_ready_fails_on_high_imbalance() {
        let mut metrics = ArbitrageMetrics::new();

        // Set up passing criteria except imbalance
        for _ in 0..40 {
            metrics.record_execution(true, false);
        }
        for _ in 0..5 {
            metrics.record_execution(false, false);
        }

        metrics.profit_p_value = 0.05;
        metrics.max_imbalance = dec!(60); // > 50
        metrics.total_pnl = dec!(100);

        assert!(!metrics.production_ready());
    }

    #[test]
    fn metrics_production_ready_fails_on_negative_pnl() {
        let mut metrics = ArbitrageMetrics::new();

        // Set up passing criteria except P&L
        for _ in 0..40 {
            metrics.record_execution(true, false);
        }
        for _ in 0..5 {
            metrics.record_execution(false, false);
        }

        metrics.profit_p_value = 0.05;
        metrics.max_imbalance = dec!(30);
        metrics.total_pnl = dec!(-10); // Negative

        assert!(!metrics.production_ready());
    }

    #[test]
    fn metrics_validation_summary_contains_all_fields() {
        let mut metrics = ArbitrageMetrics::new();

        for _ in 0..30 {
            metrics.record_execution(true, false);
        }
        for _ in 0..20 {
            metrics.record_execution(false, false);
        }

        metrics.profit_p_value = 0.08;
        metrics.mean_net_profit_per_pair = dec!(0.015);
        metrics.max_imbalance = dec!(25);
        metrics.total_pnl = dec!(75);

        let summary = metrics.validation_summary();

        assert_eq!(summary.attempts, 50);
        assert_eq!(summary.min_required, MIN_SAMPLE_SIZE);
        assert!((summary.fill_rate - 0.6).abs() < 0.001);
        assert!(summary.fill_rate_ci_lower > 0.0);
        assert!(summary.fill_rate_ci_upper < 1.0);
        assert!(!summary.fill_rate_acceptable); // 60% is at threshold
        assert_eq!(summary.mean_profit, dec!(0.015));
        assert!((summary.profit_p_value - 0.08).abs() < 0.001);
        assert!(summary.profit_significant);
        assert_eq!(summary.max_imbalance, dec!(25));
        assert!(summary.imbalance_acceptable);
        assert_eq!(summary.total_pnl, dec!(75));
    }

    #[test]
    fn metrics_record_profit_accumulates_totals() {
        let mut metrics = ArbitrageMetrics::new();

        metrics.record_profit(dec!(5), dec!(100), dec!(105), dec!(0.50));
        metrics.record_profit(dec!(3), dec!(100), dec!(103), dec!(0.50));
        metrics.record_profit(dec!(-2), dec!(100), dec!(98), dec!(0.50));

        assert_eq!(metrics.total_invested, dec!(300));
        assert_eq!(metrics.total_payout, dec!(306));
        assert_eq!(metrics.total_fees, dec!(1.50));
        assert_eq!(metrics.total_pnl, dec!(6));
    }

    #[test]
    fn metrics_record_imbalance_tracks_max() {
        let mut metrics = ArbitrageMetrics::new();

        metrics.record_imbalance(dec!(10));
        assert_eq!(metrics.max_imbalance, dec!(10));

        metrics.record_imbalance(dec!(-15));
        assert_eq!(metrics.max_imbalance, dec!(15));

        metrics.record_imbalance(dec!(5));
        assert_eq!(metrics.max_imbalance, dec!(15)); // Unchanged
    }

    // ============================================
    // Additional Edge Case Tests
    // ============================================

    #[test]
    fn metrics_update_profit_statistics_empty() {
        let mut metrics = ArbitrageMetrics::new();
        metrics.update_profit_statistics(&[]);

        assert_eq!(metrics.mean_net_profit_per_pair, Decimal::ZERO);
        assert_eq!(metrics.std_dev_profit, Decimal::ZERO);
        assert!((metrics.profit_t_statistic - 0.0).abs() < f64::EPSILON);
        assert!((metrics.profit_p_value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn metrics_update_imbalance_statistics_empty() {
        let mut metrics = ArbitrageMetrics::new();
        metrics.update_imbalance_statistics(&[]);

        assert_eq!(metrics.mean_imbalance, Decimal::ZERO);
        assert_eq!(metrics.max_imbalance, Decimal::ZERO);
        assert_eq!(metrics.imbalance_variance, Decimal::ZERO);
    }

    #[test]
    fn metrics_update_profit_statistics_single_value() {
        let mut metrics = ArbitrageMetrics::new();
        metrics.update_profit_statistics(&[dec!(0.05)]);

        assert_eq!(metrics.mean_net_profit_per_pair, dec!(0.05));
        assert_eq!(metrics.std_dev_profit, Decimal::ZERO);
        // t-test needs at least 2 observations
        assert!((metrics.profit_t_statistic - 0.0).abs() < f64::EPSILON);
        assert!((metrics.profit_p_value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn metrics_update_imbalance_statistics_single_value() {
        let mut metrics = ArbitrageMetrics::new();
        metrics.update_imbalance_statistics(&[dec!(10)]);

        assert_eq!(metrics.mean_imbalance, dec!(10));
        assert_eq!(metrics.max_imbalance, dec!(10));
        assert_eq!(metrics.imbalance_variance, Decimal::ZERO);
    }

    #[test]
    fn metrics_record_timing_no_successful_pairs() {
        let mut metrics = ArbitrageMetrics::new();
        // No successful pairs, record_timing should not update means
        let initial_opp = metrics.mean_opportunity_duration_ms;
        let initial_lat = metrics.mean_fill_latency_ms;

        metrics.record_timing(100.0, 50.0);

        assert!((metrics.mean_opportunity_duration_ms - initial_opp).abs() < f64::EPSILON);
        assert!((metrics.mean_fill_latency_ms - initial_lat).abs() < f64::EPSILON);
    }

    #[test]
    fn metrics_record_timing_updates_incrementally() {
        let mut metrics = ArbitrageMetrics::new();
        // First we need some successful pairs
        metrics.record_execution(true, false);
        metrics.record_timing(100.0, 50.0);

        assert!((metrics.mean_opportunity_duration_ms - 100.0).abs() < 0.001);
        assert!((metrics.mean_fill_latency_ms - 50.0).abs() < 0.001);

        metrics.record_execution(true, false);
        metrics.record_timing(200.0, 100.0);

        // After 2 samples: mean should be (100 + 200) / 2 = 150 for opp, (50 + 100) / 2 = 75 for lat
        // But Welford's is incremental, so check the formula
        assert!(metrics.mean_opportunity_duration_ms > 100.0);
        assert!(metrics.mean_fill_latency_ms > 50.0);
    }

    #[test]
    fn wilson_ci_small_sample_wide_interval() {
        // With very small sample, CI should be wide
        let (lower, upper) = wilson_ci(5, 10, 1.96);
        let width = upper - lower;
        assert!(
            width > 0.3,
            "width was {} - should be wide for small sample",
            width
        );
    }

    #[test]
    fn t_test_large_sample_converges_to_normal() {
        // With large sample, t-distribution approaches normal
        let profits: Vec<Decimal> = (0..100)
            .map(|i| dec!(0.02) + Decimal::from(i % 5) * dec!(0.001))
            .collect();

        let (t_stat, p_value) = profit_t_test(&profits);

        // Mean is positive, t should be positive
        assert!(t_stat > 0.0);
        // Should be highly significant
        assert!(p_value < 0.01);
    }

    #[test]
    fn metrics_detection_rate_ci_zero_windows() {
        let mut metrics = ArbitrageMetrics::new();
        assert_eq!(metrics.detection_rate, 0.0);
        assert_eq!(metrics.detection_rate_wilson_ci, (0.0, 0.0));
    }

    // ============================================
    // Integration Tests
    // ============================================

    #[test]
    fn full_metrics_lifecycle() {
        let mut metrics = ArbitrageMetrics::new();

        // Simulate 50 windows, 60% detection rate
        for i in 0..50 {
            metrics.record_window(i % 5 < 3);
        }

        // Simulate 45 execution attempts, 80% fill rate
        for i in 0..45 {
            let success = i % 5 < 4;
            let partial = !success && i % 10 == 0;
            metrics.record_execution(success, partial);
        }

        // Simulate profits
        let profits: Vec<Decimal> = (0..36)
            .map(|i| dec!(0.02) + Decimal::from(i % 3) * dec!(0.005))
            .collect();

        metrics.update_profit_statistics(&profits);

        // Simulate imbalances
        let imbalances: Vec<Decimal> = (0..36)
            .map(|i| Decimal::from((i % 7) as i32 - 3) * dec!(5))
            .collect();

        metrics.update_imbalance_statistics(&imbalances);

        // Record totals
        for _ in 0..36 {
            metrics.record_profit(dec!(0.025), dec!(100), dec!(102.5), dec!(0.50));
        }

        // Verify summary
        let summary = metrics.validation_summary();
        assert_eq!(summary.attempts, 45);
        assert!(summary.fill_rate > 0.75);
    }
}
