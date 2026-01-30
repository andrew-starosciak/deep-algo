//! Information Coefficient (IC) analysis for signal validation.
//!
//! The Information Coefficient is the Spearman rank correlation between
//! signal values and forward returns. It measures how well the signal
//! ranks outcomes, regardless of the exact predicted values.

use algo_trade_data::models::SignalSnapshotRecord;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Result of Information Coefficient analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICAnalysis {
    /// Name of the signal analyzed
    pub signal_name: String,
    /// Information Coefficient (Spearman rank correlation)
    pub ic: f64,
    /// T-statistic for the IC
    pub ic_t_stat: f64,
    /// P-value for the IC (two-tailed)
    pub ic_p_value: f64,
    /// Number of samples used
    pub sample_size: usize,
}

impl ICAnalysis {
    /// Returns true if the IC is statistically significant at alpha = 0.05.
    #[must_use]
    pub fn is_significant(&self) -> bool {
        self.ic_p_value < 0.05
    }

    /// Returns true if the IC indicates positive predictive power.
    #[must_use]
    pub fn has_predictive_power(&self) -> bool {
        self.ic > 0.0 && self.is_significant()
    }
}

/// Calculates ranks for a slice of values, handling ties with average rank.
///
/// # Arguments
/// * `values` - Slice of values to rank
///
/// # Returns
/// Vector of ranks (1-based, with ties averaged)
pub fn calculate_ranks(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return vec![];
    }

    let n = values.len();
    let mut indexed: Vec<(usize, f64)> = values.iter().cloned().enumerate().collect();

    // Sort by value, keeping original indices
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut ranks = vec![0.0; n];

    // Assign ranks, handling ties
    let mut i = 0;
    while i < n {
        // Find the extent of ties
        let mut j = i + 1;
        while j < n && (indexed[j].1 - indexed[i].1).abs() < f64::EPSILON {
            j += 1;
        }

        // Average rank for ties
        // Ranks are 1-based: positions i..j map to ranks (i+1)..(j+1)
        let avg_rank = (i + 1..j + 1).map(|r| r as f64).sum::<f64>() / (j - i) as f64;

        // Assign average rank to all tied values
        for k in i..j {
            ranks[indexed[k].0] = avg_rank;
        }

        i = j;
    }

    ranks
}

/// Calculates the Spearman rank correlation coefficient.
fn spearman_correlation(x: &[f64], y: &[f64]) -> f64 {
    if x.len() != y.len() || x.len() < 2 {
        return 0.0;
    }

    let ranks_x = calculate_ranks(x);
    let ranks_y = calculate_ranks(y);

    pearson_correlation(&ranks_x, &ranks_y)
}

/// Calculates Pearson correlation (used internally for rank correlation).
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

/// Calculates the t-statistic for a correlation coefficient.
fn correlation_t_stat(r: f64, n: usize) -> f64 {
    if n < 3 {
        return 0.0;
    }

    let r_clamped = r.clamp(-0.9999, 0.9999);
    let df = n as f64 - 2.0;
    r_clamped * (df / (1.0 - r_clamped * r_clamped)).sqrt()
}

/// Calculates the p-value from a t-statistic.
fn t_stat_to_p_value(t: f64, df: f64) -> f64 {
    // Two-tailed p-value using normal approximation for large df
    if df > 30.0 {
        2.0 * (1.0 - standard_normal_cdf(t.abs()))
    } else {
        // For small df, use a rough approximation
        2.0 * (1.0 - t_cdf_approx(t.abs(), df))
    }
}

/// Rough approximation of t-distribution CDF.
fn t_cdf_approx(t: f64, df: f64) -> f64 {
    // For df > 30, t-distribution is close to normal
    // For smaller df, we use a simple scaling
    let scale = (df / (df - 2.0)).sqrt().min(2.0);
    standard_normal_cdf(t / scale)
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

/// Calculates the Information Coefficient for a signal.
///
/// The IC is the Spearman rank correlation between signal values
/// and forward returns.
///
/// # Arguments
/// * `snapshots` - Signal snapshot records with forward returns
///
/// # Returns
/// An `ICAnalysis` result
///
/// # Errors
/// Returns error if insufficient validated data
pub fn calculate_ic(snapshots: &[SignalSnapshotRecord]) -> Result<ICAnalysis> {
    // Filter to validated snapshots
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
        return Err(anyhow!(
            "Mismatched or insufficient data for IC calculation"
        ));
    }

    let ic = spearman_correlation(&signal_values, &returns);
    let n = signal_values.len();
    let ic_t_stat = correlation_t_stat(ic, n);
    let df = n as f64 - 2.0;
    let ic_p_value = t_stat_to_p_value(ic_t_stat, df).clamp(0.0, 1.0);

    Ok(ICAnalysis {
        signal_name,
        ic,
        ic_t_stat,
        ic_p_value,
        sample_size: n,
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

    // ============================================
    // calculate_ranks Tests
    // ============================================

    #[test]
    fn ranks_calculated_correctly() {
        let values = vec![3.0, 1.0, 4.0, 1.0, 5.0];
        let ranks = calculate_ranks(&values);

        // 1.0 appears twice at positions 1 and 3 (0-indexed), ranks 1 and 2
        // Average rank = 1.5 for both
        // 3.0 at position 0, rank 3
        // 4.0 at position 2, rank 4
        // 5.0 at position 4, rank 5
        assert!(
            (ranks[0] - 3.0).abs() < f64::EPSILON,
            "rank[0]={}",
            ranks[0]
        );
        assert!(
            (ranks[1] - 1.5).abs() < f64::EPSILON,
            "rank[1]={}",
            ranks[1]
        );
        assert!(
            (ranks[2] - 4.0).abs() < f64::EPSILON,
            "rank[2]={}",
            ranks[2]
        );
        assert!(
            (ranks[3] - 1.5).abs() < f64::EPSILON,
            "rank[3]={}",
            ranks[3]
        );
        assert!(
            (ranks[4] - 5.0).abs() < f64::EPSILON,
            "rank[4]={}",
            ranks[4]
        );
    }

    #[test]
    fn ranks_handles_ties() {
        let values = vec![1.0, 1.0, 1.0, 4.0, 5.0];
        let ranks = calculate_ranks(&values);

        // First three are tied at value 1.0, should get average of ranks 1,2,3 = 2.0
        assert!((ranks[0] - 2.0).abs() < f64::EPSILON);
        assert!((ranks[1] - 2.0).abs() < f64::EPSILON);
        assert!((ranks[2] - 2.0).abs() < f64::EPSILON);
        assert!((ranks[3] - 4.0).abs() < f64::EPSILON);
        assert!((ranks[4] - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ranks_empty_returns_empty() {
        let ranks = calculate_ranks(&[]);
        assert!(ranks.is_empty());
    }

    #[test]
    fn ranks_single_element() {
        let ranks = calculate_ranks(&[42.0]);
        assert_eq!(ranks.len(), 1);
        assert!((ranks[0] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ranks_already_sorted() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let ranks = calculate_ranks(&values);

        for (i, rank) in ranks.iter().enumerate() {
            assert!(
                (rank - (i + 1) as f64).abs() < f64::EPSILON,
                "rank[{}]={}, expected {}",
                i,
                rank,
                i + 1
            );
        }
    }

    #[test]
    fn ranks_reverse_sorted() {
        let values = vec![5.0, 4.0, 3.0, 2.0, 1.0];
        let ranks = calculate_ranks(&values);

        assert!((ranks[0] - 5.0).abs() < f64::EPSILON);
        assert!((ranks[1] - 4.0).abs() < f64::EPSILON);
        assert!((ranks[2] - 3.0).abs() < f64::EPSILON);
        assert!((ranks[3] - 2.0).abs() < f64::EPSILON);
        assert!((ranks[4] - 1.0).abs() < f64::EPSILON);
    }

    // ============================================
    // IC Calculation Tests
    // ============================================

    #[test]
    fn ic_perfect_rank_correlation() {
        // Perfect monotonic relationship
        let snapshots = vec![
            create_snapshot(SignalDirection::Down, 0.9, -0.03),
            create_snapshot(SignalDirection::Down, 0.6, -0.02),
            create_snapshot(SignalDirection::Down, 0.3, -0.01),
            create_snapshot(SignalDirection::Up, 0.3, 0.01),
            create_snapshot(SignalDirection::Up, 0.6, 0.02),
            create_snapshot(SignalDirection::Up, 0.9, 0.03),
        ];

        let result = calculate_ic(&snapshots).unwrap();

        assert!(result.ic > 0.95, "IC was {}", result.ic);
        assert!(result.is_significant(), "p-value was {}", result.ic_p_value);
    }

    #[test]
    fn ic_perfect_negative_rank() {
        // Perfect inverse monotonic relationship
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.9, -0.03),
            create_snapshot(SignalDirection::Up, 0.6, -0.02),
            create_snapshot(SignalDirection::Up, 0.3, -0.01),
            create_snapshot(SignalDirection::Down, 0.3, 0.01),
            create_snapshot(SignalDirection::Down, 0.6, 0.02),
            create_snapshot(SignalDirection::Down, 0.9, 0.03),
        ];

        let result = calculate_ic(&snapshots).unwrap();

        assert!(result.ic < -0.95, "IC was {}", result.ic);
    }

    #[test]
    fn ic_handles_ties_correctly() {
        // Some tied values
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.5, 0.01),
            create_snapshot(SignalDirection::Up, 0.5, 0.02), // Same signal, different return
            create_snapshot(SignalDirection::Down, 0.5, -0.01),
            create_snapshot(SignalDirection::Down, 0.5, -0.02), // Same signal, different return
        ];

        let result = calculate_ic(&snapshots).unwrap();

        // Should compute without error
        assert!(result.ic.is_finite(), "IC should be finite");
    }

    #[test]
    fn ic_analysis_has_predictive_power() {
        let analysis = ICAnalysis {
            signal_name: "test".to_string(),
            ic: 0.3,
            ic_t_stat: 3.0,
            ic_p_value: 0.01,
            sample_size: 100,
        };

        assert!(analysis.is_significant());
        assert!(analysis.has_predictive_power());
    }

    #[test]
    fn ic_analysis_no_predictive_power_negative_ic() {
        let analysis = ICAnalysis {
            signal_name: "test".to_string(),
            ic: -0.3,
            ic_t_stat: -3.0,
            ic_p_value: 0.01,
            sample_size: 100,
        };

        assert!(analysis.is_significant());
        assert!(!analysis.has_predictive_power()); // Negative IC = contrarian
    }

    #[test]
    fn calculate_ic_insufficient_data() {
        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.02),
            create_snapshot(SignalDirection::Down, 0.6, -0.01),
        ];

        let result = calculate_ic(&snapshots);

        assert!(result.is_err());
    }
}
