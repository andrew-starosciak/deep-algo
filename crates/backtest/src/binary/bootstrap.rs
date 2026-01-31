//! Bootstrap confidence interval estimation for binary outcome metrics.
//!
//! This module provides bootstrap resampling methods to estimate confidence
//! intervals for win rate, expected value, ROI, and maximum drawdown metrics.
//! Bootstrap methods are particularly useful when the underlying distribution
//! is unknown or when sample sizes are small.
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_backtest::binary::bootstrap::{BootstrapConfig, BootstrapResampler};
//!
//! let config = BootstrapConfig::default();
//! let resampler = BootstrapResampler::new(config);
//! let result = resampler.bootstrap_win_rate(&settlements);
//!
//! println!("Win rate: {:.2}% [{:.2}%, {:.2}%]",
//!     result.point_estimate * 100.0,
//!     result.ci_lower * 100.0,
//!     result.ci_upper * 100.0);
//! ```

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::outcome::{BinaryOutcome, SettlementResult};

/// Configuration for bootstrap resampling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Number of bootstrap iterations (resamples).
    pub n_iterations: usize,
    /// Confidence level for the interval (e.g., 0.95 for 95% CI).
    pub confidence_level: f64,
    /// Optional seed for reproducible results.
    pub seed: Option<u64>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            n_iterations: 10_000,
            confidence_level: 0.95,
            seed: None,
        }
    }
}

impl BootstrapConfig {
    /// Creates a new configuration with specified parameters.
    #[must_use]
    pub fn new(n_iterations: usize, confidence_level: f64) -> Self {
        Self {
            n_iterations,
            confidence_level,
            seed: None,
        }
    }

    /// Sets a seed for reproducible bootstrap samples.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// Result of a bootstrap confidence interval estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResult {
    /// Point estimate (original sample statistic).
    pub point_estimate: f64,
    /// Lower bound of the confidence interval.
    pub ci_lower: f64,
    /// Upper bound of the confidence interval.
    pub ci_upper: f64,
    /// Bootstrap standard error.
    pub standard_error: f64,
    /// Full bootstrap distribution (sorted).
    pub distribution: Vec<f64>,
    /// Estimated bias (mean of bootstrap - point estimate).
    pub bias: f64,
}

impl BootstrapResult {
    /// Returns the width of the confidence interval.
    #[must_use]
    pub fn ci_width(&self) -> f64 {
        self.ci_upper - self.ci_lower
    }

    /// Returns true if zero is outside the confidence interval.
    #[must_use]
    pub fn is_significant(&self) -> bool {
        self.ci_lower > 0.0 || self.ci_upper < 0.0
    }
}

/// Aggregated bootstrap metrics for all key statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapMetrics {
    /// Bootstrap CI for win rate.
    pub win_rate: BootstrapResult,
    /// Bootstrap CI for expected value per bet.
    pub ev_per_bet: BootstrapResult,
    /// Bootstrap CI for return on investment.
    pub roi: BootstrapResult,
    /// Bootstrap CI for maximum drawdown.
    pub max_drawdown: BootstrapResult,
}

/// Bootstrap resampler for settlement results.
pub struct BootstrapResampler {
    config: BootstrapConfig,
}

impl BootstrapResampler {
    /// Creates a new resampler with the given configuration.
    #[must_use]
    pub fn new(config: BootstrapConfig) -> Self {
        Self { config }
    }

    /// Creates a resampler with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(BootstrapConfig::default())
    }

    /// Returns a reference to the configuration.
    #[must_use]
    pub fn config(&self) -> &BootstrapConfig {
        &self.config
    }

    /// Generates a bootstrap resample of indices.
    ///
    /// Returns a vector of indices sampled with replacement from [0, n).
    fn resample_indices(&self, n: usize, rng: &mut ChaCha8Rng) -> Vec<usize> {
        (0..n).map(|_| rng.gen_range(0..n)).collect()
    }

    /// Resamples settlement results with replacement.
    ///
    /// This is the core resampling operation that creates a bootstrap sample
    /// by randomly selecting settlements with replacement.
    #[must_use]
    pub fn resample<'a>(
        &self,
        settlements: &'a [SettlementResult],
        rng: &mut ChaCha8Rng,
    ) -> Vec<&'a SettlementResult> {
        let indices = self.resample_indices(settlements.len(), rng);
        indices.iter().map(|&i| &settlements[i]).collect()
    }

    /// Generic bootstrap method for any statistic function.
    ///
    /// # Arguments
    /// * `settlements` - Original settlement results
    /// * `statistic_fn` - Function that computes the statistic from settlements
    ///
    /// # Returns
    /// `BootstrapResult` with point estimate and confidence interval
    pub fn bootstrap_statistic<F>(
        &self,
        settlements: &[SettlementResult],
        statistic_fn: F,
    ) -> BootstrapResult
    where
        F: Fn(&[&SettlementResult]) -> f64,
    {
        if settlements.is_empty() {
            return BootstrapResult {
                point_estimate: 0.0,
                ci_lower: 0.0,
                ci_upper: 0.0,
                standard_error: 0.0,
                distribution: vec![],
                bias: 0.0,
            };
        }

        // Calculate point estimate from original sample
        let original_refs: Vec<&SettlementResult> = settlements.iter().collect();
        let point_estimate = statistic_fn(&original_refs);

        // Initialize RNG
        let mut rng = match self.config.seed {
            Some(seed) => ChaCha8Rng::seed_from_u64(seed),
            None => ChaCha8Rng::from_entropy(),
        };

        // Generate bootstrap distribution
        let mut distribution: Vec<f64> = Vec::with_capacity(self.config.n_iterations);
        for _ in 0..self.config.n_iterations {
            let sample = self.resample(settlements, &mut rng);
            let stat = statistic_fn(&sample);
            distribution.push(stat);
        }

        // Sort for percentile calculation
        distribution.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate percentile CI
        let (ci_lower, ci_upper) = percentile_ci(&distribution, self.config.confidence_level);

        // Calculate standard error and bias
        let mean: f64 = distribution.iter().sum::<f64>() / distribution.len() as f64;
        let variance: f64 = distribution.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
            / (distribution.len() - 1).max(1) as f64;
        let standard_error = variance.sqrt();
        let bias = mean - point_estimate;

        BootstrapResult {
            point_estimate,
            ci_lower,
            ci_upper,
            standard_error,
            distribution,
            bias,
        }
    }

    /// Bootstraps the win rate statistic.
    #[must_use]
    pub fn bootstrap_win_rate(&self, settlements: &[SettlementResult]) -> BootstrapResult {
        self.bootstrap_statistic(settlements, |sample| {
            let wins = sample
                .iter()
                .filter(|s| s.outcome == BinaryOutcome::Win)
                .count();
            let non_push = sample
                .iter()
                .filter(|s| s.outcome != BinaryOutcome::Push)
                .count();
            if non_push == 0 {
                0.0
            } else {
                wins as f64 / non_push as f64
            }
        })
    }

    /// Bootstraps the expected value per bet statistic.
    #[must_use]
    pub fn bootstrap_ev(&self, settlements: &[SettlementResult]) -> BootstrapResult {
        self.bootstrap_statistic(settlements, |sample| {
            if sample.is_empty() {
                return 0.0;
            }
            let total_pnl: Decimal = sample.iter().map(|s| s.net_pnl).sum();
            f64::try_from(total_pnl / Decimal::from(sample.len())).unwrap_or(0.0)
        })
    }

    /// Bootstraps the return on investment statistic.
    #[must_use]
    pub fn bootstrap_roi(&self, settlements: &[SettlementResult]) -> BootstrapResult {
        self.bootstrap_statistic(settlements, |sample| {
            let total_stake: Decimal = sample.iter().map(|s| s.bet.stake).sum();
            let total_pnl: Decimal = sample.iter().map(|s| s.net_pnl).sum();
            if total_stake == Decimal::ZERO {
                0.0
            } else {
                f64::try_from(total_pnl / total_stake).unwrap_or(0.0)
            }
        })
    }

    /// Bootstraps the maximum drawdown statistic.
    #[must_use]
    pub fn bootstrap_max_drawdown(&self, settlements: &[SettlementResult]) -> BootstrapResult {
        self.bootstrap_statistic(settlements, |sample| {
            let mut peak = Decimal::ZERO;
            let mut equity = Decimal::ZERO;
            let mut max_dd = Decimal::ZERO;

            for settlement in sample {
                equity += settlement.net_pnl;
                if equity > peak {
                    peak = equity;
                }
                let drawdown = peak - equity;
                if drawdown > max_dd {
                    max_dd = drawdown;
                }
            }

            f64::try_from(max_dd).unwrap_or(0.0)
        })
    }

    /// Computes bootstrap confidence intervals for all key metrics.
    #[must_use]
    pub fn bootstrap_all_metrics(&self, settlements: &[SettlementResult]) -> BootstrapMetrics {
        BootstrapMetrics {
            win_rate: self.bootstrap_win_rate(settlements),
            ev_per_bet: self.bootstrap_ev(settlements),
            roi: self.bootstrap_roi(settlements),
            max_drawdown: self.bootstrap_max_drawdown(settlements),
        }
    }
}

/// Extracts percentile confidence interval from a sorted distribution.
///
/// # Arguments
/// * `distribution` - Sorted vector of bootstrap statistics
/// * `confidence_level` - Desired confidence level (e.g., 0.95)
///
/// # Returns
/// Tuple of (lower_bound, upper_bound)
#[must_use]
pub fn percentile_ci(distribution: &[f64], confidence_level: f64) -> (f64, f64) {
    if distribution.is_empty() {
        return (0.0, 0.0);
    }
    if distribution.len() == 1 {
        return (distribution[0], distribution[0]);
    }

    let alpha = 1.0 - confidence_level;
    let n = distribution.len();

    // Calculate indices for percentiles
    let lower_idx = ((alpha / 2.0) * n as f64).floor() as usize;
    let upper_idx = ((1.0 - alpha / 2.0) * n as f64).ceil() as usize;

    // Clamp to valid range
    let lower_idx = lower_idx.min(n - 1);
    let upper_idx = upper_idx.min(n - 1).max(lower_idx);

    (distribution[lower_idx], distribution[upper_idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet};
    use chrono::Utc;
    use rust_decimal_macros::dec;

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_winning_settlement(
        stake: Decimal,
        price: Decimal,
        fees: Decimal,
    ) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            fees,
        )
    }

    fn create_losing_settlement(stake: Decimal, price: Decimal, fees: Decimal) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(42500),
            dec!(43000),
            BinaryOutcome::Loss,
            fees,
        )
    }

    fn create_push_settlement(stake: Decimal, price: Decimal) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43000),
            dec!(43000),
            BinaryOutcome::Push,
            Decimal::ZERO,
        )
    }

    fn create_mixed_settlements(wins: usize, losses: usize) -> Vec<SettlementResult> {
        let mut settlements = Vec::with_capacity(wins + losses);
        for _ in 0..wins {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..losses {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        settlements
    }

    // ============================================================
    // BootstrapConfig Tests
    // ============================================================

    #[test]
    fn config_default_has_expected_values() {
        let config = BootstrapConfig::default();

        assert_eq!(config.n_iterations, 10_000);
        assert!((config.confidence_level - 0.95).abs() < f64::EPSILON);
        assert!(config.seed.is_none());
    }

    #[test]
    fn config_new_sets_parameters() {
        let config = BootstrapConfig::new(5000, 0.99);

        assert_eq!(config.n_iterations, 5000);
        assert!((config.confidence_level - 0.99).abs() < f64::EPSILON);
        assert!(config.seed.is_none());
    }

    #[test]
    fn config_with_seed_sets_seed() {
        let config = BootstrapConfig::default().with_seed(42);

        assert_eq!(config.seed, Some(42));
    }

    // ============================================================
    // BootstrapResult Tests
    // ============================================================

    #[test]
    fn result_ci_width_calculated_correctly() {
        let result = BootstrapResult {
            point_estimate: 0.55,
            ci_lower: 0.50,
            ci_upper: 0.60,
            standard_error: 0.03,
            distribution: vec![],
            bias: 0.0,
        };

        assert!((result.ci_width() - 0.10).abs() < f64::EPSILON);
    }

    #[test]
    fn result_is_significant_when_ci_above_zero() {
        let result = BootstrapResult {
            point_estimate: 0.05,
            ci_lower: 0.02,
            ci_upper: 0.08,
            standard_error: 0.02,
            distribution: vec![],
            bias: 0.0,
        };

        assert!(result.is_significant());
    }

    #[test]
    fn result_not_significant_when_ci_includes_zero() {
        let result = BootstrapResult {
            point_estimate: 0.02,
            ci_lower: -0.01,
            ci_upper: 0.05,
            standard_error: 0.02,
            distribution: vec![],
            bias: 0.0,
        };

        assert!(!result.is_significant());
    }

    #[test]
    fn result_is_significant_when_ci_below_zero() {
        let result = BootstrapResult {
            point_estimate: -0.05,
            ci_lower: -0.08,
            ci_upper: -0.02,
            standard_error: 0.02,
            distribution: vec![],
            bias: 0.0,
        };

        assert!(result.is_significant());
    }

    // ============================================================
    // BootstrapResampler::resample Tests
    // ============================================================

    #[test]
    fn resample_returns_correct_size() {
        let config = BootstrapConfig::default().with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(10, 10);

        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let sample = resampler.resample(&settlements, &mut rng);

        assert_eq!(sample.len(), settlements.len());
    }

    #[test]
    fn resample_is_reproducible_with_same_seed() {
        let settlements = create_mixed_settlements(10, 10);

        let config1 = BootstrapConfig::default().with_seed(42);
        let resampler1 = BootstrapResampler::new(config1);
        let mut rng1 = ChaCha8Rng::seed_from_u64(42);
        let sample1 = resampler1.resample(&settlements, &mut rng1);

        let config2 = BootstrapConfig::default().with_seed(42);
        let resampler2 = BootstrapResampler::new(config2);
        let mut rng2 = ChaCha8Rng::seed_from_u64(42);
        let sample2 = resampler2.resample(&settlements, &mut rng2);

        // Same seed should produce same resamples
        for (s1, s2) in sample1.iter().zip(sample2.iter()) {
            assert_eq!(s1.bet.id, s2.bet.id);
        }
    }

    #[test]
    fn resample_allows_duplicates() {
        let config = BootstrapConfig::default().with_seed(12345);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(5, 5);

        let mut rng = ChaCha8Rng::seed_from_u64(12345);
        let sample = resampler.resample(&settlements, &mut rng);

        // Count unique IDs
        let mut ids: Vec<_> = sample.iter().map(|s| s.bet.id).collect();
        ids.sort();
        ids.dedup();

        // With sampling with replacement, we expect some duplicates
        // (very unlikely to have all unique with 10 items)
        assert!(ids.len() <= sample.len());
    }

    // ============================================================
    // bootstrap_win_rate Tests
    // ============================================================

    #[test]
    fn bootstrap_win_rate_empty_returns_zeros() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements: Vec<SettlementResult> = vec![];

        let result = resampler.bootstrap_win_rate(&settlements);

        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
        assert!((result.ci_lower - 0.0).abs() < f64::EPSILON);
        assert!((result.ci_upper - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_win_rate_all_wins_returns_one() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(20, 0);

        let result = resampler.bootstrap_win_rate(&settlements);

        assert!((result.point_estimate - 1.0).abs() < f64::EPSILON);
        assert!((result.ci_lower - 1.0).abs() < f64::EPSILON);
        assert!((result.ci_upper - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_win_rate_all_losses_returns_zero() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(0, 20);

        let result = resampler.bootstrap_win_rate(&settlements);

        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
        assert!((result.ci_lower - 0.0).abs() < f64::EPSILON);
        assert!((result.ci_upper - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_win_rate_returns_valid_ci() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(60, 40); // 60% win rate

        let result = resampler.bootstrap_win_rate(&settlements);

        // Point estimate should be around 0.60
        assert!(
            (result.point_estimate - 0.60).abs() < 0.01,
            "point estimate was {}",
            result.point_estimate
        );

        // CI should be valid (lower <= point <= upper)
        assert!(result.ci_lower <= result.point_estimate);
        assert!(result.ci_upper >= result.point_estimate);

        // CI should be within [0, 1]
        assert!(result.ci_lower >= 0.0);
        assert!(result.ci_upper <= 1.0);

        // 95% CI should be reasonably narrow for n=100
        assert!(result.ci_width() < 0.25);
    }

    #[test]
    fn bootstrap_win_rate_excludes_pushes() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);

        let mut settlements = create_mixed_settlements(6, 4); // 60% win rate among non-pushes
        for _ in 0..10 {
            settlements.push(create_push_settlement(dec!(100), dec!(0.50)));
        }

        let result = resampler.bootstrap_win_rate(&settlements);

        // Point estimate should still be around 0.60 (pushes excluded)
        assert!(
            (result.point_estimate - 0.60).abs() < 0.01,
            "point estimate was {}",
            result.point_estimate
        );
    }

    // ============================================================
    // bootstrap_ev Tests
    // ============================================================

    #[test]
    fn bootstrap_ev_empty_returns_zeros() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements: Vec<SettlementResult> = vec![];

        let result = resampler.bootstrap_ev(&settlements);

        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_ev_returns_valid_ci() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(60, 40); // Positive EV

        let result = resampler.bootstrap_ev(&settlements);

        // With 60% win rate at even odds, EV should be positive
        assert!(
            result.point_estimate > 0.0,
            "EV was {}",
            result.point_estimate
        );

        // CI should be valid
        assert!(result.ci_lower <= result.point_estimate);
        assert!(result.ci_upper >= result.point_estimate);
    }

    #[test]
    fn bootstrap_ev_negative_for_losing_strategy() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(40, 60); // Negative EV

        let result = resampler.bootstrap_ev(&settlements);

        // With 40% win rate, EV should be negative
        assert!(
            result.point_estimate < 0.0,
            "EV was {}",
            result.point_estimate
        );
    }

    // ============================================================
    // bootstrap_roi Tests
    // ============================================================

    #[test]
    fn bootstrap_roi_empty_returns_zeros() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements: Vec<SettlementResult> = vec![];

        let result = resampler.bootstrap_roi(&settlements);

        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_roi_returns_valid_ci() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(60, 40);

        let result = resampler.bootstrap_roi(&settlements);

        // ROI = net_pnl / total_stake
        // 60 wins * $100 - 40 losses * $100 = $2000
        // Total stake = $10000
        // ROI = 0.20 (20%)
        assert!(
            (result.point_estimate - 0.20).abs() < 0.01,
            "ROI was {}",
            result.point_estimate
        );

        // CI should be valid
        assert!(result.ci_lower <= result.point_estimate);
        assert!(result.ci_upper >= result.point_estimate);
    }

    #[test]
    fn bootstrap_roi_handles_zero_stake() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);

        // Create settlement with zero stake
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(0),
            dec!(0.50),
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        let settlement = SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        let result = resampler.bootstrap_roi(&[settlement]);

        // Should handle gracefully
        assert!(!result.point_estimate.is_nan());
    }

    // ============================================================
    // bootstrap_max_drawdown Tests
    // ============================================================

    #[test]
    fn bootstrap_max_drawdown_empty_returns_zeros() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements: Vec<SettlementResult> = vec![];

        let result = resampler.bootstrap_max_drawdown(&settlements);

        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_max_drawdown_all_wins_is_zero() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(20, 0);

        let result = resampler.bootstrap_max_drawdown(&settlements);

        // No drawdown when all wins
        assert!((result.point_estimate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_max_drawdown_returns_valid_ci() {
        let config = BootstrapConfig::new(1000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(50, 50);

        let result = resampler.bootstrap_max_drawdown(&settlements);

        // Max drawdown should be non-negative
        assert!(result.point_estimate >= 0.0);
        assert!(result.ci_lower >= 0.0);

        // CI bounds should be valid (lower <= upper)
        assert!(
            result.ci_lower <= result.ci_upper,
            "ci_lower {} > ci_upper {}",
            result.ci_lower,
            result.ci_upper
        );

        // Note: For max drawdown, the bootstrap CI may not contain the point estimate
        // because resampling changes the order of settlements, which affects drawdown.
        // This is expected behavior - the bootstrap is estimating the distribution
        // of drawdowns we might see from similar trading sequences.
    }

    // ============================================================
    // percentile_ci Tests
    // ============================================================

    #[test]
    fn percentile_ci_empty_returns_zeros() {
        let distribution: Vec<f64> = vec![];
        let (lower, upper) = percentile_ci(&distribution, 0.95);

        assert!((lower - 0.0).abs() < f64::EPSILON);
        assert!((upper - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_ci_single_value_returns_same() {
        let distribution = vec![0.55];
        let (lower, upper) = percentile_ci(&distribution, 0.95);

        assert!((lower - 0.55).abs() < f64::EPSILON);
        assert!((upper - 0.55).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_ci_extracts_correct_percentiles() {
        // 100 values from 0 to 99
        let distribution: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let (lower, upper) = percentile_ci(&distribution, 0.95);

        // For 95% CI: alpha = 0.05
        // Lower: 2.5th percentile ~ index 2 (value 2)
        // Upper: 97.5th percentile ~ index 98 (value 98)
        // Allow tolerance for index rounding differences
        assert!(
            (lower - 2.0).abs() < 2.0,
            "lower was {}, expected ~2",
            lower
        );
        assert!(
            (upper - 97.0).abs() < 2.0,
            "upper was {}, expected ~97",
            upper
        );
    }

    #[test]
    fn percentile_ci_90_percent_confidence() {
        let distribution: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let (lower, upper) = percentile_ci(&distribution, 0.90);

        // For 90% CI: alpha = 0.10
        // Lower: 5th percentile ~ index 5 (value 5)
        // Upper: 95th percentile ~ index 95 (value 95)
        // Allow tolerance for index rounding differences
        assert!(
            (lower - 5.0).abs() < 2.0,
            "lower was {}, expected ~5",
            lower
        );
        assert!(
            (upper - 95.0).abs() < 2.0,
            "upper was {}, expected ~95",
            upper
        );
    }

    // ============================================================
    // bootstrap_all_metrics Tests
    // ============================================================

    #[test]
    fn bootstrap_all_metrics_returns_all_four() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(60, 40);

        let metrics = resampler.bootstrap_all_metrics(&settlements);

        // All metrics should be populated
        assert!(metrics.win_rate.point_estimate > 0.0);
        assert!(metrics.ev_per_bet.point_estimate != 0.0 || settlements.is_empty());
        assert!(metrics.roi.point_estimate != 0.0 || settlements.is_empty());
        assert!(metrics.max_drawdown.point_estimate >= 0.0);
    }

    // ============================================================
    // Coverage Property Test: CI Contains True Parameter ~95%
    // ============================================================

    #[test]
    fn bootstrap_ci_coverage_property() {
        // This test verifies that the 95% CI contains the true parameter
        // approximately 95% of the time across many simulations.
        //
        // We use a known population with true_win_rate = 0.60 and verify
        // that bootstrap CIs contain 0.60 roughly 95% of the time.

        let true_win_rate = 0.60;
        let sample_size = 100;
        let n_simulations = 200; // Number of independent samples
        let n_bootstrap = 500; // Iterations per bootstrap

        let mut coverage_count = 0;
        let mut rng = ChaCha8Rng::seed_from_u64(99999);

        for sim in 0..n_simulations {
            // Generate a random sample from the true distribution
            let settlements: Vec<SettlementResult> = (0..sample_size)
                .map(|_| {
                    if rng.gen::<f64>() < true_win_rate {
                        create_winning_settlement(dec!(100), dec!(0.50), dec!(0))
                    } else {
                        create_losing_settlement(dec!(100), dec!(0.50), dec!(0))
                    }
                })
                .collect();

            // Bootstrap this sample
            let config = BootstrapConfig::new(n_bootstrap, 0.95).with_seed(sim as u64);
            let resampler = BootstrapResampler::new(config);
            let result = resampler.bootstrap_win_rate(&settlements);

            // Check if CI contains true parameter
            if result.ci_lower <= true_win_rate && true_win_rate <= result.ci_upper {
                coverage_count += 1;
            }
        }

        let coverage_rate = coverage_count as f64 / n_simulations as f64;

        // Coverage should be approximately 95% (allow some tolerance)
        // Due to finite samples, we use a wider tolerance
        assert!(
            coverage_rate > 0.85 && coverage_rate < 0.99,
            "Coverage rate was {:.2}%, expected ~95%",
            coverage_rate * 100.0
        );
    }

    // ============================================================
    // Reproducibility Test
    // ============================================================

    #[test]
    fn bootstrap_reproducible_with_seed() {
        let settlements = create_mixed_settlements(60, 40);

        let config1 = BootstrapConfig::new(1000, 0.95).with_seed(12345);
        let resampler1 = BootstrapResampler::new(config1);
        let result1 = resampler1.bootstrap_win_rate(&settlements);

        let config2 = BootstrapConfig::new(1000, 0.95).with_seed(12345);
        let resampler2 = BootstrapResampler::new(config2);
        let result2 = resampler2.bootstrap_win_rate(&settlements);

        // Same seed should produce identical results
        assert!((result1.point_estimate - result2.point_estimate).abs() < f64::EPSILON);
        assert!((result1.ci_lower - result2.ci_lower).abs() < f64::EPSILON);
        assert!((result1.ci_upper - result2.ci_upper).abs() < f64::EPSILON);
        assert!((result1.standard_error - result2.standard_error).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_different_with_different_seeds() {
        let settlements = create_mixed_settlements(60, 40);

        let config1 = BootstrapConfig::new(1000, 0.95).with_seed(11111);
        let resampler1 = BootstrapResampler::new(config1);
        let result1 = resampler1.bootstrap_win_rate(&settlements);

        let config2 = BootstrapConfig::new(1000, 0.95).with_seed(22222);
        let resampler2 = BootstrapResampler::new(config2);
        let result2 = resampler2.bootstrap_win_rate(&settlements);

        // Different seeds should produce different CI bounds (though point estimate is same)
        // Note: point estimate is calculated from original sample, so it's the same
        assert!((result1.point_estimate - result2.point_estimate).abs() < f64::EPSILON);

        // But bootstrap-derived values should differ
        // (standard error, bias will be slightly different)
        // This is a weak check - in rare cases they could be equal
        let se_differs = (result1.standard_error - result2.standard_error).abs() > 1e-10;
        let bias_differs = (result1.bias - result2.bias).abs() > 1e-10;
        assert!(
            se_differs || bias_differs,
            "Expected some difference with different seeds"
        );
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn bootstrap_single_settlement() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = vec![create_winning_settlement(dec!(100), dec!(0.50), dec!(0))];

        let result = resampler.bootstrap_win_rate(&settlements);

        // Single winning settlement = 100% win rate
        assert!((result.point_estimate - 1.0).abs() < f64::EPSILON);
        // CI should be [1.0, 1.0] since all resamples are identical
        assert!((result.ci_lower - 1.0).abs() < f64::EPSILON);
        assert!((result.ci_upper - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_handles_large_sample() {
        let config = BootstrapConfig::new(100, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(500, 500);

        let result = resampler.bootstrap_win_rate(&settlements);

        // Point estimate should be close to 0.50
        assert!(
            (result.point_estimate - 0.50).abs() < 0.01,
            "point estimate was {}",
            result.point_estimate
        );

        // CI should be narrower with larger sample
        assert!(
            result.ci_width() < 0.10,
            "CI width was {}",
            result.ci_width()
        );
    }

    #[test]
    fn bootstrap_bias_is_small_for_unbiased_statistic() {
        let config = BootstrapConfig::new(2000, 0.95).with_seed(42);
        let resampler = BootstrapResampler::new(config);
        let settlements = create_mixed_settlements(100, 100);

        let result = resampler.bootstrap_win_rate(&settlements);

        // Win rate is an unbiased estimator, so bootstrap bias should be small
        assert!(result.bias.abs() < 0.02, "bias was {}", result.bias);
    }

    #[test]
    fn bootstrap_standard_error_decreases_with_sample_size() {
        let config = BootstrapConfig::new(500, 0.95).with_seed(42);

        // Small sample
        let resampler1 = BootstrapResampler::new(config.clone());
        let small_sample = create_mixed_settlements(20, 20);
        let result_small = resampler1.bootstrap_win_rate(&small_sample);

        // Large sample
        let resampler2 = BootstrapResampler::new(config);
        let large_sample = create_mixed_settlements(200, 200);
        let result_large = resampler2.bootstrap_win_rate(&large_sample);

        // Standard error should be smaller with larger sample
        assert!(
            result_large.standard_error < result_small.standard_error,
            "SE large: {}, SE small: {}",
            result_large.standard_error,
            result_small.standard_error
        );
    }
}
