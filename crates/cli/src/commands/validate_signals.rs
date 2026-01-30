//! Validate signals CLI command.
//!
//! Generates statistical validation reports for signal generators
//! based on historical predictions and outcomes.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;

use algo_trade_signals::Recommendation;

/// Arguments for the validate-signals command.
#[derive(Args, Debug, Clone)]
pub struct ValidateSignalsArgs {
    /// Start timestamp (ISO 8601 format)
    #[arg(long)]
    pub start: String,

    /// End timestamp (ISO 8601 format)
    #[arg(long)]
    pub end: String,

    /// Minimum samples required for valid inference (default: 100)
    #[arg(long, default_value = "100")]
    pub min_samples: usize,

    /// Output format: text, json (default: text)
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Comma-separated signal names to validate (default: all)
    #[arg(long, default_value = "all")]
    pub signals: String,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

/// Output format for validation reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    /// Parses an output format from string.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "text" | "txt" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            _ => Err(anyhow!(
                "Unknown format: '{}'. Valid formats: text, json",
                s
            )),
        }
    }
}

/// Summary of validation results for multiple signals.
#[derive(Debug)]
pub struct ValidationSummary {
    /// Total signals validated
    pub total_signals: usize,
    /// Signals approved (p < 0.05)
    pub approved: usize,
    /// Signals conditionally approved (p < 0.10)
    pub conditional: usize,
    /// Signals needing more data
    pub needs_data: usize,
    /// Signals rejected
    pub rejected: usize,
    /// Signal names by recommendation
    pub by_recommendation: std::collections::HashMap<Recommendation, Vec<String>>,
}

impl ValidationSummary {
    /// Creates an empty summary.
    pub fn new() -> Self {
        Self {
            total_signals: 0,
            approved: 0,
            conditional: 0,
            needs_data: 0,
            rejected: 0,
            by_recommendation: std::collections::HashMap::new(),
        }
    }

    /// Adds a signal's result to the summary.
    pub fn add(&mut self, signal_name: &str, recommendation: Recommendation) {
        self.total_signals += 1;

        match recommendation {
            Recommendation::Approved => self.approved += 1,
            Recommendation::ConditionalApproval => self.conditional += 1,
            Recommendation::NeedsMoreData => self.needs_data += 1,
            Recommendation::Rejected => self.rejected += 1,
        }

        self.by_recommendation
            .entry(recommendation)
            .or_default()
            .push(signal_name.to_string());
    }

    /// Returns whether any signal passed the Go/No-Go criteria.
    pub fn has_go_signal(&self) -> bool {
        self.approved > 0 || self.conditional > 0
    }

    /// Formats a text summary.
    pub fn to_text(&self) -> String {
        let mut output = String::new();

        output.push('\n');
        output.push_str("╔══════════════════════════════════════════════════════════════╗\n");
        output.push_str("║               SIGNAL VALIDATION SUMMARY                       ║\n");
        output.push_str("╠══════════════════════════════════════════════════════════════╣\n");
        output.push_str(&format!(
            "║  Total Signals Validated: {:>36} ║\n",
            self.total_signals
        ));
        output.push_str("╠══════════════════════════════════════════════════════════════╣\n");
        output.push_str(&format!(
            "║  APPROVED (p < 0.05):      {:>35} ║\n",
            self.approved
        ));
        output.push_str(&format!(
            "║  CONDITIONAL (p < 0.10):   {:>35} ║\n",
            self.conditional
        ));
        output.push_str(&format!(
            "║  NEEDS MORE DATA:          {:>35} ║\n",
            self.needs_data
        ));
        output.push_str(&format!(
            "║  REJECTED:                 {:>35} ║\n",
            self.rejected
        ));
        output.push_str("╠══════════════════════════════════════════════════════════════╣\n");

        // Go/No-Go decision
        if self.has_go_signal() {
            output.push_str("║                        >>> GO <<<                             ║\n");
            output.push_str("║  At least one signal demonstrates predictive power.          ║\n");
        } else if self.needs_data > 0 && self.rejected == 0 {
            output.push_str("║                    >>> PENDING <<<                            ║\n");
            output.push_str("║  Signals need more data for conclusive validation.           ║\n");
        } else {
            output.push_str("║                      >>> NO-GO <<<                            ║\n");
            output.push_str("║  No signals show statistically significant edge.             ║\n");
        }

        output.push_str("╚══════════════════════════════════════════════════════════════╝\n");

        // List signals by recommendation
        if !self.by_recommendation.is_empty() {
            output.push_str("\nSignals by Recommendation:\n");
            output.push_str("─────────────────────────\n");

            if let Some(signals) = self.by_recommendation.get(&Recommendation::Approved) {
                output.push_str(&format!("  APPROVED: {}\n", signals.join(", ")));
            }
            if let Some(signals) = self
                .by_recommendation
                .get(&Recommendation::ConditionalApproval)
            {
                output.push_str(&format!("  CONDITIONAL: {}\n", signals.join(", ")));
            }
            if let Some(signals) = self.by_recommendation.get(&Recommendation::NeedsMoreData) {
                output.push_str(&format!("  NEEDS DATA: {}\n", signals.join(", ")));
            }
            if let Some(signals) = self.by_recommendation.get(&Recommendation::Rejected) {
                output.push_str(&format!("  REJECTED: {}\n", signals.join(", ")));
            }
        }

        output
    }
}

impl Default for ValidationSummary {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs the validate-signals command.
///
/// # Errors
/// Returns an error if database connection fails or validation fails.
pub async fn run_validate_signals(args: ValidateSignalsArgs) -> Result<()> {
    use algo_trade_data::SignalSnapshotRepository;
    use algo_trade_signals::ValidationReport;
    use sqlx::postgres::PgPoolOptions;

    // Parse arguments
    let start: DateTime<Utc> = args
        .start
        .parse()
        .map_err(|_| anyhow!("Invalid start time. Use ISO 8601 format"))?;
    let end: DateTime<Utc> = args
        .end
        .parse()
        .map_err(|_| anyhow!("Invalid end time. Use ISO 8601 format"))?;

    if start >= end {
        return Err(anyhow!("Start time must be before end time"));
    }

    let format = OutputFormat::parse(&args.format)?;

    tracing::info!(
        "Validating signals from {} to {}",
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    tracing::info!("Minimum samples: {}", args.min_samples);

    // Get database URL
    let db_url = args
        .db_url
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Create repository
    let snapshot_repo = SignalSnapshotRepository::new(pool.clone());

    // Get signal names to validate
    let signal_names = if args.signals.to_lowercase() == "all" {
        snapshot_repo.get_signal_names().await?
    } else {
        args.signals
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    if signal_names.is_empty() {
        tracing::warn!("No signals found to validate");
        return Ok(());
    }

    tracing::info!(
        "Validating {} signal(s): {:?}",
        signal_names.len(),
        signal_names
    );

    // Query all snapshots with forward returns
    let all_snapshots = snapshot_repo.query_with_forward_return(start, end).await?;

    tracing::info!(
        "Found {} validated snapshots in time range",
        all_snapshots.len()
    );

    // Generate reports and summary
    let mut summary = ValidationSummary::new();
    let mut reports = Vec::new();

    for signal_name in &signal_names {
        // Filter snapshots for this signal
        let signal_snapshots: Vec<_> = all_snapshots
            .iter()
            .filter(|s| s.signal_name == *signal_name)
            .cloned()
            .collect();

        if signal_snapshots.is_empty() {
            tracing::warn!("No validated snapshots for signal '{}'", signal_name);
            continue;
        }

        tracing::info!(
            "Generating report for '{}' ({} samples)",
            signal_name,
            signal_snapshots.len()
        );

        // Generate validation report
        match ValidationReport::generate(signal_name, &signal_snapshots, args.min_samples) {
            Ok(report) => {
                summary.add(signal_name, report.recommendation);
                reports.push(report);
            }
            Err(e) => {
                tracing::warn!("Failed to generate report for '{}': {}", signal_name, e);
                summary.add(signal_name, Recommendation::NeedsMoreData);
            }
        }
    }

    // Output reports
    match format {
        OutputFormat::Text => {
            for report in &reports {
                println!("{}", report.to_text());
                println!();
            }
            println!("{}", summary.to_text());
        }
        OutputFormat::Json => {
            let json_reports: Vec<_> = reports
                .iter()
                .map(|r| serde_json::to_value(r).unwrap_or_default())
                .collect();

            let output = serde_json::json!({
                "reports": json_reports,
                "summary": {
                    "total_signals": summary.total_signals,
                    "approved": summary.approved,
                    "conditional": summary.conditional,
                    "needs_data": summary.needs_data,
                    "rejected": summary.rejected,
                    "go_decision": summary.has_go_signal()
                }
            });

            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    // Return appropriate exit status based on Go/No-Go
    if summary.has_go_signal() {
        tracing::info!("Validation complete: GO - signals show predictive power");
    } else if summary.needs_data > 0 {
        tracing::info!("Validation complete: PENDING - need more data");
    } else {
        tracing::warn!("Validation complete: NO-GO - no signals show edge");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // OutputFormat Tests
    // ============================================

    #[test]
    fn output_format_parse_text() {
        assert_eq!(OutputFormat::parse("text").unwrap(), OutputFormat::Text);
        assert_eq!(OutputFormat::parse("TEXT").unwrap(), OutputFormat::Text);
        assert_eq!(OutputFormat::parse("txt").unwrap(), OutputFormat::Text);
    }

    #[test]
    fn output_format_parse_json() {
        assert_eq!(OutputFormat::parse("json").unwrap(), OutputFormat::Json);
        assert_eq!(OutputFormat::parse("JSON").unwrap(), OutputFormat::Json);
    }

    #[test]
    fn output_format_parse_invalid() {
        assert!(OutputFormat::parse("xml").is_err());
        assert!(OutputFormat::parse("").is_err());
    }

    // ============================================
    // ValidationSummary Tests
    // ============================================

    #[test]
    fn validation_summary_new_is_empty() {
        let summary = ValidationSummary::new();
        assert_eq!(summary.total_signals, 0);
        assert_eq!(summary.approved, 0);
        assert_eq!(summary.conditional, 0);
        assert_eq!(summary.needs_data, 0);
        assert_eq!(summary.rejected, 0);
    }

    #[test]
    fn validation_summary_add_approved() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Approved);

        assert_eq!(summary.total_signals, 1);
        assert_eq!(summary.approved, 1);
        assert!(summary.has_go_signal());
    }

    #[test]
    fn validation_summary_add_conditional() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::ConditionalApproval);

        assert_eq!(summary.total_signals, 1);
        assert_eq!(summary.conditional, 1);
        assert!(summary.has_go_signal());
    }

    #[test]
    fn validation_summary_add_needs_data() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::NeedsMoreData);

        assert_eq!(summary.total_signals, 1);
        assert_eq!(summary.needs_data, 1);
        assert!(!summary.has_go_signal());
    }

    #[test]
    fn validation_summary_add_rejected() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Rejected);

        assert_eq!(summary.total_signals, 1);
        assert_eq!(summary.rejected, 1);
        assert!(!summary.has_go_signal());
    }

    #[test]
    fn validation_summary_multiple_signals() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Approved);
        summary.add("signal_b", Recommendation::Rejected);
        summary.add("signal_c", Recommendation::NeedsMoreData);
        summary.add("signal_d", Recommendation::ConditionalApproval);

        assert_eq!(summary.total_signals, 4);
        assert_eq!(summary.approved, 1);
        assert_eq!(summary.conditional, 1);
        assert_eq!(summary.needs_data, 1);
        assert_eq!(summary.rejected, 1);
        assert!(summary.has_go_signal());
    }

    #[test]
    fn validation_summary_tracks_signal_names() {
        let mut summary = ValidationSummary::new();
        summary.add("obi", Recommendation::Approved);
        summary.add("funding", Recommendation::Approved);
        summary.add("news", Recommendation::Rejected);

        let approved = summary
            .by_recommendation
            .get(&Recommendation::Approved)
            .unwrap();
        assert_eq!(approved.len(), 2);
        assert!(approved.contains(&"obi".to_string()));
        assert!(approved.contains(&"funding".to_string()));

        let rejected = summary
            .by_recommendation
            .get(&Recommendation::Rejected)
            .unwrap();
        assert_eq!(rejected.len(), 1);
        assert!(rejected.contains(&"news".to_string()));
    }

    #[test]
    fn validation_summary_to_text_includes_counts() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Approved);
        summary.add("signal_b", Recommendation::Rejected);

        let text = summary.to_text();
        assert!(text.contains("2")); // total
        assert!(text.contains("APPROVED"));
        assert!(text.contains("REJECTED"));
    }

    #[test]
    fn validation_summary_go_decision() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Approved);

        let text = summary.to_text();
        assert!(text.contains("GO"));
    }

    #[test]
    fn validation_summary_no_go_decision() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::Rejected);
        summary.add("signal_b", Recommendation::Rejected);

        let text = summary.to_text();
        assert!(text.contains("NO-GO"));
    }

    #[test]
    fn validation_summary_pending_decision() {
        let mut summary = ValidationSummary::new();
        summary.add("signal_a", Recommendation::NeedsMoreData);

        let text = summary.to_text();
        assert!(text.contains("PENDING"));
    }

    // ============================================
    // Timestamp Validation Tests
    // ============================================

    #[test]
    fn timestamp_parsing_valid() {
        use chrono::TimeZone;

        let valid = "2026-01-28T00:00:00Z";
        let parsed: DateTime<Utc> = valid.parse().unwrap();
        assert_eq!(parsed, Utc.with_ymd_and_hms(2026, 1, 28, 0, 0, 0).unwrap());
    }

    #[test]
    fn timestamp_parsing_invalid() {
        let invalid = "not-a-date";
        let result: std::result::Result<DateTime<Utc>, _> = invalid.parse();
        assert!(result.is_err());
    }
}
