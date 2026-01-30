//! News event repository.
//!
//! Provides operations for storing and querying news events.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::NewsEventRecord;

/// Repository for news event operations.
#[derive(Debug, Clone)]
pub struct NewsEventRepository {
    pool: PgPool,
}

impl NewsEventRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single news event.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &NewsEventRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO news_events
                (timestamp, source, title, url, categories, currencies,
                 sentiment, urgency_score, raw_data)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (timestamp, source, title) DO NOTHING
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.source)
        .bind(&record.title)
        .bind(&record.url)
        .bind(&record.categories)
        .bind(&record.currencies)
        .bind(&record.sentiment)
        .bind(record.urgency_score)
        .bind(&record.raw_data)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of news events.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[NewsEventRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO news_events
                        (timestamp, source, title, url, categories, currencies,
                         sentiment, urgency_score, raw_data)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    ON CONFLICT (timestamp, source, title) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.source)
                .bind(&record.title)
                .bind(&record.url)
                .bind(&record.categories)
                .bind(&record.currencies)
                .bind(&record.sentiment)
                .bind(record.urgency_score)
                .bind(&record.raw_data)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries news events within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<NewsEventRecord>> {
        let records = sqlx::query_as::<_, NewsEventRecord>(
            r#"
            SELECT timestamp, source, title, url, categories, currencies,
                   sentiment, urgency_score, raw_data
            FROM news_events
            WHERE timestamp >= $1 AND timestamp <= $2
            ORDER BY timestamp DESC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries news events for a specific currency.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_currency(
        &self,
        currency: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<NewsEventRecord>> {
        let records = sqlx::query_as::<_, NewsEventRecord>(
            r#"
            SELECT timestamp, source, title, url, categories, currencies,
                   sentiment, urgency_score, raw_data
            FROM news_events
            WHERE $1 = ANY(currencies)
              AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp DESC
            "#,
        )
        .bind(currency)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries high-urgency news events.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_high_urgency(
        &self,
        min_urgency: Decimal,
        limit: i64,
    ) -> Result<Vec<NewsEventRecord>> {
        let records = sqlx::query_as::<_, NewsEventRecord>(
            r#"
            SELECT timestamp, source, title, url, categories, currencies,
                   sentiment, urgency_score, raw_data
            FROM news_events
            WHERE urgency_score >= $1
            ORDER BY timestamp DESC
            LIMIT $2
            "#,
        )
        .bind(min_urgency)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries news events by sentiment.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_sentiment(
        &self,
        sentiment: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<NewsEventRecord>> {
        let records = sqlx::query_as::<_, NewsEventRecord>(
            r#"
            SELECT timestamp, source, title, url, categories, currencies,
                   sentiment, urgency_score, raw_data
            FROM news_events
            WHERE sentiment = $1
              AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp DESC
            "#,
        )
        .bind(sentiment)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets recent news events.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_recent(&self, limit: i64) -> Result<Vec<NewsEventRecord>> {
        let records = sqlx::query_as::<_, NewsEventRecord>(
            r#"
            SELECT timestamp, source, title, url, categories, currencies,
                   sentiment, urgency_score, raw_data
            FROM news_events
            ORDER BY timestamp DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Calculates sentiment ratio for a currency in a time window.
    ///
    /// Returns (positive_count, negative_count, neutral_count).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_sentiment_ratio(
        &self,
        currency: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(i64, i64, i64)> {
        let result: (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE sentiment = 'positive') as positive,
                COUNT(*) FILTER (WHERE sentiment = 'negative') as negative,
                COUNT(*) FILTER (WHERE sentiment = 'neutral') as neutral
            FROM news_events
            WHERE $1 = ANY(currencies)
              AND timestamp >= $2 AND timestamp <= $3
            "#,
        )
        .bind(currency)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok((
            result.0.unwrap_or(0),
            result.1.unwrap_or(0),
            result.2.unwrap_or(0),
        ))
    }

    /// Calculates average urgency score for a currency in a time window.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_average_urgency(
        &self,
        currency: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Option<Decimal>> {
        let result: (Option<Decimal>,) = sqlx::query_as(
            r#"
            SELECT AVG(urgency_score)
            FROM news_events
            WHERE $1 = ANY(currencies)
              AND timestamp >= $2 AND timestamp <= $3
              AND urgency_score IS NOT NULL
            "#,
        )
        .bind(currency)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0)
    }

    /// Lists all unique news sources.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn list_sources(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT DISTINCT source
            FROM news_events
            ORDER BY source
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(s,)| s).collect())
    }

    /// Deletes old records before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM news_events
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
    use crate::models::NewsSentiment;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn test_repository_new() {
        assert!(std::mem::size_of::<NewsEventRepository>() > 0);
    }

    #[test]
    fn test_news_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = NewsEventRecord::new(
            timestamp,
            "cryptopanic".to_string(),
            "Bitcoin hits new ATH".to_string(),
        )
        .with_currencies(vec!["BTC".to_string()])
        .with_sentiment(NewsSentiment::Positive, dec!(0.85));

        assert!(record.mentions_currency("BTC"));
        assert!(record.is_positive());
        assert_eq!(record.urgency_score, Some(dec!(0.85)));

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_signal_strength() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();

        let positive = NewsEventRecord::new(timestamp, "test".to_string(), "Test".to_string())
            .with_sentiment(NewsSentiment::Positive, dec!(0.8));

        assert_eq!(positive.signal_strength(), Some(dec!(0.8)));

        let negative = NewsEventRecord::new(timestamp, "test".to_string(), "Test".to_string())
            .with_sentiment(NewsSentiment::Negative, dec!(0.7));

        assert_eq!(negative.signal_strength(), Some(dec!(-0.7)));
    }
}
