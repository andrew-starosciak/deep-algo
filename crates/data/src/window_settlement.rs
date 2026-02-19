//! Window settlement record and repository.
//!
//! Stores the resolved outcome (UP/DOWN) for each 15-minute market window,
//! sourced from the Gamma API after UMA resolution.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// A resolved 15-minute window settlement record.
#[derive(Debug, Clone)]
pub struct WindowSettlementRecord {
    /// Start of the 15-minute window
    pub window_start: DateTime<Utc>,
    /// Coin slug (e.g. "btc", "eth")
    pub coin: String,
    /// End of the 15-minute window
    pub window_end: DateTime<Utc>,
    /// Resolved outcome: "UP" or "DOWN"
    pub outcome: String,
    /// Source of the settlement (e.g. "gamma")
    pub settlement_source: String,
    /// Gamma event slug
    pub gamma_slug: Option<String>,
    /// Market condition ID
    pub condition_id: Option<String>,
    /// When the settlement was recorded
    pub settled_at: DateTime<Utc>,
    /// Session identifier for grouping
    pub session_id: Option<String>,
}

/// Repository for window settlement persistence.
#[derive(Debug, Clone)]
pub struct WindowSettlementRepository {
    pool: PgPool,
}

impl WindowSettlementRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a batch of window settlement records.
    ///
    /// Uses `ON CONFLICT DO NOTHING` to skip duplicates (same window_start + coin).
    ///
    /// # Errors
    /// Returns an error if the database transaction fails.
    pub async fn insert_batch(&self, records: &[WindowSettlementRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in records.chunks(100) {
            for record in chunk {
                sqlx::query(
                    r#"
                    INSERT INTO window_settlements
                        (window_start, coin, window_end, outcome, settlement_source,
                         gamma_slug, condition_id, settled_at, session_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    ON CONFLICT (window_start, coin) DO NOTHING
                    "#,
                )
                .bind(record.window_start)
                .bind(&record.coin)
                .bind(record.window_end)
                .bind(&record.outcome)
                .bind(&record.settlement_source)
                .bind(&record.gamma_slug)
                .bind(&record.condition_id)
                .bind(record.settled_at)
                .bind(&record.session_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }
}
