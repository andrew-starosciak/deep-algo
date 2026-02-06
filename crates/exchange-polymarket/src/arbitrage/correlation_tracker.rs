//! Dynamic correlation tracker for cross-market arbitrage.
//!
//! Replaces the static `assumed_correlation: 0.85` with real-time
//! correlation estimation using Wilson Score confidence intervals.
//!
//! After each settlement, the tracker records whether the correlated
//! outcome was correct, building a rolling window of observations.
//! The conservative (lower CI bound) estimate is used for EV calculations
//! to protect against overestimating correlation.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use super::cross_market_types::CoinPair;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the correlation tracker.
#[derive(Debug, Clone)]
pub struct CorrelationTrackerConfig {
    /// Maximum number of observations per pair.
    pub max_observations: usize,
    /// Maximum age of observations before pruning.
    pub max_age: Duration,
    /// Minimum observations before using tracked estimate.
    pub min_observations: usize,
    /// Default correlation when insufficient data.
    pub default_correlation: f64,
    /// Z-score for confidence interval (1.96 = 95% CI).
    pub z_score: f64,
}

impl Default for CorrelationTrackerConfig {
    fn default() -> Self {
        Self {
            max_observations: 200,
            max_age: Duration::from_secs(24 * 3600), // 24 hours
            min_observations: 20,
            default_correlation: 0.85,
            z_score: 1.96, // 95% CI
        }
    }
}

// =============================================================================
// Observation
// =============================================================================

/// A single correlation observation from a settled trade.
#[derive(Debug, Clone)]
pub struct CorrelationObservation {
    /// When this observation was recorded.
    pub timestamp: DateTime<Utc>,
    /// Whether the correlated outcome was correct (both moved same direction).
    pub correlation_correct: bool,
    /// The coin pair observed.
    pub pair: CoinPair,
}

// =============================================================================
// Estimate
// =============================================================================

/// A correlation estimate with confidence interval.
#[derive(Debug, Clone)]
pub struct CorrelationEstimate {
    /// Point estimate (observed proportion).
    pub correlation: f64,
    /// Wilson CI lower bound.
    pub ci_lower: f64,
    /// Wilson CI upper bound.
    pub ci_upper: f64,
    /// Number of observations used.
    pub sample_size: usize,
    /// Whether this is the default (insufficient data).
    pub is_default: bool,
}

impl CorrelationEstimate {
    /// Returns the conservative (lower CI bound) estimate for risk-averse EV calculation.
    #[must_use]
    pub fn conservative(&self) -> f64 {
        self.ci_lower
    }
}

// =============================================================================
// Tracker
// =============================================================================

/// Tracks correlation between coin pairs using rolling observations.
///
/// Thread-safe via `RwLock` for concurrent read access from detector
/// and write access from settlement handler.
pub struct CorrelationTracker {
    config: CorrelationTrackerConfig,
    state: RwLock<HashMap<CoinPair, VecDeque<CorrelationObservation>>>,
}

impl CorrelationTracker {
    /// Creates a new correlation tracker with the given config.
    #[must_use]
    pub fn new(config: CorrelationTrackerConfig) -> Self {
        Self {
            config,
            state: RwLock::new(HashMap::new()),
        }
    }

    /// Records a correlation observation after settlement.
    pub fn record_observation(&self, pair: CoinPair, correlation_correct: bool) {
        let obs = CorrelationObservation {
            timestamp: Utc::now(),
            correlation_correct,
            pair,
        };

        let mut state = self.state.write();
        let observations = state.entry(normalize_pair(pair)).or_default();

        observations.push_back(obs);

        // Trim by max_observations
        while observations.len() > self.config.max_observations {
            observations.pop_front();
        }

        // Trim by max_age
        let cutoff =
            Utc::now() - chrono::Duration::from_std(self.config.max_age).unwrap_or_default();
        while observations.front().is_some_and(|o| o.timestamp < cutoff) {
            observations.pop_front();
        }
    }

    /// Gets the correlation estimate for a coin pair.
    ///
    /// Returns a default estimate if insufficient data is available.
    #[must_use]
    pub fn get_correlation(&self, pair: &CoinPair) -> CorrelationEstimate {
        let state = self.state.read();
        let normalized = normalize_pair(*pair);

        let observations = match state.get(&normalized) {
            Some(obs) if obs.len() >= self.config.min_observations => obs,
            _ => {
                return CorrelationEstimate {
                    correlation: self.config.default_correlation,
                    ci_lower: self.config.default_correlation,
                    ci_upper: self.config.default_correlation,
                    sample_size: 0,
                    is_default: true,
                };
            }
        };

        // Count wins (correlation was correct)
        let n = observations.len();
        let wins = observations
            .iter()
            .filter(|o| o.correlation_correct)
            .count();

        let p = wins as f64 / n as f64;
        let (ci_lower, ci_upper) = wilson_ci(wins, n, self.config.z_score);

        CorrelationEstimate {
            correlation: p,
            ci_lower,
            ci_upper,
            sample_size: n,
            is_default: false,
        }
    }

    /// Returns the conservative correlation for a pair (CI lower bound).
    ///
    /// Uses the lower bound of the Wilson CI for risk-averse EV calculations.
    /// Falls back to default if insufficient data.
    #[must_use]
    pub fn get_conservative_correlation(&self, pair: &CoinPair) -> f64 {
        self.get_correlation(pair).conservative()
    }

    /// Returns all tracked pair estimates (for monitoring/display).
    #[must_use]
    pub fn all_estimates(&self) -> Vec<(CoinPair, CorrelationEstimate)> {
        let state = self.state.read();
        state
            .keys()
            .map(|pair| {
                let estimate = self.get_correlation(pair);
                (*pair, estimate)
            })
            .collect()
    }

    /// Seeds the tracker with historical observations (e.g., from DB on startup).
    pub fn seed_from_history(&self, observations: Vec<CorrelationObservation>) {
        let mut state = self.state.write();
        for obs in observations {
            let pair = normalize_pair(obs.pair);
            let deque = state.entry(pair).or_default();
            deque.push_back(obs);
        }

        // Trim each pair
        for observations in state.values_mut() {
            while observations.len() > self.config.max_observations {
                observations.pop_front();
            }
        }
    }

    /// Returns the config for inspection.
    #[must_use]
    pub fn config(&self) -> &CorrelationTrackerConfig {
        &self.config
    }

    /// Returns total observation count across all pairs.
    #[must_use]
    pub fn total_observations(&self) -> usize {
        let state = self.state.read();
        state.values().map(|v| v.len()).sum()
    }
}

impl std::fmt::Debug for CorrelationTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.read();
        f.debug_struct("CorrelationTracker")
            .field("config", &self.config)
            .field("tracked_pairs", &state.len())
            .field(
                "total_observations",
                &state.values().map(|v| v.len()).sum::<usize>(),
            )
            .finish()
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Normalizes a pair to canonical order for consistent lookups.
///
/// Always orders alphabetically by the first coin's slug prefix.
fn normalize_pair(pair: CoinPair) -> CoinPair {
    let a_slug = pair.coin1.slug_prefix();
    let b_slug = pair.coin2.slug_prefix();
    if a_slug <= b_slug {
        pair
    } else {
        CoinPair {
            coin1: pair.coin2,
            coin2: pair.coin1,
        }
    }
}

/// Computes the Wilson Score confidence interval for a proportion.
///
/// # Arguments
/// * `wins` - Number of successes
/// * `n` - Total trials
/// * `z` - Z-score (1.96 for 95% CI)
///
/// # Returns
/// `(lower_bound, upper_bound)` of the confidence interval.
fn wilson_ci(wins: usize, n: usize, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }

    let p = wins as f64 / n as f64;
    let n_f = n as f64;
    let z2 = z * z;

    let denom = 1.0 + z2 / n_f;
    let center = p + z2 / (2.0 * n_f);
    let spread = z * (p * (1.0 - p) / n_f + z2 / (4.0 * n_f * n_f)).sqrt();

    let lower = ((center - spread) / denom).max(0.0);
    let upper = ((center + spread) / denom).min(1.0);

    (lower, upper)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Coin;

    fn btc_eth_pair() -> CoinPair {
        CoinPair {
            coin1: Coin::Btc,
            coin2: Coin::Eth,
        }
    }

    fn eth_btc_pair() -> CoinPair {
        CoinPair {
            coin1: Coin::Eth,
            coin2: Coin::Btc,
        }
    }

    // -------------------------------------------------------------------------
    // Default behavior
    // -------------------------------------------------------------------------

    #[test]
    fn returns_default_with_no_data() {
        let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());
        let estimate = tracker.get_correlation(&btc_eth_pair());

        assert!(estimate.is_default);
        assert_eq!(estimate.correlation, 0.85);
        assert_eq!(estimate.sample_size, 0);
    }

    #[test]
    fn returns_default_with_insufficient_data() {
        let config = CorrelationTrackerConfig {
            min_observations: 20,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        // Add 10 observations (below min of 20)
        for _ in 0..10 {
            tracker.record_observation(btc_eth_pair(), true);
        }

        let estimate = tracker.get_correlation(&btc_eth_pair());
        assert!(estimate.is_default);
    }

    // -------------------------------------------------------------------------
    // Estimation accuracy
    // -------------------------------------------------------------------------

    #[test]
    fn accurate_estimate_with_sufficient_data() {
        let config = CorrelationTrackerConfig {
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        // 80 correct out of 100
        for _ in 0..80 {
            tracker.record_observation(btc_eth_pair(), true);
        }
        for _ in 0..20 {
            tracker.record_observation(btc_eth_pair(), false);
        }

        let estimate = tracker.get_correlation(&btc_eth_pair());
        assert!(!estimate.is_default);
        assert_eq!(estimate.sample_size, 100);
        assert!((estimate.correlation - 0.80).abs() < 0.01);
        assert!(estimate.ci_lower < 0.80);
        assert!(estimate.ci_upper > 0.80);
    }

    #[test]
    fn ci_shrinks_with_more_data() {
        let config = CorrelationTrackerConfig {
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        // Add 10 observations (80% correct)
        for _ in 0..8 {
            tracker.record_observation(btc_eth_pair(), true);
        }
        for _ in 0..2 {
            tracker.record_observation(btc_eth_pair(), false);
        }

        let est_small = tracker.get_correlation(&btc_eth_pair());
        let width_small = est_small.ci_upper - est_small.ci_lower;

        // Add 90 more (still 80% correct)
        for _ in 0..72 {
            tracker.record_observation(btc_eth_pair(), true);
        }
        for _ in 0..18 {
            tracker.record_observation(btc_eth_pair(), false);
        }

        let est_large = tracker.get_correlation(&btc_eth_pair());
        let width_large = est_large.ci_upper - est_large.ci_lower;

        assert!(width_large < width_small, "CI should shrink with more data");
    }

    // -------------------------------------------------------------------------
    // Symmetric pair lookup
    // -------------------------------------------------------------------------

    #[test]
    fn symmetric_pair_lookup() {
        let config = CorrelationTrackerConfig {
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        // Record with BTC-ETH
        for _ in 0..10 {
            tracker.record_observation(btc_eth_pair(), true);
        }

        // Query with ETH-BTC should return same result
        let est_btc_eth = tracker.get_correlation(&btc_eth_pair());
        let est_eth_btc = tracker.get_correlation(&eth_btc_pair());

        assert_eq!(est_btc_eth.sample_size, est_eth_btc.sample_size);
        assert_eq!(est_btc_eth.correlation, est_eth_btc.correlation);
    }

    // -------------------------------------------------------------------------
    // Max observations trimming
    // -------------------------------------------------------------------------

    #[test]
    fn trims_to_max_observations() {
        let config = CorrelationTrackerConfig {
            max_observations: 10,
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        // Add 20 observations
        for _ in 0..20 {
            tracker.record_observation(btc_eth_pair(), true);
        }

        let estimate = tracker.get_correlation(&btc_eth_pair());
        assert_eq!(estimate.sample_size, 10);
    }

    // -------------------------------------------------------------------------
    // Conservative estimate
    // -------------------------------------------------------------------------

    #[test]
    fn conservative_returns_ci_lower() {
        let config = CorrelationTrackerConfig {
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        for _ in 0..80 {
            tracker.record_observation(btc_eth_pair(), true);
        }
        for _ in 0..20 {
            tracker.record_observation(btc_eth_pair(), false);
        }

        let estimate = tracker.get_correlation(&btc_eth_pair());
        let conservative = tracker.get_conservative_correlation(&btc_eth_pair());

        assert_eq!(conservative, estimate.ci_lower);
        assert!(conservative < estimate.correlation);
    }

    // -------------------------------------------------------------------------
    // Seeding from history
    // -------------------------------------------------------------------------

    #[test]
    fn seed_from_history() {
        let config = CorrelationTrackerConfig {
            min_observations: 5,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        let observations: Vec<CorrelationObservation> = (0..10)
            .map(|_| CorrelationObservation {
                timestamp: Utc::now(),
                correlation_correct: true,
                pair: btc_eth_pair(),
            })
            .collect();

        tracker.seed_from_history(observations);

        let estimate = tracker.get_correlation(&btc_eth_pair());
        assert!(!estimate.is_default);
        assert_eq!(estimate.sample_size, 10);
    }

    // -------------------------------------------------------------------------
    // Wilson CI
    // -------------------------------------------------------------------------

    #[test]
    fn wilson_ci_perfect_record() {
        let (lower, upper) = wilson_ci(100, 100, 1.96);
        assert!(lower > 0.95);
        assert!((upper - 1.0).abs() < 0.01);
    }

    #[test]
    fn wilson_ci_zero_wins() {
        let (lower, upper) = wilson_ci(0, 100, 1.96);
        assert!(lower < 0.01);
        assert!(upper < 0.10);
    }

    #[test]
    fn wilson_ci_fifty_fifty() {
        let (lower, upper) = wilson_ci(50, 100, 1.96);
        assert!(lower > 0.35);
        assert!(upper < 0.65);
        assert!((lower + upper) / 2.0 - 0.5 < 0.05);
    }

    #[test]
    fn wilson_ci_zero_n() {
        let (lower, upper) = wilson_ci(0, 0, 1.96);
        assert_eq!(lower, 0.0);
        assert_eq!(upper, 1.0);
    }

    // -------------------------------------------------------------------------
    // All estimates
    // -------------------------------------------------------------------------

    #[test]
    fn all_estimates_returns_tracked_pairs() {
        let config = CorrelationTrackerConfig {
            min_observations: 1,
            ..Default::default()
        };
        let tracker = CorrelationTracker::new(config);

        tracker.record_observation(btc_eth_pair(), true);
        tracker.record_observation(
            CoinPair {
                coin1: Coin::Btc,
                coin2: Coin::Sol,
            },
            false,
        );

        let estimates = tracker.all_estimates();
        assert_eq!(estimates.len(), 2);
    }

    // -------------------------------------------------------------------------
    // Total observations
    // -------------------------------------------------------------------------

    #[test]
    fn total_observations_counts_all() {
        let tracker = CorrelationTracker::new(CorrelationTrackerConfig::default());

        tracker.record_observation(btc_eth_pair(), true);
        tracker.record_observation(btc_eth_pair(), false);
        tracker.record_observation(
            CoinPair {
                coin1: Coin::Sol,
                coin2: Coin::Xrp,
            },
            true,
        );

        assert_eq!(tracker.total_observations(), 3);
    }

    // -------------------------------------------------------------------------
    // Normalize pair
    // -------------------------------------------------------------------------

    #[test]
    fn normalize_pair_is_stable() {
        let p1 = normalize_pair(btc_eth_pair());
        let p2 = normalize_pair(eth_btc_pair());
        assert_eq!(p1.coin1.slug_prefix(), p2.coin1.slug_prefix());
        assert_eq!(p1.coin2.slug_prefix(), p2.coin2.slug_prefix());
    }
}
