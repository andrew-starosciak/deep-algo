//! Paper trade repository.
//!
//! Provides operations for storing and querying paper trades.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::PaperTradeRecord;

/// Repository for paper trade operations.
#[derive(Debug, Clone)]
pub struct PaperTradeRepository {
    pool: PgPool,
}

impl PaperTradeRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a new paper trade and returns the generated ID.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &PaperTradeRecord) -> Result<i32> {
        let row: (i32,) = sqlx::query_as(
            r#"
            INSERT INTO paper_trades
                (timestamp, market_id, market_question, direction, shares, entry_price, stake,
                 estimated_prob, expected_value, kelly_fraction, signal_strength,
                 signals_snapshot, status, outcome, pnl, fees, settled_at, session_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
            RETURNING id
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.market_id)
        .bind(&record.market_question)
        .bind(&record.direction)
        .bind(record.shares)
        .bind(record.entry_price)
        .bind(record.stake)
        .bind(record.estimated_prob)
        .bind(record.expected_value)
        .bind(record.kelly_fraction)
        .bind(record.signal_strength)
        .bind(&record.signals_snapshot)
        .bind(&record.status)
        .bind(&record.outcome)
        .bind(record.pnl)
        .bind(record.fees)
        .bind(record.settled_at)
        .bind(&record.session_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    /// Updates a paper trade with settlement information.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn settle(
        &self,
        id: i32,
        outcome: &str,
        pnl: Decimal,
        fees: Decimal,
        settled_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE paper_trades
            SET status = 'settled', outcome = $2, pnl = $3, fees = $4, settled_at = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(outcome)
        .bind(pnl)
        .bind(fees)
        .bind(settled_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Cancels a paper trade.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn cancel(&self, id: i32) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE paper_trades
            SET status = 'cancelled'
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Gets a paper trade by ID.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_by_id(&self, id: i32) -> Result<Option<PaperTradeRecord>> {
        let record = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Queries paper trades by session ID.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_session(&self, session_id: &str) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
            WHERE session_id = $1
            ORDER BY timestamp ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries paper trades for a market within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_market(
        &self,
        market_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
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

    /// Queries all paper trades within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
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

    /// Queries pending paper trades.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_pending(&self) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
            WHERE status = 'pending'
            ORDER BY timestamp ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries pending paper trades for a specific session.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_pending_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
            WHERE status = 'pending' AND session_id = $1
            ORDER BY timestamp ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries settled paper trades within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_settled(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
            WHERE status = 'settled'
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

    /// Gets paper trade statistics for a session.
    ///
    /// Returns aggregate statistics for the session.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    #[allow(clippy::type_complexity)]
    pub async fn get_session_statistics(&self, session_id: &str) -> Result<PaperTradeStatistics> {
        let result: (
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<Decimal>,
            Option<Decimal>,
            Option<Decimal>,
        ) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE status = 'settled') as settled,
                COUNT(*) FILTER (WHERE outcome = 'win') as wins,
                SUM(stake) as total_stake,
                SUM(pnl) FILTER (WHERE status = 'settled') as total_pnl,
                SUM(fees) FILTER (WHERE status = 'settled') as total_fees
            FROM paper_trades
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;

        let total = result.0.unwrap_or(0) as u32;
        let settled = result.1.unwrap_or(0) as u32;
        let wins = result.2.unwrap_or(0) as u32;
        let losses = settled.saturating_sub(wins);

        Ok(PaperTradeStatistics {
            total_trades: total,
            settled_trades: settled,
            pending_trades: total.saturating_sub(settled),
            wins,
            losses,
            win_rate: if settled > 0 {
                wins as f64 / settled as f64
            } else {
                0.0
            },
            total_stake: result.3.unwrap_or(Decimal::ZERO),
            total_pnl: result.4.unwrap_or(Decimal::ZERO),
            total_fees: result.5.unwrap_or(Decimal::ZERO),
        })
    }

    /// Gets recent paper trades.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_recent(&self, limit: i64) -> Result<Vec<PaperTradeRecord>> {
        let records = sqlx::query_as::<_, PaperTradeRecord>(
            r#"
            SELECT id, timestamp, market_id, market_question, direction, shares, entry_price, stake,
                   estimated_prob, expected_value, kelly_fraction, signal_strength,
                   signals_snapshot, status, outcome, pnl, fees, settled_at, session_id
            FROM paper_trades
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
            DELETE FROM paper_trades
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

/// Aggregate paper trade statistics.
#[derive(Debug, Clone)]
pub struct PaperTradeStatistics {
    /// Total number of trades.
    pub total_trades: u32,
    /// Number of settled trades.
    pub settled_trades: u32,
    /// Number of pending trades.
    pub pending_trades: u32,
    /// Number of winning trades.
    pub wins: u32,
    /// Number of losing trades.
    pub losses: u32,
    /// Win rate (wins / settled).
    pub win_rate: f64,
    /// Total stake amount.
    pub total_stake: Decimal,
    /// Total profit/loss.
    pub total_pnl: Decimal,
    /// Total fees paid.
    pub total_fees: Decimal,
}

impl PaperTradeStatistics {
    /// Returns the net P&L (total_pnl after fees).
    #[must_use]
    pub fn net_pnl(&self) -> Decimal {
        self.total_pnl
    }

    /// Returns the ROI (total_pnl / total_stake).
    #[must_use]
    pub fn roi(&self) -> f64 {
        if self.total_stake > Decimal::ZERO {
            let pnl_f64: f64 = self.total_pnl.try_into().unwrap_or(0.0);
            let stake_f64: f64 = self.total_stake.try_into().unwrap_or(1.0);
            pnl_f64 / stake_f64
        } else {
            0.0
        }
    }

    /// Formats a summary for logging.
    #[must_use]
    pub fn format_summary(&self) -> String {
        format!(
            "Paper Trade Stats:\n\
             - Total trades: {} (pending: {}, settled: {})\n\
             - Wins: {} | Losses: {} | Win rate: {:.1}%\n\
             - Total stake: ${:.2}\n\
             - Total P&L: ${:.2} | Fees: ${:.2}\n\
             - ROI: {:.2}%",
            self.total_trades,
            self.pending_trades,
            self.settled_trades,
            self.wins,
            self.losses,
            self.win_rate * 100.0,
            self.total_stake,
            self.total_pnl,
            self.total_fees,
            self.roi() * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PaperTradeDirection;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    // =========================================================================
    // Test Helpers
    // =========================================================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 31, 12, 0, 0).unwrap()
    }

    fn sample_paper_trade() -> PaperTradeRecord {
        PaperTradeRecord::new(
            sample_timestamp(),
            "btc-100k-feb".to_string(),
            "Will Bitcoin exceed $100k by Feb 2025?".to_string(),
            PaperTradeDirection::Yes,
            dec!(100),
            dec!(0.60),
            dec!(0.70),
            dec!(0.25),
            dec!(0.75),
            "session-test".to_string(),
        )
    }

    // =========================================================================
    // Repository Structure Tests (no DB needed)
    // =========================================================================

    #[test]
    fn test_repository_new() {
        // Verify the repository struct compiles correctly
        assert!(std::mem::size_of::<PaperTradeRepository>() > 0);
    }

    #[test]
    fn test_paper_trade_record_structure() {
        let record = sample_paper_trade();

        assert_eq!(record.market_id, "btc-100k-feb");
        assert_eq!(record.direction, "yes");
        assert_eq!(record.stake, dec!(60));
        assert!(record.is_pending());
        assert!(!record.is_settled());
    }

    #[test]
    fn test_paper_trade_with_signals() {
        let record = sample_paper_trade().with_signals(json!({
            "imbalance": 0.15,
            "funding_zscore": 2.1
        }));

        assert!(record.signals_snapshot.is_some());
        let signals = record.signals_snapshot.unwrap();
        assert_eq!(signals["imbalance"], 0.15);
    }

    #[test]
    fn test_paper_trade_settlement() {
        let mut record = sample_paper_trade();
        let settle_time = sample_timestamp() + chrono::Duration::hours(1);

        record.settle(true, dec!(2), settle_time);

        assert!(record.is_settled());
        assert!(record.is_win());
        assert_eq!(record.pnl, Some(dec!(38))); // 100 - 60 - 2
        assert_eq!(record.fees, Some(dec!(2)));
        assert_eq!(record.settled_at, Some(settle_time));
    }

    // =========================================================================
    // PaperTradeStatistics Tests
    // =========================================================================

    #[test]
    fn test_statistics_net_pnl() {
        let stats = PaperTradeStatistics {
            total_trades: 10,
            settled_trades: 8,
            pending_trades: 2,
            wins: 5,
            losses: 3,
            win_rate: 0.625,
            total_stake: dec!(1000),
            total_pnl: dec!(150),
            total_fees: dec!(20),
        };

        assert_eq!(stats.net_pnl(), dec!(150));
    }

    #[test]
    fn test_statistics_roi() {
        let stats = PaperTradeStatistics {
            total_trades: 10,
            settled_trades: 8,
            pending_trades: 2,
            wins: 5,
            losses: 3,
            win_rate: 0.625,
            total_stake: dec!(1000),
            total_pnl: dec!(100),
            total_fees: dec!(20),
        };

        // ROI = 100 / 1000 = 0.10 (10%)
        assert!((stats.roi() - 0.10).abs() < 0.001);
    }

    #[test]
    fn test_statistics_roi_zero_stake() {
        let stats = PaperTradeStatistics {
            total_trades: 0,
            settled_trades: 0,
            pending_trades: 0,
            wins: 0,
            losses: 0,
            win_rate: 0.0,
            total_stake: dec!(0),
            total_pnl: dec!(0),
            total_fees: dec!(0),
        };

        assert!((stats.roi() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_statistics_format_summary() {
        let stats = PaperTradeStatistics {
            total_trades: 10,
            settled_trades: 8,
            pending_trades: 2,
            wins: 5,
            losses: 3,
            win_rate: 0.625,
            total_stake: dec!(1000),
            total_pnl: dec!(150),
            total_fees: dec!(20),
        };

        let summary = stats.format_summary();

        assert!(summary.contains("Total trades: 10"));
        assert!(summary.contains("pending: 2"));
        assert!(summary.contains("settled: 8"));
        assert!(summary.contains("Wins: 5"));
        assert!(summary.contains("Losses: 3"));
        assert!(summary.contains("62.5%"));
        assert!(summary.contains("$1000"));
        assert!(summary.contains("$150"));
        assert!(summary.contains("Fees: $20"));
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_statistics_all_wins() {
        let stats = PaperTradeStatistics {
            total_trades: 5,
            settled_trades: 5,
            pending_trades: 0,
            wins: 5,
            losses: 0,
            win_rate: 1.0,
            total_stake: dec!(500),
            total_pnl: dec!(250),
            total_fees: dec!(10),
        };

        assert!((stats.win_rate - 1.0).abs() < f64::EPSILON);
        assert_eq!(stats.losses, 0);
    }

    #[test]
    fn test_statistics_all_losses() {
        let stats = PaperTradeStatistics {
            total_trades: 5,
            settled_trades: 5,
            pending_trades: 0,
            wins: 0,
            losses: 5,
            win_rate: 0.0,
            total_stake: dec!(500),
            total_pnl: dec!(-500),
            total_fees: dec!(10),
        };

        assert!((stats.win_rate - 0.0).abs() < f64::EPSILON);
        assert_eq!(stats.wins, 0);
        assert!(stats.roi() < 0.0);
    }

    #[test]
    fn test_statistics_negative_pnl() {
        let stats = PaperTradeStatistics {
            total_trades: 10,
            settled_trades: 10,
            pending_trades: 0,
            wins: 3,
            losses: 7,
            win_rate: 0.3,
            total_stake: dec!(1000),
            total_pnl: dec!(-200),
            total_fees: dec!(20),
        };

        assert_eq!(stats.net_pnl(), dec!(-200));
        // ROI = -200 / 1000 = -0.20 (-20%)
        assert!((stats.roi() - (-0.20)).abs() < 0.001);
    }
}
