//! CLOB price snapshot record and repository.
//!
//! Stores periodic snapshots of Polymarket CLOB prices for 15-minute
//! Up/Down markets, enabling post-hoc analysis of price evolution.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

/// A snapshot of CLOB prices for a single coin's 15-minute market.
#[derive(Debug, Clone)]
pub struct ClobPriceSnapshotRecord {
    /// Snapshot timestamp
    pub timestamp: DateTime<Utc>,
    /// Coin slug (e.g. "btc", "eth")
    pub coin: String,
    /// Up token price (0.0 to 1.0)
    pub up_price: Decimal,
    /// Down token price (0.0 to 1.0)
    pub down_price: Decimal,
    /// Up token CLOB ID
    pub up_token_id: String,
    /// Down token CLOB ID
    pub down_token_id: String,
    /// Session identifier for grouping snapshots
    pub session_id: Option<String>,
}

/// Repository for CLOB price snapshot persistence.
#[derive(Debug, Clone)]
pub struct ClobPriceSnapshotRepository {
    pool: PgPool,
}

impl ClobPriceSnapshotRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a batch of CLOB price snapshot records.
    ///
    /// Uses `ON CONFLICT DO NOTHING` to skip duplicates (same timestamp + coin).
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[ClobPriceSnapshotRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO clob_price_snapshots
                        (timestamp, coin, up_price, down_price,
                         up_token_id, down_token_id, session_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (timestamp, coin) DO NOTHING
                    "#,
                )
                .bind(record.timestamp)
                .bind(&record.coin)
                .bind(record.up_price)
                .bind(record.down_price)
                .bind(&record.up_token_id)
                .bind(&record.down_token_id)
                .bind(&record.session_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }
}
