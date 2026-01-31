//! Binary backtest CLI command.
//!
//! Runs backtests for binary outcome prediction markets using
//! historical signal data and price movements.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use algo_trade_backtest::binary::{
    BacktestResults, BinaryBacktestConfig, BinaryBacktestEngine, BinaryMetrics, FeeTier,
    PointInTimeProvider, PolymarketFees,
};
use algo_trade_core::signal::{Direction, SignalValue};
use algo_trade_data::{SignalSnapshotRecord, SignalSnapshotRepository};

/// Arguments for the binary-backtest command.
#[derive(Args, Debug, Clone)]
pub struct BinaryBacktestArgs {
    /// Start date for backtest (ISO 8601 format)
    #[arg(long)]
    pub start: String,

    /// End date for backtest (ISO 8601 format)
    #[arg(long)]
    pub end: String,

    /// Signal to use (default: liquidation_cascade)
    #[arg(long, default_value = "liquidation_cascade")]
    pub signal: String,

    /// Minimum signal strength threshold (default: 0.5)
    #[arg(long, default_value = "0.5")]
    pub min_strength: f64,

    /// Stake per bet in USD (default: 100)
    #[arg(long, default_value = "100")]
    pub stake: f64,

    /// Symbol to backtest (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Exchange for price data (default: binance)
    #[arg(long, default_value = "binance")]
    pub exchange: String,

    /// Fee tier: tier0, tier1, tier2, tier3, maker (default: tier0)
    #[arg(long, default_value = "tier0")]
    pub fee_tier: String,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// Output JSON results to file
    #[arg(long)]
    pub output: Option<String>,

    /// Output format: text, json (default: text)
    #[arg(long, default_value = "text")]
    pub format: String,
}

/// Output format for backtest reports.
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

/// Parses fee tier from string.
fn parse_fee_tier(s: &str) -> Result<FeeTier> {
    match s.to_lowercase().as_str() {
        "tier0" | "0" => Ok(FeeTier::Tier0),
        "tier1" | "1" => Ok(FeeTier::Tier1),
        "tier2" | "2" => Ok(FeeTier::Tier2),
        "tier3" | "3" => Ok(FeeTier::Tier3),
        "maker" => Ok(FeeTier::Maker),
        _ => Err(anyhow!(
            "Unknown fee tier: '{}'. Valid tiers: tier0, tier1, tier2, tier3, maker",
            s
        )),
    }
}

/// Converts a SignalSnapshotRecord to a (timestamp, SignalValue) tuple.
fn snapshot_to_signal(record: &SignalSnapshotRecord) -> Option<(DateTime<Utc>, SignalValue)> {
    let direction = match record.direction.to_lowercase().as_str() {
        "up" => Direction::Up,
        "down" => Direction::Down,
        "neutral" => Direction::Neutral,
        _ => return None,
    };

    // Convert Decimal strength/confidence to f64
    let strength: f64 = record.strength.to_string().parse().unwrap_or(0.0);
    let confidence: f64 = record.confidence.to_string().parse().unwrap_or(0.0);

    let signal = SignalValue::new(direction, strength, confidence).ok()?;
    Some((record.timestamp, signal))
}

/// Formats the backtest results as a text report.
fn format_text_report(results: &BacktestResults) -> String {
    let metrics = &results.metrics;
    let config = &results.config;

    let mut output = String::new();

    // Header
    output.push('\n');
    output.push_str("===============================================================\n");
    output.push_str("                    BINARY BACKTEST RESULTS                    \n");
    output.push_str("===============================================================\n");

    // Period and signal info
    output.push_str(&format!(
        "Period: {} to {}\n",
        results.start_time.format("%Y-%m-%d %H:%M"),
        results.end_time.format("%Y-%m-%d %H:%M")
    ));
    output.push_str(&format!("Signal: {}\n", config.symbol));
    output.push_str(&format!(
        "Min Strength: {:.1}%\n",
        config.min_strength * 100.0
    ));
    output.push_str(&format!("Stake per Bet: ${:.2}\n", config.stake_per_bet));
    output.push('\n');

    // Core metrics
    output.push_str("CORE METRICS\n");
    output.push_str("---------------------------------------------------------------\n");
    output.push_str(&format!("Total Bets:     {}\n", metrics.total_bets));
    output.push_str(&format!(
        "Wins:           {} ({:.1}%)\n",
        metrics.wins,
        metrics.win_rate * 100.0
    ));
    output.push_str(&format!(
        "Losses:         {} ({:.1}%)\n",
        metrics.losses,
        (1.0 - metrics.win_rate) * 100.0
    ));
    if metrics.pushes > 0 {
        output.push_str(&format!("Pushes:         {}\n", metrics.pushes));
    }
    output.push('\n');

    output.push_str(&format!(
        "Win Rate:       {:.1}%\n",
        metrics.win_rate * 100.0
    ));
    output.push_str(&format!(
        "Wilson 95% CI:  [{:.1}%, {:.1}%]\n",
        metrics.wilson_ci_lower * 100.0,
        metrics.wilson_ci_upper * 100.0
    ));

    // Significance
    let sig_label = if metrics.is_significant {
        "SIGNIFICANT"
    } else {
        "NOT SIGNIFICANT"
    };
    output.push_str(&format!(
        "Binomial p:     {:.4} ({})\n",
        metrics.binomial_p_value, sig_label
    ));
    output.push('\n');

    // Financial metrics
    output.push_str("FINANCIAL METRICS\n");
    output.push_str("---------------------------------------------------------------\n");
    output.push_str(&format!("Total Stake:    ${:.2}\n", metrics.total_stake));
    output.push_str(&format!(
        "Gross P&L:      ${:.2}\n",
        metrics.total_gross_pnl
    ));
    output.push_str(&format!("Fees:           ${:.2}\n", metrics.total_fees));
    output.push_str(&format!("Net P&L:        ${:.2}\n", metrics.net_pnl));
    output.push_str(&format!("EV per Bet:     ${:.2}\n", metrics.ev_per_bet));

    // ROI
    let roi_pct: f64 = metrics.roi.to_string().parse().unwrap_or(0.0) * 100.0;
    output.push_str(&format!("ROI:            {:.1}%\n", roi_pct));
    output.push('\n');

    // Break-even analysis
    output.push_str(&format!(
        "Break-even WR:  {:.1}%\n",
        metrics.break_even_win_rate * 100.0
    ));
    output.push_str(&format!(
        "Edge over BE:   {:.1}%\n",
        metrics.edge_over_break_even * 100.0
    ));
    output.push('\n');

    // Risk metrics
    output.push_str("RISK METRICS\n");
    output.push_str("---------------------------------------------------------------\n");
    output.push_str(&format!("Max Drawdown:   ${:.2}\n", metrics.max_drawdown));
    output.push_str(&format!(
        "Max Consec. Losses: {}\n",
        metrics.max_consecutive_losses
    ));
    output.push('\n');

    // Processing stats
    output.push_str("PROCESSING STATS\n");
    output.push_str("---------------------------------------------------------------\n");
    output.push_str(&format!(
        "Signals Processed: {}\n",
        results.signals_processed
    ));
    output.push_str(&format!(
        "Signals Skipped:   {} (below threshold)\n",
        results.signals_skipped
    ));
    output.push_str(&format!(
        "Fill Rate:         {:.1}%\n",
        results.fill_rate() * 100.0
    ));
    output.push('\n');

    // Go/No-Go decision
    output.push_str("===============================================================\n");
    let go_decision = determine_go_no_go(metrics);
    output.push_str(&format!("GO/NO-GO: {}\n", go_decision));
    output.push_str("===============================================================\n");

    output
}

/// Determines the Go/No-Go decision based on metrics.
fn determine_go_no_go(metrics: &BinaryMetrics) -> &'static str {
    // Criteria:
    // 1. At least 100 bets (sufficient sample size)
    // 2. Win rate > 53%
    // 3. p-value < 0.05 (statistically significant)
    // 4. Wilson CI lower bound > 50%
    // 5. Positive net P&L

    let has_sufficient_samples = metrics.total_bets >= 100;
    let has_good_win_rate = metrics.win_rate > 0.53;
    let is_significant = metrics.binomial_p_value < 0.05;
    let ci_above_50 = metrics.wilson_ci_lower > 0.50;
    let is_profitable = metrics.net_pnl > Decimal::ZERO;

    if has_sufficient_samples && has_good_win_rate && is_significant && ci_above_50 && is_profitable
    {
        "GO - All criteria met, proceed to paper trading"
    } else if metrics.total_bets < 100 {
        "PENDING - Insufficient sample size (need 100+ bets)"
    } else if metrics.binomial_p_value < 0.10 && metrics.win_rate > 0.52 {
        "CONDITIONAL - Shows promise, needs more validation"
    } else {
        "NO-GO - Strategy does not show significant edge"
    }
}

/// Runs the binary-backtest command.
///
/// # Errors
/// Returns an error if database connection fails, parsing fails, or backtest fails.
pub async fn run_binary_backtest(args: BinaryBacktestArgs) -> Result<()> {
    use sqlx::postgres::PgPoolOptions;

    // Parse arguments
    let start: DateTime<Utc> = args.start.parse().map_err(|_| {
        anyhow!("Invalid start time. Use ISO 8601 format (e.g., 2026-01-01T00:00:00Z)")
    })?;
    let end: DateTime<Utc> = args.end.parse().map_err(|_| {
        anyhow!("Invalid end time. Use ISO 8601 format (e.g., 2026-01-31T00:00:00Z)")
    })?;

    if start >= end {
        return Err(anyhow!("Start time must be before end time"));
    }

    let format = OutputFormat::parse(&args.format)?;
    let fee_tier = parse_fee_tier(&args.fee_tier)?;
    let stake = Decimal::try_from(args.stake)
        .map_err(|_| anyhow!("Invalid stake amount: {}", args.stake))?;

    tracing::info!(
        "Running binary backtest from {} to {}",
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    tracing::info!("Signal: {}", args.signal);
    tracing::info!("Min strength: {:.1}%", args.min_strength * 100.0);
    tracing::info!("Stake: ${:.2}", args.stake);
    tracing::info!("Fee tier: {:?}", fee_tier);

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

    // Create repositories
    let snapshot_repo = SignalSnapshotRepository::new(pool.clone());

    // Query signal snapshots
    tracing::info!("Querying signal snapshots for '{}'...", args.signal);
    let snapshots = snapshot_repo
        .query_by_signal_name(&args.signal, start, end)
        .await?;

    if snapshots.is_empty() {
        tracing::warn!(
            "No signal snapshots found for '{}' in the specified time range",
            args.signal
        );
        println!(
            "\nNo signal data found for '{}' between {} and {}.",
            args.signal,
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d")
        );
        println!("Try running 'backfill-signals' first to generate signal data.");
        return Ok(());
    }

    tracing::info!("Found {} signal snapshots", snapshots.len());

    // Convert snapshots to (timestamp, SignalValue) pairs
    let signals: Vec<(DateTime<Utc>, SignalValue)> =
        snapshots.iter().filter_map(snapshot_to_signal).collect();

    if signals.is_empty() {
        tracing::warn!("No valid signals after conversion");
        return Err(anyhow!("No valid signals found in snapshots"));
    }

    tracing::info!("Converted {} valid signals", signals.len());

    // Create backtest config
    let config = BinaryBacktestConfig::new(&args.symbol, &args.exchange)
        .with_window_duration(Duration::minutes(15))
        .with_min_strength(args.min_strength)
        .with_stake_per_bet(stake)
        .with_min_ev(dec!(0)); // Allow any positive EV

    // Create point-in-time provider with a larger lookback window for 15-minute candles
    // Using 900 seconds (15 minutes) to ensure we can find candle data
    let pit_provider = PointInTimeProvider::with_lookback(
        pool.clone(),
        &args.symbol,
        &args.exchange,
        900, // 15 minutes lookback
    );

    // Create fee model
    let fee_model = Box::new(PolymarketFees::new(fee_tier));

    // Create and run backtest engine
    let engine = BinaryBacktestEngine::new(config, pit_provider, fee_model);

    tracing::info!("Running backtest...");
    let results = engine
        .run(start, end, Duration::minutes(15), &signals)
        .await?;

    tracing::info!(
        "Backtest complete: {} bets placed, {:.1}% win rate",
        results.metrics.total_bets,
        results.metrics.win_rate * 100.0
    );

    // Output results
    match format {
        OutputFormat::Text => {
            let report = format_text_report(&results);
            println!("{}", report);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&results)?;
            println!("{}", json);
        }
    }

    // Write to file if requested
    if let Some(output_path) = args.output {
        let json = serde_json::to_string_pretty(&results)?;
        std::fs::write(&output_path, json)?;
        tracing::info!("Results written to {}", output_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

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
    // Fee Tier Parsing Tests
    // ============================================

    #[test]
    fn parse_fee_tier_valid() {
        assert_eq!(parse_fee_tier("tier0").unwrap(), FeeTier::Tier0);
        assert_eq!(parse_fee_tier("tier1").unwrap(), FeeTier::Tier1);
        assert_eq!(parse_fee_tier("tier2").unwrap(), FeeTier::Tier2);
        assert_eq!(parse_fee_tier("tier3").unwrap(), FeeTier::Tier3);
        assert_eq!(parse_fee_tier("maker").unwrap(), FeeTier::Maker);
        assert_eq!(parse_fee_tier("0").unwrap(), FeeTier::Tier0);
        assert_eq!(parse_fee_tier("TIER2").unwrap(), FeeTier::Tier2);
    }

    #[test]
    fn parse_fee_tier_invalid() {
        assert!(parse_fee_tier("tier4").is_err());
        assert!(parse_fee_tier("invalid").is_err());
    }

    // ============================================
    // Signal Conversion Tests
    // ============================================

    #[test]
    fn snapshot_to_signal_up_direction() {
        use algo_trade_data::SignalDirection;

        let record = SignalSnapshotRecord::new(
            Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.75),
            dec!(0.85),
        );

        let (ts, signal) = snapshot_to_signal(&record).unwrap();
        assert_eq!(signal.direction, Direction::Up);
        assert!((signal.strength - 0.75).abs() < f64::EPSILON);
        assert!((signal.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(ts.hour(), 12);
    }

    #[test]
    fn snapshot_to_signal_down_direction() {
        use algo_trade_data::SignalDirection;

        let record = SignalSnapshotRecord::new(
            Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Down,
            dec!(0.65),
            dec!(0.80),
        );

        let (_, signal) = snapshot_to_signal(&record).unwrap();
        assert_eq!(signal.direction, Direction::Down);
    }

    #[test]
    fn snapshot_to_signal_neutral_direction() {
        use algo_trade_data::SignalDirection;

        let record = SignalSnapshotRecord::new(
            Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap(),
            "test_signal",
            "BTCUSDT",
            "binance",
            SignalDirection::Neutral,
            dec!(0.0),
            dec!(0.50),
        );

        let (_, signal) = snapshot_to_signal(&record).unwrap();
        assert_eq!(signal.direction, Direction::Neutral);
    }

    // ============================================
    // Go/No-Go Decision Tests
    // ============================================

    #[test]
    fn go_no_go_insufficient_samples() {
        let metrics = BinaryMetrics {
            total_bets: 50, // Less than 100
            wins: 35,
            losses: 15,
            pushes: 0,
            win_rate: 0.70,
            wilson_ci_lower: 0.55,
            wilson_ci_upper: 0.82,
            binomial_p_value: 0.01,
            is_significant: true,
            total_stake: dec!(5000),
            total_gross_pnl: dec!(1000),
            total_fees: dec!(100),
            net_pnl: dec!(900),
            ev_per_bet: dec!(18),
            roi: dec!(0.18),
            break_even_win_rate: 0.52,
            edge_over_break_even: 0.18,
            max_drawdown: dec!(500),
            max_consecutive_losses: 3,
            avg_price: dec!(0.50),
            avg_fee_rate: dec!(0.02),
        };

        let decision = determine_go_no_go(&metrics);
        assert!(decision.contains("PENDING"));
    }

    #[test]
    fn go_no_go_all_criteria_met() {
        let metrics = BinaryMetrics {
            total_bets: 150,
            wins: 90,
            losses: 60,
            pushes: 0,
            win_rate: 0.60,
            wilson_ci_lower: 0.52,
            wilson_ci_upper: 0.67,
            binomial_p_value: 0.02,
            is_significant: true,
            total_stake: dec!(15000),
            total_gross_pnl: dec!(3000),
            total_fees: dec!(300),
            net_pnl: dec!(2700),
            ev_per_bet: dec!(18),
            roi: dec!(0.18),
            break_even_win_rate: 0.52,
            edge_over_break_even: 0.08,
            max_drawdown: dec!(1000),
            max_consecutive_losses: 5,
            avg_price: dec!(0.50),
            avg_fee_rate: dec!(0.02),
        };

        let decision = determine_go_no_go(&metrics);
        assert!(decision.contains("GO"));
    }

    #[test]
    fn go_no_go_conditional() {
        let metrics = BinaryMetrics {
            total_bets: 150,
            wins: 82, // 54.7% win rate
            losses: 68,
            pushes: 0,
            win_rate: 0.547,
            wilson_ci_lower: 0.46,
            wilson_ci_upper: 0.63,
            binomial_p_value: 0.08, // p < 0.10 but not < 0.05
            is_significant: false,
            total_stake: dec!(15000),
            total_gross_pnl: dec!(500),
            total_fees: dec!(300),
            net_pnl: dec!(200),
            ev_per_bet: dec!(1.33),
            roi: dec!(0.0133),
            break_even_win_rate: 0.52,
            edge_over_break_even: 0.027,
            max_drawdown: dec!(2000),
            max_consecutive_losses: 7,
            avg_price: dec!(0.50),
            avg_fee_rate: dec!(0.02),
        };

        let decision = determine_go_no_go(&metrics);
        assert!(decision.contains("CONDITIONAL"));
    }

    #[test]
    fn go_no_go_no_edge() {
        let metrics = BinaryMetrics {
            total_bets: 150,
            wins: 70, // 46.7% win rate
            losses: 80,
            pushes: 0,
            win_rate: 0.467,
            wilson_ci_lower: 0.38,
            wilson_ci_upper: 0.55,
            binomial_p_value: 0.60,
            is_significant: false,
            total_stake: dec!(15000),
            total_gross_pnl: dec!(-1000),
            total_fees: dec!(300),
            net_pnl: dec!(-1300),
            ev_per_bet: dec!(-8.67),
            roi: dec!(-0.0867),
            break_even_win_rate: 0.52,
            edge_over_break_even: -0.053,
            max_drawdown: dec!(3000),
            max_consecutive_losses: 8,
            avg_price: dec!(0.50),
            avg_fee_rate: dec!(0.02),
        };

        let decision = determine_go_no_go(&metrics);
        assert!(decision.contains("NO-GO"));
    }

    // ============================================
    // Timestamp Validation Tests
    // ============================================

    #[test]
    fn timestamp_parsing_valid() {
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

    // ============================================
    // Report Formatting Tests
    // ============================================

    #[test]
    fn format_text_report_contains_key_sections() {
        use algo_trade_backtest::binary::BinaryBacktestConfig;

        let config = BinaryBacktestConfig::new("BTCUSDT", "binance")
            .with_min_strength(0.5)
            .with_stake_per_bet(dec!(100));

        let metrics = BinaryMetrics {
            total_bets: 100,
            wins: 60,
            losses: 40,
            pushes: 0,
            win_rate: 0.60,
            wilson_ci_lower: 0.50,
            wilson_ci_upper: 0.69,
            binomial_p_value: 0.02,
            is_significant: true,
            total_stake: dec!(10000),
            total_gross_pnl: dec!(2000),
            total_fees: dec!(200),
            net_pnl: dec!(1800),
            ev_per_bet: dec!(18),
            roi: dec!(0.18),
            break_even_win_rate: 0.52,
            edge_over_break_even: 0.08,
            max_drawdown: dec!(500),
            max_consecutive_losses: 4,
            avg_price: dec!(0.50),
            avg_fee_rate: dec!(0.02),
        };

        let results = BacktestResults {
            settlements: vec![],
            metrics,
            config,
            start_time: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            end_time: Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap(),
            signals_processed: 200,
            signals_skipped: 100,
        };

        let report = format_text_report(&results);

        assert!(report.contains("BINARY BACKTEST RESULTS"));
        assert!(report.contains("CORE METRICS"));
        assert!(report.contains("FINANCIAL METRICS"));
        assert!(report.contains("RISK METRICS"));
        assert!(report.contains("GO/NO-GO"));
        assert!(report.contains("Win Rate"));
        assert!(report.contains("Wilson"));
        assert!(report.contains("Net P&L"));
    }
}
