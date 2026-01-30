use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value as JsonValue;
use sqlx::{postgres::PgPoolOptions, PgPool};

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
    pub async fn insert_ohlcv_batch(&self, records: Vec<OhlcvRecord>) -> Result<()> {
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

    /// Inserts a single backtest result into the database.
    ///
    /// # Errors
    /// Returns an error if the database insertion fails.
    pub async fn insert_backtest_result(&self, record: BacktestResultRecord) -> Result<()> {
        sqlx::query(
            r"
            INSERT INTO backtest_results
            (timestamp, symbol, exchange, strategy_name, sharpe_ratio, sortino_ratio,
             total_pnl, total_return, win_rate, max_drawdown, num_trades, parameters)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (timestamp, symbol, exchange, strategy_name) DO UPDATE
            SET sharpe_ratio = EXCLUDED.sharpe_ratio,
                sortino_ratio = EXCLUDED.sortino_ratio,
                total_pnl = EXCLUDED.total_pnl,
                total_return = EXCLUDED.total_return,
                win_rate = EXCLUDED.win_rate,
                max_drawdown = EXCLUDED.max_drawdown,
                num_trades = EXCLUDED.num_trades,
                parameters = EXCLUDED.parameters
            ",
        )
        .bind(record.timestamp)
        .bind(&record.symbol)
        .bind(&record.exchange)
        .bind(&record.strategy_name)
        .bind(record.sharpe_ratio)
        .bind(record.sortino_ratio)
        .bind(record.total_pnl)
        .bind(record.total_return)
        .bind(record.win_rate)
        .bind(record.max_drawdown)
        .bind(record.num_trades)
        .bind(record.parameters)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inserts a batch of backtest results into the database.
    ///
    /// # Errors
    /// Returns an error if the database transaction fails or any record insertion fails.
    pub async fn insert_backtest_results_batch(
        &self,
        records: Vec<BacktestResultRecord>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for record in records {
            sqlx::query(
                r"
                INSERT INTO backtest_results
                (timestamp, symbol, exchange, strategy_name, sharpe_ratio, sortino_ratio,
                 total_pnl, total_return, win_rate, max_drawdown, num_trades, parameters)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                ON CONFLICT (timestamp, symbol, exchange, strategy_name) DO UPDATE
                SET sharpe_ratio = EXCLUDED.sharpe_ratio,
                    sortino_ratio = EXCLUDED.sortino_ratio,
                    total_pnl = EXCLUDED.total_pnl,
                    total_return = EXCLUDED.total_return,
                    win_rate = EXCLUDED.win_rate,
                    max_drawdown = EXCLUDED.max_drawdown,
                    num_trades = EXCLUDED.num_trades,
                    parameters = EXCLUDED.parameters
                ",
            )
            .bind(record.timestamp)
            .bind(&record.symbol)
            .bind(&record.exchange)
            .bind(&record.strategy_name)
            .bind(record.sharpe_ratio)
            .bind(record.sortino_ratio)
            .bind(record.total_pnl)
            .bind(record.total_return)
            .bind(record.win_rate)
            .bind(record.max_drawdown)
            .bind(record.num_trades)
            .bind(&record.parameters)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Queries the latest backtest results for all symbols.
    ///
    /// Returns the most recent backtest result for each symbol-strategy combination
    /// within the specified time window.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_latest_backtest_results(
        &self,
        strategy_name: &str,
        lookback_hours: i64,
    ) -> Result<Vec<BacktestResultRecord>> {
        let cutoff_time = Utc::now() - chrono::Duration::hours(lookback_hours);

        let records = sqlx::query_as::<_, BacktestResultRecord>(
            r"
            SELECT DISTINCT ON (symbol)
                timestamp, symbol, exchange, strategy_name, sharpe_ratio, sortino_ratio,
                total_pnl, total_return, win_rate, max_drawdown, num_trades, parameters
            FROM backtest_results
            WHERE strategy_name = $1 AND timestamp >= $2
            ORDER BY symbol, timestamp DESC
            ",
        )
        .bind(strategy_name)
        .bind(cutoff_time)
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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BacktestResultRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub exchange: String,
    pub strategy_name: String,
    pub sharpe_ratio: f64,
    pub sortino_ratio: Option<f64>,
    pub total_pnl: Decimal,
    pub total_return: Decimal,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub num_trades: i32,
    pub parameters: Option<JsonValue>,
}
