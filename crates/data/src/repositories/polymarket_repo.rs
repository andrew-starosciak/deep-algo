//! Polymarket odds repository.
//!
//! Provides operations for storing and querying binary market odds.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::PolymarketOddsRecord;

/// Repository for Polymarket odds operations.
#[derive(Debug, Clone)]
pub struct PolymarketOddsRepository {
    pool: PgPool,
}

impl PolymarketOddsRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single odds record.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &PolymarketOddsRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO polymarket_odds
                (timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                 volume_24h, liquidity, end_date)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (timestamp, market_id) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.market_id)
        .bind(&record.question)
        .bind(record.outcome_yes_price)
        .bind(record.outcome_no_price)
        .bind(record.volume_24h)
        .bind(record.liquidity)
        .bind(record.end_date)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of odds records.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[PolymarketOddsRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO polymarket_odds
                        (timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                         volume_24h, liquidity, end_date)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (timestamp, market_id) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.market_id)
                .bind(&record.question)
                .bind(record.outcome_yes_price)
                .bind(record.outcome_no_price)
                .bind(record.volume_24h)
                .bind(record.liquidity)
                .bind(record.end_date)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries odds for a market within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        market_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<PolymarketOddsRecord>> {
        let records = sqlx::query_as::<_, PolymarketOddsRecord>(
            r#"
            SELECT timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                   volume_24h, liquidity, end_date
            FROM polymarket_odds
            WHERE market_id = $1
              AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp ASC
            "#,
        )
        .bind(market_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets the latest odds for a market.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest(&self, market_id: &str) -> Result<Option<PolymarketOddsRecord>> {
        let record = sqlx::query_as::<_, PolymarketOddsRecord>(
            r#"
            SELECT timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                   volume_24h, liquidity, end_date
            FROM polymarket_odds
            WHERE market_id = $1
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(market_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Gets the latest odds for all markets (including expired).
    ///
    /// Note: Use `get_active_markets()` instead to filter out expired markets.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_latest_all(&self) -> Result<Vec<PolymarketOddsRecord>> {
        let records = sqlx::query_as::<_, PolymarketOddsRecord>(
            r#"
            SELECT DISTINCT ON (market_id)
                timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                volume_24h, liquidity, end_date
            FROM polymarket_odds
            ORDER BY market_id, timestamp DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets the latest odds for active (non-expired) markets only.
    ///
    /// Filters to markets where `end_date > NOW()` to prevent trading on expired markets.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_active_markets(&self) -> Result<Vec<PolymarketOddsRecord>> {
        let records = sqlx::query_as::<_, PolymarketOddsRecord>(
            r#"
            SELECT DISTINCT ON (market_id)
                timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                volume_24h, liquidity, end_date
            FROM polymarket_odds
            WHERE end_date IS NOT NULL AND end_date > NOW()
            ORDER BY market_id, timestamp DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries markets with sufficient liquidity.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_liquidity(
        &self,
        min_liquidity: Decimal,
        limit: i64,
    ) -> Result<Vec<PolymarketOddsRecord>> {
        let records = sqlx::query_as::<_, PolymarketOddsRecord>(
            r#"
            SELECT DISTINCT ON (market_id)
                timestamp, market_id, question, outcome_yes_price, outcome_no_price,
                volume_24h, liquidity, end_date
            FROM polymarket_odds
            WHERE liquidity >= $1
            ORDER BY market_id, timestamp DESC
            LIMIT $2
            "#,
        )
        .bind(min_liquidity)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Calculates price changes over a time window.
    ///
    /// Returns (current_price, price_change) for the "yes" outcome.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_price_change(
        &self,
        market_id: &str,
        window_minutes: i64,
    ) -> Result<Option<(Decimal, Decimal)>> {
        let now = Utc::now();
        let window_start = now - chrono::Duration::minutes(window_minutes);

        let result: Option<(Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
            r#"
            WITH current_price AS (
                SELECT outcome_yes_price
                FROM polymarket_odds
                WHERE market_id = $1
                ORDER BY timestamp DESC
                LIMIT 1
            ),
            past_price AS (
                SELECT outcome_yes_price
                FROM polymarket_odds
                WHERE market_id = $1 AND timestamp <= $2
                ORDER BY timestamp DESC
                LIMIT 1
            )
            SELECT
                (SELECT outcome_yes_price FROM current_price) as current,
                (SELECT outcome_yes_price FROM current_price) -
                    (SELECT outcome_yes_price FROM past_price) as change
            "#,
        )
        .bind(market_id)
        .bind(window_start)
        .fetch_optional(&self.pool)
        .await?;

        Ok(
            result.and_then(|(current, change)| match (current, change) {
                (Some(c), Some(ch)) => Some((c, ch)),
                _ => None,
            }),
        )
    }

    /// Lists all unique market IDs.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn list_markets(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT DISTINCT market_id
            FROM polymarket_odds
            ORDER BY market_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Deletes old records before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM polymarket_odds
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
        assert!(std::mem::size_of::<PolymarketOddsRepository>() > 0);
    }

    #[test]
    fn test_odds_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = PolymarketOddsRecord {
            timestamp,
            market_id: "btc-100k".to_string(),
            question: "Will BTC exceed $100k?".to_string(),
            outcome_yes_price: dec!(0.65),
            outcome_no_price: dec!(0.36),
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
            end_date: Some(timestamp),
        };

        assert_eq!(record.implied_yes_probability(), dec!(0.65));

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_odds_new_and_with_metadata() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = PolymarketOddsRecord::new(
            timestamp,
            "btc-100k".to_string(),
            "Will BTC exceed $100k?".to_string(),
            dec!(0.65),
            dec!(0.35),
        )
        .with_metadata(Some(dec!(50000)), Some(dec!(100000)), None);

        assert_eq!(record.volume_24h, Some(dec!(50000)));
        assert_eq!(record.liquidity, Some(dec!(100000)));
        assert!(record.has_sufficient_liquidity(dec!(50000)));
    }

    #[test]
    fn test_kelly_calculation() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = PolymarketOddsRecord::new(
            timestamp,
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.50),
            dec!(0.50),
        );

        // At 50% price with 60% estimated probability:
        // b = 1.0, kelly = (0.6 * 2 - 1) / 1 = 0.2
        let kelly = record.kelly_yes(dec!(0.60));
        assert_eq!(kelly, Some(dec!(0.20)));
    }
}
