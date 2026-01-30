//! Liquidation repository.
//!
//! Provides operations for both individual liquidation events
//! and rolling window aggregates.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::{LiquidationAggregateRecord, LiquidationRecord};

/// Repository for liquidation operations.
#[derive(Debug, Clone)]
pub struct LiquidationRepository {
    pool: PgPool,
}

impl LiquidationRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // =========================================================================
    // Individual Liquidation Events
    // =========================================================================

    /// Inserts a single liquidation event.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert_event(&self, record: &LiquidationRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO liquidations
                (timestamp, symbol, exchange, side, quantity, price, usd_value)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (timestamp, symbol, exchange, side, price) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(&record.side)
        .bind(record.quantity)
        .bind(record.price)
        .bind(record.usd_value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of liquidation events.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_events_batch(&self, records: &[LiquidationRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO liquidations
                        (timestamp, symbol, exchange, side, quantity, price, usd_value)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (timestamp, symbol, exchange, side, price) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.symbol)
                .bind(&record.exchange)
                .bind(&record.side)
                .bind(record.quantity)
                .bind(record.price)
                .bind(record.usd_value)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries liquidation events for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_events_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<LiquidationRecord>> {
        let records = sqlx::query_as::<_, LiquidationRecord>(
            r#"
            SELECT timestamp, symbol, exchange, side, quantity, price, usd_value
            FROM liquidations
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

    /// Queries large liquidation events above a USD threshold.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_large_events(
        &self,
        symbol: &str,
        exchange: &str,
        min_usd: Decimal,
        limit: i64,
    ) -> Result<Vec<LiquidationRecord>> {
        let records = sqlx::query_as::<_, LiquidationRecord>(
            r#"
            SELECT timestamp, symbol, exchange, side, quantity, price, usd_value
            FROM liquidations
            WHERE symbol = $1 AND exchange = $2
              AND usd_value >= $3
            ORDER BY timestamp DESC
            LIMIT $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(min_usd)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    // =========================================================================
    // Liquidation Aggregates
    // =========================================================================

    /// Inserts a single liquidation aggregate.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert_aggregate(&self, record: &LiquidationAggregateRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO liquidation_aggregates
                (timestamp, symbol, exchange, window_minutes,
                 long_volume, short_volume, net_delta, count_long, count_short)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (timestamp, symbol, exchange, window_minutes)
            DO UPDATE SET
                long_volume = EXCLUDED.long_volume,
                short_volume = EXCLUDED.short_volume,
                net_delta = EXCLUDED.net_delta,
                count_long = EXCLUDED.count_long,
                count_short = EXCLUDED.count_short
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(record.window_minutes)
        .bind(record.long_volume)
        .bind(record.short_volume)
        .bind(record.net_delta)
        .bind(record.count_long)
        .bind(record.count_short)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of liquidation aggregates.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_aggregates_batch(
        &self,
        records: &[LiquidationAggregateRecord],
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for record in records {
            sqlx::query(
                r#"
                INSERT INTO liquidation_aggregates
                    (timestamp, symbol, exchange, window_minutes,
                     long_volume, short_volume, net_delta, count_long, count_short)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (timestamp, symbol, exchange, window_minutes)
                DO UPDATE SET
                    long_volume = EXCLUDED.long_volume,
                    short_volume = EXCLUDED.short_volume,
                    net_delta = EXCLUDED.net_delta,
                    count_long = EXCLUDED.count_long,
                    count_short = EXCLUDED.count_short
                "#,
            )
            .bind(record.timestamp)
            .bind(&record.symbol)
            .bind(&record.exchange)
            .bind(record.window_minutes)
            .bind(record.long_volume)
            .bind(record.short_volume)
            .bind(record.net_delta)
            .bind(record.count_long)
            .bind(record.count_short)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries aggregates for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_aggregates_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        window_minutes: i32,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<LiquidationAggregateRecord>> {
        let records = sqlx::query_as::<_, LiquidationAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_minutes,
                   long_volume, short_volume, net_delta, count_long, count_short
            FROM liquidation_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_minutes = $3
              AND timestamp >= $4 AND timestamp <= $5
            ORDER BY timestamp ASC
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_minutes)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets the latest aggregate for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest_aggregate(
        &self,
        symbol: &str,
        exchange: &str,
        window_minutes: i32,
    ) -> Result<Option<LiquidationAggregateRecord>> {
        let record = sqlx::query_as::<_, LiquidationAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_minutes,
                   long_volume, short_volume, net_delta, count_long, count_short
            FROM liquidation_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_minutes = $3
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_minutes)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Queries aggregates with cascade conditions.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_cascade_events(
        &self,
        symbol: &str,
        exchange: &str,
        window_minutes: i32,
        volume_threshold: Decimal,
        limit: i64,
    ) -> Result<Vec<LiquidationAggregateRecord>> {
        let records = sqlx::query_as::<_, LiquidationAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_minutes,
                   long_volume, short_volume, net_delta, count_long, count_short
            FROM liquidation_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_minutes = $3
              AND (long_volume >= $4 OR short_volume >= $4)
            ORDER BY timestamp DESC
            LIMIT $5
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_minutes)
        .bind(volume_threshold)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    // =========================================================================
    // Cleanup
    // =========================================================================

    /// Deletes old liquidation events before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_events_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM liquidations
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Deletes old aggregates before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_aggregates_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM liquidation_aggregates
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LiquidationSide;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn test_repository_new() {
        assert!(std::mem::size_of::<LiquidationRepository>() > 0);
    }

    #[test]
    fn test_liquidation_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = LiquidationRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            LiquidationSide::Long,
            dec!(1.5),
            dec!(50000),
        );

        assert_eq!(record.usd_value, dec!(75000));
        assert!(record.is_long());

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_aggregate_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = LiquidationAggregateRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(100000),
            short_volume: dec!(50000),
            net_delta: dec!(50000),
            count_long: 10,
            count_short: 5,
        };

        assert_eq!(record.total_volume(), dec!(150000));

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_aggregate_from_liquidations() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let liquidations = vec![
            LiquidationRecord::new(
                timestamp,
                "BTCUSDT".to_string(),
                "binance".to_string(),
                LiquidationSide::Long,
                dec!(1.0),
                dec!(50000),
            ),
            LiquidationRecord::new(
                timestamp,
                "BTCUSDT".to_string(),
                "binance".to_string(),
                LiquidationSide::Short,
                dec!(0.5),
                dec!(50000),
            ),
        ];

        let agg = LiquidationAggregateRecord::from_liquidations(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            5,
            &liquidations,
        );

        assert_eq!(agg.long_volume, dec!(50000));
        assert_eq!(agg.short_volume, dec!(25000));
        assert_eq!(agg.count_long, 1);
        assert_eq!(agg.count_short, 1);
    }
}
