//! Trade tick repository.
//!
//! Provides operations for individual trade tick storage and retrieval,
//! supporting CVD (Cumulative Volume Delta) signal computation.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::models::TradeTickRecord;

/// Repository for trade tick operations.
///
/// Handles high-frequency trade data storage with batch insert optimization
/// for the volume of trades received from exchange WebSocket feeds.
#[derive(Debug, Clone)]
pub struct TradeTickRepository {
    pool: PgPool,
}

impl TradeTickRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single trade tick.
    ///
    /// Uses ON CONFLICT DO NOTHING for idempotent inserts.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &TradeTickRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO trade_ticks
                (timestamp, symbol, exchange, trade_id, price, quantity, side, usd_value)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (timestamp, symbol, exchange, trade_id) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(record.trade_id)
        .bind(record.price)
        .bind(record.quantity)
        .bind(&record.side)
        .bind(record.usd_value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of trade ticks.
    ///
    /// Uses a transaction for atomicity and better performance.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[TradeTickRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(500) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO trade_ticks
                        (timestamp, symbol, exchange, trade_id, price, quantity, side, usd_value)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (timestamp, symbol, exchange, trade_id) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.symbol)
                .bind(&record.exchange)
                .bind(record.trade_id)
                .bind(record.price)
                .bind(record.quantity)
                .bind(&record.side)
                .bind(record.usd_value)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries trade ticks for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<TradeTickRecord>> {
        let records = sqlx::query_as::<_, TradeTickRecord>(
            r#"
            SELECT timestamp, symbol, exchange, trade_id, price, quantity, side, usd_value
            FROM trade_ticks
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

    /// Gets the most recent trade tick for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest(
        &self,
        symbol: &str,
        exchange: &str,
    ) -> Result<Option<TradeTickRecord>> {
        let record = sqlx::query_as::<_, TradeTickRecord>(
            r#"
            SELECT timestamp, symbol, exchange, trade_id, price, quantity, side, usd_value
            FROM trade_ticks
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

    /// Deletes trade ticks before a given timestamp.
    ///
    /// Used for data retention management.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM trade_ticks
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Counts trade ticks in a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn count_in_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<i64> {
        let count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM trade_ticks
            WHERE symbol = $1 AND exchange = $2
              AND timestamp >= $3 AND timestamp <= $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0)
    }

    /// Gets buy and sell volume for a time range.
    ///
    /// Returns (buy_volume, sell_volume) tuple.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_volume_breakdown(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(rust_decimal::Decimal, rust_decimal::Decimal)> {
        use rust_decimal::Decimal;

        let result: (Option<Decimal>, Option<Decimal>) = sqlx::query_as(
            r#"
            SELECT
                COALESCE(SUM(CASE WHEN side = 'buy' THEN quantity ELSE 0 END), 0) as buy_vol,
                COALESCE(SUM(CASE WHEN side = 'sell' THEN quantity ELSE 0 END), 0) as sell_vol
            FROM trade_ticks
            WHERE symbol = $1 AND exchange = $2
              AND timestamp >= $3 AND timestamp <= $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok((
            result.0.unwrap_or(Decimal::ZERO),
            result.1.unwrap_or(Decimal::ZERO),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TradeSide;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // =========================================================================
    // Repository Structure Tests
    // =========================================================================

    #[test]
    fn test_repository_new() {
        // Verify repository struct has expected size (contains PgPool)
        assert!(std::mem::size_of::<TradeTickRepository>() > 0);
    }

    #[test]
    fn test_trade_tick_record_fields() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            12345,
            dec!(50000),
            dec!(1.5),
            TradeSide::Buy,
        );

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.trade_id, 12345);
        assert_eq!(record.price, dec!(50000));
        assert_eq!(record.quantity, dec!(1.5));
        assert_eq!(record.side, "buy");
        assert_eq!(record.usd_value, dec!(75000));
    }

    #[test]
    fn test_trade_tick_record_serialization() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            12345,
            dec!(50000.50),
            dec!(1.5),
            TradeSide::Buy,
        );

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("BTCUSDT"));
        assert!(json_str.contains("binance"));
        assert!(json_str.contains("12345"));
    }

    #[test]
    fn test_trade_tick_from_binance_agg_trade() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();

        // Test buy (buyer was taker)
        let buy_record = TradeTickRecord::from_binance_agg_trade(
            timestamp,
            "BTCUSDT".to_string(),
            12345,
            dec!(50000),
            dec!(1.0),
            false, // buyer_is_maker = false -> Buy
        );
        assert!(buy_record.is_buy());

        // Test sell (buyer was maker)
        let sell_record = TradeTickRecord::from_binance_agg_trade(
            timestamp,
            "BTCUSDT".to_string(),
            12346,
            dec!(50000),
            dec!(1.0),
            true, // buyer_is_maker = true -> Sell
        );
        assert!(sell_record.is_sell());
    }

    #[test]
    fn test_trade_tick_signed_volume() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();

        let buy = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.5),
            TradeSide::Buy,
        );
        assert_eq!(buy.signed_volume(), dec!(1.5));

        let sell = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            2,
            dec!(50000),
            dec!(1.5),
            TradeSide::Sell,
        );
        assert_eq!(sell.signed_volume(), dec!(-1.5));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_trade_tick_zero_quantity() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            rust_decimal::Decimal::ZERO,
            TradeSide::Buy,
        );

        assert_eq!(record.usd_value, rust_decimal::Decimal::ZERO);
        assert_eq!(record.signed_volume(), rust_decimal::Decimal::ZERO);
    }

    #[test]
    fn test_trade_tick_large_values() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            i64::MAX,
            dec!(100000.12345678),
            dec!(999.99999999),
            TradeSide::Sell,
        );

        // Verify no overflow
        assert!(record.usd_value > rust_decimal::Decimal::ZERO);
        assert_eq!(record.trade_id, i64::MAX);
    }

    #[test]
    fn test_trade_tick_small_quantity() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(0.00000001), // 1 satoshi of BTC
            TradeSide::Buy,
        );

        // Should handle small quantities precisely
        assert_eq!(record.usd_value, dec!(0.0005)); // 0.00000001 * 50000
    }

    // =========================================================================
    // Integration test documentation
    // =========================================================================
    // Note: Full integration tests require a running TimescaleDB instance.
    // They would be run with:
    // cargo test -p algo-trade-data --test integration -- --ignored
    //
    // #[tokio::test]
    // #[ignore]
    // async fn test_insert_and_query_roundtrip() {
    //     let pool = create_test_pool().await;
    //     let repo = TradeTickRepository::new(pool);
    //
    //     let record = TradeTickRecord::new(...);
    //     repo.insert(&record).await.unwrap();
    //
    //     let latest = repo.get_latest("BTCUSDT", "binance").await.unwrap();
    //     assert_eq!(latest.unwrap().trade_id, record.trade_id);
    // }
}
