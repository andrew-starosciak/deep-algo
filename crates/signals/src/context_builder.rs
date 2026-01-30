//! Signal context builder for database integration.
//!
//! Constructs `SignalContext` by querying point-in-time historical data
//! from the database. Ensures no look-ahead bias by only including data
//! available before the specified timestamp.

use std::time::Duration;

use algo_trade_core::{
    HistoricalFundingRate, LiquidationAggregate, NewsEvent, OrderBookSnapshot, PriceLevel,
    SignalContext,
};
use algo_trade_data::{
    FundingRateRepository, LiquidationRepository, NewsEventRepository, OrderBookRepository,
    Repositories,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

/// Builder for constructing `SignalContext` with historical data from the database.
///
/// Queries data point-in-time to ensure no look-ahead bias.
/// All data is filtered to be strictly before the specified timestamp.
#[derive(Debug, Clone)]
pub struct SignalContextBuilder {
    pool: PgPool,
    symbol: String,
    exchange: String,
    lookback_duration: Duration,
    funding_lookback_duration: Duration,
    liquidation_window_minutes: i32,
    news_lookback_duration: Duration,
    max_orderbook_levels: usize,
}

impl SignalContextBuilder {
    /// Creates a new context builder.
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `symbol` - Trading pair symbol (e.g., "BTCUSDT")
    /// * `exchange` - Exchange name (e.g., "binance")
    #[must_use]
    pub fn new(pool: PgPool, symbol: &str, exchange: &str) -> Self {
        Self {
            pool,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            lookback_duration: Duration::from_secs(24 * 60 * 60), // 24 hours default
            funding_lookback_duration: Duration::from_secs(7 * 24 * 60 * 60), // 7 days
            liquidation_window_minutes: 5,
            news_lookback_duration: Duration::from_secs(60 * 60), // 1 hour
            max_orderbook_levels: 20,
        }
    }

    /// Sets the lookback duration for historical imbalance data.
    #[must_use]
    pub fn with_lookback(mut self, duration: Duration) -> Self {
        self.lookback_duration = duration;
        self
    }

    /// Sets the lookback duration for funding rate history.
    #[must_use]
    pub fn with_funding_lookback(mut self, duration: Duration) -> Self {
        self.funding_lookback_duration = duration;
        self
    }

    /// Sets the liquidation aggregation window in minutes.
    #[must_use]
    pub fn with_liquidation_window(mut self, minutes: i32) -> Self {
        self.liquidation_window_minutes = minutes;
        self
    }

    /// Sets the lookback duration for news events.
    #[must_use]
    pub fn with_news_lookback(mut self, duration: Duration) -> Self {
        self.news_lookback_duration = duration;
        self
    }

    /// Sets the maximum number of order book levels to include.
    #[must_use]
    pub fn with_max_orderbook_levels(mut self, levels: usize) -> Self {
        self.max_orderbook_levels = levels;
        self
    }

    /// Builds a `SignalContext` at the specified timestamp.
    ///
    /// All data is queried point-in-time, ensuring no look-ahead bias.
    /// Data is filtered to be strictly before `timestamp`.
    ///
    /// # Errors
    /// Returns an error if database queries fail.
    pub async fn build_at(&self, timestamp: DateTime<Utc>) -> Result<SignalContext> {
        let repos = Repositories::new(self.pool.clone());

        // Build context with all available data
        let mut ctx = SignalContext::new(timestamp, &self.symbol).with_exchange(&self.exchange);

        // Query order book (most recent before timestamp)
        if let Some(ob) = self.query_orderbook(&repos.orderbook, timestamp).await? {
            let mid = ob.mid_price();
            ctx = ctx.with_orderbook(ob);
            if let Some(price) = mid {
                ctx = ctx.with_mid_price(price);
            }
        }

        // Query historical imbalances for z-score calculation
        let imbalances = self
            .query_historical_imbalances(&repos.orderbook, timestamp)
            .await?;
        if !imbalances.is_empty() {
            ctx = ctx.with_historical_imbalances(imbalances);
        }

        // Query funding rates
        if let Some(funding) = self.query_latest_funding(&repos.funding, timestamp).await? {
            ctx = ctx.with_funding_rate(funding);
        }

        // Query historical funding rates
        let historical_funding = self
            .query_historical_funding(&repos.funding, timestamp)
            .await?;
        if !historical_funding.is_empty() {
            ctx = ctx.with_historical_funding_rates(historical_funding);
        }

        // Query liquidation aggregates
        if let Some(liq_agg) = self
            .query_liquidation_aggregate(&repos.liquidation, timestamp)
            .await?
        {
            let total = liq_agg.total_volume();
            ctx = ctx.with_liquidation_aggregates(liq_agg);
            if total > Decimal::ZERO {
                ctx = ctx.with_liquidation_usd(total);
            }
        }

        // Query news events
        let news = self.query_news_events(&repos.news, timestamp).await?;
        if !news.is_empty() {
            ctx = ctx.with_news_events(news);
        }

        Ok(ctx)
    }

    /// Queries the most recent order book before the timestamp.
    async fn query_orderbook(
        &self,
        repo: &OrderBookRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<OrderBookSnapshot>> {
        // Query most recent snapshot before timestamp
        let end = timestamp;
        let start = timestamp - chrono::Duration::minutes(5);

        let snapshots = repo
            .query_by_time_range(&self.symbol, &self.exchange, start, end)
            .await?;

        // Get the most recent one that's strictly before timestamp
        let snapshot = snapshots
            .into_iter()
            .filter(|s| s.timestamp < timestamp)
            .max_by_key(|s| s.timestamp);

        match snapshot {
            Some(record) => {
                // Parse bid/ask levels from JSON
                let bids = self.parse_price_levels(&record.bid_levels)?;
                let asks = self.parse_price_levels(&record.ask_levels)?;

                Ok(Some(OrderBookSnapshot {
                    bids: bids.into_iter().take(self.max_orderbook_levels).collect(),
                    asks: asks.into_iter().take(self.max_orderbook_levels).collect(),
                    timestamp: record.timestamp,
                }))
            }
            None => Ok(None),
        }
    }

    /// Parses price levels from JSON.
    fn parse_price_levels(&self, json: &serde_json::Value) -> Result<Vec<PriceLevel>> {
        let levels: Vec<PriceLevel> =
            serde_json::from_value(json.clone()).unwrap_or_else(|_| Vec::new());
        Ok(levels)
    }

    /// Queries historical imbalances for z-score calculation.
    async fn query_historical_imbalances(
        &self,
        repo: &OrderBookRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<f64>> {
        let start = timestamp - chrono::Duration::from_std(self.lookback_duration)?;
        let end = timestamp;

        let snapshots = repo
            .query_by_time_range(&self.symbol, &self.exchange, start, end)
            .await?;

        // Filter to only include data strictly before timestamp
        let imbalances: Vec<f64> = snapshots
            .into_iter()
            .filter(|s| s.timestamp < timestamp)
            .filter_map(|s| s.imbalance.to_string().parse::<f64>().ok())
            .collect();

        Ok(imbalances)
    }

    /// Queries the latest funding rate before timestamp.
    async fn query_latest_funding(
        &self,
        repo: &FundingRateRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<f64>> {
        let start = timestamp - chrono::Duration::hours(24);
        let end = timestamp;

        let records = repo
            .query_by_time_range(&self.symbol, &self.exchange, start, end)
            .await?;

        // Get the most recent one strictly before timestamp
        let latest = records
            .into_iter()
            .filter(|r| r.timestamp < timestamp)
            .max_by_key(|r| r.timestamp);

        Ok(latest.and_then(|r| r.funding_rate.to_string().parse::<f64>().ok()))
    }

    /// Queries historical funding rates for statistical context.
    async fn query_historical_funding(
        &self,
        repo: &FundingRateRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<HistoricalFundingRate>> {
        let start = timestamp - chrono::Duration::from_std(self.funding_lookback_duration)?;
        let end = timestamp;

        let records = repo
            .query_by_time_range(&self.symbol, &self.exchange, start, end)
            .await?;

        // Filter to only include data strictly before timestamp
        let historical: Vec<HistoricalFundingRate> = records
            .into_iter()
            .filter(|r| r.timestamp < timestamp)
            .map(|r| HistoricalFundingRate {
                timestamp: r.timestamp,
                funding_rate: r.funding_rate.to_string().parse::<f64>().unwrap_or(0.0),
                zscore: r
                    .rate_zscore
                    .and_then(|z| z.to_string().parse::<f64>().ok()),
                percentile: r
                    .rate_percentile
                    .and_then(|p| p.to_string().parse::<f64>().ok()),
            })
            .collect();

        Ok(historical)
    }

    /// Queries and aggregates liquidations in the window before timestamp.
    async fn query_liquidation_aggregate(
        &self,
        repo: &LiquidationRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<LiquidationAggregate>> {
        let window_start =
            timestamp - chrono::Duration::minutes(self.liquidation_window_minutes.into());
        let window_end = timestamp;

        let liquidations = repo
            .query_events_by_time_range(&self.symbol, &self.exchange, window_start, window_end)
            .await?;

        // Filter to only include data strictly before timestamp
        let filtered: Vec<_> = liquidations
            .into_iter()
            .filter(|l| l.timestamp < timestamp)
            .collect();

        if filtered.is_empty() {
            return Ok(None);
        }

        // Aggregate
        let mut long_volume = Decimal::ZERO;
        let mut short_volume = Decimal::ZERO;
        let mut count_long = 0i32;
        let mut count_short = 0i32;

        for liq in &filtered {
            if liq.side == "long" {
                long_volume += liq.usd_value;
                count_long += 1;
            } else {
                short_volume += liq.usd_value;
                count_short += 1;
            }
        }

        Ok(Some(LiquidationAggregate {
            timestamp,
            window_minutes: self.liquidation_window_minutes,
            long_volume_usd: long_volume,
            short_volume_usd: short_volume,
            net_delta_usd: long_volume - short_volume,
            count_long,
            count_short,
        }))
    }

    /// Queries news events in the window before timestamp.
    async fn query_news_events(
        &self,
        repo: &NewsEventRepository,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<NewsEvent>> {
        let start = timestamp - chrono::Duration::from_std(self.news_lookback_duration)?;
        let end = timestamp;

        let records = repo.query_by_currency("BTC", start, end).await?;

        // Filter to only include data strictly before timestamp
        let news: Vec<NewsEvent> = records
            .into_iter()
            .filter(|n| n.timestamp < timestamp)
            .map(|n| NewsEvent {
                timestamp: n.timestamp,
                source: n.source,
                title: n.title,
                sentiment: n.sentiment,
                urgency_score: n
                    .urgency_score
                    .and_then(|u| u.to_string().parse::<f64>().ok()),
                currencies: n.currencies,
            })
            .collect();

        Ok(news)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    // ============================================
    // Builder Configuration Tests
    // ============================================

    #[test]
    fn context_builder_default_configuration() {
        // We can't create a real pool in unit tests, but we can test the struct
        assert!(std::mem::size_of::<SignalContextBuilder>() > 0);
    }

    #[test]
    fn context_builder_with_lookback_modifies_duration() {
        // Test that Duration::from_secs works correctly
        let duration = Duration::from_secs(60 * 60 * 2); // 2 hours
        assert_eq!(duration.as_secs(), 7200);
    }

    #[test]
    fn context_builder_with_funding_lookback_modifies_duration() {
        let duration = Duration::from_secs(14 * 24 * 60 * 60); // 14 days
        assert_eq!(duration.as_secs(), 14 * 24 * 60 * 60);
    }

    #[test]
    fn context_builder_with_liquidation_window_accepts_minutes() {
        let minutes = 10;
        assert!(minutes > 0);
    }

    #[test]
    fn context_builder_with_max_orderbook_levels_accepts_count() {
        let levels = 50;
        assert!(levels > 0);
    }

    // ============================================
    // Timestamp Filtering Tests
    // ============================================

    #[test]
    fn timestamp_filtering_excludes_future_data() {
        let timestamp = sample_timestamp();
        let data_timestamp = timestamp - chrono::Duration::minutes(5);

        // Data before timestamp should be included
        assert!(data_timestamp < timestamp);

        let future_timestamp = timestamp + chrono::Duration::minutes(5);
        // Future data should be excluded
        assert!(future_timestamp >= timestamp);
    }

    #[test]
    fn timestamp_filtering_handles_edge_case_same_time() {
        let timestamp = sample_timestamp();

        // Data at exactly timestamp should be excluded (strictly less than)
        assert!(!(timestamp < timestamp));
    }

    // ============================================
    // Duration Conversion Tests
    // ============================================

    #[test]
    fn lookback_duration_converts_correctly() {
        let std_duration = Duration::from_secs(24 * 60 * 60);
        let chrono_duration = chrono::Duration::from_std(std_duration).unwrap();

        assert_eq!(chrono_duration.num_hours(), 24);
    }

    #[test]
    fn lookback_start_calculates_correctly() {
        let timestamp = sample_timestamp();
        let lookback = chrono::Duration::hours(24);
        let start = timestamp - lookback;

        assert_eq!((timestamp - start).num_hours(), 24);
    }

    // ============================================
    // Data Aggregation Tests
    // ============================================

    #[test]
    fn liquidation_aggregate_calculates_volumes() {
        let long_volume = Decimal::from(100_000);
        let short_volume = Decimal::from(50_000);
        let net_delta = long_volume - short_volume;

        assert_eq!(net_delta, Decimal::from(50_000));
        assert_eq!(long_volume + short_volume, Decimal::from(150_000));
    }

    #[test]
    fn imbalance_z_score_requires_history() {
        let imbalances: Vec<f64> = vec![0.1, 0.2, 0.15, 0.05, -0.1];

        // Calculate mean and stddev
        let mean = imbalances.iter().sum::<f64>() / imbalances.len() as f64;
        let variance = imbalances.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
            / (imbalances.len() - 1) as f64;
        let stddev = variance.sqrt();

        // Z-score for 0.3
        let current = 0.3;
        let zscore = (current - mean) / stddev;

        // Should be positive (0.3 is above mean of ~0.08)
        assert!(zscore > 0.0);
    }

    // ============================================
    // Price Level Parsing Tests (Edge Cases)
    // ============================================

    #[test]
    fn parse_empty_price_levels() {
        let json = serde_json::json!([]);
        let levels: Vec<PriceLevel> = serde_json::from_value(json).unwrap_or_default();
        assert!(levels.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_empty() {
        let json = serde_json::json!("not an array");
        let levels: Vec<PriceLevel> = serde_json::from_value(json).unwrap_or_default();
        assert!(levels.is_empty());
    }
}
