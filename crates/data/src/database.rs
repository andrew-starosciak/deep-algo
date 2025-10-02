use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgPool, postgres::PgPoolOptions};

pub struct DatabaseClient {
    pool: PgPool,
}

impl DatabaseClient {
    /// Creates a new database client connected to the specified `PostgreSQL` database.
    ///
    /// # Errors
    /// Returns an error if the database connection cannot be established.
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Inserts a batch of OHLCV records into the database.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails or any record insertion fails.
    pub async fn insert_ohlcv_batch(
        &self,
        records: Vec<OhlcvRecord>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for record in records {
            sqlx::query(
                r"
                INSERT INTO ohlcv (timestamp, symbol, exchange, open, high, low, close, volume)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
                ",
            )
            .bind(record.timestamp)
            .bind(&record.symbol)
            .bind(&record.exchange)
            .bind(record.open)
            .bind(record.high)
            .bind(record.low)
            .bind(record.close)
            .bind(record.volume)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries OHLCV records for a symbol within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_ohlcv(
        &self,
        symbol: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let records = sqlx::query_as::<_, OhlcvRecord>(
            r"
            SELECT timestamp, symbol, exchange, open, high, low, close, volume
            FROM ohlcv
            WHERE symbol = $1 AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp ASC
            ",
        )
        .bind(symbol)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OhlcvRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub exchange: String,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}
