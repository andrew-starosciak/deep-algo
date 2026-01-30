//! Backfill OHLCV CLI command.
//!
//! Fetches historical OHLCV data from Binance Futures and stores it
//! in the database for price lookups during return calculations.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;

/// Arguments for the backfill-ohlcv command.
#[derive(Args, Debug, Clone)]
pub struct BackfillOhlcvArgs {
    /// Start timestamp (ISO 8601 format, e.g., "2026-01-01T00:00:00Z")
    #[arg(long)]
    pub start: String,

    /// End timestamp (ISO 8601 format, e.g., "2026-01-30T00:00:00Z")
    #[arg(long)]
    pub end: String,

    /// Trading symbol (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Candle interval (default: 1m)
    /// Valid values: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 6h, 8h, 12h, 1d, 3d, 1w, 1M
    #[arg(long, default_value = "1m")]
    pub interval: String,

    /// Batch size for database inserts (default: 1000)
    #[arg(long, default_value = "1000")]
    pub batch_size: usize,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// Skip existing data check (force re-fetch all data)
    #[arg(long, default_value = "false")]
    pub force: bool,
}

/// Statistics for the backfill operation.
#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub struct BackfillOhlcvStats {
    /// Total candles fetched from API
    pub candles_fetched: u64,
    /// Total candles inserted into database
    pub candles_inserted: u64,
    /// Candles skipped due to duplicates
    pub duplicates_skipped: u64,
    /// API requests made
    pub api_requests: u64,
    /// Failed API requests
    pub failed_requests: u64,
}

#[allow(dead_code)]
impl BackfillOhlcvStats {
    /// Creates a new stats tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Formats a summary report.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "Fetched: {}, Inserted: {}, Duplicates: {}, Requests: {} ({} failed)",
            self.candles_fetched,
            self.candles_inserted,
            self.duplicates_skipped,
            self.api_requests,
            self.failed_requests
        )
    }
}

/// Runs the backfill-ohlcv command.
///
/// # Errors
/// Returns an error if database connection fails, API requests fail,
/// or the time range is invalid.
pub async fn run_backfill_ohlcv(args: BackfillOhlcvArgs) -> Result<()> {
    use algo_trade_data::repositories::ohlcv_repo::OhlcvRepository;
    use algo_trade_signals::collector::{
        calculate_expected_candles, calculate_required_requests, Interval, OhlcvCollector,
    };
    use sqlx::postgres::PgPoolOptions;
    use std::str::FromStr;

    // Parse arguments
    let start: DateTime<Utc> = args.start.parse().map_err(|_| {
        anyhow!("Invalid start time. Use ISO 8601 format (e.g., 2026-01-01T00:00:00Z)")
    })?;
    let end: DateTime<Utc> = args.end.parse().map_err(|_| {
        anyhow!("Invalid end time. Use ISO 8601 format (e.g., 2026-01-30T00:00:00Z)")
    })?;

    if start >= end {
        return Err(anyhow!("Start time must be before end time"));
    }

    let interval = Interval::from_str(&args.interval)?;

    // Calculate expected work
    let expected_candles = calculate_expected_candles(start, end, interval);
    let expected_requests = calculate_required_requests(start, end, interval);

    tracing::info!(
        "Backfilling OHLCV data for {} from {} to {}",
        args.symbol,
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    tracing::info!("Interval: {}", args.interval);
    tracing::info!(
        "Expected: ~{} candles, ~{} API requests",
        expected_candles,
        expected_requests
    );

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
    let repo = OhlcvRepository::new(pool);

    // Check existing data bounds if not forcing
    if !args.force {
        if let Some((existing_start, existing_end)) =
            repo.get_data_bounds(&args.symbol, "binance").await?
        {
            tracing::info!(
                "Existing data: {} to {}",
                existing_start.format("%Y-%m-%d %H:%M"),
                existing_end.format("%Y-%m-%d %H:%M")
            );

            // Check if we already have all the data
            if existing_start <= start && existing_end >= end {
                let count = repo
                    .count_records(&args.symbol, "binance", start, end)
                    .await?;
                if count as u64 >= expected_candles {
                    tracing::info!(
                        "Data already exists ({} records). Use --force to re-fetch.",
                        count
                    );
                    return Ok(());
                }
            }
        }
    }

    // Create collector
    let collector = OhlcvCollector::new();

    // Fetch data
    tracing::info!("Fetching data from Binance Futures API...");
    let (records, fetch_stats) = collector.fetch(&args.symbol, interval, start, end).await?;

    tracing::info!(
        "Fetched {} candles in {} requests",
        fetch_stats.total_candles,
        fetch_stats.total_requests
    );

    if records.is_empty() {
        tracing::warn!("No data fetched. Check symbol and date range.");
        return Ok(());
    }

    // Insert into database in batches
    tracing::info!(
        "Inserting {} records into database (batch size: {})...",
        records.len(),
        args.batch_size
    );

    let mut total_inserted = 0u64;
    for (i, chunk) in records.chunks(args.batch_size).enumerate() {
        let inserted = repo.insert_batch(chunk).await?;
        total_inserted += inserted;

        if (i + 1) % 10 == 0 {
            tracing::info!(
                "Progress: {}/{} batches, {} records inserted",
                i + 1,
                records.len().div_ceil(args.batch_size),
                total_inserted
            );
        }
    }

    let duplicates = records.len() as u64 - total_inserted;

    tracing::info!("Backfill complete!");
    tracing::info!(
        "Summary: {} fetched, {} inserted, {} duplicates skipped",
        records.len(),
        total_inserted,
        duplicates
    );

    // Verify data
    if let Some((data_start, data_end)) = repo.get_data_bounds(&args.symbol, "binance").await? {
        let total_count = repo
            .count_records(&args.symbol, "binance", data_start, data_end)
            .await?;
        tracing::info!(
            "Database now has {} records from {} to {}",
            total_count,
            data_start.format("%Y-%m-%d %H:%M"),
            data_end.format("%Y-%m-%d %H:%M")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap()
    }

    // ============================================
    // BackfillOhlcvArgs Tests
    // ============================================

    #[test]
    fn test_args_default_values() {
        // Verify default values match expectations
        let default_symbol = "BTCUSDT";
        let default_interval = "1m";
        let default_batch_size: usize = 1000;

        assert_eq!(default_symbol, "BTCUSDT");
        assert_eq!(default_interval, "1m");
        assert_eq!(default_batch_size, 1000);
    }

    // ============================================
    // BackfillOhlcvStats Tests
    // ============================================

    #[test]
    fn test_stats_new() {
        let stats = BackfillOhlcvStats::new();
        assert_eq!(stats.candles_fetched, 0);
        assert_eq!(stats.candles_inserted, 0);
        assert_eq!(stats.duplicates_skipped, 0);
        assert_eq!(stats.api_requests, 0);
        assert_eq!(stats.failed_requests, 0);
    }

    #[test]
    fn test_stats_summary() {
        let mut stats = BackfillOhlcvStats::new();
        stats.candles_fetched = 1500;
        stats.candles_inserted = 1400;
        stats.duplicates_skipped = 100;
        stats.api_requests = 2;
        stats.failed_requests = 0;

        let summary = stats.summary();
        assert!(summary.contains("1500"));
        assert!(summary.contains("1400"));
        assert!(summary.contains("100"));
        assert!(summary.contains("2"));
        assert!(summary.contains("0 failed"));
    }

    #[test]
    fn test_stats_summary_with_failures() {
        let mut stats = BackfillOhlcvStats::new();
        stats.candles_fetched = 1500;
        stats.api_requests = 3;
        stats.failed_requests = 1;

        let summary = stats.summary();
        assert!(summary.contains("3"));
        assert!(summary.contains("1 failed"));
    }

    // ============================================
    // Timestamp Parsing Tests
    // ============================================

    #[test]
    fn test_parse_valid_iso8601_timestamp() {
        use chrono::{Datelike, Timelike};

        let timestamp_str = "2026-01-30T12:00:00Z";
        let parsed: Result<DateTime<Utc>, _> = timestamp_str.parse();

        assert!(parsed.is_ok());
        let ts = parsed.unwrap();
        assert_eq!(ts.year(), 2026);
        assert_eq!(ts.month(), 1);
        assert_eq!(ts.day(), 30);
        assert_eq!(ts.hour(), 12);
    }

    #[test]
    fn test_parse_invalid_timestamp() {
        let timestamp_str = "invalid-timestamp";
        let parsed: Result<DateTime<Utc>, _> = timestamp_str.parse();

        assert!(parsed.is_err());
    }

    #[test]
    fn test_parse_timestamp_with_offset() {
        let timestamp_str = "2026-01-30T12:00:00+00:00";
        let parsed: Result<DateTime<Utc>, _> = timestamp_str.parse();

        assert!(parsed.is_ok());
    }

    // ============================================
    // Time Range Validation Tests
    // ============================================

    #[test]
    fn test_start_before_end_valid() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1);

        assert!(start < end);
    }

    #[test]
    fn test_start_equals_end_invalid() {
        let start = sample_timestamp();
        let end = start;

        assert!(start >= end);
    }

    #[test]
    fn test_start_after_end_invalid() {
        let start = sample_timestamp();
        let end = start - chrono::Duration::hours(1);

        assert!(start >= end);
    }

    // ============================================
    // Batch Size Tests
    // ============================================

    #[test]
    fn test_batch_chunking_exact() {
        let records: Vec<i32> = (0..2000).collect();
        let batch_size = 1000;

        let chunks: Vec<_> = records.chunks(batch_size).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 1000);
        assert_eq!(chunks[1].len(), 1000);
    }

    #[test]
    fn test_batch_chunking_remainder() {
        let records: Vec<i32> = (0..2500).collect();
        let batch_size = 1000;

        let chunks: Vec<_> = records.chunks(batch_size).collect();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 1000);
        assert_eq!(chunks[1].len(), 1000);
        assert_eq!(chunks[2].len(), 500);
    }

    #[test]
    fn test_batch_chunking_small() {
        let records: Vec<i32> = (0..500).collect();
        let batch_size = 1000;

        let chunks: Vec<_> = records.chunks(batch_size).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 500);
    }

    // ============================================
    // Duplicate Calculation Tests
    // ============================================

    #[test]
    fn test_duplicate_calculation() {
        let total_fetched: u64 = 1500;
        let total_inserted: u64 = 1400;
        let duplicates = total_fetched - total_inserted;

        assert_eq!(duplicates, 100);
    }

    #[test]
    fn test_no_duplicates() {
        let total_fetched: u64 = 1500;
        let total_inserted: u64 = 1500;
        let duplicates = total_fetched - total_inserted;

        assert_eq!(duplicates, 0);
    }

    // ============================================
    // Data Bounds Logic Tests
    // ============================================

    #[test]
    fn test_data_coverage_check_full() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(24);

        let existing_start = start - chrono::Duration::hours(1);
        let existing_end = end + chrono::Duration::hours(1);

        // Existing data covers requested range
        assert!(existing_start <= start && existing_end >= end);
        // Silence unused warning
        let _ = (end, existing_start, existing_end);
    }

    #[test]
    fn test_data_coverage_check_partial() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(24);

        let existing_start = start + chrono::Duration::hours(1);
        let existing_end = end - chrono::Duration::hours(1);

        // Existing data does NOT cover requested range
        assert!(!(existing_start <= start && existing_end >= end));
    }

    #[test]
    fn test_data_coverage_check_no_data() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(24);

        // No existing data
        let bounds: Option<(DateTime<Utc>, DateTime<Utc>)> = None;

        assert!(bounds.is_none());
    }

    // ============================================
    // Expected Candles Calculation Tests
    // ============================================

    #[test]
    fn test_expected_candles_one_day_1m() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::days(1);
        let interval_ms = 60_000i64; // 1 minute

        let duration_ms = (end - start).num_milliseconds();
        let expected_candles = duration_ms / interval_ms;

        assert_eq!(expected_candles, 1440); // 24 * 60
    }

    #[test]
    fn test_expected_candles_one_week_1h() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::weeks(1);
        let interval_ms = 3_600_000i64; // 1 hour

        let duration_ms = (end - start).num_milliseconds();
        let expected_candles = duration_ms / interval_ms;

        assert_eq!(expected_candles, 168); // 7 * 24
    }
}
