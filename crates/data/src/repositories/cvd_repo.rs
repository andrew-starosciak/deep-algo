//! CVD (Cumulative Volume Delta) aggregate repository.
//!
//! Provides operations for storing and querying pre-computed CVD aggregates
//! used in divergence signal detection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::CvdAggregateRecord;

/// Repository for CVD aggregate operations.
///
/// Stores pre-computed CVD values over configurable time windows
/// for efficient signal computation.
#[derive(Debug, Clone)]
pub struct CvdRepository {
    pool: PgPool,
}

impl CvdRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single CVD aggregate.
    ///
    /// Uses upsert semantics to update existing aggregates.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &CvdAggregateRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO cvd_aggregates
                (timestamp, symbol, exchange, window_seconds,
                 buy_volume, sell_volume, cvd, trade_count, avg_price, close_price)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (timestamp, symbol, exchange, window_seconds)
            DO UPDATE SET
                buy_volume = EXCLUDED.buy_volume,
                sell_volume = EXCLUDED.sell_volume,
                cvd = EXCLUDED.cvd,
                trade_count = EXCLUDED.trade_count,
                avg_price = EXCLUDED.avg_price,
                close_price = EXCLUDED.close_price
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(record.window_seconds)
        .bind(record.buy_volume)
        .bind(record.sell_volume)
        .bind(record.cvd)
        .bind(record.trade_count)
        .bind(record.avg_price)
        .bind(record.close_price)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of CVD aggregates.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[CvdAggregateRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for record in records {
            sqlx::query(
                r#"
                INSERT INTO cvd_aggregates
                    (timestamp, symbol, exchange, window_seconds,
                     buy_volume, sell_volume, cvd, trade_count, avg_price, close_price)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (timestamp, symbol, exchange, window_seconds)
                DO UPDATE SET
                    buy_volume = EXCLUDED.buy_volume,
                    sell_volume = EXCLUDED.sell_volume,
                    cvd = EXCLUDED.cvd,
                    trade_count = EXCLUDED.trade_count,
                    avg_price = EXCLUDED.avg_price,
                    close_price = EXCLUDED.close_price
                "#,
            )
            .bind(record.timestamp)
            .bind(&record.symbol)
            .bind(&record.exchange)
            .bind(record.window_seconds)
            .bind(record.buy_volume)
            .bind(record.sell_volume)
            .bind(record.cvd)
            .bind(record.trade_count)
            .bind(record.avg_price)
            .bind(record.close_price)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries CVD aggregates for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        window_seconds: i32,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CvdAggregateRecord>> {
        let records = sqlx::query_as::<_, CvdAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_seconds,
                   buy_volume, sell_volume, cvd, trade_count, avg_price, close_price
            FROM cvd_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_seconds = $3
              AND timestamp >= $4 AND timestamp <= $5
            ORDER BY timestamp ASC
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_seconds)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets the most recent CVD aggregate for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest(
        &self,
        symbol: &str,
        exchange: &str,
        window_seconds: i32,
    ) -> Result<Option<CvdAggregateRecord>> {
        let record = sqlx::query_as::<_, CvdAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_seconds,
                   buy_volume, sell_volume, cvd, trade_count, avg_price, close_price
            FROM cvd_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_seconds = $3
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_seconds)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Gets the N most recent CVD aggregates for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest_n(
        &self,
        symbol: &str,
        exchange: &str,
        window_seconds: i32,
        n: i64,
    ) -> Result<Vec<CvdAggregateRecord>> {
        let records = sqlx::query_as::<_, CvdAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_seconds,
                   buy_volume, sell_volume, cvd, trade_count, avg_price, close_price
            FROM cvd_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_seconds = $3
            ORDER BY timestamp DESC
            LIMIT $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_seconds)
        .bind(n)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Deletes CVD aggregates before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cvd_aggregates
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Calculates cumulative CVD for a time range.
    ///
    /// Returns the running sum of CVD values for divergence analysis.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_cumulative_cvd(
        &self,
        symbol: &str,
        exchange: &str,
        window_seconds: i32,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Decimal> {
        let result: (Option<Decimal>,) = sqlx::query_as(
            r#"
            SELECT COALESCE(SUM(cvd), 0)
            FROM cvd_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_seconds = $3
              AND timestamp >= $4 AND timestamp <= $5
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_seconds)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0.unwrap_or(Decimal::ZERO))
    }

    /// Queries aggregates where CVD exceeds a threshold (for extreme conditions).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_extreme_cvd(
        &self,
        symbol: &str,
        exchange: &str,
        window_seconds: i32,
        threshold: Decimal,
        limit: i64,
    ) -> Result<Vec<CvdAggregateRecord>> {
        let records = sqlx::query_as::<_, CvdAggregateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, window_seconds,
                   buy_volume, sell_volume, cvd, trade_count, avg_price, close_price
            FROM cvd_aggregates
            WHERE symbol = $1 AND exchange = $2 AND window_seconds = $3
              AND ABS(cvd) >= $4
            ORDER BY timestamp DESC
            LIMIT $5
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(window_seconds)
        .bind(threshold)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TradeSide, TradeTickRecord};
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // =========================================================================
    // Repository Structure Tests
    // =========================================================================

    #[test]
    fn test_repository_new() {
        // Verify repository struct has expected size (contains PgPool)
        assert!(std::mem::size_of::<CvdRepository>() > 0);
    }

    // =========================================================================
    // CvdAggregateRecord Tests
    // =========================================================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    fn make_trade(quantity: Decimal, price: Decimal, side: TradeSide) -> TradeTickRecord {
        TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            price,
            quantity,
            side,
        )
    }

    #[test]
    fn test_cvd_aggregate_record_fields() {
        let agg = CvdAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_seconds: 60,
            buy_volume: dec!(10.5),
            sell_volume: dec!(8.3),
            cvd: dec!(2.2),
            trade_count: 150,
            avg_price: Some(dec!(50100.50)),
            close_price: Some(dec!(50150.00)),
        };

        assert_eq!(agg.symbol, "BTCUSDT");
        assert_eq!(agg.window_seconds, 60);
        assert_eq!(agg.total_volume(), dec!(18.8));
        assert!(agg.is_buy_dominant());
    }

    #[test]
    fn test_cvd_aggregate_from_trades() {
        let trades = vec![
            make_trade(dec!(2.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.5), dec!(50100), TradeSide::Sell),
            make_trade(dec!(0.5), dec!(50050), TradeSide::Buy),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_volume, dec!(2.5)); // 2.0 + 0.5
        assert_eq!(agg.sell_volume, dec!(1.5));
        assert_eq!(agg.cvd, dec!(1.0)); // 2.5 - 1.5
        assert_eq!(agg.trade_count, 3);
        assert!(agg.close_price.is_some());
        assert_eq!(agg.close_price.unwrap(), dec!(50050)); // Last trade price
    }

    #[test]
    fn test_cvd_aggregate_empty() {
        let agg = CvdAggregateRecord::empty(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
        );

        assert_eq!(agg.buy_volume, Decimal::ZERO);
        assert_eq!(agg.sell_volume, Decimal::ZERO);
        assert_eq!(agg.cvd, Decimal::ZERO);
        assert_eq!(agg.trade_count, 0);
        assert!(agg.avg_price.is_none());
        assert!(agg.close_price.is_none());
    }

    #[test]
    fn test_cvd_aggregate_imbalance_ratio() {
        let trades = vec![
            make_trade(dec!(3.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        // (3.0 - 1.0) / (3.0 + 1.0) = 2.0 / 4.0 = 0.5
        assert_eq!(agg.imbalance_ratio(), Some(dec!(0.5)));
    }

    #[test]
    fn test_cvd_aggregate_buy_sell_ratio() {
        let trades = vec![
            make_trade(dec!(2.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_sell_ratio(), Some(dec!(2.0)));
    }

    #[test]
    fn test_cvd_aggregate_serialization() {
        let agg = CvdAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_seconds: 60,
            buy_volume: dec!(10.5),
            sell_volume: dec!(8.3),
            cvd: dec!(2.2),
            trade_count: 150,
            avg_price: Some(dec!(50100.50)),
            close_price: Some(dec!(50150.00)),
        };

        let json = serde_json::to_string(&agg);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("BTCUSDT"));
        assert!(json_str.contains("binance"));
        assert!(json_str.contains("60")); // window_seconds
    }

    // =========================================================================
    // CVD Calculation Helper Function Tests
    // =========================================================================

    #[test]
    fn test_calculate_cumulative_cvd() {
        use crate::models::calculate_cumulative_cvd;

        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(2.0),
                sell_volume: dec!(1.0),
                cvd: dec!(1.0),
                trade_count: 10,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(1.0),
                sell_volume: dec!(2.0),
                cvd: dec!(-1.0),
                trade_count: 10,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(3.0),
                sell_volume: dec!(1.0),
                cvd: dec!(2.0),
                trade_count: 10,
                avg_price: None,
                close_price: None,
            },
        ];

        let cumulative = calculate_cumulative_cvd(&aggregates);

        assert_eq!(cumulative.len(), 3);
        assert_eq!(cumulative[0], dec!(1.0));
        assert_eq!(cumulative[1], dec!(0.0)); // 1.0 + (-1.0)
        assert_eq!(cumulative[2], dec!(2.0)); // 0.0 + 2.0
    }

    #[test]
    fn test_calculate_rolling_cvd() {
        use crate::models::calculate_rolling_cvd;

        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(1.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(1.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(2.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(2.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(3.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(3.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
        ];

        // 2-period rolling sum
        let rolling = calculate_rolling_cvd(&aggregates, 2);

        assert_eq!(rolling.len(), 3);
        assert_eq!(rolling[0], dec!(1.0)); // Just first
        assert_eq!(rolling[1], dec!(3.0)); // 1.0 + 2.0
        assert_eq!(rolling[2], dec!(5.0)); // 2.0 + 3.0
    }

    // =========================================================================
    // Integration test documentation
    // =========================================================================
    // Note: Full integration tests require a running TimescaleDB instance.
    //
    // #[tokio::test]
    // #[ignore]
    // async fn test_insert_and_query_roundtrip() {
    //     let pool = create_test_pool().await;
    //     let repo = CvdRepository::new(pool);
    //
    //     let agg = CvdAggregateRecord::empty(...);
    //     repo.insert(&agg).await.unwrap();
    //
    //     let latest = repo.get_latest("BTCUSDT", "binance", 60).await.unwrap();
    //     assert!(latest.is_some());
    // }
}
