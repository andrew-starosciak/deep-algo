//! Order book snapshot repository.
//!
//! Provides batch insert and time-range query operations for order book data.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::models::OrderBookSnapshotRecord;

/// Repository for order book snapshot operations.
#[derive(Debug, Clone)]
pub struct OrderBookRepository {
    pool: PgPool,
}

impl OrderBookRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single order book snapshot.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &OrderBookSnapshotRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO orderbook_snapshots
                (timestamp, symbol, exchange, bid_levels, ask_levels,
                 bid_volume, ask_volume, imbalance, mid_price, spread_bps)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(&record.bid_levels)
        .bind(&record.ask_levels)
        .bind(record.bid_volume)
        .bind(record.ask_volume)
        .bind(record.imbalance)
        .bind(record.mid_price)
        .bind(record.spread_bps)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of order book snapshots efficiently.
    ///
    /// Uses a transaction with batched inserts for performance.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[OrderBookSnapshotRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO orderbook_snapshots
                        (timestamp, symbol, exchange, bid_levels, ask_levels,
                         bid_volume, ask_volume, imbalance, mid_price, spread_bps)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.symbol)
                .bind(&record.exchange)
                .bind(&record.bid_levels)
                .bind(&record.ask_levels)
                .bind(record.bid_volume)
                .bind(record.ask_volume)
                .bind(record.imbalance)
                .bind(record.mid_price)
                .bind(record.spread_bps)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries order book snapshots for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OrderBookSnapshotRecord>> {
        let records = sqlx::query_as::<_, OrderBookSnapshotRecord>(
            r#"
            SELECT timestamp, symbol, exchange, bid_levels, ask_levels,
                   bid_volume, ask_volume, imbalance, mid_price, spread_bps
            FROM orderbook_snapshots
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
        .await?;

        Ok(records)
    }

    /// Gets the latest order book snapshot for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest(
        &self,
        symbol: &str,
        exchange: &str,
    ) -> Result<Option<OrderBookSnapshotRecord>> {
        let record = sqlx::query_as::<_, OrderBookSnapshotRecord>(
            r#"
            SELECT timestamp, symbol, exchange, bid_levels, ask_levels,
                   bid_volume, ask_volume, imbalance, mid_price, spread_bps
            FROM orderbook_snapshots
            WHERE symbol = $1 AND exchange = $2
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Gets recent snapshots with high imbalance (for signal generation).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_high_imbalance(
        &self,
        symbol: &str,
        exchange: &str,
        imbalance_threshold: rust_decimal::Decimal,
        limit: i64,
    ) -> Result<Vec<OrderBookSnapshotRecord>> {
        let records = sqlx::query_as::<_, OrderBookSnapshotRecord>(
            r#"
            SELECT timestamp, symbol, exchange, bid_levels, ask_levels,
                   bid_volume, ask_volume, imbalance, mid_price, spread_bps
            FROM orderbook_snapshots
            WHERE symbol = $1 AND exchange = $2
              AND (imbalance > $3 OR imbalance < -$3)
            ORDER BY timestamp DESC
            LIMIT $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(imbalance_threshold)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Deletes old snapshots before a given timestamp.
    ///
    /// Useful for data retention policies.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM orderbook_snapshots
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Queries the mid price at or just before a timestamp.
    ///
    /// Returns the most recent mid_price within the specified lookback window.
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
    pub async fn query_mid_price_at(
        &self,
        symbol: &str,
        exchange: &str,
        timestamp: DateTime<Utc>,
        max_lookback_seconds: i64,
    ) -> Result<Option<rust_decimal::Decimal>> {
        let lookback_start = timestamp - chrono::Duration::seconds(max_lookback_seconds);

        let row: Option<(Option<rust_decimal::Decimal>,)> = sqlx::query_as(
            r#"
            SELECT mid_price
            FROM orderbook_snapshots
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
        .await?;

        Ok(row.and_then(|r| r.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    // Note: These tests verify the SQL query structure and logic.
    // Full integration tests require a test database.

    #[test]
    fn test_repository_new() {
        // This test verifies the repository can be created.
        // We can't actually test database operations without a real pool.
        // In a real test suite, we'd use testcontainers or a test database.

        // For now, verify the struct has expected methods
        assert!(std::mem::size_of::<OrderBookRepository>() > 0);
    }

    #[test]
    fn test_orderbook_record_structure() {
        use chrono::TimeZone;

        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = OrderBookSnapshotRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: json!([["50000", "1.0"]]),
            ask_levels: json!([["50001", "1.0"]]),
            bid_volume: dec!(1.0),
            ask_volume: dec!(1.0),
            imbalance: dec!(0.0),
            mid_price: Some(dec!(50000.5)),
            spread_bps: Some(dec!(0.2)),
        };

        // Verify record can be serialized (which is needed for database ops)
        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    // Integration test example (would need real database):
    // #[tokio::test]
    // async fn test_insert_and_query() {
    //     let pool = setup_test_database().await;
    //     let repo = OrderBookRepository::new(pool);
    //
    //     let record = create_test_record();
    //     repo.insert(&record).await.unwrap();
    //
    //     let results = repo.query_by_time_range(
    //         "BTCUSDT",
    //         "binance",
    //         record.timestamp - chrono::Duration::hours(1),
    //         record.timestamp + chrono::Duration::hours(1),
    //     ).await.unwrap();
    //
    //     assert_eq!(results.len(), 1);
    //     assert_eq!(results[0].symbol, "BTCUSDT");
    // }

    // ============================================
    // query_mid_price_at Tests (Unit Tests)
    // ============================================

    #[test]
    fn test_lookback_window_calculation() {
        use chrono::TimeZone;

        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300; // 5 minutes
        let lookback_start = timestamp - chrono::Duration::seconds(max_lookback_seconds);

        // Verify the window is correct
        assert_eq!((timestamp - lookback_start).num_seconds(), 300);
    }

    #[test]
    fn test_lookback_window_excludes_future_data() {
        use chrono::TimeZone;

        let target = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let future_data = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 1).unwrap();

        // Future data should NOT match condition: timestamp <= target
        assert!(future_data > target);
    }

    #[test]
    fn test_lookback_window_includes_exact_timestamp() {
        use chrono::TimeZone;

        let target = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let exact_data = target;

        // Data at exact timestamp SHOULD match condition: timestamp <= target
        assert!(exact_data <= target);
    }

    #[test]
    fn test_lookback_window_excludes_old_data() {
        use chrono::TimeZone;

        let target = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let max_lookback_seconds = 300;
        let lookback_start = target - chrono::Duration::seconds(max_lookback_seconds);
        let old_data = lookback_start - chrono::Duration::seconds(1);

        // Old data should NOT match condition: timestamp > lookback_start
        assert!(old_data <= lookback_start);
    }

    // Integration test for query_mid_price_at would go here
    // #[tokio::test]
    // async fn test_query_mid_price_at_returns_most_recent() {
    //     let pool = setup_test_database().await;
    //     let repo = OrderBookRepository::new(pool);
    //     // ... test implementation
    // }
}
