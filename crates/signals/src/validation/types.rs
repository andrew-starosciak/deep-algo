//! Core types for signal validation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Result of statistical validation for a signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Name of the signal being validated
    pub signal_name: String,
    /// Start of the validation period
    pub period_start: DateTime<Utc>,
    /// End of the validation period
    pub period_end: DateTime<Utc>,
    /// Number of samples used in validation
    pub sample_size: usize,
}

impl ValidationResult {
    /// Creates a new validation result.
    #[must_use]
    pub fn new(
        signal_name: impl Into<String>,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        sample_size: usize,
    ) -> Self {
        Self {
            signal_name: signal_name.into(),
            period_start,
            period_end,
            sample_size,
        }
    }
}

/// Recommendation based on validation results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recommendation {
    /// Signal approved for production use (p < 0.05)
    Approved,
    /// Signal conditionally approved, needs monitoring (0.05 <= p < 0.10)
    ConditionalApproval,
    /// Insufficient data for reliable conclusion
    NeedsMoreData,
    /// Signal rejected - no significant edge detected
    Rejected,
}

impl Recommendation {
    /// Returns a human-readable description of the recommendation.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Approved => "Approved for production use (p < 0.05)",
            Self::ConditionalApproval => "Conditionally approved - monitor closely (p < 0.10)",
            Self::NeedsMoreData => "Insufficient data - collect more samples",
            Self::Rejected => "Rejected - no significant edge detected",
        }
    }

    /// Returns true if the signal can be used (Approved or ConditionalApproval).
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        matches!(self, Self::Approved | Self::ConditionalApproval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn validation_result_new_creates_correctly() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 28, 0, 0, 0).unwrap();

        let result = ValidationResult::new("test_signal", start, end, 500);

        assert_eq!(result.signal_name, "test_signal");
        assert_eq!(result.period_start, start);
        assert_eq!(result.period_end, end);
        assert_eq!(result.sample_size, 500);
    }

    #[test]
    fn recommendation_description_returns_text() {
        assert!(!Recommendation::Approved.description().is_empty());
        assert!(!Recommendation::ConditionalApproval.description().is_empty());
        assert!(!Recommendation::NeedsMoreData.description().is_empty());
        assert!(!Recommendation::Rejected.description().is_empty());
    }

    #[test]
    fn recommendation_is_usable_correct() {
        assert!(Recommendation::Approved.is_usable());
        assert!(Recommendation::ConditionalApproval.is_usable());
        assert!(!Recommendation::NeedsMoreData.is_usable());
        assert!(!Recommendation::Rejected.is_usable());
    }

    #[test]
    fn recommendation_serializes_correctly() {
        let json = serde_json::to_string(&Recommendation::Approved).unwrap();
        assert!(json.contains("Approved"));

        let deserialized: Recommendation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Recommendation::Approved);
    }
}
