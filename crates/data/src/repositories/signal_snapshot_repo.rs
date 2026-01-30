//! Signal snapshot repository.
//!
//! Provides batch insert and time-range query operations for signal snapshot data.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::SignalSnapshotRecord;

/// Repository for signal snapshot operations.
#[derive(Debug, Clone)]
pub struct SignalSnapshotRepository {
    pool: PgPool,
}

impl SignalSnapshotRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a single signal snapshot record.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &SignalSnapshotRecord) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            r#"
            INSERT INTO signal_snapshots
                (timestamp, signal_name, symbol, exchange, direction,
                 strength, confidence, metadata, forward_return_15m)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.signal_name)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(&record.direction)
        .bind(record.strength)
        .bind(record.confidence)
        .bind(&record.metadata)
        .bind(record.forward_return_15m)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    /// Inserts a batch of signal snapshot records efficiently.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[SignalSnapshotRecord]) -> Result<Vec<i64>> {
        if records.is_empty() {
            return Ok(vec![]);
        }

        let mut ids = Vec::with_capacity(records.len());
        let mut tx = self.pool.begin().await?;

        for record in records {
            let row: (i64,) = sqlx::query_as(
                r#"
                INSERT INTO signal_snapshots
                    (timestamp, signal_name, symbol, exchange, direction,
                     strength, confidence, metadata, forward_return_15m)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                RETURNING id
                "#,
            )
            .bind(record.timestamp)
            .bind(&record.signal_name)
            .bind(&record.symbol)
            .bind(&record.exchange)
            .bind(&record.direction)
            .bind(record.strength)
            .bind(record.confidence)
            .bind(&record.metadata)
            .bind(record.forward_return_15m)
            .fetch_one(&mut *tx)
            .await?;

            ids.push(row.0);
        }

        tx.commit().await?;
        Ok(ids)
    }

    /// Queries signal snapshots by signal name within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_signal_name(
        &self,
        signal_name: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<SignalSnapshotRecord>> {
        let records = sqlx::query_as::<_, SignalSnapshotRecord>(
            r#"
            SELECT id, timestamp, signal_name, symbol, exchange, direction,
                   strength, confidence, metadata, forward_return_15m, created_at
            FROM signal_snapshots
            WHERE signal_name = $1
              AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp ASC
            "#,
        )
        .bind(signal_name)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries signal snapshots by symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_symbol(
        &self,
        symbol: &str,
        exchange: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<SignalSnapshotRecord>> {
        let records = sqlx::query_as::<_, SignalSnapshotRecord>(
            r#"
            SELECT id, timestamp, signal_name, symbol, exchange, direction,
                   strength, confidence, metadata, forward_return_15m, created_at
            FROM signal_snapshots
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

    /// Queries snapshots pending validation (without forward return).
    ///
    /// Returns snapshots older than `min_age` that don't have forward returns yet.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_pending_validation(
        &self,
        min_age: chrono::Duration,
        limit: i64,
    ) -> Result<Vec<SignalSnapshotRecord>> {
        let cutoff = Utc::now() - min_age;

        let records = sqlx::query_as::<_, SignalSnapshotRecord>(
            r#"
            SELECT id, timestamp, signal_name, symbol, exchange, direction,
                   strength, confidence, metadata, forward_return_15m, created_at
            FROM signal_snapshots
            WHERE forward_return_15m IS NULL
              AND timestamp <= $1
            ORDER BY timestamp ASC
            LIMIT $2
            "#,
        )
        .bind(cutoff)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Updates the forward return for a snapshot.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn update_forward_return(&self, id: i64, forward_return: Decimal) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE signal_snapshots
            SET forward_return_15m = $1
            WHERE id = $2
            "#,
        )
        .bind(forward_return)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Gets validation statistics for a signal.
    ///
    /// Returns (total_count, correct_predictions, win_rate).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_validation_stats(
        &self,
        signal_name: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<ValidationStats> {
        let row: (i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE
                    (direction = 'up' AND forward_return_15m > 0) OR
                    (direction = 'down' AND forward_return_15m < 0)
                ) as correct,
                COUNT(*) FILTER (WHERE forward_return_15m IS NOT NULL AND direction != 'neutral') as validated
            FROM signal_snapshots
            WHERE signal_name = $1
              AND timestamp >= $2 AND timestamp <= $3
            "#,
        )
        .bind(signal_name)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        let win_rate = if row.2 > 0 {
            Some(row.1 as f64 / row.2 as f64)
        } else {
            None
        };

        Ok(ValidationStats {
            total_snapshots: row.0,
            correct_predictions: row.1,
            validated_snapshots: row.2,
            win_rate,
        })
    }

    /// Deletes old records before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM signal_snapshots
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

/// Validation statistics for a signal.
#[derive(Debug, Clone)]
pub struct ValidationStats {
    /// Total number of snapshots
    pub total_snapshots: i64,
    /// Number of correct directional predictions
    pub correct_predictions: i64,
    /// Number of validated snapshots (with forward return, non-neutral)
    pub validated_snapshots: i64,
    /// Win rate (correct / validated), None if no validated snapshots
    pub win_rate: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::signal_snapshot::SignalDirection;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn test_repository_struct_size() {
        // Ensure the repository can be created (size check)
        assert!(std::mem::size_of::<SignalSnapshotRepository>() > 0);
    }

    #[test]
    fn test_signal_snapshot_record_for_insertion() {
        let record = SignalSnapshotRecord::new(
            sample_timestamp(),
            "order_book_imbalance",
            "BTCUSDT",
            "binance",
            SignalDirection::Up,
            dec!(0.75),
            dec!(0.85),
        )
        .with_metadata(json!({"imbalance": 0.15}));

        // Verify record is ready for insertion
        assert!(record.id.is_none()); // ID should be None for new records
        assert_eq!(record.signal_name, "order_book_imbalance");
        assert_eq!(record.direction, "up");
    }

    #[test]
    fn test_validation_stats_with_data() {
        let stats = ValidationStats {
            total_snapshots: 100,
            correct_predictions: 60,
            validated_snapshots: 95,
            win_rate: Some(60.0 / 95.0),
        };

        assert_eq!(stats.total_snapshots, 100);
        assert!(stats.win_rate.unwrap() > 0.6 && stats.win_rate.unwrap() < 0.65);
    }

    #[test]
    fn test_validation_stats_no_validated() {
        let stats = ValidationStats {
            total_snapshots: 10,
            correct_predictions: 0,
            validated_snapshots: 0,
            win_rate: None,
        };

        assert!(stats.win_rate.is_none());
    }
}
