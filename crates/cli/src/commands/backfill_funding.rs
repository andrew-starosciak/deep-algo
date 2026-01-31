//! Backfill funding rate CLI command.
//!
//! Fetches historical funding rates from Binance Futures API and stores them
//! in the database for signal generation during backtests.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;

use algo_trade_data::models::FundingRateRecord;
use algo_trade_data::repositories::FundingRateRepository;

/// Arguments for the backfill-funding command.
#[derive(Args, Debug, Clone)]
pub struct BackfillFundingArgs {
    /// Start timestamp (ISO 8601 format, e.g., "2025-01-01T00:00:00Z")
    #[arg(long)]
    pub start: String,

    /// End timestamp (ISO 8601 format, e.g., "2025-01-30T00:00:00Z")
    #[arg(long)]
    pub end: String,

    /// Trading symbol (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Exchange (currently only binance is supported for historical funding)
    #[arg(long, default_value = "binance")]
    pub exchange: String,

    /// Maximum records per API request (default: 1000, max 1000)
    #[arg(long, default_value = "1000")]
    pub limit: i32,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// Skip existing data check (force re-fetch all data)
    #[arg(long, default_value = "false")]
    pub force: bool,
}

/// Binance funding rate API response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct BinanceFundingRate {
    symbol: String,
    funding_rate: String,
    funding_time: i64,
    mark_price: String, // Returned by API but not used
}

/// Runs the backfill-funding command.
///
/// # Errors
/// Returns an error if database connection fails, API requests fail,
/// or the time range is invalid.
pub async fn run_backfill_funding(args: BackfillFundingArgs) -> Result<()> {
    // Parse arguments
    let start: DateTime<Utc> = args.start.parse().map_err(|_| {
        anyhow!("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")
    })?;
    let end: DateTime<Utc> = args.end.parse().map_err(|_| {
        anyhow!("Invalid end time. Use ISO 8601 format (e.g., 2025-01-30T00:00:00Z)")
    })?;

    if start >= end {
        return Err(anyhow!("Start time must be before end time"));
    }

    let exchange = args.exchange.to_lowercase();
    if exchange != "binance" {
        return Err(anyhow!(
            "Only 'binance' is supported for historical funding rate backfill. \
            Hyperliquid support can be added when needed."
        ));
    }

    // Calculate expected funding rate intervals (8-hour intervals for most exchanges)
    let duration_hours = (end - start).num_hours();
    let expected_records = (duration_hours / 8) as u64;

    tracing::info!(
        "Backfilling funding rates for {} from {} to {} (exchange: {})",
        args.symbol,
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M"),
        exchange
    );
    tracing::info!(
        "Duration: {} days (~{} expected funding rate records)",
        duration_hours / 24,
        expected_records
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
    let repo = FundingRateRepository::new(pool);

    // Check existing data if not forcing
    if !args.force {
        let existing = repo
            .query_by_time_range(&args.symbol, &exchange, start, end)
            .await?;
        if !existing.is_empty() {
            let coverage = existing.len() as f64 / expected_records as f64 * 100.0;
            if coverage > 90.0 {
                tracing::info!(
                    "Already have {} records ({:.1}% coverage). Use --force to re-fetch.",
                    existing.len(),
                    coverage
                );
                return Ok(());
            }
            tracing::info!(
                "Found {} existing records ({:.1}% coverage). Fetching missing data...",
                existing.len(),
                coverage
            );
        }
    }

    // Fetch funding rates from Binance
    let records = fetch_binance_funding(&args.symbol, start, end, args.limit).await?;

    tracing::info!("Fetched {} funding rate records", records.len());

    if records.is_empty() {
        tracing::warn!("No funding rate data fetched. Check symbol and date range.");
        return Ok(());
    }

    // Insert into database
    tracing::info!("Inserting {} records into database...", records.len());
    repo.insert_batch(&records).await?;

    // Verify insertion
    let final_count = repo
        .query_by_time_range(&args.symbol, &exchange, start, end)
        .await?;

    tracing::info!(
        "Backfill complete! Database now has {} funding rate records for {} to {}",
        final_count.len(),
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d")
    );

    Ok(())
}

/// Fetches funding rates from Binance Futures API.
async fn fetch_binance_funding(
    symbol: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: i32,
) -> Result<Vec<FundingRateRecord>> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let mut all_records = Vec::new();
    let mut current_start = start.timestamp_millis();
    let end_ms = end.timestamp_millis();
    let limit = limit.min(1000); // Binance max is 1000

    let base_url = std::env::var("BINANCE_FUTURES_API_URL")
        .unwrap_or_else(|_| "https://fapi.binance.com".to_string());

    tracing::info!("Fetching from Binance Futures API: {}", base_url);

    let mut request_count = 0;
    loop {
        let url = format!(
            "{}/fapi/v1/fundingRate?symbol={}&startTime={}&endTime={}&limit={}",
            base_url, symbol, current_start, end_ms, limit
        );

        tracing::debug!("Fetching: {}", url);

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Binance API error {}: {}", status, text));
        }

        let funding_rates: Vec<BinanceFundingRate> = response.json().await?;

        if funding_rates.is_empty() {
            break;
        }

        let batch_count = funding_rates.len();
        let last_time = funding_rates
            .last()
            .map(|f| f.funding_time)
            .unwrap_or(end_ms);

        for rate in funding_rates {
            let timestamp = DateTime::from_timestamp_millis(rate.funding_time)
                .ok_or_else(|| anyhow!("Invalid funding_time timestamp: {}", rate.funding_time))?;

            let funding_rate = Decimal::from_str(&rate.funding_rate)
                .map_err(|e| anyhow!("Invalid funding_rate '{}': {}", rate.funding_rate, e))?;

            // FundingRateRecord::new() automatically calculates annualized rate (3x per day)
            all_records.push(FundingRateRecord::new(
                timestamp,
                rate.symbol,
                "binance".to_string(),
                funding_rate,
            ));
        }

        request_count += 1;

        // Move past the last record we received
        current_start = last_time + 1;

        if current_start >= end_ms || batch_count < limit as usize {
            break;
        }

        // Rate limiting: Binance allows 2400 req/min, but be conservative
        if request_count % 10 == 0 {
            tracing::info!(
                "Progress: {} records fetched ({} API requests)",
                all_records.len(),
                request_count
            );
            sleep(Duration::from_millis(100)).await;
        }
    }

    tracing::info!(
        "Completed: {} records in {} API requests",
        all_records.len(),
        request_count
    );

    Ok(all_records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone};

    #[test]
    fn test_parse_valid_timestamps() {
        let start_str = "2025-01-01T00:00:00Z";
        let end_str = "2025-01-30T00:00:00Z";

        let start: Result<DateTime<Utc>, _> = start_str.parse();
        let end: Result<DateTime<Utc>, _> = end_str.parse();

        assert!(start.is_ok());
        assert!(end.is_ok());
        assert!(start.unwrap() < end.unwrap());
    }

    #[test]
    fn test_expected_records_calculation() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 8, 0, 0, 0).unwrap();

        let duration_hours = (end - start).num_hours();
        let expected_records = duration_hours / 8; // 8-hour funding intervals

        assert_eq!(duration_hours, 168); // 7 days * 24 hours
        assert_eq!(expected_records, 21); // 21 funding events in 7 days
    }

    #[test]
    fn test_annual_rate_calculation_binance() {
        let funding_rate = Decimal::from_str("0.0001").unwrap();
        let annual_rate = funding_rate * Decimal::from(3 * 365);

        // 0.0001 * 1095 = 0.1095 (10.95% APR)
        assert_eq!(annual_rate, Decimal::from_str("0.1095").unwrap());
    }

    #[test]
    fn test_annual_rate_calculation_hyperliquid() {
        let funding_rate = Decimal::from_str("0.00001").unwrap();
        let annual_rate = funding_rate * Decimal::from(24 * 365);

        // 0.00001 * 8760 = 0.0876 (8.76% APR)
        assert_eq!(annual_rate, Decimal::from_str("0.0876").unwrap());
    }

    #[test]
    fn test_binance_funding_rate_deserialization() {
        let json = r#"{
            "symbol": "BTCUSDT",
            "fundingRate": "0.00010000",
            "fundingTime": 1704067200000,
            "markPrice": "42000.00"
        }"#;

        let rate: BinanceFundingRate = serde_json::from_str(json).unwrap();
        assert_eq!(rate.symbol, "BTCUSDT");
        assert_eq!(rate.funding_rate, "0.00010000");
        assert_eq!(rate.funding_time, 1704067200000);
    }

    #[test]
    fn test_timestamp_conversion() {
        let funding_time: i64 = 1704067200000; // 2024-01-01 00:00:00 UTC
        let timestamp = DateTime::from_timestamp_millis(funding_time);

        assert!(timestamp.is_some());
        let ts = timestamp.unwrap();
        assert_eq!(ts.year(), 2024);
        assert_eq!(ts.month(), 1);
        assert_eq!(ts.day(), 1);
    }

    #[test]
    fn test_funding_rate_decimal_parsing() {
        let rate_str = "0.00010000";
        let rate = Decimal::from_str(rate_str);

        assert!(rate.is_ok());
        assert_eq!(rate.unwrap(), Decimal::from_str("0.0001").unwrap());
    }

    #[test]
    fn test_limit_capping() {
        let user_limit = 5000;
        let capped_limit = user_limit.min(1000);
        assert_eq!(capped_limit, 1000);

        let small_limit = 500;
        let capped_small = small_limit.min(1000);
        assert_eq!(capped_small, 500);
    }
}
