//! Funding rate repository.
//!
//! Provides batch insert and time-range query operations for funding rate data.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::FundingRateRecord;

/// Repository for funding rate operations.
#[derive(Debug, Clone)]
pub struct FundingRateRepository {
    pool: PgPool,
}

impl FundingRateRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single funding rate record.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &FundingRateRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO funding_rates
                (timestamp, symbol, exchange, funding_rate, annual_rate,
                 rate_percentile, rate_zscore)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(record.funding_rate)
        .bind(record.annual_rate)
        .bind(record.rate_percentile)
        .bind(record.rate_zscore)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of funding rate records efficiently.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[FundingRateRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO funding_rates
                        (timestamp, symbol, exchange, funding_rate, annual_rate,
                         rate_percentile, rate_zscore)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.symbol)
                .bind(&record.exchange)
                .bind(record.funding_rate)
                .bind(record.annual_rate)
                .bind(record.rate_percentile)
                .bind(record.rate_zscore)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries funding rates for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<FundingRateRecord>> {
        let records = sqlx::query_as::<_, FundingRateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, funding_rate, annual_rate,
                   rate_percentile, rate_zscore
            FROM funding_rates
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

    /// Gets the latest funding rate for a symbol.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest(
        &self,
        symbol: &str,
        exchange: &str,
    ) -> Result<Option<FundingRateRecord>> {
        let record = sqlx::query_as::<_, FundingRateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, funding_rate, annual_rate,
                   rate_percentile, rate_zscore
            FROM funding_rates
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

    /// Queries extreme funding rates (for signal generation).
    ///
    /// Returns rates where zscore exceeds threshold (positive or negative).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_extreme_rates(
        &self,
        symbol: &str,
        exchange: &str,
        zscore_threshold: Decimal,
        limit: i64,
    ) -> Result<Vec<FundingRateRecord>> {
        let records = sqlx::query_as::<_, FundingRateRecord>(
            r#"
            SELECT timestamp, symbol, exchange, funding_rate, annual_rate,
                   rate_percentile, rate_zscore
            FROM funding_rates
            WHERE symbol = $1 AND exchange = $2
              AND rate_zscore IS NOT NULL
              AND (rate_zscore > $3 OR rate_zscore < -$3)
            ORDER BY timestamp DESC
            LIMIT $4
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(zscore_threshold)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Calculates statistical context for a funding rate.
    ///
    /// Computes percentile and z-score based on historical data.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn calculate_statistics(
        &self,
        symbol: &str,
        exchange: &str,
        funding_rate: Decimal,
        lookback_days: i64,
    ) -> Result<(Option<Decimal>, Option<Decimal>)> {
        let cutoff = Utc::now() - chrono::Duration::days(lookback_days);

        // Get percentile
        let percentile_row: Option<(Option<Decimal>,)> = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*)::DECIMAL / NULLIF(COUNT(*) OVER(), 0)
                 FROM funding_rates
                 WHERE symbol = $1 AND exchange = $2
                   AND timestamp >= $3
                   AND funding_rate <= $4) as percentile
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(cutoff)
        .bind(funding_rate)
        .fetch_optional(&self.pool)
        .await?;

        // Get z-score (requires mean and stddev)
        let stats_row: Option<(Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
            r#"
            SELECT
                AVG(funding_rate) as mean,
                STDDEV_SAMP(funding_rate) as stddev
            FROM funding_rates
            WHERE symbol = $1 AND exchange = $2
              AND timestamp >= $3
            "#,
        )
        .bind(symbol)
        .bind(exchange)
        .bind(cutoff)
        .fetch_optional(&self.pool)
        .await?;

        let percentile = percentile_row.and_then(|r| r.0);

        let zscore = stats_row.and_then(|(mean, stddev)| match (mean, stddev) {
            (Some(m), Some(s)) if s > Decimal::ZERO => Some((funding_rate - m) / s),
            _ => None,
        });

        Ok((percentile, zscore))
    }

    /// Deletes old records before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM funding_rates
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
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn test_repository_new() {
        assert!(std::mem::size_of::<FundingRateRepository>() > 0);
    }

    #[test]
    fn test_funding_rate_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = FundingRateRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.0001),
            annual_rate: Some(dec!(0.1095)),
            rate_percentile: Some(dec!(0.75)),
            rate_zscore: Some(dec!(1.5)),
        };

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_funding_rate_with_statistics() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = FundingRateRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        )
        .with_statistics(dec!(0.85), dec!(2.1));

        assert_eq!(record.rate_percentile, Some(dec!(0.85)));
        assert_eq!(record.rate_zscore, Some(dec!(2.1)));
    }
}
