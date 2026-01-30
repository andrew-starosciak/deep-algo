//! Binary trade repository.
//!
//! Provides operations for storing and querying binary trades.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::BinaryTradeRecord;

/// Repository for binary trade operations.
#[derive(Debug, Clone)]
pub struct BinaryTradeRepository {
    pool: PgPool,
}

impl BinaryTradeRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a new trade and returns the generated ID.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &BinaryTradeRecord) -> Result<i32> {
        let row: (i32,) = sqlx::query_as(
            r#"
            INSERT INTO binary_trades
                (timestamp, market_id, direction, shares, price, stake,
                 signals_snapshot, outcome, pnl, settled_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.market_id)
        .bind(&record.direction)
        .bind(record.shares)
        .bind(record.price)
        .bind(record.stake)
        .bind(&record.signals_snapshot)
        .bind(&record.outcome)
        .bind(record.pnl)
        .bind(record.settled_at)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    /// Updates a trade with settlement information.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn settle(
        &self,
        id: i32,
        outcome: &str,
        pnl: Decimal,
        settled_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE binary_trades
            SET outcome = $2, pnl = $3, settled_at = $4
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(outcome)
        .bind(pnl)
        .bind(settled_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Gets a trade by ID.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_by_id(&self, id: i32) -> Result<Option<BinaryTradeRecord>> {
        let record = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Queries trades for a market within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_market(
        &self,
        market_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BinaryTradeRecord>> {
        let records = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
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

    /// Queries all trades within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BinaryTradeRecord>> {
        let records = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
            WHERE timestamp >= $1 AND timestamp <= $2
            ORDER BY timestamp ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries unsettled trades.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_unsettled(&self) -> Result<Vec<BinaryTradeRecord>> {
        let records = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
            WHERE outcome IS NULL
            ORDER BY timestamp ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries settled trades (wins or losses).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_settled(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BinaryTradeRecord>> {
        let records = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
            WHERE outcome IS NOT NULL
              AND timestamp >= $1 AND timestamp <= $2
            ORDER BY timestamp ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets trade statistics within a time range.
    ///
    /// Returns (total_trades, wins, total_stake, total_pnl).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_statistics(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<TradeStatistics> {
        let result: (Option<i64>, Option<i64>, Option<Decimal>, Option<Decimal>) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE outcome = 'win') as wins,
                SUM(stake) as total_stake,
                SUM(pnl) as total_pnl
            FROM binary_trades
            WHERE outcome IS NOT NULL
              AND timestamp >= $1 AND timestamp <= $2
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        let total = result.0.unwrap_or(0) as u32;
        let wins = result.1.unwrap_or(0) as u32;

        Ok(TradeStatistics {
            total_trades: total,
            wins,
            losses: total - wins,
            win_rate: if total > 0 {
                wins as f64 / total as f64
            } else {
                0.0
            },
            total_stake: result.2.unwrap_or(Decimal::ZERO),
            total_pnl: result.3.unwrap_or(Decimal::ZERO),
        })
    }

    /// Gets statistics grouped by market.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    #[allow(clippy::type_complexity)]
    pub async fn get_statistics_by_market(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<MarketStatistics>> {
        let rows: Vec<(
            String,
            Option<i64>,
            Option<i64>,
            Option<Decimal>,
            Option<Decimal>,
        )> = sqlx::query_as(
            r#"
            SELECT
                market_id,
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE outcome = 'win') as wins,
                SUM(stake) as total_stake,
                SUM(pnl) as total_pnl
            FROM binary_trades
            WHERE outcome IS NOT NULL
              AND timestamp >= $1 AND timestamp <= $2
            GROUP BY market_id
            ORDER BY market_id
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(market_id, total, wins, stake, pnl)| {
                let total = total.unwrap_or(0) as u32;
                let wins = wins.unwrap_or(0) as u32;
                MarketStatistics {
                    market_id,
                    total_trades: total,
                    wins,
                    losses: total - wins,
                    win_rate: if total > 0 {
                        wins as f64 / total as f64
                    } else {
                        0.0
                    },
                    total_stake: stake.unwrap_or(Decimal::ZERO),
                    total_pnl: pnl.unwrap_or(Decimal::ZERO),
                }
            })
            .collect())
    }

    /// Gets recent trades.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_recent(&self, limit: i64) -> Result<Vec<BinaryTradeRecord>> {
        let records = sqlx::query_as::<_, BinaryTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, direction, shares, price, stake,
                   signals_snapshot, outcome, pnl, settled_at
            FROM binary_trades
            ORDER BY timestamp DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Deletes old records before a given timestamp.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn delete_before(&self, before: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM binary_trades
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

/// Aggregate trade statistics.
#[derive(Debug, Clone)]
pub struct TradeStatistics {
    pub total_trades: u32,
    pub wins: u32,
    pub losses: u32,
    pub win_rate: f64,
    pub total_stake: Decimal,
    pub total_pnl: Decimal,
}

/// Statistics for a specific market.
#[derive(Debug, Clone)]
pub struct MarketStatistics {
    pub market_id: String,
    pub total_trades: u32,
    pub wins: u32,
    pub losses: u32,
    pub win_rate: f64,
    pub total_stake: Decimal,
    pub total_pnl: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TradeDirection;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    #[test]
    fn test_repository_new() {
        assert!(std::mem::size_of::<BinaryTradeRepository>() > 0);
    }

    #[test]
    fn test_trade_record_structure() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = BinaryTradeRecord::new(
            timestamp,
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        )
        .with_signals(json!({"imbalance": 0.15}));

        assert_eq!(record.stake, dec!(65));
        assert!(record.is_yes());
        assert!(record.signals_snapshot.is_some());

        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
    }

    #[test]
    fn test_trade_settlement() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let mut record = BinaryTradeRecord::new(
            timestamp,
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        assert!(!record.is_settled());

        record.settle(true, timestamp);

        assert!(record.is_settled());
        assert!(record.is_win());
        assert_eq!(record.pnl, Some(dec!(35))); // 100 - 65
    }

    #[test]
    fn test_trade_roi() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let mut record = BinaryTradeRecord::new(
            timestamp,
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.50),
        );

        record.settle(true, timestamp);

        // stake = 50, pnl = 50, roi = 100%
        assert_eq!(record.roi(), Some(dec!(1.0)));
    }

    #[test]
    fn test_trade_expected_value() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();
        let record = BinaryTradeRecord::new(
            timestamp,
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.60),
        );

        // stake = 60, profit = 40
        // EV = 0.70 * 40 - 0.30 * 60 = 28 - 18 = 10
        let ev = record.expected_value(dec!(0.70));
        assert_eq!(ev, dec!(10));
    }

    #[test]
    fn test_trade_statistics() {
        let stats = TradeStatistics {
            total_trades: 100,
            wins: 55,
            losses: 45,
            win_rate: 0.55,
            total_stake: dec!(10000),
            total_pnl: dec!(500),
        };

        assert_eq!(stats.total_trades, 100);
        assert_eq!(stats.wins + stats.losses, stats.total_trades);
    }
}
