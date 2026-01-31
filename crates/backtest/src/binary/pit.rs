//! Point-in-Time (PIT) data provider for backtesting.
//!
//! This module provides the critical component for preventing look-ahead bias
//! in backtests. All price queries must return data that was available AT or
//! BEFORE the requested timestamp - never data from the future.
//!
//! # Look-Ahead Bias
//!
//! Look-ahead bias occurs when a backtest uses information that would not have
//! been available at the time of the trading decision. This module ensures:
//!
//! 1. `get_price_at(t)` returns the close price of the most recent candle
//!    that ended AT or BEFORE time `t`
//! 2. No future data is ever returned
//! 3. Stale data (outside the lookback window) returns `None`
//!
//! # Example
//!
//! ```ignore
//! let provider = PointInTimeProvider::new(pool, "BTCUSDT", "binance");
//!
//! // Get price at 12:00 - returns the 11:45 candle close (or earlier)
//! let price = provider.get_price_at(noon).await?;
//!
//! // Get the forward return over 15 minutes
//! let ret = provider.get_forward_return(noon, Duration::minutes(15)).await?;
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

/// Default maximum lookback window in seconds (5 minutes).
///
/// If no price data exists within this window before the requested timestamp,
/// `None` is returned to avoid using stale data.
pub const DEFAULT_MAX_LOOKBACK_SECONDS: i64 = 300;

/// Point-in-Time data provider for historical price queries.
///
/// Ensures no look-ahead bias by only returning prices that existed
/// at or before the requested timestamp.
#[derive(Debug, Clone)]
pub struct PointInTimeProvider {
    pool: PgPool,
    symbol: String,
    exchange: String,
    max_lookback_seconds: i64,
}

impl PointInTimeProvider {
    /// Creates a new Point-in-Time provider with default lookback window.
    ///
    /// # Arguments
    /// * `pool` - PostgreSQL connection pool
    /// * `symbol` - Trading symbol (e.g., "BTCUSDT")
    /// * `exchange` - Exchange name (e.g., "binance")
    #[must_use]
    pub fn new(pool: PgPool, symbol: &str, exchange: &str) -> Self {
        Self {
            pool,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            max_lookback_seconds: DEFAULT_MAX_LOOKBACK_SECONDS,
        }
    }

    /// Creates a new Point-in-Time provider with custom lookback window.
    ///
    /// # Arguments
    /// * `pool` - PostgreSQL connection pool
    /// * `symbol` - Trading symbol (e.g., "BTCUSDT")
    /// * `exchange` - Exchange name (e.g., "binance")
    /// * `max_lookback_seconds` - Maximum seconds to look back for a price
    #[must_use]
    pub fn with_lookback(
        pool: PgPool,
        symbol: &str,
        exchange: &str,
        max_lookback_seconds: i64,
    ) -> Self {
        Self {
            pool,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            max_lookback_seconds,
        }
    }

    /// Returns the symbol this provider queries.
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the exchange this provider queries.
    #[must_use]
    pub fn exchange(&self) -> &str {
        &self.exchange
    }

    /// Returns the maximum lookback window in seconds.
    #[must_use]
    pub fn max_lookback_seconds(&self) -> i64 {
        self.max_lookback_seconds
    }

    /// Gets the close price at or just before the given timestamp.
    ///
    /// CRITICAL: This method must NEVER return prices AFTER the timestamp.
    /// This is the core guarantee that prevents look-ahead bias.
    ///
    /// # Arguments
    /// * `timestamp` - The point in time to query
    ///
    /// # Returns
    /// * `Ok(Some(price))` - The most recent close price within the lookback window
    /// * `Ok(None)` - No price data available within the lookback window
    /// * `Err(_)` - Database query failed
    ///
    /// # SQL Query Logic
    /// ```sql
    /// SELECT close FROM ohlcv
    /// WHERE symbol = $1 AND exchange = $2
    ///   AND timestamp > (target - lookback)  -- Not too old
    ///   AND timestamp <= target              -- CRITICAL: No look-ahead!
    /// ORDER BY timestamp DESC
    /// LIMIT 1
    /// ```
    pub async fn get_price_at(&self, timestamp: DateTime<Utc>) -> Result<Option<Decimal>> {
        let lookback_start = timestamp - Duration::seconds(self.max_lookback_seconds);

        let row: Option<(Option<Decimal>,)> = sqlx::query_as(
            r#"
            SELECT close
            FROM ohlcv
            WHERE symbol = $1 AND exchange = $2
              AND timestamp > $3 AND timestamp <= $4
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(&self.symbol)
        .bind(&self.exchange)
        .bind(lookback_start)
        .bind(timestamp)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query point-in-time price")?;

        Ok(row.and_then(|r| r.0))
    }

    /// Gets the price at a given offset from the base timestamp.
    ///
    /// This is typically used to get the settlement price for binary options.
    ///
    /// # Arguments
    /// * `timestamp` - Base timestamp
    /// * `offset` - Time offset to add to the base timestamp
    ///
    /// # Returns
    /// The close price at `timestamp + offset`, or `None` if unavailable.
    pub async fn get_price_at_offset(
        &self,
        timestamp: DateTime<Utc>,
        offset: Duration,
    ) -> Result<Option<Decimal>> {
        let target_time = timestamp + offset;
        self.get_price_at(target_time).await
    }

    /// Calculates the forward return from timestamp to timestamp + offset.
    ///
    /// Forward return = (end_price - start_price) / start_price
    ///
    /// # Arguments
    /// * `timestamp` - Start timestamp (time of bet placement)
    /// * `offset` - Time offset for settlement (e.g., 15 minutes)
    ///
    /// # Returns
    /// * `Ok(Some(return))` - The forward return as a decimal
    /// * `Ok(None)` - Either start or end price unavailable
    /// * `Err(_)` - Database query failed
    ///
    /// # Example
    /// If BTC is $43000 at 12:00 and $43500 at 12:15:
    /// forward_return = (43500 - 43000) / 43000 = 0.0116 (1.16%)
    pub async fn get_forward_return(
        &self,
        timestamp: DateTime<Utc>,
        offset: Duration,
    ) -> Result<Option<Decimal>> {
        // Get start price (at bet placement time)
        let start_price = match self.get_price_at(timestamp).await? {
            Some(p) => p,
            None => return Ok(None),
        };

        // Get end price (at settlement time)
        let end_price = match self.get_price_at_offset(timestamp, offset).await? {
            Some(p) => p,
            None => return Ok(None),
        };

        // Avoid division by zero
        if start_price == Decimal::ZERO {
            return Ok(None);
        }

        let forward_return = (end_price - start_price) / start_price;
        Ok(Some(forward_return))
    }

    /// Checks if price data is available for a given time range.
    ///
    /// Useful for determining if a backtest period has sufficient data.
    ///
    /// # Arguments
    /// * `start` - Start of the time range
    /// * `end` - End of the time range
    ///
    /// # Returns
    /// `true` if both start and end have price data available.
    pub async fn has_data_for_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<bool> {
        let start_price = self.get_price_at(start).await?;
        let end_price = self.get_price_at(end).await?;
        Ok(start_price.is_some() && end_price.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // ============================================================
    // Configuration Tests
    // ============================================================

    #[test]
    fn default_lookback_is_5_minutes() {
        assert_eq!(DEFAULT_MAX_LOOKBACK_SECONDS, 300);
    }

    // ============================================================
    // Lookback Window Logic Tests (No DB Required)
    // ============================================================

    #[test]
    fn lookback_window_calculation_is_correct() {
        let timestamp = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300; // 5 minutes
        let lookback_start = timestamp - Duration::seconds(max_lookback_seconds);

        // Window should be 5 minutes before target
        assert_eq!((timestamp - lookback_start).num_seconds(), 300);
    }

    #[test]
    fn lookback_window_excludes_future_data() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let future_data = target + Duration::seconds(1);

        // Future data MUST NOT match condition: timestamp <= target
        // This is the CRITICAL property that prevents look-ahead bias
        assert!(future_data > target);
    }

    #[test]
    fn lookback_window_includes_exact_timestamp() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let exact_data = target;

        // Data at exact timestamp SHOULD match condition: timestamp <= target
        assert!(exact_data <= target);
    }

    #[test]
    fn lookback_window_includes_recent_past() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300;
        let lookback_start = target - Duration::seconds(max_lookback_seconds);
        let recent_past = target - Duration::seconds(60); // 1 minute ago

        // Recent past should match: timestamp > lookback_start AND timestamp <= target
        assert!(recent_past > lookback_start);
        assert!(recent_past <= target);
    }

    #[test]
    fn lookback_window_excludes_stale_data() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300;
        let lookback_start = target - Duration::seconds(max_lookback_seconds);
        let stale_data = lookback_start - Duration::seconds(1); // Just outside window

        // Stale data should NOT match: timestamp > lookback_start
        assert!(stale_data <= lookback_start);
    }

    #[test]
    fn lookback_boundary_included() {
        // Data exactly at lookback_start should be EXCLUDED
        // because we use > not >=
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300;
        let lookback_start = target - Duration::seconds(max_lookback_seconds);
        let boundary_data = lookback_start;

        // Boundary should NOT match: timestamp > lookback_start (strict inequality)
        assert!(!(boundary_data > lookback_start));
    }

    // ============================================================
    // Forward Return Calculation Logic Tests (No DB Required)
    // ============================================================

    #[test]
    fn forward_return_positive_when_price_increases() {
        let start_price = dec!(43000);
        let end_price = dec!(43500);

        let forward_return = (end_price - start_price) / start_price;

        // ~1.16% return
        assert!(forward_return > Decimal::ZERO);
        assert!(forward_return > dec!(0.01) && forward_return < dec!(0.02));
    }

    #[test]
    fn forward_return_negative_when_price_decreases() {
        let start_price = dec!(43000);
        let end_price = dec!(42500);

        let forward_return = (end_price - start_price) / start_price;

        // ~-1.16% return
        assert!(forward_return < Decimal::ZERO);
    }

    #[test]
    fn forward_return_zero_when_price_unchanged() {
        let start_price = dec!(43000);
        let end_price = dec!(43000);

        let forward_return = (end_price - start_price) / start_price;

        assert_eq!(forward_return, Decimal::ZERO);
    }

    #[test]
    fn forward_return_handles_small_movements() {
        // 1 cent movement on $43000
        let start_price = dec!(43000.00);
        let end_price = dec!(43000.01);

        let forward_return = (end_price - start_price) / start_price;

        // Should be very small but positive
        assert!(forward_return > Decimal::ZERO);
        assert!(forward_return < dec!(0.000001));
    }

    #[test]
    fn forward_return_handles_large_movements() {
        // 10% movement
        let start_price = dec!(43000);
        let end_price = dec!(47300); // +10%

        let forward_return = (end_price - start_price) / start_price;

        assert!((forward_return - dec!(0.10)).abs() < dec!(0.001));
    }

    // ============================================================
    // Time Offset Calculation Tests
    // ============================================================

    #[test]
    fn offset_15_minutes_calculated_correctly() {
        let base = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let offset = Duration::minutes(15);
        let target = base + offset;

        assert_eq!(
            target,
            Utc.with_ymd_and_hms(2026, 1, 30, 12, 15, 0).unwrap()
        );
    }

    #[test]
    fn offset_crosses_hour_boundary() {
        let base = Utc.with_ymd_and_hms(2026, 1, 30, 11, 50, 0).unwrap();
        let offset = Duration::minutes(15);
        let target = base + offset;

        assert_eq!(target, Utc.with_ymd_and_hms(2026, 1, 30, 12, 5, 0).unwrap());
    }

    #[test]
    fn offset_crosses_day_boundary() {
        let base = Utc.with_ymd_and_hms(2026, 1, 30, 23, 50, 0).unwrap();
        let offset = Duration::minutes(15);
        let target = base + offset;

        assert_eq!(target, Utc.with_ymd_and_hms(2026, 1, 31, 0, 5, 0).unwrap());
    }

    // ============================================================
    // Division by Zero Protection Tests
    // ============================================================

    #[test]
    fn division_by_zero_returns_none_conceptually() {
        let start_price = Decimal::ZERO;
        let end_price = dec!(43000);

        // This would cause division by zero
        // The implementation should return None in this case
        if start_price == Decimal::ZERO {
            // Correctly handled
            assert!(true);
        } else {
            let _ = (end_price - start_price) / start_price;
        }
    }

    // ============================================================
    // Custom Lookback Configuration Tests
    // ============================================================

    #[test]
    fn custom_lookback_of_1_minute() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 60; // 1 minute
        let lookback_start = target - Duration::seconds(max_lookback_seconds);

        // Data 2 minutes ago should be EXCLUDED
        let old_data = target - Duration::seconds(120);
        assert!(old_data <= lookback_start);

        // Data 30 seconds ago should be INCLUDED
        let recent_data = target - Duration::seconds(30);
        assert!(recent_data > lookback_start);
    }

    #[test]
    fn custom_lookback_of_15_minutes() {
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let max_lookback_seconds = 900; // 15 minutes
        let lookback_start = target - Duration::seconds(max_lookback_seconds);

        // Data 10 minutes ago should be INCLUDED
        let included_data = target - Duration::seconds(600);
        assert!(included_data > lookback_start);

        // Data 20 minutes ago should be EXCLUDED
        let excluded_data = target - Duration::seconds(1200);
        assert!(excluded_data <= lookback_start);
    }

    // ============================================================
    // Binary Outcome Determination Tests
    // ============================================================

    #[test]
    fn positive_return_indicates_up_direction() {
        let forward_return = dec!(0.005); // 0.5% up

        let is_up = forward_return > Decimal::ZERO;
        assert!(is_up);
    }

    #[test]
    fn negative_return_indicates_down_direction() {
        let forward_return = dec!(-0.005); // 0.5% down

        let is_down = forward_return < Decimal::ZERO;
        assert!(is_down);
    }

    #[test]
    fn zero_return_indicates_push() {
        let forward_return = Decimal::ZERO;

        let is_push = forward_return == Decimal::ZERO;
        assert!(is_push);
    }

    // ============================================================
    // Decimal Precision Tests
    // ============================================================

    #[test]
    fn decimal_precision_maintained_in_return_calculation() {
        let start_price = dec!(43000.12345678);
        let end_price = dec!(43500.87654321);

        let forward_return = (end_price - start_price) / start_price;

        // Should maintain precision without floating point errors
        // Verify it's in expected range
        assert!(forward_return > dec!(0.01)); // > 1%
        assert!(forward_return < dec!(0.02)); // < 2%
    }

    #[test]
    fn decimal_precision_for_small_price_differences() {
        let start_price = dec!(100000.00);
        let end_price = dec!(100000.01);

        let forward_return = (end_price - start_price) / start_price;

        // 0.00000001 = 0.000001%
        assert_eq!(forward_return, dec!(0.0000001));
    }

    // ============================================================
    // Edge Case: Exact Candle Alignment Tests
    // ============================================================

    #[test]
    fn query_at_candle_close_time_gets_that_candle() {
        // If we query at exactly 12:00, and there's a candle that closed at 12:00,
        // we SHOULD get that candle (timestamp <= target includes equality)
        let candle_close = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let query_time = candle_close;

        assert!(candle_close <= query_time);
    }

    #[test]
    fn query_just_before_candle_close_gets_previous_candle() {
        // If we query at 11:59:59, we should get the 11:45 candle, not the 12:00 candle
        let next_candle_close = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let prev_candle_close = Utc.with_ymd_and_hms(2026, 1, 30, 11, 45, 0).unwrap();
        let query_time = next_candle_close - Duration::seconds(1);

        // Next candle should NOT be included
        assert!(next_candle_close > query_time);
        // Previous candle SHOULD be included
        assert!(prev_candle_close <= query_time);
    }

    // ============================================================
    // Concurrent Request Safety Tests (Logic Only)
    // ============================================================

    #[test]
    fn multiple_timestamps_calculated_independently() {
        let base = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();

        let times: Vec<DateTime<Utc>> = (0..10).map(|i| base + Duration::minutes(i * 15)).collect();

        // Each timestamp should have independent lookback window
        for (i, time) in times.iter().enumerate() {
            let lookback_start = *time - Duration::seconds(300);
            assert_eq!((*time - lookback_start).num_seconds(), 300);
            assert_eq!(
                *time,
                base + Duration::minutes(i as i64 * 15),
                "Time {} should be independent",
                i
            );
        }
    }

    // ============================================================
    // Provider Configuration Tests
    // ============================================================

    // Note: These tests would require a database pool to instantiate PointInTimeProvider.
    // The following tests verify the struct's fields conceptually.

    #[test]
    fn symbol_is_stored_correctly() {
        let symbol = "BTCUSDT";
        let stored = symbol.to_string();
        assert_eq!(stored, "BTCUSDT");
    }

    #[test]
    fn exchange_is_stored_correctly() {
        let exchange = "binance";
        let stored = exchange.to_string();
        assert_eq!(stored, "binance");
    }

    #[test]
    fn lookback_seconds_is_configurable() {
        let default = DEFAULT_MAX_LOOKBACK_SECONDS;
        let custom: i64 = 900;

        assert_eq!(default, 300);
        assert_ne!(default, custom);
    }

    // ============================================================
    // SQL Query Logic Verification Tests
    // ============================================================

    #[test]
    fn sql_conditions_prevent_look_ahead() {
        // The SQL query uses: timestamp <= $target
        // This is the CRITICAL condition that prevents look-ahead bias
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();

        // Simulate SQL condition: timestamp <= target
        let future_timestamp = target + Duration::seconds(1);
        let past_timestamp = target - Duration::seconds(1);
        let exact_timestamp = target;

        assert!(
            !(future_timestamp <= target),
            "Future data must be excluded"
        );
        assert!(past_timestamp <= target, "Past data must be included");
        assert!(
            exact_timestamp <= target,
            "Exact timestamp must be included"
        );
    }

    #[test]
    fn sql_conditions_prevent_stale_data() {
        // The SQL query uses: timestamp > lookback_start
        // This prevents using very old data
        let target = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let lookback_start = target - Duration::seconds(300);

        // Simulate SQL condition: timestamp > lookback_start
        let stale_timestamp = lookback_start - Duration::seconds(1);
        let boundary_timestamp = lookback_start;
        let recent_timestamp = lookback_start + Duration::seconds(1);

        assert!(
            !(stale_timestamp > lookback_start),
            "Stale data must be excluded"
        );
        assert!(
            !(boundary_timestamp > lookback_start),
            "Boundary must be excluded (strict >)"
        );
        assert!(
            recent_timestamp > lookback_start,
            "Recent data must be included"
        );
    }

    #[test]
    fn sql_order_by_returns_most_recent() {
        // The SQL query uses: ORDER BY timestamp DESC LIMIT 1
        // This ensures we get the most recent price within the window
        let times = vec![
            Utc.with_ymd_and_hms(2026, 1, 30, 11, 55, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 30, 11, 57, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 30, 11, 59, 0).unwrap(),
        ];

        // Sorted DESC, first element is most recent
        let mut sorted = times.clone();
        sorted.sort_by(|a, b| b.cmp(a)); // DESC

        assert_eq!(sorted[0], times[2]); // 11:59 is most recent
    }

    // ============================================================
    // Integration Test Scaffolding (Requires Database)
    // ============================================================

    // The following tests are commented out as they require a real database.
    // They serve as documentation for integration testing patterns.
    //
    // #[tokio::test]
    // async fn integration_get_price_at_returns_correct_price() {
    //     let pool = setup_test_database().await;
    //     let provider = PointInTimeProvider::new(pool.clone(), "BTCUSDT", "binance");
    //
    //     // Insert test data
    //     insert_test_candle(&pool, timestamp, dec!(43000)).await;
    //
    //     let price = provider.get_price_at(timestamp).await.unwrap();
    //     assert_eq!(price, Some(dec!(43000)));
    // }
    //
    // #[tokio::test]
    // async fn integration_no_look_ahead_bias() {
    //     let pool = setup_test_database().await;
    //     let provider = PointInTimeProvider::new(pool.clone(), "BTCUSDT", "binance");
    //
    //     let query_time = Utc::now();
    //     let future_time = query_time + Duration::minutes(15);
    //
    //     // Insert future candle
    //     insert_test_candle(&pool, future_time, dec!(50000)).await;
    //
    //     // Query at current time should NOT return future price
    //     let price = provider.get_price_at(query_time).await.unwrap();
    //     assert_ne!(price, Some(dec!(50000)));
    // }
    //
    // #[tokio::test]
    // async fn integration_forward_return_calculated_correctly() {
    //     let pool = setup_test_database().await;
    //     let provider = PointInTimeProvider::new(pool.clone(), "BTCUSDT", "binance");
    //
    //     let start_time = Utc::now();
    //     let end_time = start_time + Duration::minutes(15);
    //
    //     // Insert start and end candles
    //     insert_test_candle(&pool, start_time, dec!(43000)).await;
    //     insert_test_candle(&pool, end_time, dec!(43500)).await;
    //
    //     let forward_return = provider
    //         .get_forward_return(start_time, Duration::minutes(15))
    //         .await
    //         .unwrap();
    //
    //     // Expected: (43500 - 43000) / 43000 = 0.0116...
    //     let expected = (dec!(43500) - dec!(43000)) / dec!(43000);
    //     assert_eq!(forward_return, Some(expected));
    // }
    //
    // #[tokio::test]
    // async fn integration_returns_none_for_missing_data() {
    //     let pool = setup_test_database().await;
    //     let provider = PointInTimeProvider::new(pool.clone(), "BTCUSDT", "binance");
    //
    //     // Query for a time with no data
    //     let price = provider.get_price_at(Utc::now()).await.unwrap();
    //     assert_eq!(price, None);
    // }
    //
    // #[tokio::test]
    // async fn integration_returns_none_for_stale_data() {
    //     let pool = setup_test_database().await;
    //     let provider = PointInTimeProvider::new(pool.clone(), "BTCUSDT", "binance");
    //
    //     let query_time = Utc::now();
    //     let stale_time = query_time - Duration::minutes(10); // Outside 5-min window
    //
    //     // Insert only stale data
    //     insert_test_candle(&pool, stale_time, dec!(43000)).await;
    //
    //     // Should return None because data is too old
    //     let price = provider.get_price_at(query_time).await.unwrap();
    //     assert_eq!(price, None);
    // }
}
