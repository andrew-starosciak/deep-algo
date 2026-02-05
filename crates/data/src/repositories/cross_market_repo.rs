//! Cross-market opportunity repository.
//!
//! Provides operations for storing and querying cross-market arbitrage opportunities.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::models::CrossMarketOpportunityRecord;

/// Repository for cross-market opportunity operations.
#[derive(Debug, Clone)]
pub struct CrossMarketRepository {
    pool: PgPool,
}

impl CrossMarketRepository {
    /// Creates a new repository instance.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a new opportunity and returns the generated ID.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert(&self, record: &CrossMarketOpportunityRecord) -> Result<i32> {
        let row: (i32,) = sqlx::query_as(
            r#"
            INSERT INTO cross_market_opportunities
                (timestamp, coin1, coin2, combination,
                 leg1_direction, leg1_price, leg1_token_id,
                 leg2_direction, leg2_price, leg2_token_id,
                 total_cost, spread, expected_value, win_probability,
                 assumed_correlation, session_id, status, window_end,
                 leg1_bid_depth, leg1_ask_depth, leg1_spread_bps,
                 leg2_bid_depth, leg2_ask_depth, leg2_spread_bps, executed)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                    $19, $20, $21, $22, $23, $24, $25)
            RETURNING id
            "#,
        )
        .bind(record.timestamp)
        .bind(&record.coin1)
        .bind(&record.coin2)
        .bind(&record.combination)
        .bind(&record.leg1_direction)
        .bind(record.leg1_price)
        .bind(&record.leg1_token_id)
        .bind(&record.leg2_direction)
        .bind(record.leg2_price)
        .bind(&record.leg2_token_id)
        .bind(record.total_cost)
        .bind(record.spread)
        .bind(record.expected_value)
        .bind(record.win_probability)
        .bind(record.assumed_correlation)
        .bind(&record.session_id)
        .bind(&record.status)
        .bind(record.window_end)
        .bind(record.leg1_bid_depth)
        .bind(record.leg1_ask_depth)
        .bind(record.leg1_spread_bps)
        .bind(record.leg2_bid_depth)
        .bind(record.leg2_ask_depth)
        .bind(record.leg2_spread_bps)
        .bind(record.executed)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    /// Batch inserts multiple opportunities.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn insert_batch(&self, records: &[CrossMarketOpportunityRecord]) -> Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }

        let mut count = 0u64;

        // Insert in chunks of 100 for efficiency
        for chunk in records.chunks(100) {
            let timestamps: Vec<_> = chunk.iter().map(|r| r.timestamp).collect();
            let coin1s: Vec<_> = chunk.iter().map(|r| &r.coin1).collect();
            let coin2s: Vec<_> = chunk.iter().map(|r| &r.coin2).collect();
            let combinations: Vec<_> = chunk.iter().map(|r| &r.combination).collect();
            let leg1_directions: Vec<_> = chunk.iter().map(|r| &r.leg1_direction).collect();
            let leg1_prices: Vec<_> = chunk.iter().map(|r| r.leg1_price).collect();
            let leg1_token_ids: Vec<_> = chunk.iter().map(|r| &r.leg1_token_id).collect();
            let leg2_directions: Vec<_> = chunk.iter().map(|r| &r.leg2_direction).collect();
            let leg2_prices: Vec<_> = chunk.iter().map(|r| r.leg2_price).collect();
            let leg2_token_ids: Vec<_> = chunk.iter().map(|r| &r.leg2_token_id).collect();
            let total_costs: Vec<_> = chunk.iter().map(|r| r.total_cost).collect();
            let spreads: Vec<_> = chunk.iter().map(|r| r.spread).collect();
            let expected_values: Vec<_> = chunk.iter().map(|r| r.expected_value).collect();
            let win_probs: Vec<_> = chunk.iter().map(|r| r.win_probability).collect();
            let correlations: Vec<_> = chunk.iter().map(|r| r.assumed_correlation).collect();
            let session_ids: Vec<_> = chunk.iter().map(|r| r.session_id.as_deref()).collect();

            let result = sqlx::query(
                r#"
                INSERT INTO cross_market_opportunities
                    (timestamp, coin1, coin2, combination,
                     leg1_direction, leg1_price, leg1_token_id,
                     leg2_direction, leg2_price, leg2_token_id,
                     total_cost, spread, expected_value, win_probability,
                     assumed_correlation, session_id)
                SELECT * FROM UNNEST(
                    $1::timestamptz[], $2::text[], $3::text[], $4::text[],
                    $5::text[], $6::decimal[], $7::text[],
                    $8::text[], $9::decimal[], $10::text[],
                    $11::decimal[], $12::decimal[], $13::decimal[], $14::decimal[],
                    $15::decimal[], $16::text[]
                )
                "#,
            )
            .bind(&timestamps)
            .bind(&coin1s)
            .bind(&coin2s)
            .bind(&combinations)
            .bind(&leg1_directions)
            .bind(&leg1_prices)
            .bind(&leg1_token_ids)
            .bind(&leg2_directions)
            .bind(&leg2_prices)
            .bind(&leg2_token_ids)
            .bind(&total_costs)
            .bind(&spreads)
            .bind(&expected_values)
            .bind(&win_probs)
            .bind(&correlations)
            .bind(&session_ids)
            .execute(&self.pool)
            .await?;

            count += result.rows_affected();
        }

        Ok(count)
    }

    /// Queries opportunities by coin pair within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_pair(
        &self,
        coin1: &str,
        coin2: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id
            FROM cross_market_opportunities
            WHERE ((coin1 = $1 AND coin2 = $2) OR (coin1 = $2 AND coin2 = $1))
              AND timestamp >= $3 AND timestamp <= $4
            ORDER BY timestamp DESC
            "#,
        )
        .bind(coin1)
        .bind(coin2)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries opportunities by combination type within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_combination(
        &self,
        combination: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id
            FROM cross_market_opportunities
            WHERE combination = $1
              AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp DESC
            "#,
        )
        .bind(combination)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Queries all opportunities within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn query_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: Option<i64>,
    ) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let limit_clause = limit.map_or(String::new(), |l| format!("LIMIT {l}"));

        let query = format!(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id
            FROM cross_market_opportunities
            WHERE timestamp >= $1 AND timestamp <= $2
            ORDER BY timestamp DESC
            {limit_clause}
            "#
        );

        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(&query)
            .bind(start)
            .bind(end)
            .fetch_all(&self.pool)
            .await?;

        Ok(records)
    }

    /// Gets the best opportunities (highest spread) within a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_best_by_spread(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id
            FROM cross_market_opportunities
            WHERE timestamp >= $1 AND timestamp <= $2
            ORDER BY spread DESC
            LIMIT $3
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets aggregate statistics for a time range.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_statistics(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<CrossMarketStatistics> {
        let result: (
            Option<i64>,
            Option<Decimal>,
            Option<Decimal>,
            Option<Decimal>,
            Option<Decimal>,
            Option<Decimal>,
            Option<Decimal>,
        ) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                AVG(total_cost) as avg_cost,
                AVG(spread) as avg_spread,
                AVG(expected_value) as avg_ev,
                MIN(total_cost) as min_cost,
                MAX(spread) as max_spread,
                AVG(win_probability) as avg_win_prob
            FROM cross_market_opportunities
            WHERE timestamp >= $1 AND timestamp <= $2
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        Ok(CrossMarketStatistics {
            total_opportunities: result.0.unwrap_or(0) as u64,
            avg_cost: result.1.unwrap_or(Decimal::ZERO),
            avg_spread: result.2.unwrap_or(Decimal::ZERO),
            avg_ev: result.3.unwrap_or(Decimal::ZERO),
            min_cost: result.4.unwrap_or(Decimal::ONE),
            max_spread: result.5.unwrap_or(Decimal::ZERO),
            avg_win_prob: result.6.unwrap_or(Decimal::ZERO),
        })
    }

    /// Gets statistics grouped by coin pair.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_statistics_by_pair(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<PairStatistics>> {
        let records = sqlx::query_as::<_, PairStatistics>(
            r#"
            SELECT
                coin1,
                coin2,
                COUNT(*)::bigint as total_opportunities,
                AVG(spread) as avg_spread,
                AVG(expected_value) as avg_ev,
                MAX(spread) as best_spread
            FROM cross_market_opportunities
            WHERE timestamp >= $1 AND timestamp <= $2
            GROUP BY coin1, coin2
            ORDER BY total_opportunities DESC
            "#,
        )
        .bind(start)
        .bind(end)
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
            DELETE FROM cross_market_opportunities
            WHERE timestamp < $1
            "#,
        )
        .bind(before)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Gets recent opportunities.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_recent(&self, limit: i64) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id,
                   status, window_end, coin1_outcome, coin2_outcome,
                   trade_result, actual_pnl, correlation_correct, settled_at
            FROM cross_market_opportunities
            ORDER BY timestamp DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    // ========================================================================
    // Settlement Methods
    // ========================================================================

    /// Gets opportunities that are ready for settlement (window closed, still pending).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_pending_settlement(&self, limit: i64) -> Result<Vec<CrossMarketOpportunityRecord>> {
        let records = sqlx::query_as::<_, CrossMarketOpportunityRecord>(
            r#"
            SELECT id, timestamp, coin1, coin2, combination,
                   leg1_direction, leg1_price, leg1_token_id,
                   leg2_direction, leg2_price, leg2_token_id,
                   total_cost, spread, expected_value, win_probability,
                   assumed_correlation, session_id,
                   status, window_end, coin1_outcome, coin2_outcome,
                   trade_result, actual_pnl, correlation_correct, settled_at
            FROM cross_market_opportunities
            WHERE status = 'pending'
              AND window_end <= NOW()
            ORDER BY window_end ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Updates an opportunity with settlement results.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn settle_opportunity(
        &self,
        id: i32,
        coin1_outcome: &str,
        coin2_outcome: &str,
        trade_result: &str,
        actual_pnl: Decimal,
        correlation_correct: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE cross_market_opportunities
            SET status = 'settled',
                coin1_outcome = $2,
                coin2_outcome = $3,
                trade_result = $4,
                actual_pnl = $5,
                correlation_correct = $6,
                settled_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(coin1_outcome)
        .bind(coin2_outcome)
        .bind(trade_result)
        .bind(actual_pnl)
        .bind(correlation_correct)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Marks an opportunity as expired (no settlement data available).
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn mark_expired(&self, id: i32) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE cross_market_opportunities
            SET status = 'expired', settled_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Marks an opportunity as having a settlement error.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn mark_error(&self, id: i32) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE cross_market_opportunities
            SET status = 'error', settled_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ========================================================================
    // Performance Analytics Methods
    // ========================================================================

    /// Gets settlement performance statistics.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_settlement_stats(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<SettlementStats> {
        let result: (
            Option<i64>,   // total_settled
            Option<i64>,   // wins
            Option<i64>,   // double_wins
            Option<i64>,   // losses
            Option<Decimal>, // total_pnl
            Option<Decimal>, // avg_pnl
            Option<i64>,   // correlation_correct_count
        ) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE status = 'settled') as total_settled,
                COUNT(*) FILTER (WHERE trade_result = 'WIN') as wins,
                COUNT(*) FILTER (WHERE trade_result = 'DOUBLE_WIN') as double_wins,
                COUNT(*) FILTER (WHERE trade_result = 'LOSE') as losses,
                SUM(actual_pnl) FILTER (WHERE status = 'settled') as total_pnl,
                AVG(actual_pnl) FILTER (WHERE status = 'settled') as avg_pnl,
                COUNT(*) FILTER (WHERE correlation_correct = true) as correlation_correct_count
            FROM cross_market_opportunities
            WHERE timestamp >= $1 AND timestamp <= $2
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;

        let total_settled = result.0.unwrap_or(0) as u64;
        let wins = result.1.unwrap_or(0) as u64;
        let double_wins = result.2.unwrap_or(0) as u64;
        let losses = result.3.unwrap_or(0) as u64;
        let correlation_correct = result.6.unwrap_or(0) as u64;

        let win_rate = if total_settled > 0 {
            (wins + double_wins) as f64 / total_settled as f64
        } else {
            0.0
        };

        let correlation_accuracy = if total_settled > 0 {
            correlation_correct as f64 / total_settled as f64
        } else {
            0.0
        };

        Ok(SettlementStats {
            total_settled,
            wins,
            double_wins,
            losses,
            win_rate,
            total_pnl: result.4.unwrap_or(Decimal::ZERO),
            avg_pnl: result.5.unwrap_or(Decimal::ZERO),
            correlation_accuracy,
        })
    }

    /// Gets performance statistics grouped by coin.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_coin_performance(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CoinPerformance>> {
        let records = sqlx::query_as::<_, CoinPerformance>(
            r#"
            WITH coin_stats AS (
                SELECT
                    coin1 as coin,
                    COUNT(*) FILTER (WHERE status = 'settled') as trades,
                    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
                    SUM(actual_pnl) FILTER (WHERE status = 'settled') as total_pnl,
                    AVG(leg1_price) as avg_price,
                    AVG(spread) as avg_spread
                FROM cross_market_opportunities
                WHERE timestamp >= $1 AND timestamp <= $2
                GROUP BY coin1

                UNION ALL

                SELECT
                    coin2 as coin,
                    COUNT(*) FILTER (WHERE status = 'settled') as trades,
                    COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN')) as wins,
                    SUM(actual_pnl) FILTER (WHERE status = 'settled') as total_pnl,
                    AVG(leg2_price) as avg_price,
                    AVG(spread) as avg_spread
                FROM cross_market_opportunities
                WHERE timestamp >= $1 AND timestamp <= $2
                GROUP BY coin2
            )
            SELECT
                coin,
                SUM(trades)::bigint as total_trades,
                SUM(wins)::bigint as total_wins,
                SUM(total_pnl) as total_pnl,
                AVG(avg_price) as avg_price,
                AVG(avg_spread) as avg_spread
            FROM coin_stats
            GROUP BY coin
            ORDER BY total_pnl DESC NULLS LAST
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Gets calibration data (predicted vs actual win rates by probability bucket).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_calibration_data(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CalibrationBucket>> {
        let records = sqlx::query_as::<_, CalibrationBucket>(
            r#"
            SELECT
                FLOOR(win_probability * 20) / 20 as prob_bucket,
                COUNT(*)::bigint as total,
                COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::bigint as wins,
                AVG(win_probability) as avg_predicted,
                (COUNT(*) FILTER (WHERE trade_result IN ('WIN', 'DOUBLE_WIN'))::decimal /
                 NULLIF(COUNT(*), 0)) as actual_rate
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND timestamp >= $1 AND timestamp <= $2
            GROUP BY FLOOR(win_probability * 20) / 20
            ORDER BY prob_bucket
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

/// Aggregate statistics for cross-market opportunities.
#[derive(Debug, Clone)]
pub struct CrossMarketStatistics {
    /// Total number of opportunities.
    pub total_opportunities: u64,
    /// Average total cost.
    pub avg_cost: Decimal,
    /// Average spread.
    pub avg_spread: Decimal,
    /// Average expected value.
    pub avg_ev: Decimal,
    /// Minimum cost seen.
    pub min_cost: Decimal,
    /// Maximum spread seen.
    pub max_spread: Decimal,
    /// Average win probability.
    pub avg_win_prob: Decimal,
}

impl CrossMarketStatistics {
    /// Formats a summary for display.
    #[must_use]
    pub fn format_summary(&self) -> String {
        format!(
            "Cross-Market Stats:\n\
             - Total opportunities: {}\n\
             - Avg cost: ${:.4} | Avg spread: ${:.4}\n\
             - Avg EV: ${:.4} | Avg P(win): {:.1}%\n\
             - Best cost: ${:.4} | Best spread: ${:.4}",
            self.total_opportunities,
            self.avg_cost,
            self.avg_spread,
            self.avg_ev,
            self.avg_win_prob * Decimal::from(100),
            self.min_cost,
            self.max_spread
        )
    }
}

/// Statistics for a specific coin pair.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PairStatistics {
    /// First coin.
    pub coin1: String,
    /// Second coin.
    pub coin2: String,
    /// Total opportunities for this pair.
    pub total_opportunities: i64,
    /// Average spread.
    pub avg_spread: Option<Decimal>,
    /// Average expected value.
    pub avg_ev: Option<Decimal>,
    /// Best spread seen.
    pub best_spread: Option<Decimal>,
}

impl PairStatistics {
    /// Returns the pair name.
    #[must_use]
    pub fn pair_name(&self) -> String {
        format!("{}/{}", self.coin1, self.coin2)
    }
}

/// Settlement performance statistics.
#[derive(Debug, Clone)]
pub struct SettlementStats {
    /// Total opportunities that have been settled.
    pub total_settled: u64,
    /// Single-leg wins.
    pub wins: u64,
    /// Both-legs wins (jackpot).
    pub double_wins: u64,
    /// Both-legs losses.
    pub losses: u64,
    /// Win rate (wins + double_wins) / total.
    pub win_rate: f64,
    /// Total P&L across all settled trades.
    pub total_pnl: Decimal,
    /// Average P&L per trade.
    pub avg_pnl: Decimal,
    /// How often correlation held (coins moved together).
    pub correlation_accuracy: f64,
}

impl SettlementStats {
    /// Formats a summary for display.
    #[must_use]
    pub fn format_summary(&self) -> String {
        format!(
            "Settlement Stats:\n\
             - Settled: {} | Wins: {} | Double Wins: {} | Losses: {}\n\
             - Win Rate: {:.1}% (predicted ~96%)\n\
             - Total P&L: ${:.2} | Avg P&L: ${:.4}\n\
             - Correlation Accuracy: {:.1}%",
            self.total_settled,
            self.wins,
            self.double_wins,
            self.losses,
            self.win_rate * 100.0,
            self.total_pnl,
            self.avg_pnl,
            self.correlation_accuracy * 100.0
        )
    }
}

/// Performance statistics for a single coin.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CoinPerformance {
    /// Coin symbol.
    pub coin: String,
    /// Total trades involving this coin.
    pub total_trades: i64,
    /// Total wins involving this coin.
    pub total_wins: i64,
    /// Total P&L from trades involving this coin.
    pub total_pnl: Option<Decimal>,
    /// Average entry price for this coin.
    pub avg_price: Option<Decimal>,
    /// Average spread when this coin is involved.
    pub avg_spread: Option<Decimal>,
}

impl CoinPerformance {
    /// Returns the win rate for this coin.
    #[must_use]
    pub fn win_rate(&self) -> f64 {
        if self.total_trades > 0 {
            self.total_wins as f64 / self.total_trades as f64
        } else {
            0.0
        }
    }
}

/// Calibration bucket for model validation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CalibrationBucket {
    /// Probability bucket (e.g., 0.90 for 90-95% predictions).
    pub prob_bucket: Option<Decimal>,
    /// Total predictions in this bucket.
    pub total: i64,
    /// Actual wins in this bucket.
    pub wins: i64,
    /// Average predicted probability.
    pub avg_predicted: Option<Decimal>,
    /// Actual win rate.
    pub actual_rate: Option<Decimal>,
}

impl CalibrationBucket {
    /// Returns the calibration error (|predicted - actual|).
    #[must_use]
    pub fn calibration_error(&self) -> f64 {
        match (self.avg_predicted, self.actual_rate) {
            (Some(pred), Some(actual)) => {
                let pred_f64: f64 = pred.try_into().unwrap_or(0.0);
                let actual_f64: f64 = actual.try_into().unwrap_or(0.0);
                (pred_f64 - actual_f64).abs()
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn statistics_format_summary() {
        let stats = CrossMarketStatistics {
            total_opportunities: 100,
            avg_cost: dec!(0.94),
            avg_spread: dec!(0.06),
            avg_ev: dec!(0.03),
            min_cost: dec!(0.90),
            max_spread: dec!(0.10),
            avg_win_prob: dec!(0.92),
        };

        let summary = stats.format_summary();
        assert!(summary.contains("100"));
        assert!(summary.contains("0.94"));
        assert!(summary.contains("0.06"));
        assert!(summary.contains("92.0%"));
    }

    #[test]
    fn pair_statistics_pair_name() {
        let stats = PairStatistics {
            coin1: "BTC".to_string(),
            coin2: "ETH".to_string(),
            total_opportunities: 50,
            avg_spread: Some(dec!(0.05)),
            avg_ev: Some(dec!(0.02)),
            best_spread: Some(dec!(0.08)),
        };

        assert_eq!(stats.pair_name(), "BTC/ETH");
    }

    #[test]
    fn repository_struct_compiles() {
        // Verify the repository struct compiles correctly
        assert!(std::mem::size_of::<CrossMarketRepository>() > 0);
    }
}
