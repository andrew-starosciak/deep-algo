//! Statistical validation framework for signal generators.
//!
//! This module provides comprehensive statistical validation tools for
//! evaluating signal predictive power, including correlation analysis,
//! information coefficient calculation, and hypothesis testing.

mod correlation;
mod hypothesis;
mod ic;
mod report;
mod types;

pub use correlation::{analyze_signal_correlation, CorrelationAnalysis};
pub use hypothesis::{
    test_directional_accuracy, test_return_significance, SignificanceTest, TestType,
};
pub use ic::{calculate_ic, calculate_ranks, ICAnalysis};
pub use report::{determine_recommendation, ValidationReport};
pub use types::{Recommendation, ValidationResult};
