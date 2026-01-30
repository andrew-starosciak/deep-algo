//! Validation report generation.
//!
//! Combines all validation metrics into a comprehensive report
//! with actionable recommendations.

use super::{
    analyze_signal_correlation, calculate_ic, test_directional_accuracy, test_return_significance,
    CorrelationAnalysis, ICAnalysis, Recommendation, SignificanceTest,
};
use algo_trade_data::models::SignalSnapshotRecord;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Comprehensive validation report for a signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Name of the signal
    pub signal_name: String,
    /// Validation period
    pub period: (DateTime<Utc>, DateTime<Utc>),
    /// Number of samples used
    pub sample_size: usize,
    /// Correlation analysis results
    pub correlation: CorrelationAnalysis,
    /// Information Coefficient analysis
    pub ic: ICAnalysis,
    /// Directional accuracy test
    pub directional_accuracy: SignificanceTest,
    /// Return significance test
    pub return_significance: SignificanceTest,
    /// Overall recommendation
    pub recommendation: Recommendation,
}

impl ValidationReport {
    /// Generates a comprehensive validation report for a signal.
    ///
    /// # Arguments
    /// * `signal_name` - Name of the signal being validated
    /// * `snapshots` - Signal snapshot records with forward returns
    /// * `min_samples` - Minimum samples required for valid inference
    ///
    /// # Returns
    /// A complete `ValidationReport`
    ///
    /// # Errors
    /// Returns error if validation cannot be performed
    pub fn generate(
        signal_name: &str,
        snapshots: &[SignalSnapshotRecord],
        min_samples: usize,
    ) -> Result<Self> {
        // Filter to validated snapshots
        let validated: Vec<_> = snapshots
            .iter()
            .filter(|s| s.forward_return_15m.is_some())
            .cloned()
            .collect();

        let sample_size = validated.len();

        // Determine period from data
        let period = if validated.is_empty() {
            let now = Utc::now();
            (now, now)
        } else {
            let start = validated
                .iter()
                .map(|s| s.timestamp)
                .min()
                .unwrap_or_else(Utc::now);
            let end = validated
                .iter()
                .map(|s| s.timestamp)
                .max()
                .unwrap_or_else(Utc::now);
            (start, end)
        };

        // Run all analyses
        let correlation = analyze_signal_correlation(&validated)?;
        let ic = calculate_ic(&validated)?;
        let directional_accuracy = test_directional_accuracy(&validated)?;
        let return_significance = test_return_significance(&validated)?;

        // Determine recommendation
        let recommendation = determine_recommendation(
            sample_size,
            min_samples,
            directional_accuracy.p_value,
            return_significance.p_value,
        );

        Ok(Self {
            signal_name: signal_name.to_string(),
            period,
            sample_size,
            correlation,
            ic,
            directional_accuracy,
            return_significance,
            recommendation,
        })
    }

    /// Converts the report to a human-readable text format.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "=== Signal Validation Report: {} ===\n\n",
            self.signal_name
        ));

        output.push_str(&format!(
            "Period: {} to {}\n",
            self.period.0.format("%Y-%m-%d %H:%M"),
            self.period.1.format("%Y-%m-%d %H:%M")
        ));
        output.push_str(&format!("Sample Size: {}\n\n", self.sample_size));

        // Correlation
        output.push_str("--- Correlation Analysis ---\n");
        output.push_str(&format!(
            "Pearson Correlation: {:.4}\n",
            self.correlation.correlation
        ));
        output.push_str(&format!("P-value: {:.4}\n", self.correlation.p_value));
        output.push_str(&format!(
            "Significant (p<0.05): {}\n\n",
            if self.correlation.is_significant() {
                "Yes"
            } else {
                "No"
            }
        ));

        // IC
        output.push_str("--- Information Coefficient ---\n");
        output.push_str(&format!("IC (Spearman): {:.4}\n", self.ic.ic));
        output.push_str(&format!("T-statistic: {:.4}\n", self.ic.ic_t_stat));
        output.push_str(&format!("P-value: {:.4}\n", self.ic.ic_p_value));
        output.push_str(&format!(
            "Predictive Power: {}\n\n",
            if self.ic.has_predictive_power() {
                "Yes"
            } else {
                "No"
            }
        ));

        // Directional Accuracy
        output.push_str("--- Directional Accuracy (Binomial Test) ---\n");
        output.push_str(&format!(
            "Win Rate: {:.1}%\n",
            (self.directional_accuracy.confidence_interval.0
                + self.directional_accuracy.confidence_interval.1)
                / 2.0
                * 100.0
        ));
        output.push_str(&format!(
            "95% CI: [{:.1}%, {:.1}%]\n",
            self.directional_accuracy.confidence_interval.0 * 100.0,
            self.directional_accuracy.confidence_interval.1 * 100.0
        ));
        output.push_str(&format!(
            "P-value: {:.4}\n",
            self.directional_accuracy.p_value
        ));
        output.push_str(&format!(
            "Significant (p<0.05): {}\n",
            if self.directional_accuracy.is_significant_05 {
                "Yes"
            } else {
                "No"
            }
        ));
        output.push_str(&format!(
            "Positive Edge: {}\n\n",
            if self.directional_accuracy.has_positive_edge() {
                "Yes"
            } else {
                "No"
            }
        ));

        // Return Significance
        output.push_str("--- Return Significance (T-Test) ---\n");
        output.push_str(&format!(
            "T-statistic: {:.4}\n",
            self.return_significance.statistic
        ));
        output.push_str(&format!(
            "95% CI: [{:.6}, {:.6}]\n",
            self.return_significance.confidence_interval.0,
            self.return_significance.confidence_interval.1
        ));
        output.push_str(&format!(
            "P-value: {:.4}\n",
            self.return_significance.p_value
        ));
        output.push_str(&format!(
            "Significant (p<0.05): {}\n\n",
            if self.return_significance.is_significant_05 {
                "Yes"
            } else {
                "No"
            }
        ));

        // Recommendation
        output.push_str("=== RECOMMENDATION ===\n");
        output.push_str(&format!(
            "{:?}: {}\n",
            self.recommendation,
            self.recommendation.description()
        ));

        output
    }

    /// Converts the report to JSON format.
    ///
    /// # Errors
    /// Returns error if serialization fails
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Determines the recommendation based on validation metrics.
///
/// # Arguments
/// * `sample_size` - Number of samples in validation
/// * `min_samples` - Minimum required samples
/// * `directional_p_value` - P-value from directional accuracy test
/// * `return_p_value` - P-value from return significance test
///
/// # Returns
/// A `Recommendation` based on the metrics
#[must_use]
pub fn determine_recommendation(
    sample_size: usize,
    min_samples: usize,
    directional_p_value: f64,
    return_p_value: f64,
) -> Recommendation {
    // Check sample size first
    if sample_size < min_samples {
        return Recommendation::NeedsMoreData;
    }

    // Check if either test is significant at 0.05
    let significant_05 = directional_p_value < 0.05 || return_p_value < 0.05;

    // Check if either test is significant at 0.10
    let significant_10 = directional_p_value < 0.10 || return_p_value < 0.10;

    if significant_05 {
        Recommendation::Approved
    } else if significant_10 {
        Recommendation::ConditionalApproval
    } else {
        Recommendation::Rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_data::models::SignalDirection;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn create_snapshot(
        direction: SignalDirection,
        strength: f64,
        forward_return: f64,
        timestamp: DateTime<Utc>,
    ) -> SignalSnapshotRecord {
        let mut record = SignalSnapshotRecord::new(
            timestamp,
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

    fn create_good_signal_data() -> Vec<SignalSnapshotRecord> {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut snapshots = Vec::new();

        // Create data with ~65% accuracy
        for i in 0..100 {
            let time = base_time + chrono::Duration::minutes(i * 15);
            if i % 100 < 65 {
                // Correct predictions
                if i % 2 == 0 {
                    snapshots.push(create_snapshot(SignalDirection::Up, 0.8, 0.005, time));
                } else {
                    snapshots.push(create_snapshot(SignalDirection::Down, 0.8, -0.005, time));
                }
            } else {
                // Incorrect predictions
                if i % 2 == 0 {
                    snapshots.push(create_snapshot(SignalDirection::Up, 0.8, -0.003, time));
                } else {
                    snapshots.push(create_snapshot(SignalDirection::Down, 0.8, 0.003, time));
                }
            }
        }

        snapshots
    }

    // ============================================
    // determine_recommendation Tests
    // ============================================

    #[test]
    fn recommendation_approved_when_p_below_05() {
        let rec = determine_recommendation(200, 100, 0.03, 0.12);
        assert_eq!(rec, Recommendation::Approved);
    }

    #[test]
    fn recommendation_conditional_when_p_below_10() {
        let rec = determine_recommendation(200, 100, 0.08, 0.15);
        assert_eq!(rec, Recommendation::ConditionalApproval);
    }

    #[test]
    fn recommendation_needs_data_when_low_samples() {
        let rec = determine_recommendation(50, 100, 0.01, 0.01);
        assert_eq!(rec, Recommendation::NeedsMoreData);
    }

    #[test]
    fn recommendation_rejected_when_no_edge() {
        let rec = determine_recommendation(200, 100, 0.5, 0.6);
        assert_eq!(rec, Recommendation::Rejected);
    }

    #[test]
    fn recommendation_approved_from_return_significance() {
        // Directional not significant, but return is
        let rec = determine_recommendation(200, 100, 0.15, 0.03);
        assert_eq!(rec, Recommendation::Approved);
    }

    // ============================================
    // ValidationReport Tests
    // ============================================

    #[test]
    fn report_generates_all_sections() {
        let snapshots = create_good_signal_data();

        let report = ValidationReport::generate("test_signal", &snapshots, 50).unwrap();

        assert_eq!(report.signal_name, "test_signal");
        assert_eq!(report.sample_size, 100);
        assert!(!report.correlation.signal_name.is_empty());
        assert!(!report.ic.signal_name.is_empty());
        assert!(!report.directional_accuracy.signal_name.is_empty());
        assert!(!report.return_significance.signal_name.is_empty());
    }

    #[test]
    fn to_json_parses_correctly() {
        let snapshots = create_good_signal_data();
        let report = ValidationReport::generate("test_signal", &snapshots, 50).unwrap();

        let json = report.to_json().unwrap();

        // Should be valid JSON
        let parsed: ValidationReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.signal_name, "test_signal");
        assert_eq!(parsed.sample_size, 100);
    }

    #[test]
    fn to_text_includes_all_metrics() {
        let snapshots = create_good_signal_data();
        let report = ValidationReport::generate("test_signal", &snapshots, 50).unwrap();

        let text = report.to_text();

        // Check that all sections are present
        assert!(text.contains("Signal Validation Report"));
        assert!(text.contains("test_signal"));
        assert!(text.contains("Correlation Analysis"));
        assert!(text.contains("Information Coefficient"));
        assert!(text.contains("Directional Accuracy"));
        assert!(text.contains("Return Significance"));
        assert!(text.contains("RECOMMENDATION"));
        assert!(text.contains("Sample Size: 100"));
    }

    #[test]
    fn report_determines_correct_period() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 28, 0, 0, 0).unwrap();

        let snapshots = vec![
            create_snapshot(SignalDirection::Up, 0.8, 0.01, start),
            create_snapshot(
                SignalDirection::Up,
                0.8,
                0.01,
                start + chrono::Duration::days(14),
            ),
            create_snapshot(SignalDirection::Down, 0.8, -0.01, end),
        ];

        let report = ValidationReport::generate("test_signal", &snapshots, 3).unwrap();

        assert_eq!(report.period.0, start);
        assert_eq!(report.period.1, end);
    }

    #[test]
    fn report_insufficient_data_returns_error() {
        let snapshots = vec![create_snapshot(SignalDirection::Up, 0.8, 0.01, Utc::now())];

        let result = ValidationReport::generate("test_signal", &snapshots, 50);

        assert!(result.is_err());
    }
}
