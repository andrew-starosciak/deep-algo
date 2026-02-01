//! Backfill signals CLI command.
//!
//! Iterates through historical timestamps and computes signals from
//! stored data, creating SignalSnapshotRecords for validation.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use std::time::Duration;

use super::collect_signals::parse_duration;

/// Arguments for the backfill-signals command.
#[derive(Args, Debug, Clone)]
pub struct BackfillSignalsArgs {
    /// Start timestamp (ISO 8601 format, e.g., "2026-01-28T00:00:00Z")
    #[arg(long)]
    pub start: String,

    /// End timestamp (ISO 8601 format, e.g., "2026-01-30T00:00:00Z")
    #[arg(long)]
    pub end: String,

    /// Comma-separated signal names (obi,funding,liquidation,news)
    /// Use "all" or leave empty for all signals
    #[arg(long, default_value = "all")]
    pub signals: String,

    /// Trading symbol (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Exchange name (default: binance)
    #[arg(long, default_value = "binance")]
    pub exchange: String,

    /// Interval between signal computations (default: 15m)
    #[arg(long, default_value = "15m")]
    pub interval: String,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

/// Known signal names with their short aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    OrderBookImbalance,
    FundingRate,
    LiquidationCascade,
    News,
    // Phase 2.2 Enhanced Signals
    FundingPercentile,
    MomentumExhaustion,
    WallBias,
    CompositeRequireN,
    LiquidationRatio,
}

impl SignalType {
    /// Returns all available signal types.
    pub fn all() -> Vec<SignalType> {
        vec![
            SignalType::OrderBookImbalance,
            SignalType::FundingRate,
            SignalType::LiquidationCascade,
            SignalType::News,
            // Phase 2.2 Enhanced Signals
            SignalType::FundingPercentile,
            SignalType::MomentumExhaustion,
            SignalType::WallBias,
            SignalType::CompositeRequireN,
            SignalType::LiquidationRatio,
        ]
    }

    /// Returns the full signal name used in the registry.
    pub fn name(&self) -> &'static str {
        match self {
            SignalType::OrderBookImbalance => "order_book_imbalance",
            SignalType::FundingRate => "funding_rate",
            SignalType::LiquidationCascade => "liquidation_cascade",
            SignalType::News => "news",
            // Phase 2.2 Enhanced Signals
            SignalType::FundingPercentile => "funding_percentile",
            SignalType::MomentumExhaustion => "momentum_exhaustion",
            SignalType::WallBias => "wall_bias",
            SignalType::CompositeRequireN => "composite_require_n",
            SignalType::LiquidationRatio => "liquidation_ratio",
        }
    }

    /// Parses a signal type from a short name or full name.
    pub fn parse(s: &str) -> Option<SignalType> {
        match s.to_lowercase().as_str() {
            "obi" | "order_book_imbalance" | "orderbook" => Some(SignalType::OrderBookImbalance),
            "funding" | "funding_rate" => Some(SignalType::FundingRate),
            "liquidation" | "liquidation_cascade" | "liq" => Some(SignalType::LiquidationCascade),
            "news" => Some(SignalType::News),
            // Phase 2.2 Enhanced Signals
            "funding_percentile" | "fp" | "percentile" => Some(SignalType::FundingPercentile),
            "momentum" | "momentum_exhaustion" | "me" => Some(SignalType::MomentumExhaustion),
            "wall" | "wall_bias" | "wb" => Some(SignalType::WallBias),
            "composite" | "composite_require_n" | "require_n" | "rn" => {
                Some(SignalType::CompositeRequireN)
            }
            "liq_ratio" | "liquidation_ratio" | "lr" | "ratio" => {
                Some(SignalType::LiquidationRatio)
            }
            _ => None,
        }
    }
}

/// Parses a comma-separated list of signal names.
///
/// # Errors
/// Returns an error if any signal name is invalid.
pub fn parse_signals(s: &str) -> Result<Vec<SignalType>> {
    let s = s.trim();

    if s.is_empty() || s.to_lowercase() == "all" {
        return Ok(SignalType::all());
    }

    let mut signals = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for part in s.split(',') {
        let name = part.trim().to_lowercase();

        if name.is_empty() {
            continue;
        }

        let signal = SignalType::parse(&name).ok_or_else(|| {
            anyhow!(
                "Unknown signal: '{}'. Valid signals: obi, funding, liquidation, news, fp (funding_percentile), momentum, wall, composite, liq_ratio",
                name
            )
        })?;

        if seen.insert(signal.name()) {
            signals.push(signal);
        }
    }

    if signals.is_empty() {
        return Err(anyhow!("At least one signal must be specified"));
    }

    Ok(signals)
}

/// Progress tracker for long-running operations.
#[derive(Debug)]
pub struct ProgressTracker {
    total: usize,
    processed: usize,
    start_time: std::time::Instant,
}

impl ProgressTracker {
    /// Creates a new progress tracker.
    pub fn new(total: usize) -> Self {
        Self {
            total,
            processed: 0,
            start_time: std::time::Instant::now(),
        }
    }

    /// Increments the processed count.
    pub fn increment(&mut self) {
        self.processed += 1;
    }

    /// Increments by a specified amount.
    #[allow(dead_code)]
    pub fn increment_by(&mut self, n: usize) {
        self.processed += n;
    }

    /// Returns the current progress as a percentage.
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        (self.processed as f64 / self.total as f64) * 100.0
    }

    /// Returns the estimated time remaining.
    pub fn eta(&self) -> Option<Duration> {
        if self.processed == 0 {
            return None;
        }

        let elapsed = self.start_time.elapsed();
        let rate = self.processed as f64 / elapsed.as_secs_f64();

        if rate == 0.0 {
            return None;
        }

        let remaining = self.total - self.processed;
        let eta_secs = remaining as f64 / rate;

        Some(Duration::from_secs_f64(eta_secs))
    }

    /// Formats a progress report string.
    pub fn report(&self) -> String {
        let eta_str = match self.eta() {
            Some(d) => format_duration(d),
            None => "calculating...".to_string(),
        };

        format!(
            "{}/{} ({:.1}%) - ETA: {}",
            self.processed,
            self.total,
            self.percent(),
            eta_str
        )
    }
}

/// Formats a duration in a human-readable format.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Generates timestamps at regular intervals between start and end.
pub fn generate_timestamps(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: Duration,
) -> Vec<DateTime<Utc>> {
    let mut timestamps = Vec::new();
    let mut current = start;
    let interval_chrono =
        chrono::Duration::from_std(interval).unwrap_or(chrono::Duration::minutes(15));

    while current <= end {
        timestamps.push(current);
        current += interval_chrono;
    }

    timestamps
}

/// Runs the backfill-signals command.
///
/// # Errors
/// Returns an error if database connection fails or signal computation fails.
pub async fn run_backfill_signals(args: BackfillSignalsArgs) -> Result<()> {
    use algo_trade_core::Direction;
    use algo_trade_data::{SignalDirection, SignalSnapshotRecord, SignalSnapshotRepository};
    use algo_trade_signals::{
        CompositeSignal, FundingPercentileConfig, FundingRateSignal, LiquidationCascadeSignal,
        LiquidationRatioSignal, MomentumExhaustionConfig, MomentumExhaustionSignal, NewsSignal,
        OrderBookImbalanceSignal, SignalContextBuilder, SignalRegistry, WallDetectionConfig,
    };
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
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

    let signals = parse_signals(&args.signals)?;
    let interval = parse_duration(&args.interval)?;

    tracing::info!(
        "Backfilling signals from {} to {}",
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    tracing::info!(
        "Signals: {:?}",
        signals.iter().map(|s| s.name()).collect::<Vec<_>>()
    );
    tracing::info!("Interval: {:?}", interval);

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

    // Create signal context builder
    let context_builder = SignalContextBuilder::new(pool.clone(), &args.symbol, &args.exchange);

    // Create signal registry and register signals
    let mut registry = SignalRegistry::new();

    for signal_type in &signals {
        match signal_type {
            SignalType::OrderBookImbalance => {
                registry.register(Box::new(OrderBookImbalanceSignal::default()));
            }
            SignalType::FundingRate => {
                registry.register(Box::new(FundingRateSignal::default()));
            }
            SignalType::LiquidationCascade => {
                registry.register(Box::new(LiquidationCascadeSignal::default()));
            }
            SignalType::News => {
                registry.register(Box::new(NewsSignal::default()));
            }
            // Phase 2.2 Enhanced Signals
            SignalType::FundingPercentile => {
                // Funding rate with 30-day percentile analysis
                let percentile_config = FundingPercentileConfig {
                    lookback_periods: 90, // 30 days * 3 funding periods/day
                    high_threshold: 0.80, // Top 20%
                    low_threshold: 0.20,  // Bottom 20%
                    min_data_points: 30,
                };
                let signal = FundingRateSignal::default().with_percentile_config(percentile_config);
                registry.register(Box::new(signal));
            }
            SignalType::MomentumExhaustion => {
                // Momentum exhaustion signal (stalling after big moves)
                let config = MomentumExhaustionConfig {
                    big_move_threshold: 0.02, // 2% move threshold
                    big_move_lookback: 6,     // 6 candles (~90 min at 15m)
                    stall_ratio: 0.3,         // Stall if range is 30% of move
                    stall_lookback: 2,        // 2 candles for stall detection
                    min_candles: 8,           // Minimum candles required
                };
                let signal = MomentumExhaustionSignal::new(config, 1.0);
                registry.register(Box::new(signal));
            }
            SignalType::WallBias => {
                // Order book wall bias with floor/ceiling semantics
                let wall_config = WallDetectionConfig {
                    min_wall_size_btc: dec!(5),
                    proximity_bps: 200,
                };
                let signal = OrderBookImbalanceSignal::default().with_wall_detection(wall_config);
                // Register with a different name to distinguish from base OBI
                registry.register_with_name(Box::new(signal), "wall_bias");
            }
            SignalType::CompositeRequireN => {
                // Composite signal requiring 2+ signals to agree
                // Uses the base signals that are already registered
                let composite = CompositeSignal::require_n("composite_require_n", 2)
                    .with_generator(Box::new(FundingRateSignal::default()))
                    .with_generator(Box::new(LiquidationCascadeSignal::default()))
                    .with_generator(Box::new(OrderBookImbalanceSignal::default()));
                registry.register(Box::new(composite));
            }
            SignalType::LiquidationRatio => {
                // 24-hour liquidation ratio signal (contrarian)
                registry.register(Box::new(LiquidationRatioSignal::default()));
            }
        }
    }

    // Generate timestamps
    let timestamps = generate_timestamps(start, end, interval);
    tracing::info!("Processing {} timestamps", timestamps.len());

    // Progress tracking
    let mut progress = ProgressTracker::new(timestamps.len());
    let mut total_snapshots = 0usize;
    let mut batch: Vec<SignalSnapshotRecord> = Vec::with_capacity(100);

    // Process each timestamp
    for timestamp in timestamps {
        // Build context from historical data
        let ctx = match context_builder.build_at(timestamp).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::debug!("Failed to build context at {}: {}", timestamp, e);
                progress.increment();
                continue;
            }
        };

        // Compute all signals
        let results = registry.compute_all(&ctx).await?;

        // Create snapshot records
        for (signal_name, signal_value) in results {
            let direction = match signal_value.direction {
                Direction::Up => SignalDirection::Up,
                Direction::Down => SignalDirection::Down,
                Direction::Neutral => SignalDirection::Neutral,
            };

            let strength = Decimal::try_from(signal_value.strength).unwrap_or(Decimal::ZERO);
            let confidence = Decimal::try_from(signal_value.confidence).unwrap_or(Decimal::ZERO);

            let record = SignalSnapshotRecord::new(
                timestamp,
                &signal_name,
                &args.symbol,
                &args.exchange,
                direction,
                strength,
                confidence,
            );

            batch.push(record);
        }

        // Flush batch if full
        if batch.len() >= 100 {
            match snapshot_repo.insert_batch(&batch).await {
                Ok(ids) => {
                    total_snapshots += ids.len();
                }
                Err(e) => {
                    tracing::error!("Failed to insert batch: {}", e);
                }
            }
            batch.clear();
        }

        progress.increment();

        // Report progress every 100 timestamps
        if progress.processed.is_multiple_of(100) {
            tracing::info!("Progress: {}", progress.report());
        }
    }

    // Flush remaining batch
    if !batch.is_empty() {
        match snapshot_repo.insert_batch(&batch).await {
            Ok(ids) => {
                total_snapshots += ids.len();
            }
            Err(e) => {
                tracing::error!("Failed to insert final batch: {}", e);
            }
        }
    }

    tracing::info!("Backfill complete!");
    tracing::info!("Total snapshots created: {}", total_snapshots);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 29, 12, 0, 0).unwrap()
    }

    // ============================================
    // SignalType Tests
    // ============================================

    #[test]
    fn signal_type_all_returns_nine_types() {
        let all = SignalType::all();
        assert_eq!(all.len(), 9); // 4 original + 5 Phase 2.2 enhanced signals
    }

    #[test]
    fn signal_type_name_returns_correct_strings() {
        assert_eq!(
            SignalType::OrderBookImbalance.name(),
            "order_book_imbalance"
        );
        assert_eq!(SignalType::FundingRate.name(), "funding_rate");
        assert_eq!(SignalType::LiquidationCascade.name(), "liquidation_cascade");
        assert_eq!(SignalType::News.name(), "news");
        // Phase 2.2 signals
        assert_eq!(SignalType::FundingPercentile.name(), "funding_percentile");
        assert_eq!(SignalType::MomentumExhaustion.name(), "momentum_exhaustion");
        assert_eq!(SignalType::WallBias.name(), "wall_bias");
        assert_eq!(SignalType::CompositeRequireN.name(), "composite_require_n");
        assert_eq!(SignalType::LiquidationRatio.name(), "liquidation_ratio");
    }

    #[test]
    fn signal_type_parse_handles_short_names() {
        assert_eq!(
            SignalType::parse("obi"),
            Some(SignalType::OrderBookImbalance)
        );
        assert_eq!(SignalType::parse("funding"), Some(SignalType::FundingRate));
        assert_eq!(
            SignalType::parse("liq"),
            Some(SignalType::LiquidationCascade)
        );
        assert_eq!(SignalType::parse("news"), Some(SignalType::News));
        // Phase 2.2 short names
        assert_eq!(SignalType::parse("fp"), Some(SignalType::FundingPercentile));
        assert_eq!(
            SignalType::parse("me"),
            Some(SignalType::MomentumExhaustion)
        );
        assert_eq!(SignalType::parse("wb"), Some(SignalType::WallBias));
        assert_eq!(SignalType::parse("rn"), Some(SignalType::CompositeRequireN));
        assert_eq!(SignalType::parse("lr"), Some(SignalType::LiquidationRatio));
        assert_eq!(
            SignalType::parse("liq_ratio"),
            Some(SignalType::LiquidationRatio)
        );
    }

    #[test]
    fn signal_type_parse_handles_full_names() {
        assert_eq!(
            SignalType::parse("order_book_imbalance"),
            Some(SignalType::OrderBookImbalance)
        );
        assert_eq!(
            SignalType::parse("funding_rate"),
            Some(SignalType::FundingRate)
        );
        assert_eq!(
            SignalType::parse("liquidation_cascade"),
            Some(SignalType::LiquidationCascade)
        );
    }

    #[test]
    fn signal_type_parse_is_case_insensitive() {
        assert_eq!(
            SignalType::parse("OBI"),
            Some(SignalType::OrderBookImbalance)
        );
        assert_eq!(SignalType::parse("FUNDING"), Some(SignalType::FundingRate));
        assert_eq!(SignalType::parse("News"), Some(SignalType::News));
    }

    #[test]
    fn signal_type_parse_returns_none_for_invalid() {
        assert_eq!(SignalType::parse("invalid"), None);
        assert_eq!(SignalType::parse(""), None);
    }

    // ============================================
    // parse_signals Tests
    // ============================================

    #[test]
    fn parse_signals_all_returns_all_types() {
        let signals = parse_signals("all").unwrap();
        assert_eq!(signals.len(), 9); // 4 original + 5 Phase 2.2
    }

    #[test]
    fn parse_signals_empty_returns_all_types() {
        let signals = parse_signals("").unwrap();
        assert_eq!(signals.len(), 9); // 4 original + 5 Phase 2.2
    }

    #[test]
    fn parse_signals_parses_single() {
        let signals = parse_signals("obi").unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0], SignalType::OrderBookImbalance);
    }

    #[test]
    fn parse_signals_parses_multiple() {
        let signals = parse_signals("obi,funding,news").unwrap();
        assert_eq!(signals.len(), 3);
    }

    #[test]
    fn parse_signals_handles_whitespace() {
        let signals = parse_signals("  obi , funding , news  ").unwrap();
        assert_eq!(signals.len(), 3);
    }

    #[test]
    fn parse_signals_deduplicates() {
        let signals = parse_signals("obi,obi,funding,funding").unwrap();
        assert_eq!(signals.len(), 2);
    }

    #[test]
    fn parse_signals_rejects_invalid() {
        let result = parse_signals("obi,invalid_signal");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown signal"));
    }

    // ============================================
    // ProgressTracker Tests
    // ============================================

    #[test]
    fn progress_tracker_starts_at_zero() {
        let tracker = ProgressTracker::new(100);
        assert_eq!(tracker.processed, 0);
        assert_eq!(tracker.total, 100);
    }

    #[test]
    fn progress_tracker_increments() {
        let mut tracker = ProgressTracker::new(100);
        tracker.increment();
        assert_eq!(tracker.processed, 1);

        tracker.increment_by(5);
        assert_eq!(tracker.processed, 6);
    }

    #[test]
    fn progress_tracker_percent_calculates_correctly() {
        let mut tracker = ProgressTracker::new(100);
        assert!((tracker.percent() - 0.0).abs() < f64::EPSILON);

        tracker.increment_by(50);
        assert!((tracker.percent() - 50.0).abs() < f64::EPSILON);

        tracker.increment_by(50);
        assert!((tracker.percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_tracker_percent_handles_zero_total() {
        let tracker = ProgressTracker::new(0);
        assert!((tracker.percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_tracker_report_formats_correctly() {
        let mut tracker = ProgressTracker::new(100);
        tracker.increment_by(25);
        let report = tracker.report();

        assert!(report.contains("25/100"));
        assert!(report.contains("25.0%"));
    }

    // ============================================
    // generate_timestamps Tests
    // ============================================

    #[test]
    fn generate_timestamps_creates_correct_count() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(2);
        let interval = Duration::from_secs(15 * 60); // 15 minutes

        let timestamps = generate_timestamps(start, end, interval);

        // 2 hours = 120 minutes, at 15 minute intervals = 9 timestamps (0, 15, 30, 45, 60, 75, 90, 105, 120)
        assert_eq!(timestamps.len(), 9);
    }

    #[test]
    fn generate_timestamps_includes_start_and_end() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1);
        let interval = Duration::from_secs(30 * 60); // 30 minutes

        let timestamps = generate_timestamps(start, end, interval);

        assert_eq!(timestamps.first(), Some(&start));
        assert_eq!(timestamps.last(), Some(&end));
    }

    #[test]
    fn generate_timestamps_handles_single_point() {
        let start = sample_timestamp();
        let end = start;
        let interval = Duration::from_secs(15 * 60);

        let timestamps = generate_timestamps(start, end, interval);

        assert_eq!(timestamps.len(), 1);
        assert_eq!(timestamps[0], start);
    }

    #[test]
    fn generate_timestamps_empty_if_start_after_end() {
        let start = sample_timestamp();
        let end = start - chrono::Duration::hours(1);
        let interval = Duration::from_secs(15 * 60);

        let timestamps = generate_timestamps(start, end, interval);

        assert!(timestamps.is_empty());
    }

    // ============================================
    // format_duration Tests
    // ============================================

    #[test]
    fn format_duration_seconds() {
        let d = Duration::from_secs(45);
        assert_eq!(format_duration(d), "45s");
    }

    #[test]
    fn format_duration_minutes() {
        let d = Duration::from_secs(125);
        assert_eq!(format_duration(d), "2m 5s");
    }

    #[test]
    fn format_duration_hours() {
        let d = Duration::from_secs(3725);
        assert_eq!(format_duration(d), "1h 2m");
    }
}
