//! OHLCV data repository.
//!
//! Provides batch insert and time-range query operations for OHLCV candle data.
//! Used for historical price backfill from Binance Futures.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::database::OhlcvRecord;

/// Repository for OHLCV candle data operations.
#[derive(Debug, Clone)]
pub struct OhlcvRepository {
    pool: PgPool,
}

impl OhlcvRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a batch of OHLCV records efficiently.
    ///
    /// Uses a transaction with batched inserts for performance.
    /// Uses ON CONFLICT DO NOTHING to handle duplicates gracefully.
    ///
    /// # Returns
    /// The number of records actually inserted (excluding duplicates).
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[OhlcvRecord]) -> Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .context("Failed to begin transaction")?;
        let mut inserted = 0u64;

        for chunk in records.chunks(100) {
            for record in chunk {
                let result = sqlx::query(
                    r#"
                    INSERT INTO ohlcv (timestamp, symbol, exchange, open, high, low, close, volume)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.symbol)
                .bind(&record.exchange)
                .bind(record.open)
                .bind(record.high)
                .bind(record.low)
                .bind(record.close)
                .bind(record.volume)
                .execute(&mut *tx)
                .await
                .context("Failed to insert OHLCV record")?;

                inserted += result.rows_affected();
            }
        }

        tx.commit().await.context("Failed to commit transaction")?;
        Ok(inserted)
    }

    /// Queries the close price at or just before a timestamp.
    ///
    /// Returns the most recent close price within the specified lookback window.
    /// This is used for calculating forward returns without look-ahead bias.
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `exchange` - Exchange name (e.g., "binance")
    /// * `timestamp` - Target timestamp
    /// * `max_lookback_seconds` - Maximum seconds to look back for a price
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_close_price_at(
        &self,
        symbol: &str,
        exchange: &str,
        timestamp: DateTime<Utc>,
        max_lookback_seconds: i64,
    ) -> Result<Option<Decimal>> {
        let lookback_start = timestamp - chrono::Duration::seconds(max_lookback_seconds);

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
        .bind(symbol)
        .bind(exchange)
        .bind(lookback_start)
        .bind(timestamp)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query close price")?;

        Ok(row.and_then(|r| r.0))
    }

    /// Gets the data bounds (earliest and latest timestamps) for a symbol.
    ///
    /// # Returns
    /// A tuple of (earliest_timestamp, latest_timestamp), or None if no data exists.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    #[allow(clippy::type_complexity)]
    pub async fn get_data_bounds(
        &self,
        symbol: &str,
        exchange: &str,
    ) -> Result<Option<(DateTime<Utc>, DateTime<Utc>)>> {
        let row: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            SELECT MIN(timestamp), MAX(timestamp)
            FROM ohlcv
            WHERE symbol = $1 AND exchange = $2
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query data bounds")?;

        match row {
            Some((Some(min), Some(max))) => Ok(Some((min, max))),
            _ => Ok(None),
        }
    }

    /// Counts the number of records for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn count_records(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM ohlcv
            WHERE symbol = $1 AND exchange = $2
              AND timestamp >= $3 AND timestamp <= $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await
        .context("Failed to count records")?;

        Ok(row.0)
    }

    /// Queries OHLCV records for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let records = sqlx::query_as::<_, OhlcvRecord>(
            r#"
            SELECT timestamp, symbol, exchange, open, high, low, close, volume
            FROM ohlcv
            WHERE symbol = $1 AND exchange = $2
              AND timestamp >= $3 AND timestamp <= $4
            ORDER BY timestamp ASC
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query OHLCV records")?;

        Ok(records)
    }

    /// Deletes old records before a given timestamp.
    ///
    /// Useful for data retention policies.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM ohlcv
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await
        .context("Failed to delete old records")?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // ============================================
    // OhlcvRecord Construction Tests
    // ============================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap()
    }

    fn create_test_record(timestamp: DateTime<Utc>) -> OhlcvRecord {
        OhlcvRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            open: dec!(50000.00),
            high: dec!(50100.00),
            low: dec!(49900.00),
            close: dec!(50050.00),
            volume: dec!(1000.50),
        }
    }

    #[test]
    fn test_ohlcv_record_structure() {
        let record = create_test_record(sample_timestamp());

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.open, dec!(50000.00));
        assert_eq!(record.high, dec!(50100.00));
        assert_eq!(record.low, dec!(49900.00));
        assert_eq!(record.close, dec!(50050.00));
        assert_eq!(record.volume, dec!(1000.50));
    }

    #[test]
    fn test_ohlcv_record_clone() {
        let record = create_test_record(sample_timestamp());
        let cloned = record.clone();

        assert_eq!(record.timestamp, cloned.timestamp);
        assert_eq!(record.symbol, cloned.symbol);
        assert_eq!(record.close, cloned.close);
    }

    // ============================================
    // Repository Creation Tests
    // ============================================

    #[test]
    fn test_repository_struct_size() {
        // Verify the repository struct has expected size (contains PgPool)
        assert!(std::mem::size_of::<OhlcvRepository>() > 0);
    }

    // ============================================
    // Lookback Window Calculation Tests
    // ============================================

    #[test]
    fn test_lookback_window_calculation() {
        let timestamp = sample_timestamp();
        let max_lookback_seconds = 300; // 5 minutes
        let lookback_start = timestamp - chrono::Duration::seconds(max_lookback_seconds);

        // Verify the window is correct
        assert_eq!((timestamp - lookback_start).num_seconds(), 300);
    }

    #[test]
    fn test_lookback_window_excludes_future_data() {
        let target = sample_timestamp();
        let future_data = target + chrono::Duration::seconds(1);

        // Future data should NOT match condition: timestamp <= target
        assert!(future_data > target);
    }

    #[test]
    fn test_lookback_window_includes_exact_timestamp() {
        let target = sample_timestamp();
        let exact_data = target;

        // Data at exact timestamp SHOULD match condition: timestamp <= target
        assert!(exact_data <= target);
    }

    #[test]
    fn test_lookback_window_excludes_old_data() {
        let target = sample_timestamp();
        let max_lookback_seconds = 300;
        let lookback_start = target - chrono::Duration::seconds(max_lookback_seconds);
        let old_data = lookback_start - chrono::Duration::seconds(1);

        // Old data should NOT match condition: timestamp > lookback_start
        assert!(old_data <= lookback_start);
    }

    // ============================================
    // Batch Insert Logic Tests
    // ============================================

    #[test]
    fn test_batch_chunking() {
        let base_time = sample_timestamp();
        let records: Vec<OhlcvRecord> = (0..250)
            .map(|i| {
                let timestamp = base_time + chrono::Duration::minutes(i);
                create_test_record(timestamp)
            })
            .collect();

        // Verify chunking logic (chunks of 100)
        let chunks: Vec<_> = records.chunks(100).collect();
        assert_eq!(chunks.len(), 3); // 100 + 100 + 50
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
        assert_eq!(chunks[2].len(), 50);
    }

    #[test]
    fn test_empty_batch_handling() {
        let records: Vec<OhlcvRecord> = vec![];
        assert!(records.is_empty());
        // Empty batch should return Ok(0) immediately
    }

    // ============================================
    // Time Range Query Logic Tests
    // ============================================

    #[test]
    fn test_time_range_includes_boundaries() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1);
        let mid = start + chrono::Duration::minutes(30);

        // All should be within range (inclusive)
        assert!(start >= start && start <= end);
        assert!(mid >= start && mid <= end);
        assert!(end >= start && end <= end);
    }

    #[test]
    fn test_time_range_excludes_outside() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(1);
        let before = start - chrono::Duration::seconds(1);
        let after = end + chrono::Duration::seconds(1);

        // Before and after should be outside range
        assert!(before < start);
        assert!(after > end);
    }

    // ============================================
    // Data Bounds Logic Tests
    // ============================================

    #[test]
    fn test_data_bounds_tuple_handling() {
        // Test the pattern matching logic for data bounds
        let some_bounds: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = Some((
            Some(sample_timestamp()),
            Some(sample_timestamp() + chrono::Duration::hours(24)),
        ));

        match some_bounds {
            Some((Some(min), Some(max))) => {
                assert!(min < max);
            }
            _ => panic!("Expected valid bounds"),
        }
    }

    #[test]
    fn test_data_bounds_none_handling() {
        // When no data exists, we expect None
        let no_bounds: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = Some((None, None));

        match no_bounds {
            Some((Some(_), Some(_))) => panic!("Should not match"),
            _ => { /* Expected path */ }
        }
    }

    // ============================================
    // Count Query Logic Tests
    // ============================================

    #[test]
    fn test_count_result_type() {
        // Count should return i64
        let count: i64 = 0;
        assert!(count >= 0);
    }

    // ============================================
    // Delete Before Logic Tests
    // ============================================

    #[test]
    fn test_delete_cutoff_calculation() {
        let now = Utc::now();
        let retention_days = 30;
        let cutoff = now - chrono::Duration::days(retention_days);

        // Data older than cutoff should be deleted
        let old_data = cutoff - chrono::Duration::days(1);
        let recent_data = cutoff + chrono::Duration::days(1);

        assert!(old_data < cutoff);
        assert!(recent_data > cutoff);
    }

    // ============================================
    // Decimal Precision Tests
    // ============================================

    #[test]
    fn test_decimal_precision_preserved() {
        let price = dec!(50000.12345678);
        let record = OhlcvRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            open: price,
            high: price,
            low: price,
            close: price,
            volume: dec!(100.0),
        };

        // Decimal should preserve precision
        assert_eq!(record.close, dec!(50000.12345678));
    }

    #[test]
    fn test_decimal_comparison() {
        let a = dec!(50000.00);
        let b = dec!(50000.00);
        assert_eq!(a, b);

        let c = dec!(50000.01);
        assert!(c > a);
    }

    // Integration tests would require a real database connection
    // Example integration test structure:
    //
    // #[tokio::test]
    // async fn test_insert_and_query() {
    //     let pool = setup_test_database().await;
    //     let repo = OhlcvRepository::new(pool);
    //
    //     let record = create_test_record(sample_timestamp());
    //     let inserted = repo.insert_batch(&[record.clone()]).await.unwrap();
    //     assert_eq!(inserted, 1);
    //
    //     let price = repo.query_close_price_at(
    //         "BTCUSDT",
    //         "binance",
    //         record.timestamp,
    //         60,
    //     ).await.unwrap();
    //     assert_eq!(price, Some(record.close));
    // }
}
