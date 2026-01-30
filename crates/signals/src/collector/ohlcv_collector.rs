//! OHLCV historical data collector for Binance Futures.
//!
//! Fetches historical kline/candlestick data from Binance Futures API
//! with proper rate limiting and pagination support.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use governor::{Quota, RateLimiter};
use rust_decimal::Decimal;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::time::Duration;

use algo_trade_data::OhlcvRecord;

/// Maximum candles per request (Binance limit is 1500)
const MAX_CANDLES_PER_REQUEST: usize = 1500;

/// Default rate limit (20 requests per second)
const DEFAULT_RATE_LIMIT_PER_SECOND: u32 = 20;

/// Binance Futures API base URL
const BINANCE_FUTURES_API: &str = "https://fapi.binance.com";

/// OHLCV collector for Binance Futures historical data.
pub struct OhlcvCollector {
    client: reqwest::Client,
    base_url: String,
    rate_limiter: RateLimiter<
        governor::state::NotKeyed,
        governor::state::InMemoryState,
        governor::clock::DefaultClock,
    >,
}

/// Interval specification for OHLCV data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    OneMinute,
    ThreeMinutes,
    FiveMinutes,
    FifteenMinutes,
    ThirtyMinutes,
    OneHour,
    TwoHours,
    FourHours,
    SixHours,
    EightHours,
    TwelveHours,
    OneDay,
    ThreeDays,
    OneWeek,
    OneMonth,
}

impl Interval {
    /// Returns the Binance API string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Interval::OneMinute => "1m",
            Interval::ThreeMinutes => "3m",
            Interval::FiveMinutes => "5m",
            Interval::FifteenMinutes => "15m",
            Interval::ThirtyMinutes => "30m",
            Interval::OneHour => "1h",
            Interval::TwoHours => "2h",
            Interval::FourHours => "4h",
            Interval::SixHours => "6h",
            Interval::EightHours => "8h",
            Interval::TwelveHours => "12h",
            Interval::OneDay => "1d",
            Interval::ThreeDays => "3d",
            Interval::OneWeek => "1w",
            Interval::OneMonth => "1M",
        }
    }

    /// Returns the interval duration in milliseconds.
    #[must_use]
    pub fn duration_ms(&self) -> i64 {
        match self {
            Interval::OneMinute => 60_000,
            Interval::ThreeMinutes => 180_000,
            Interval::FiveMinutes => 300_000,
            Interval::FifteenMinutes => 900_000,
            Interval::ThirtyMinutes => 1_800_000,
            Interval::OneHour => 3_600_000,
            Interval::TwoHours => 7_200_000,
            Interval::FourHours => 14_400_000,
            Interval::SixHours => 21_600_000,
            Interval::EightHours => 28_800_000,
            Interval::TwelveHours => 43_200_000,
            Interval::OneDay => 86_400_000,
            Interval::ThreeDays => 259_200_000,
            Interval::OneWeek => 604_800_000,
            Interval::OneMonth => 2_592_000_000, // ~30 days
        }
    }
}

impl FromStr for Interval {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // Special case for month: "1M" is case-sensitive to distinguish from "1m" (minute)
        if s == "1M" {
            return Ok(Interval::OneMonth);
        }

        // All other intervals can be case-insensitive
        match s.to_lowercase().as_str() {
            "1m" => Ok(Interval::OneMinute),
            "3m" => Ok(Interval::ThreeMinutes),
            "5m" => Ok(Interval::FiveMinutes),
            "15m" => Ok(Interval::FifteenMinutes),
            "30m" => Ok(Interval::ThirtyMinutes),
            "1h" => Ok(Interval::OneHour),
            "2h" => Ok(Interval::TwoHours),
            "4h" => Ok(Interval::FourHours),
            "6h" => Ok(Interval::SixHours),
            "8h" => Ok(Interval::EightHours),
            "12h" => Ok(Interval::TwelveHours),
            "1d" => Ok(Interval::OneDay),
            "3d" => Ok(Interval::ThreeDays),
            "1w" => Ok(Interval::OneWeek),
            _ => Err(anyhow!(
                "Invalid interval: '{}'. Valid values: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 6h, 8h, 12h, 1d, 3d, 1w, 1M",
                s
            )),
        }
    }
}

/// Statistics for a backfill operation.
#[derive(Debug, Default, Clone)]
pub struct BackfillStats {
    /// Total candles fetched
    pub total_candles: u64,
    /// Total API requests made
    pub total_requests: u64,
    /// Number of failed requests
    pub failed_requests: u64,
    /// Number of duplicate candles skipped
    pub duplicates_skipped: u64,
}

impl BackfillStats {
    /// Creates a new stats tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Formats a summary report.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "Candles: {}, Requests: {}, Failed: {}, Duplicates: {}",
            self.total_candles, self.total_requests, self.failed_requests, self.duplicates_skipped
        )
    }
}

impl OhlcvCollector {
    /// Creates a new OHLCV collector with default settings.
    ///
    /// Uses the default Binance Futures API URL and rate limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(BINANCE_FUTURES_API, DEFAULT_RATE_LIMIT_PER_SECOND)
    }

    /// Creates a new OHLCV collector with custom configuration.
    ///
    /// # Arguments
    /// * `base_url` - Base URL for the Binance Futures API
    /// * `rate_limit_per_second` - Maximum requests per second
    #[must_use]
    pub fn with_config(base_url: &str, rate_limit_per_second: u32) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        let quota = Quota::per_second(
            NonZeroU32::new(rate_limit_per_second).expect("Rate limit must be > 0"),
        );
        let rate_limiter = RateLimiter::direct(quota);

        Self {
            client,
            base_url: base_url.to_string(),
            rate_limiter,
        }
    }

    /// Fetches OHLCV data for a symbol within a time range.
    ///
    /// Handles pagination automatically when the time range exceeds
    /// the maximum candles per request.
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `interval` - Candle interval
    /// * `start` - Start timestamp (inclusive)
    /// * `end` - End timestamp (inclusive)
    ///
    /// # Returns
    /// A vector of OHLCV records and statistics about the fetch operation.
    ///
    /// # Errors
    /// Returns an error if the API request fails.
    pub async fn fetch(
        &self,
        symbol: &str,
        interval: Interval,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(Vec<OhlcvRecord>, BackfillStats)> {
        if start > end {
            return Err(anyhow!("Start time must be before end time"));
        }

        let mut records = Vec::new();
        let mut stats = BackfillStats::new();
        let mut current_start = start;

        while current_start < end {
            // Wait for rate limiter
            self.rate_limiter.until_ready().await;

            // Fetch a batch
            let batch = self.fetch_batch(symbol, interval, current_start, end).await;

            stats.total_requests += 1;

            match batch {
                Ok(batch_records) => {
                    if batch_records.is_empty() {
                        // No more data
                        break;
                    }

                    // Find the latest timestamp for pagination
                    if let Some(last_record) = batch_records.last() {
                        // Move start to one interval after the last record
                        current_start = last_record.timestamp
                            + chrono::Duration::milliseconds(interval.duration_ms());
                    }

                    stats.total_candles += batch_records.len() as u64;
                    records.extend(batch_records);
                }
                Err(e) => {
                    stats.failed_requests += 1;
                    tracing::error!("Failed to fetch batch: {}", e);

                    // Exponential backoff on failure
                    tokio::time::sleep(Duration::from_secs(5)).await;

                    // Skip ahead if too many failures
                    if stats.failed_requests > 10 {
                        return Err(anyhow!(
                            "Too many failed requests ({}). Last error: {}",
                            stats.failed_requests,
                            e
                        ));
                    }
                }
            }
        }

        Ok((records, stats))
    }

    /// Fetches a single batch of OHLCV data.
    async fn fetch_batch(
        &self,
        symbol: &str,
        interval: Interval,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let url = format!("{}/fapi/v1/klines", self.base_url);

        let start_ms = start.timestamp_millis();
        let end_ms = end.timestamp_millis();

        let response = self
            .client
            .get(&url)
            .query(&[
                ("symbol", symbol),
                ("interval", interval.as_str()),
                ("startTime", &start_ms.to_string()),
                ("endTime", &end_ms.to_string()),
                ("limit", &MAX_CANDLES_PER_REQUEST.to_string()),
            ])
            .send()
            .await
            .context("Failed to send request to Binance API")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Binance API error ({}): {}", status, error_text));
        }

        let data: Vec<Vec<serde_json::Value>> = response
            .json()
            .await
            .context("Failed to parse Binance API response")?;

        let records = data
            .into_iter()
            .filter_map(|kline| parse_kline(&kline, symbol))
            .collect();

        Ok(records)
    }
}

impl Default for OhlcvCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Parses a single kline from the Binance API response.
///
/// Binance kline format:
/// ```text
/// [
///   1499040000000,      // 0: Open time
///   "0.01634000",       // 1: Open
///   "0.80000000",       // 2: High
///   "0.01575800",       // 3: Low
///   "0.01577100",       // 4: Close
///   "148976.11427815",  // 5: Volume
///   1499644799999,      // 6: Close time
///   "2434.19055334",    // 7: Quote asset volume
///   308,                // 8: Number of trades
///   "1756.87402397",    // 9: Taker buy base asset volume
///   "28.46694368",      // 10: Taker buy quote asset volume
///   "17928899.62484339" // 11: Ignore
/// ]
/// ```
fn parse_kline(kline: &[serde_json::Value], symbol: &str) -> Option<OhlcvRecord> {
    if kline.len() < 6 {
        return None;
    }

    let open_time_ms = kline[0].as_i64()?;
    let timestamp = Utc.timestamp_millis_opt(open_time_ms).single()?;

    let open = parse_decimal_from_json(&kline[1])?;
    let high = parse_decimal_from_json(&kline[2])?;
    let low = parse_decimal_from_json(&kline[3])?;
    let close = parse_decimal_from_json(&kline[4])?;
    let volume = parse_decimal_from_json(&kline[5])?;

    Some(OhlcvRecord {
        timestamp,
        symbol: symbol.to_string(),
        exchange: "binance".to_string(),
        open,
        high,
        low,
        close,
        volume,
    })
}

/// Parses a Decimal from a JSON value (handles both string and number formats).
fn parse_decimal_from_json(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::String(s) => Decimal::from_str(s).ok(),
        serde_json::Value::Number(n) => {
            // Convert number to string first to preserve precision
            Decimal::from_str(&n.to_string()).ok()
        }
        _ => None,
    }
}

/// Calculates the expected number of candles for a time range.
#[must_use]
pub fn calculate_expected_candles(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: Interval,
) -> u64 {
    if start >= end {
        return 0;
    }

    let duration_ms = (end - start).num_milliseconds();
    let interval_ms = interval.duration_ms();

    if interval_ms == 0 {
        return 0;
    }

    (duration_ms / interval_ms) as u64
}

/// Calculates the number of API requests needed for a time range.
#[must_use]
pub fn calculate_required_requests(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: Interval,
) -> u64 {
    let candles = calculate_expected_candles(start, end, interval);
    if candles == 0 {
        return 0;
    }

    candles.div_ceil(MAX_CANDLES_PER_REQUEST as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap()
    }

    // ============================================
    // Interval Tests
    // ============================================

    #[test]
    fn test_interval_as_str() {
        assert_eq!(Interval::OneMinute.as_str(), "1m");
        assert_eq!(Interval::FiveMinutes.as_str(), "5m");
        assert_eq!(Interval::FifteenMinutes.as_str(), "15m");
        assert_eq!(Interval::OneHour.as_str(), "1h");
        assert_eq!(Interval::OneDay.as_str(), "1d");
        assert_eq!(Interval::OneWeek.as_str(), "1w");
        assert_eq!(Interval::OneMonth.as_str(), "1M");
    }

    #[test]
    fn test_interval_duration_ms() {
        assert_eq!(Interval::OneMinute.duration_ms(), 60_000);
        assert_eq!(Interval::FiveMinutes.duration_ms(), 300_000);
        assert_eq!(Interval::FifteenMinutes.duration_ms(), 900_000);
        assert_eq!(Interval::OneHour.duration_ms(), 3_600_000);
        assert_eq!(Interval::OneDay.duration_ms(), 86_400_000);
    }

    #[test]
    fn test_interval_from_str_valid() {
        assert_eq!(Interval::from_str("1m").unwrap(), Interval::OneMinute);
        assert_eq!(Interval::from_str("5m").unwrap(), Interval::FiveMinutes);
        assert_eq!(Interval::from_str("15m").unwrap(), Interval::FifteenMinutes);
        assert_eq!(Interval::from_str("1h").unwrap(), Interval::OneHour);
        assert_eq!(Interval::from_str("1d").unwrap(), Interval::OneDay);
    }

    #[test]
    fn test_interval_from_str_case_insensitive() {
        assert_eq!(Interval::from_str("1M").unwrap(), Interval::OneMonth);
        assert_eq!(Interval::from_str("1H").unwrap(), Interval::OneHour);
    }

    #[test]
    fn test_interval_from_str_invalid() {
        let result = Interval::from_str("invalid");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid interval"));
    }

    // ============================================
    // BackfillStats Tests
    // ============================================

    #[test]
    fn test_backfill_stats_new() {
        let stats = BackfillStats::new();
        assert_eq!(stats.total_candles, 0);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.failed_requests, 0);
        assert_eq!(stats.duplicates_skipped, 0);
    }

    #[test]
    fn test_backfill_stats_summary() {
        let mut stats = BackfillStats::new();
        stats.total_candles = 1500;
        stats.total_requests = 2;
        stats.failed_requests = 1;
        stats.duplicates_skipped = 10;

        let summary = stats.summary();
        assert!(summary.contains("1500"));
        assert!(summary.contains("2"));
        assert!(summary.contains("1"));
        assert!(summary.contains("10"));
    }

    // ============================================
    // OhlcvCollector Creation Tests
    // ============================================

    #[test]
    fn test_collector_default() {
        let collector = OhlcvCollector::default();
        assert_eq!(collector.base_url, BINANCE_FUTURES_API);
    }

    #[test]
    fn test_collector_with_config() {
        let collector = OhlcvCollector::with_config("https://custom.api.com", 10);
        assert_eq!(collector.base_url, "https://custom.api.com");
    }

    // ============================================
    // Kline Parsing Tests
    // ============================================

    #[test]
    fn test_parse_kline_valid() {
        let kline = vec![
            serde_json::json!(1706616000000i64), // Open time (2024-01-30 12:00:00 UTC)
            serde_json::json!("50000.00"),       // Open
            serde_json::json!("50100.00"),       // High
            serde_json::json!("49900.00"),       // Low
            serde_json::json!("50050.00"),       // Close
            serde_json::json!("1000.50"),        // Volume
        ];

        let record = parse_kline(&kline, "BTCUSDT");
        assert!(record.is_some());

        let record = record.unwrap();
        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.open, dec!(50000.00));
        assert_eq!(record.high, dec!(50100.00));
        assert_eq!(record.low, dec!(49900.00));
        assert_eq!(record.close, dec!(50050.00));
        assert_eq!(record.volume, dec!(1000.50));
    }

    #[test]
    fn test_parse_kline_insufficient_data() {
        let kline = vec![
            serde_json::json!(1706616000000i64),
            serde_json::json!("50000.00"),
            serde_json::json!("50100.00"),
        ];

        let record = parse_kline(&kline, "BTCUSDT");
        assert!(record.is_none());
    }

    #[test]
    fn test_parse_kline_invalid_timestamp() {
        let kline = vec![
            serde_json::json!("invalid"),
            serde_json::json!("50000.00"),
            serde_json::json!("50100.00"),
            serde_json::json!("49900.00"),
            serde_json::json!("50050.00"),
            serde_json::json!("1000.50"),
        ];

        let record = parse_kline(&kline, "BTCUSDT");
        assert!(record.is_none());
    }

    #[test]
    fn test_parse_kline_invalid_price() {
        let kline = vec![
            serde_json::json!(1706616000000i64),
            serde_json::json!("invalid"),
            serde_json::json!("50100.00"),
            serde_json::json!("49900.00"),
            serde_json::json!("50050.00"),
            serde_json::json!("1000.50"),
        ];

        let record = parse_kline(&kline, "BTCUSDT");
        assert!(record.is_none());
    }

    // ============================================
    // Decimal Parsing Tests
    // ============================================

    #[test]
    fn test_parse_decimal_from_string() {
        let value = serde_json::json!("50000.12345678");
        let decimal = parse_decimal_from_json(&value);
        assert_eq!(decimal, Some(dec!(50000.12345678)));
    }

    #[test]
    fn test_parse_decimal_from_number() {
        let value = serde_json::json!(50000.5);
        let decimal = parse_decimal_from_json(&value);
        assert!(decimal.is_some());
    }

    #[test]
    fn test_parse_decimal_from_invalid() {
        let value = serde_json::json!(null);
        let decimal = parse_decimal_from_json(&value);
        assert!(decimal.is_none());
    }

    // ============================================
    // Expected Candles Calculation Tests
    // ============================================

    #[test]
    fn test_calculate_expected_candles_one_day_1m() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::days(1);

        let candles = calculate_expected_candles(start, end, Interval::OneMinute);
        assert_eq!(candles, 1440); // 24 * 60 = 1440 minutes
    }

    #[test]
    fn test_calculate_expected_candles_one_hour_1m() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1);

        let candles = calculate_expected_candles(start, end, Interval::OneMinute);
        assert_eq!(candles, 60);
    }

    #[test]
    fn test_calculate_expected_candles_one_day_1h() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::days(1);

        let candles = calculate_expected_candles(start, end, Interval::OneHour);
        assert_eq!(candles, 24);
    }

    #[test]
    fn test_calculate_expected_candles_start_after_end() {
        let start = sample_timestamp();
        let end = start - chrono::Duration::hours(1);

        let candles = calculate_expected_candles(start, end, Interval::OneMinute);
        assert_eq!(candles, 0);
    }

    #[test]
    fn test_calculate_expected_candles_same_time() {
        let start = sample_timestamp();
        let end = start;

        let candles = calculate_expected_candles(start, end, Interval::OneMinute);
        assert_eq!(candles, 0);
    }

    // ============================================
    // Required Requests Calculation Tests
    // ============================================

    #[test]
    fn test_calculate_required_requests_one_batch() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1); // 60 candles at 1m

        let requests = calculate_required_requests(start, end, Interval::OneMinute);
        assert_eq!(requests, 1); // 60 < 1500, so 1 request
    }

    #[test]
    fn test_calculate_required_requests_multiple_batches() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::days(2); // 2880 candles at 1m

        let requests = calculate_required_requests(start, end, Interval::OneMinute);
        assert_eq!(requests, 2); // ceil(2880 / 1500) = 2
    }

    #[test]
    fn test_calculate_required_requests_exact_boundary() {
        let start = sample_timestamp();
        // Exactly 1500 minutes = 25 hours
        let end = start + chrono::Duration::minutes(1500);

        let requests = calculate_required_requests(start, end, Interval::OneMinute);
        assert_eq!(requests, 1);
    }

    #[test]
    fn test_calculate_required_requests_empty_range() {
        let start = sample_timestamp();
        let end = start;

        let requests = calculate_required_requests(start, end, Interval::OneMinute);
        assert_eq!(requests, 0);
    }

    // ============================================
    // Pagination Logic Tests
    // ============================================

    #[test]
    fn test_pagination_next_start_calculation() {
        let current_end_timestamp = sample_timestamp();
        let interval = Interval::OneMinute;

        // Next start should be one interval after the last record
        let next_start =
            current_end_timestamp + chrono::Duration::milliseconds(interval.duration_ms());

        assert_eq!(
            (next_start - current_end_timestamp).num_milliseconds(),
            60_000
        );
    }

    #[test]
    fn test_pagination_15m_interval() {
        let current_end_timestamp = sample_timestamp();
        let interval = Interval::FifteenMinutes;

        let next_start =
            current_end_timestamp + chrono::Duration::milliseconds(interval.duration_ms());

        assert_eq!(
            (next_start - current_end_timestamp).num_milliseconds(),
            900_000
        );
    }

    // ============================================
    // Timestamp Conversion Tests
    // ============================================

    #[test]
    fn test_timestamp_to_millis() {
        let timestamp = Utc.with_ymd_and_hms(2024, 1, 30, 12, 0, 0).unwrap();
        let millis = timestamp.timestamp_millis();

        // Should be a reasonable value
        assert!(millis > 0);
        assert_eq!(millis, 1706616000000);
    }

    #[test]
    fn test_millis_to_timestamp() {
        use chrono::{Datelike, Timelike};

        let millis: i64 = 1706616000000;
        let timestamp = Utc.timestamp_millis_opt(millis).single();

        assert!(timestamp.is_some());
        let ts = timestamp.unwrap();
        assert_eq!(ts.year(), 2024);
        assert_eq!(ts.month(), 1);
        assert_eq!(ts.day(), 30);
        assert_eq!(ts.hour(), 12);
        assert_eq!(ts.minute(), 0);
        assert_eq!(ts.second(), 0);
    }

    // ============================================
    // Rate Limiting Tests (Structure Only)
    // ============================================

    #[test]
    fn test_rate_limiter_quota() {
        let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
        // Quota should be created successfully
        let limiter: RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        > = RateLimiter::direct(quota);

        // Just verify the limiter was created
        assert!(std::mem::size_of_val(&limiter) > 0);
    }
}
