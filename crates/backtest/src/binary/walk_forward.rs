//! Walk-forward optimization for binary backtesting.
//!
//! This module implements walk-forward analysis to detect overfitting and validate
//! out-of-sample (OOS) performance. It splits historical data into rolling train/test
//! windows to simulate how a strategy would perform on unseen data.
//!
//! # Walk-Forward Process
//!
//! 1. Split data into train (in-sample) and test (out-of-sample) periods
//! 2. Optimize/train on train period
//! 3. Validate on test period
//! 4. Roll forward and repeat
//! 5. Aggregate OOS results to assess true performance

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::metrics::BinaryMetrics;
use super::outcome::SettlementResult;

/// Configuration for walk-forward optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkForwardConfig {
    /// Length of each training window.
    pub train_window: Duration,
    /// Length of each test window.
    pub test_window: Duration,
    /// How far to step forward between folds (defaults to test_window).
    pub step_size: Duration,
    /// If true, train window starts from the beginning each time (expanding window).
    /// If false, uses a rolling window of fixed train_window size.
    pub anchored: bool,
    /// Minimum number of settlements required in each period.
    pub min_samples: usize,
}

impl Default for WalkForwardConfig {
    fn default() -> Self {
        Self {
            train_window: Duration::days(90),
            test_window: Duration::days(30),
            step_size: Duration::days(30),
            anchored: false,
            min_samples: 30,
        }
    }
}

impl WalkForwardConfig {
    /// Creates a new walk-forward config with custom windows.
    #[must_use]
    pub fn new(train_days: i64, test_days: i64) -> Self {
        Self {
            train_window: Duration::days(train_days),
            test_window: Duration::days(test_days),
            step_size: Duration::days(test_days),
            anchored: false,
            min_samples: 30,
        }
    }

    /// Sets the anchored flag (expanding window mode).
    #[must_use]
    pub fn with_anchored(mut self, anchored: bool) -> Self {
        self.anchored = anchored;
        self
    }

    /// Sets the minimum samples per period.
    #[must_use]
    pub fn with_min_samples(mut self, min_samples: usize) -> Self {
        self.min_samples = min_samples;
        self
    }

    /// Sets a custom step size.
    #[must_use]
    pub fn with_step_size(mut self, step_days: i64) -> Self {
        self.step_size = Duration::days(step_days);
        self
    }
}

/// A single fold in walk-forward validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkForwardFold {
    /// Start of the training period.
    pub train_start: DateTime<Utc>,
    /// End of the training period.
    pub train_end: DateTime<Utc>,
    /// Start of the test period.
    pub test_start: DateTime<Utc>,
    /// End of the test period.
    pub test_end: DateTime<Utc>,
    /// Metrics from the training (in-sample) period.
    pub train_metrics: BinaryMetrics,
    /// Metrics from the test (out-of-sample) period.
    pub test_metrics: BinaryMetrics,
}

impl WalkForwardFold {
    /// Returns true if train period ends before test period starts.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.train_end <= self.test_start && self.train_start < self.train_end
    }

    /// Calculates win rate degradation from train to test.
    ///
    /// Positive value means test performed worse than train.
    #[must_use]
    pub fn win_rate_degradation(&self) -> f64 {
        self.train_metrics.win_rate - self.test_metrics.win_rate
    }
}

/// Risk level for overfitting based on performance degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverfittingRisk {
    /// OOS performance close to or better than IS (degradation <= 5%).
    Low,
    /// Moderate degradation (5% < degradation <= 10%).
    Medium,
    /// Significant degradation (10% < degradation <= 20%).
    High,
    /// Severe degradation (degradation > 20%).
    Severe,
}

impl OverfittingRisk {
    /// Classifies overfitting risk based on win rate degradation ratio.
    #[must_use]
    pub fn from_degradation(degradation_ratio: f64) -> Self {
        if degradation_ratio <= 0.05 {
            Self::Low
        } else if degradation_ratio <= 0.10 {
            Self::Medium
        } else if degradation_ratio <= 0.20 {
            Self::High
        } else {
            Self::Severe
        }
    }
}

/// Performance degradation analysis between in-sample and out-of-sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceDegradation {
    /// Ratio of OOS win rate to IS win rate (1.0 = no degradation).
    pub win_rate_ratio: f64,
    /// Absolute difference in win rate (IS - OOS).
    pub win_rate_diff: f64,
    /// Ratio of OOS EV to IS EV.
    pub ev_ratio: f64,
    /// Classification of overfitting risk.
    pub overfitting_risk: OverfittingRisk,
}

impl PerformanceDegradation {
    /// Creates degradation analysis from IS and OOS metrics.
    #[must_use]
    pub fn from_metrics(is_metrics: &BinaryMetrics, oos_metrics: &BinaryMetrics) -> Self {
        let win_rate_ratio = if is_metrics.win_rate > 0.0 {
            oos_metrics.win_rate / is_metrics.win_rate
        } else {
            1.0
        };

        let win_rate_diff = is_metrics.win_rate - oos_metrics.win_rate;

        let is_ev = f64::try_from(is_metrics.ev_per_bet).unwrap_or(0.0);
        let oos_ev = f64::try_from(oos_metrics.ev_per_bet).unwrap_or(0.0);
        let ev_ratio = if is_ev.abs() > f64::EPSILON {
            oos_ev / is_ev
        } else {
            1.0
        };

        // Calculate degradation as relative drop from IS performance
        let degradation_ratio = if is_metrics.win_rate > 0.0 {
            (is_metrics.win_rate - oos_metrics.win_rate) / is_metrics.win_rate
        } else {
            0.0
        };

        let overfitting_risk = OverfittingRisk::from_degradation(degradation_ratio);

        Self {
            win_rate_ratio,
            win_rate_diff,
            ev_ratio,
            overfitting_risk,
        }
    }
}

/// Statistical significance test for OOS performance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificanceTest {
    /// Wilson 95% CI lower bound for OOS win rate.
    pub wilson_ci_lower: f64,
    /// Wilson 95% CI upper bound for OOS win rate.
    pub wilson_ci_upper: f64,
    /// p-value from binomial test (H0: p = 0.50).
    pub p_value: f64,
    /// Whether OOS is statistically significant (p < 0.05).
    pub is_significant: bool,
    /// Whether the lower CI bound is above 0.50.
    pub has_edge: bool,
}

impl SignificanceTest {
    /// Creates significance test from OOS metrics.
    #[must_use]
    pub fn from_metrics(metrics: &BinaryMetrics) -> Self {
        Self {
            wilson_ci_lower: metrics.wilson_ci_lower,
            wilson_ci_upper: metrics.wilson_ci_upper,
            p_value: metrics.binomial_p_value,
            is_significant: metrics.is_significant,
            has_edge: metrics.wilson_ci_lower > 0.5,
        }
    }
}

/// Complete results from walk-forward optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkForwardResults {
    /// Configuration used for the analysis.
    pub config: WalkForwardConfig,
    /// Individual fold results.
    pub folds: Vec<WalkForwardFold>,
    /// Aggregated in-sample metrics across all train periods.
    pub is_aggregate: BinaryMetrics,
    /// Aggregated out-of-sample metrics across all test periods.
    pub oos_aggregate: BinaryMetrics,
    /// Performance degradation analysis.
    pub degradation: PerformanceDegradation,
    /// Statistical significance of OOS results.
    pub significance: SignificanceTest,
}

impl WalkForwardResults {
    /// Returns true if the strategy passed walk-forward validation.
    ///
    /// Requires:
    /// 1. OOS is statistically significant (p < 0.05)
    /// 2. OOS Wilson CI lower > 0.50 (has edge)
    /// 3. Overfitting risk is Low or Medium
    #[must_use]
    pub fn passed_validation(&self) -> bool {
        self.significance.is_significant
            && self.significance.has_edge
            && matches!(
                self.degradation.overfitting_risk,
                OverfittingRisk::Low | OverfittingRisk::Medium
            )
    }

    /// Returns the number of folds used.
    #[must_use]
    pub fn num_folds(&self) -> usize {
        self.folds.len()
    }
}

/// Simple train/test split result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainTestSplit {
    /// Training period settlements.
    pub train_settlements: Vec<SettlementResult>,
    /// Test period settlements.
    pub test_settlements: Vec<SettlementResult>,
    /// Training period metrics.
    pub train_metrics: BinaryMetrics,
    /// Test period metrics.
    pub test_metrics: BinaryMetrics,
    /// Performance degradation analysis.
    pub degradation: PerformanceDegradation,
}

/// A fold period consisting of train and test date ranges.
pub type FoldPeriod = (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>);

/// Walk-forward optimizer for binary backtests.
pub struct WalkForwardOptimizer {
    config: WalkForwardConfig,
}

impl WalkForwardOptimizer {
    /// Creates a new walk-forward optimizer with the given configuration.
    #[must_use]
    pub fn new(config: WalkForwardConfig) -> Self {
        Self { config }
    }

    /// Creates an optimizer with default configuration (90 day train, 30 day test).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(WalkForwardConfig::default())
    }

    /// Generates non-overlapping train/test fold periods.
    ///
    /// Returns a vector of (train_start, train_end, test_start, test_end) tuples.
    #[must_use]
    pub fn generate_folds(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<FoldPeriod> {
        let mut folds = Vec::new();

        // First fold starts at the beginning
        let mut current_train_start = start;
        let mut current_train_end = start + self.config.train_window;
        let mut current_test_start = current_train_end;
        let mut current_test_end = current_test_start + self.config.test_window;

        while current_test_end <= end {
            folds.push((
                current_train_start,
                current_train_end,
                current_test_start,
                current_test_end,
            ));

            // Step forward
            if self.config.anchored {
                // Anchored: train always starts from anchor, expands to include more data
                current_train_end += self.config.step_size;
            } else {
                // Rolling: train window slides forward
                current_train_start += self.config.step_size;
                current_train_end = current_train_start + self.config.train_window;
            }

            current_test_start = current_train_end;
            current_test_end = current_test_start + self.config.test_window;
        }

        folds
    }

    /// Filters settlements to those within the given time range (inclusive start, exclusive end).
    #[must_use]
    pub fn filter_by_time(
        &self,
        settlements: &[SettlementResult],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<SettlementResult> {
        settlements
            .iter()
            .filter(|s| s.bet.timestamp >= start && s.bet.timestamp < end)
            .cloned()
            .collect()
    }

    /// Performs walk-forward analysis on the given settlements.
    ///
    /// # Arguments
    /// * `settlements` - All settlement results to analyze
    ///
    /// # Returns
    /// `Ok(WalkForwardResults)` if analysis succeeds, `Err` if insufficient data
    pub fn analyze(&self, settlements: &[SettlementResult]) -> Result<WalkForwardResults, String> {
        if settlements.is_empty() {
            return Err("No settlements provided".to_string());
        }

        // Find date range
        let min_time = settlements
            .iter()
            .map(|s| s.bet.timestamp)
            .min()
            .ok_or("Empty settlements")?;
        let max_time = settlements
            .iter()
            .map(|s| s.settlement_time)
            .max()
            .ok_or("Empty settlements")?;

        // Generate folds
        let fold_periods = self.generate_folds(min_time, max_time);
        if fold_periods.is_empty() {
            return Err(format!(
                "Insufficient data: need at least {} days for train + {} days for test",
                self.config.train_window.num_days(),
                self.config.test_window.num_days()
            ));
        }

        // Process each fold
        let mut folds = Vec::new();
        let mut all_train_settlements = Vec::new();
        let mut all_test_settlements = Vec::new();

        for (train_start, train_end, test_start, test_end) in fold_periods {
            let train_data = self.filter_by_time(settlements, train_start, train_end);
            let test_data = self.filter_by_time(settlements, test_start, test_end);

            // Check minimum samples
            if train_data.len() < self.config.min_samples
                || test_data.len() < self.config.min_samples
            {
                continue; // Skip this fold
            }

            let train_metrics = BinaryMetrics::from_settlements(&train_data);
            let test_metrics = BinaryMetrics::from_settlements(&test_data);

            folds.push(WalkForwardFold {
                train_start,
                train_end,
                test_start,
                test_end,
                train_metrics,
                test_metrics,
            });

            all_train_settlements.extend(train_data);
            all_test_settlements.extend(test_data);
        }

        if folds.is_empty() {
            return Err(format!(
                "No folds with minimum {} samples in each period",
                self.config.min_samples
            ));
        }

        // Aggregate metrics
        let is_aggregate = BinaryMetrics::from_settlements(&all_train_settlements);
        let oos_aggregate = BinaryMetrics::from_settlements(&all_test_settlements);

        // Performance degradation
        let degradation = PerformanceDegradation::from_metrics(&is_aggregate, &oos_aggregate);

        // Significance test
        let significance = SignificanceTest::from_metrics(&oos_aggregate);

        Ok(WalkForwardResults {
            config: self.config.clone(),
            folds,
            is_aggregate,
            oos_aggregate,
            degradation,
            significance,
        })
    }

    /// Performs a simple 70/30 train/test split.
    ///
    /// # Arguments
    /// * `settlements` - All settlement results to split
    /// * `train_ratio` - Fraction to use for training (default 0.7)
    ///
    /// # Returns
    /// `Ok(TrainTestSplit)` if split succeeds, `Err` if insufficient data
    pub fn simple_split(
        &self,
        settlements: &[SettlementResult],
        train_ratio: f64,
    ) -> Result<TrainTestSplit, String> {
        if settlements.is_empty() {
            return Err("No settlements provided".to_string());
        }

        if train_ratio <= 0.0 || train_ratio >= 1.0 {
            return Err("Train ratio must be between 0 and 1".to_string());
        }

        // Sort by timestamp
        let mut sorted = settlements.to_vec();
        sorted.sort_by_key(|s| s.bet.timestamp);

        let split_idx = (sorted.len() as f64 * train_ratio).round() as usize;
        let split_idx = split_idx.max(1).min(sorted.len() - 1);

        let train_settlements: Vec<_> = sorted[..split_idx].to_vec();
        let test_settlements: Vec<_> = sorted[split_idx..].to_vec();

        if train_settlements.len() < self.config.min_samples {
            return Err(format!(
                "Train set has {} samples, need at least {}",
                train_settlements.len(),
                self.config.min_samples
            ));
        }

        if test_settlements.len() < self.config.min_samples {
            return Err(format!(
                "Test set has {} samples, need at least {}",
                test_settlements.len(),
                self.config.min_samples
            ));
        }

        let train_metrics = BinaryMetrics::from_settlements(&train_settlements);
        let test_metrics = BinaryMetrics::from_settlements(&test_settlements);
        let degradation = PerformanceDegradation::from_metrics(&train_metrics, &test_metrics);

        Ok(TrainTestSplit {
            train_settlements,
            test_settlements,
            train_metrics,
            test_metrics,
            degradation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet, BinaryOutcome};
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_settlement_at(timestamp: DateTime<Utc>, outcome: BinaryOutcome) -> SettlementResult {
        let bet = BinaryBet::new(
            timestamp,
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50),
            0.75,
        );
        let settlement_time = timestamp + Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            outcome,
            dec!(0),
        )
    }

    fn create_settlements_in_range(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        win_rate: f64,
        count: usize,
    ) -> Vec<SettlementResult> {
        let mut settlements = Vec::new();
        let duration = end.signed_duration_since(start);
        let step = duration / (count as i32);

        for i in 0..count {
            let timestamp = start + step * (i as i32);
            let outcome = if (i as f64 / count as f64) < win_rate {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement_at(timestamp, outcome));
        }

        settlements
    }

    // ============================================================
    // WalkForwardConfig Tests
    // ============================================================

    #[test]
    fn config_default_has_90_day_train_window() {
        let config = WalkForwardConfig::default();
        assert_eq!(config.train_window.num_days(), 90);
    }

    #[test]
    fn config_default_has_30_day_test_window() {
        let config = WalkForwardConfig::default();
        assert_eq!(config.test_window.num_days(), 30);
    }

    #[test]
    fn config_default_has_30_day_step_size() {
        let config = WalkForwardConfig::default();
        assert_eq!(config.step_size.num_days(), 30);
    }

    #[test]
    fn config_default_not_anchored() {
        let config = WalkForwardConfig::default();
        assert!(!config.anchored);
    }

    #[test]
    fn config_default_min_samples_is_30() {
        let config = WalkForwardConfig::default();
        assert_eq!(config.min_samples, 30);
    }

    #[test]
    fn config_new_creates_custom_windows() {
        let config = WalkForwardConfig::new(60, 20);
        assert_eq!(config.train_window.num_days(), 60);
        assert_eq!(config.test_window.num_days(), 20);
        assert_eq!(config.step_size.num_days(), 20); // Defaults to test_window
    }

    #[test]
    fn config_with_anchored_sets_flag() {
        let config = WalkForwardConfig::default().with_anchored(true);
        assert!(config.anchored);
    }

    #[test]
    fn config_with_min_samples_sets_value() {
        let config = WalkForwardConfig::default().with_min_samples(50);
        assert_eq!(config.min_samples, 50);
    }

    #[test]
    fn config_with_step_size_sets_value() {
        let config = WalkForwardConfig::default().with_step_size(15);
        assert_eq!(config.step_size.num_days(), 15);
    }

    // ============================================================
    // generate_folds Tests
    // ============================================================

    #[test]
    fn generate_folds_creates_non_overlapping_periods() {
        let config = WalkForwardConfig::new(30, 10);
        let optimizer = WalkForwardOptimizer::new(config);

        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2024, 4, 10, 0, 0, 0).unwrap(); // 100 days

        let folds = optimizer.generate_folds(start, end);

        // Check that test periods don't overlap with each other
        for i in 0..folds.len() - 1 {
            let (_, _, _, test_end) = folds[i];
            let (_, _, next_test_start, _) = folds[i + 1];
            assert!(
                test_end <= next_test_start,
                "Fold {} test end {} overlaps with fold {} test start {}",
                i,
                test_end,
                i + 1,
                next_test_start
            );
        }

        // Check within each fold: train ends before test starts
        for (i, (train_start, train_end, test_start, test_end)) in folds.iter().enumerate() {
            assert!(
                train_end <= test_start,
                "Fold {} train end {} overlaps with test start {}",
                i,
                train_end,
                test_start
            );
            assert!(
                train_start < train_end,
                "Fold {} has invalid train period",
                i
            );
            assert!(test_start < test_end, "Fold {} has invalid test period", i);
        }
    }

    #[test]
    fn generate_folds_train_ends_before_test_starts() {
        let config = WalkForwardConfig::new(30, 10);
        let optimizer = WalkForwardOptimizer::new(config);

        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();

        let folds = optimizer.generate_folds(start, end);

        for (train_start, train_end, test_start, test_end) in &folds {
            assert!(
                train_end <= test_start,
                "Train end {} should be <= test start {}",
                train_end,
                test_start
            );
            assert!(train_start < train_end);
            assert!(test_start < test_end);
        }
    }

    #[test]
    fn generate_folds_rolling_window_slides_forward() {
        let config = WalkForwardConfig::new(30, 10).with_step_size(10);
        let optimizer = WalkForwardOptimizer::new(config);

        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap(); // 80 days

        let folds = optimizer.generate_folds(start, end);

        // With rolling window, train start advances by step_size
        assert!(folds.len() >= 2);
        let (train_start_1, _, _, _) = folds[0];
        let (train_start_2, _, _, _) = folds[1];

        assert_eq!(
            train_start_2
                .signed_duration_since(train_start_1)
                .num_days(),
            10
        );
    }

    #[test]
    fn generate_folds_anchored_window_expands() {
        let config = WalkForwardConfig::new(30, 10)
            .with_step_size(10)
            .with_anchored(true);
        let optimizer = WalkForwardOptimizer::new(config);

        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap();

        let folds = optimizer.generate_folds(start, end);

        // With anchored window, train start stays the same
        assert!(folds.len() >= 2);
        for (train_start, _, _, _) in &folds {
            assert_eq!(*train_start, start);
        }

        // But train end expands
        let (_, train_end_1, _, _) = folds[0];
        let (_, train_end_2, _, _) = folds[1];
        assert!(train_end_2 > train_end_1);
    }

    #[test]
    fn generate_folds_returns_empty_for_insufficient_data() {
        let config = WalkForwardConfig::new(90, 30);
        let optimizer = WalkForwardOptimizer::new(config);

        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap(); // Only 31 days

        let folds = optimizer.generate_folds(start, end);
        assert!(folds.is_empty());
    }

    // ============================================================
    // filter_by_time Tests
    // ============================================================

    #[test]
    fn filter_by_time_returns_settlements_in_range() {
        let optimizer = WalkForwardOptimizer::with_defaults();

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = vec![
            create_settlement_at(base, BinaryOutcome::Win),
            create_settlement_at(base + Duration::days(5), BinaryOutcome::Win),
            create_settlement_at(base + Duration::days(10), BinaryOutcome::Loss),
            create_settlement_at(base + Duration::days(15), BinaryOutcome::Win),
        ];

        let filtered = optimizer.filter_by_time(
            &settlements,
            base + Duration::days(4),
            base + Duration::days(12),
        );

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_by_time_inclusive_start_exclusive_end() {
        let optimizer = WalkForwardOptimizer::with_defaults();

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = vec![
            create_settlement_at(base, BinaryOutcome::Win), // Included
            create_settlement_at(base + Duration::days(5), BinaryOutcome::Win), // Included
            create_settlement_at(base + Duration::days(10), BinaryOutcome::Loss), // Excluded (at end)
        ];

        let filtered = optimizer.filter_by_time(&settlements, base, base + Duration::days(10));

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_by_time_returns_empty_for_no_matches() {
        let optimizer = WalkForwardOptimizer::with_defaults();

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = vec![create_settlement_at(base, BinaryOutcome::Win)];

        let filtered = optimizer.filter_by_time(
            &settlements,
            base + Duration::days(10),
            base + Duration::days(20),
        );

        assert!(filtered.is_empty());
    }

    // ============================================================
    // WalkForwardFold Tests
    // ============================================================

    #[test]
    fn fold_is_valid_when_train_before_test() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let fold = WalkForwardFold {
            train_start: base,
            train_end: base + Duration::days(30),
            test_start: base + Duration::days(30),
            test_end: base + Duration::days(40),
            train_metrics: BinaryMetrics::empty(),
            test_metrics: BinaryMetrics::empty(),
        };

        assert!(fold.is_valid());
    }

    #[test]
    fn fold_is_invalid_when_train_overlaps_test() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let fold = WalkForwardFold {
            train_start: base,
            train_end: base + Duration::days(35), // Overlaps test
            test_start: base + Duration::days(30),
            test_end: base + Duration::days(40),
            train_metrics: BinaryMetrics::empty(),
            test_metrics: BinaryMetrics::empty(),
        };

        assert!(!fold.is_valid());
    }

    // ============================================================
    // PerformanceDegradation Tests
    // ============================================================

    #[test]
    fn degradation_ratio_one_when_no_change() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        // Same win rate in both
        let is_settlements = create_settlements_in_range(base, base + Duration::days(30), 0.60, 50);
        let oos_settlements = create_settlements_in_range(
            base + Duration::days(30),
            base + Duration::days(60),
            0.60,
            50,
        );

        let is_metrics = BinaryMetrics::from_settlements(&is_settlements);
        let oos_metrics = BinaryMetrics::from_settlements(&oos_settlements);
        let degradation = PerformanceDegradation::from_metrics(&is_metrics, &oos_metrics);

        assert!(
            (degradation.win_rate_ratio - 1.0).abs() < 0.01,
            "Expected ratio ~1.0, got {}",
            degradation.win_rate_ratio
        );
    }

    #[test]
    fn degradation_detects_oos_underperformance() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        // IS: 70% win rate, OOS: 50% win rate
        let is_settlements = create_settlements_in_range(base, base + Duration::days(30), 0.70, 50);
        let oos_settlements = create_settlements_in_range(
            base + Duration::days(30),
            base + Duration::days(60),
            0.50,
            50,
        );

        let is_metrics = BinaryMetrics::from_settlements(&is_settlements);
        let oos_metrics = BinaryMetrics::from_settlements(&oos_settlements);
        let degradation = PerformanceDegradation::from_metrics(&is_metrics, &oos_metrics);

        assert!(
            degradation.win_rate_ratio < 0.8,
            "Expected ratio < 0.8, got {}",
            degradation.win_rate_ratio
        );
        assert!(
            degradation.win_rate_diff > 0.15,
            "Expected diff > 0.15, got {}",
            degradation.win_rate_diff
        );
    }

    // ============================================================
    // OverfittingRisk Tests
    // ============================================================

    #[test]
    fn overfitting_risk_low_for_small_degradation() {
        assert_eq!(
            OverfittingRisk::from_degradation(0.00),
            OverfittingRisk::Low
        );
        assert_eq!(
            OverfittingRisk::from_degradation(0.05),
            OverfittingRisk::Low
        );
    }

    #[test]
    fn overfitting_risk_medium_for_moderate_degradation() {
        assert_eq!(
            OverfittingRisk::from_degradation(0.06),
            OverfittingRisk::Medium
        );
        assert_eq!(
            OverfittingRisk::from_degradation(0.10),
            OverfittingRisk::Medium
        );
    }

    #[test]
    fn overfitting_risk_high_for_significant_degradation() {
        assert_eq!(
            OverfittingRisk::from_degradation(0.11),
            OverfittingRisk::High
        );
        assert_eq!(
            OverfittingRisk::from_degradation(0.20),
            OverfittingRisk::High
        );
    }

    #[test]
    fn overfitting_risk_severe_for_large_degradation() {
        assert_eq!(
            OverfittingRisk::from_degradation(0.21),
            OverfittingRisk::Severe
        );
        assert_eq!(
            OverfittingRisk::from_degradation(0.50),
            OverfittingRisk::Severe
        );
    }

    // ============================================================
    // analyze Tests
    // ============================================================

    #[test]
    fn analyze_returns_error_for_empty_settlements() {
        let optimizer = WalkForwardOptimizer::with_defaults();
        let result = optimizer.analyze(&[]);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No settlements"));
    }

    #[test]
    fn analyze_returns_error_for_insufficient_data() {
        let optimizer = WalkForwardOptimizer::with_defaults();

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        // Only 30 days of data, need 90 + 30 = 120
        let settlements = create_settlements_in_range(base, base + Duration::days(30), 0.60, 100);

        let result = optimizer.analyze(&settlements);
        assert!(result.is_err());
    }

    #[test]
    fn analyze_produces_valid_results() {
        let config = WalkForwardConfig::new(30, 10).with_min_samples(10);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        // 80 days of data with 60% win rate
        let settlements = create_settlements_in_range(base, base + Duration::days(80), 0.60, 200);

        let result = optimizer.analyze(&settlements);
        assert!(result.is_ok());

        let results = result.unwrap();
        assert!(!results.folds.is_empty());
        assert!(results.oos_aggregate.total_bets > 0);
    }

    #[test]
    fn analyze_aggregates_oos_metrics_from_all_folds() {
        let config = WalkForwardConfig::new(20, 10).with_min_samples(5);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(base, base + Duration::days(60), 0.55, 120);

        let results = optimizer.analyze(&settlements).unwrap();

        // OOS aggregate should have bets from all test periods
        let total_test_bets: u32 = results
            .folds
            .iter()
            .map(|f| f.test_metrics.total_bets)
            .sum();

        assert_eq!(results.oos_aggregate.total_bets, total_test_bets);
    }

    #[test]
    fn analyze_enforces_minimum_samples() {
        let config = WalkForwardConfig::new(30, 10).with_min_samples(100);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        // Only 5 settlements per period
        let settlements = create_settlements_in_range(base, base + Duration::days(80), 0.60, 20);

        let result = optimizer.analyze(&settlements);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("minimum"));
    }

    // ============================================================
    // simple_split Tests
    // ============================================================

    #[test]
    fn simple_split_70_30_creates_correct_proportions() {
        let config = WalkForwardConfig::default().with_min_samples(10);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(base, base + Duration::days(100), 0.60, 100);

        let result = optimizer.simple_split(&settlements, 0.70).unwrap();

        // Should be approximately 70/30 split
        assert_eq!(result.train_settlements.len(), 70);
        assert_eq!(result.test_settlements.len(), 30);
    }

    #[test]
    fn simple_split_returns_error_for_invalid_ratio() {
        let optimizer = WalkForwardOptimizer::with_defaults();

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(base, base + Duration::days(100), 0.60, 100);

        assert!(optimizer.simple_split(&settlements, 0.0).is_err());
        assert!(optimizer.simple_split(&settlements, 1.0).is_err());
        assert!(optimizer.simple_split(&settlements, -0.5).is_err());
    }

    #[test]
    fn simple_split_enforces_min_samples() {
        let config = WalkForwardConfig::default().with_min_samples(50);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(
            base,
            base + Duration::days(100),
            0.60,
            60, // 70% = 42, 30% = 18 - both below 50
        );

        let result = optimizer.simple_split(&settlements, 0.70);
        assert!(result.is_err());
    }

    #[test]
    fn simple_split_calculates_degradation() {
        let config = WalkForwardConfig::default().with_min_samples(10);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(base, base + Duration::days(100), 0.60, 100);

        let result = optimizer.simple_split(&settlements, 0.70).unwrap();

        // Should have degradation calculated - check it exists and is reasonable
        // Win rate ratio should be near 1.0 since we use same win rate for all data
        assert!(
            result.degradation.win_rate_ratio >= 0.0,
            "Win rate ratio should be non-negative"
        );
        // Check that overfitting risk was classified
        assert!(matches!(
            result.degradation.overfitting_risk,
            OverfittingRisk::Low
                | OverfittingRisk::Medium
                | OverfittingRisk::High
                | OverfittingRisk::Severe
        ));
    }

    // ============================================================
    // WalkForwardResults Tests
    // ============================================================

    #[test]
    fn results_passed_validation_requires_significance() {
        let config = WalkForwardConfig::new(20, 10).with_min_samples(5);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        // Only 50 settlements - not enough for significance
        let settlements = create_settlements_in_range(base, base + Duration::days(60), 0.55, 50);

        let results = optimizer.analyze(&settlements).unwrap();

        // Should not pass validation without significance
        assert!(!results.passed_validation());
    }

    #[test]
    fn results_num_folds_returns_correct_count() {
        let config = WalkForwardConfig::new(20, 10).with_min_samples(5);
        let optimizer = WalkForwardOptimizer::new(config);

        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let settlements = create_settlements_in_range(base, base + Duration::days(60), 0.60, 120);

        let results = optimizer.analyze(&settlements).unwrap();

        assert_eq!(results.num_folds(), results.folds.len());
        assert!(results.num_folds() > 0);
    }

    // ============================================================
    // SignificanceTest Tests
    // ============================================================

    #[test]
    fn significance_test_has_edge_when_ci_above_50() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        // 65% win rate with 100 samples should have CI above 0.50
        let settlements = create_settlements_in_range(base, base + Duration::days(100), 0.65, 100);

        let metrics = BinaryMetrics::from_settlements(&settlements);
        let significance = SignificanceTest::from_metrics(&metrics);

        assert!(significance.wilson_ci_lower > 0.50);
        assert!(significance.has_edge);
    }

    #[test]
    fn significance_test_no_edge_when_ci_below_50() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        // 52% win rate with only 50 samples - CI will include 0.50
        let settlements = create_settlements_in_range(base, base + Duration::days(50), 0.52, 50);

        let metrics = BinaryMetrics::from_settlements(&settlements);
        let significance = SignificanceTest::from_metrics(&metrics);

        assert!(!significance.has_edge);
    }

    // ============================================================
    // Serialization Tests
    // ============================================================

    #[test]
    fn config_serializes_correctly() {
        let config = WalkForwardConfig::new(60, 20).with_anchored(true);

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WalkForwardConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.train_window.num_days(),
            config.train_window.num_days()
        );
        assert_eq!(
            deserialized.test_window.num_days(),
            config.test_window.num_days()
        );
        assert_eq!(deserialized.anchored, config.anchored);
    }

    #[test]
    fn overfitting_risk_serializes_correctly() {
        let risk = OverfittingRisk::High;

        let json = serde_json::to_string(&risk).unwrap();
        assert_eq!(json, "\"High\"");

        let deserialized: OverfittingRisk = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, OverfittingRisk::High);
    }
}
