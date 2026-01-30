//! Calculate returns CLI command.
//!
//! Computes forward returns for signal snapshots by comparing
//! mid prices at T and T+15m.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use rust_decimal::Decimal;

/// Price source for return calculations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceSource {
    /// Use OHLCV close prices (default, more reliable)
    #[default]
    Ohlcv,
    /// Use order book mid prices (real-time snapshots)
    Orderbook,
}

impl std::str::FromStr for PriceSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "ohlcv" | "candle" | "candles" => Ok(PriceSource::Ohlcv),
            "orderbook" | "ob" | "midprice" => Ok(PriceSource::Orderbook),
            _ => Err(anyhow!(
                "Invalid price source: '{}'. Valid values: ohlcv, orderbook",
                s
            )),
        }
    }
}

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PriceSource::Ohlcv => write!(f, "ohlcv"),
            PriceSource::Orderbook => write!(f, "orderbook"),
        }
    }
}

/// Arguments for the calculate-returns command.
#[derive(Args, Debug, Clone)]
pub struct CalculateReturnsArgs {
    /// Start timestamp (ISO 8601 format)
    #[arg(long)]
    pub start: String,

    /// End timestamp (ISO 8601 format)
    #[arg(long)]
    pub end: String,

    /// Trading symbol (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Exchange name (default: binance)
    #[arg(long, default_value = "binance")]
    pub exchange: String,

    /// Forward return window in minutes (default: 15)
    #[arg(long, default_value = "15")]
    pub forward_minutes: i64,

    /// Maximum lookback seconds for price query (default: 60)
    #[arg(long, default_value = "60")]
    pub max_lookback_seconds: i64,

    /// Batch size for updates (default: 1000)
    #[arg(long, default_value = "1000")]
    pub batch_size: i64,

    /// Price source for return calculations (default: ohlcv)
    /// Valid values: ohlcv (OHLCV close prices), orderbook (mid prices)
    #[arg(long, default_value = "ohlcv")]
    pub price_source: String,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

/// Calculates the percentage return between two prices.
///
/// Returns (price_end - price_start) / price_start as a percentage.
pub fn calculate_return(price_start: Decimal, price_end: Decimal) -> Decimal {
    if price_start == Decimal::ZERO {
        return Decimal::ZERO;
    }
    (price_end - price_start) / price_start
}

/// Statistics for the return calculation process.
#[derive(Debug, Default)]
pub struct ReturnStats {
    /// Total snapshots processed
    pub processed: u64,
    /// Snapshots with successful return calculation
    pub calculated: u64,
    /// Snapshots skipped due to missing T0 price
    pub missing_t0_price: u64,
    /// Snapshots skipped due to missing T+15 price
    pub missing_t15_price: u64,
    /// Snapshots skipped due to both prices missing
    pub missing_both_prices: u64,
}

impl ReturnStats {
    /// Creates a new stats tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the success rate as a percentage.
    pub fn success_rate(&self) -> f64 {
        if self.processed == 0 {
            return 0.0;
        }
        (self.calculated as f64 / self.processed as f64) * 100.0
    }

    /// Formats a summary report.
    pub fn summary(&self) -> String {
        format!(
            "Processed: {}, Calculated: {} ({:.1}%), Missing T0: {}, Missing T+15: {}, Missing Both: {}",
            self.processed,
            self.calculated,
            self.success_rate(),
            self.missing_t0_price,
            self.missing_t15_price,
            self.missing_both_prices
        )
    }
}

/// Runs the calculate-returns command.
///
/// # Errors
/// Returns an error if database connection fails or queries fail.
pub async fn run_calculate_returns(args: CalculateReturnsArgs) -> Result<()> {
    use algo_trade_data::{OhlcvRepository, OrderBookRepository, SignalSnapshotRepository};
    use sqlx::postgres::PgPoolOptions;
    use std::str::FromStr;

    // Parse arguments
    let start: DateTime<Utc> = args
        .start
        .parse()
        .map_err(|_| anyhow!("Invalid start time. Use ISO 8601 format"))?;
    let end: DateTime<Utc> = args
        .end
        .parse()
        .map_err(|_| anyhow!("Invalid end time. Use ISO 8601 format"))?;

    if start >= end {
        return Err(anyhow!("Start time must be before end time"));
    }

    let price_source = PriceSource::from_str(&args.price_source)?;

    // Calculate the cutoff: snapshots need T+forward_minutes to have a price
    let effective_end = end - chrono::Duration::minutes(args.forward_minutes);

    if start >= effective_end {
        return Err(anyhow!(
            "Time range too small for {}m forward returns",
            args.forward_minutes
        ));
    }

    tracing::info!(
        "Calculating {}-minute forward returns from {} to {}",
        args.forward_minutes,
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    tracing::info!("Price source: {}", price_source);

    // Get database URL
    let db_url = args
        .db_url
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Create repositories
    let snapshot_repo = SignalSnapshotRepository::new(pool.clone());
    let orderbook_repo = OrderBookRepository::new(pool.clone());
    let ohlcv_repo = OhlcvRepository::new(pool.clone());

    // Track statistics
    let mut stats = ReturnStats::new();

    // Process in batches
    loop {
        // Query snapshots without forward return
        let snapshots = snapshot_repo
            .query_without_forward_return(start, effective_end, args.batch_size)
            .await?;

        if snapshots.is_empty() {
            tracing::info!("No more snapshots to process");
            break;
        }

        tracing::info!("Processing batch of {} snapshots", snapshots.len());

        let mut updates: Vec<(i64, Decimal)> = Vec::with_capacity(snapshots.len());

        for snapshot in &snapshots {
            stats.processed += 1;

            // Get snapshot ID (skip if missing)
            let id = match snapshot.id {
                Some(id) => id,
                None => continue,
            };

            // Query price at T0 (snapshot timestamp)
            let price_t0 = match price_source {
                PriceSource::Ohlcv => {
                    ohlcv_repo
                        .query_close_price_at(
                            &args.symbol,
                            &args.exchange,
                            snapshot.timestamp,
                            args.max_lookback_seconds,
                        )
                        .await?
                }
                PriceSource::Orderbook => {
                    orderbook_repo
                        .query_mid_price_at(
                            &args.symbol,
                            &args.exchange,
                            snapshot.timestamp,
                            args.max_lookback_seconds,
                        )
                        .await?
                }
            };

            // Query price at T+forward_minutes
            let t_forward = snapshot.timestamp + chrono::Duration::minutes(args.forward_minutes);
            let price_t_forward = match price_source {
                PriceSource::Ohlcv => {
                    ohlcv_repo
                        .query_close_price_at(
                            &args.symbol,
                            &args.exchange,
                            t_forward,
                            args.max_lookback_seconds,
                        )
                        .await?
                }
                PriceSource::Orderbook => {
                    orderbook_repo
                        .query_mid_price_at(
                            &args.symbol,
                            &args.exchange,
                            t_forward,
                            args.max_lookback_seconds,
                        )
                        .await?
                }
            };

            // Calculate return if both prices available
            match (price_t0, price_t_forward) {
                (Some(p0), Some(p_forward)) => {
                    let forward_return = calculate_return(p0, p_forward);
                    updates.push((id, forward_return));
                    stats.calculated += 1;
                }
                (None, Some(_)) => {
                    stats.missing_t0_price += 1;
                }
                (Some(_), None) => {
                    stats.missing_t15_price += 1;
                }
                (None, None) => {
                    stats.missing_both_prices += 1;
                }
            }
        }

        // Batch update the database
        if !updates.is_empty() {
            let updated = snapshot_repo.update_forward_returns_batch(&updates).await?;
            tracing::info!("Updated {} snapshots with forward returns", updated);
        }

        // Report progress
        tracing::info!("Progress: {}", stats.summary());

        // If we got fewer than batch_size, we're done
        if snapshots.len() < args.batch_size as usize {
            break;
        }
    }

    tracing::info!("Forward return calculation complete!");
    tracing::info!("Final: {}", stats.summary());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================
    // calculate_return Tests
    // ============================================

    #[test]
    fn calculate_return_positive() {
        let start = dec!(100.00);
        let end = dec!(105.00);
        let ret = calculate_return(start, end);
        assert_eq!(ret, dec!(0.05)); // 5% return
    }

    #[test]
    fn calculate_return_negative() {
        let start = dec!(100.00);
        let end = dec!(95.00);
        let ret = calculate_return(start, end);
        assert_eq!(ret, dec!(-0.05)); // -5% return
    }

    #[test]
    fn calculate_return_zero() {
        let start = dec!(100.00);
        let end = dec!(100.00);
        let ret = calculate_return(start, end);
        assert_eq!(ret, dec!(0.00));
    }

    #[test]
    fn calculate_return_handles_zero_start() {
        let start = Decimal::ZERO;
        let end = dec!(100.00);
        let ret = calculate_return(start, end);
        assert_eq!(ret, Decimal::ZERO);
    }

    #[test]
    fn calculate_return_small_change() {
        let start = dec!(50000.00);
        let end = dec!(50010.00);
        let ret = calculate_return(start, end);
        // 10 / 50000 = 0.0002 = 0.02%
        assert_eq!(ret, dec!(0.0002));
    }

    #[test]
    fn calculate_return_large_change() {
        let start = dec!(100.00);
        let end = dec!(200.00);
        let ret = calculate_return(start, end);
        assert_eq!(ret, dec!(1.00)); // 100% return
    }

    // ============================================
    // ReturnStats Tests
    // ============================================

    #[test]
    fn return_stats_new_is_zeroed() {
        let stats = ReturnStats::new();
        assert_eq!(stats.processed, 0);
        assert_eq!(stats.calculated, 0);
        assert_eq!(stats.missing_t0_price, 0);
        assert_eq!(stats.missing_t15_price, 0);
        assert_eq!(stats.missing_both_prices, 0);
    }

    #[test]
    fn return_stats_success_rate_zero_when_empty() {
        let stats = ReturnStats::new();
        assert!((stats.success_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn return_stats_success_rate_calculates_correctly() {
        let mut stats = ReturnStats::new();
        stats.processed = 100;
        stats.calculated = 80;
        assert!((stats.success_rate() - 80.0).abs() < f64::EPSILON);
    }

    #[test]
    fn return_stats_summary_includes_all_fields() {
        let mut stats = ReturnStats::new();
        stats.processed = 100;
        stats.calculated = 75;
        stats.missing_t0_price = 10;
        stats.missing_t15_price = 10;
        stats.missing_both_prices = 5;

        let summary = stats.summary();
        assert!(summary.contains("100"));
        assert!(summary.contains("75"));
        assert!(summary.contains("75.0%"));
    }

    // ============================================
    // Timestamp Validation Tests
    // ============================================

    #[test]
    fn effective_end_calculation() {
        use chrono::TimeZone;

        let end = Utc.with_ymd_and_hms(2026, 1, 30, 12, 0, 0).unwrap();
        let forward_minutes = 15i64;
        let effective_end = end - chrono::Duration::minutes(forward_minutes);

        assert_eq!(
            effective_end,
            Utc.with_ymd_and_hms(2026, 1, 30, 11, 45, 0).unwrap()
        );
    }

    #[test]
    fn forward_timestamp_calculation() {
        use chrono::TimeZone;

        let t0 = Utc.with_ymd_and_hms(2026, 1, 30, 10, 0, 0).unwrap();
        let forward_minutes = 15i64;
        let t15 = t0 + chrono::Duration::minutes(forward_minutes);

        assert_eq!(t15, Utc.with_ymd_and_hms(2026, 1, 30, 10, 15, 0).unwrap());
    }

    // ============================================
    // PriceSource Tests
    // ============================================

    #[test]
    fn price_source_default_is_ohlcv() {
        let source = PriceSource::default();
        assert_eq!(source, PriceSource::Ohlcv);
    }

    #[test]
    fn price_source_from_str_ohlcv() {
        use std::str::FromStr;

        assert_eq!(PriceSource::from_str("ohlcv").unwrap(), PriceSource::Ohlcv);
        assert_eq!(PriceSource::from_str("OHLCV").unwrap(), PriceSource::Ohlcv);
        assert_eq!(PriceSource::from_str("candle").unwrap(), PriceSource::Ohlcv);
        assert_eq!(
            PriceSource::from_str("candles").unwrap(),
            PriceSource::Ohlcv
        );
    }

    #[test]
    fn price_source_from_str_orderbook() {
        use std::str::FromStr;

        assert_eq!(
            PriceSource::from_str("orderbook").unwrap(),
            PriceSource::Orderbook
        );
        assert_eq!(
            PriceSource::from_str("ORDERBOOK").unwrap(),
            PriceSource::Orderbook
        );
        assert_eq!(PriceSource::from_str("ob").unwrap(), PriceSource::Orderbook);
        assert_eq!(
            PriceSource::from_str("midprice").unwrap(),
            PriceSource::Orderbook
        );
    }

    #[test]
    fn price_source_from_str_invalid() {
        use std::str::FromStr;

        let result = PriceSource::from_str("invalid");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid price source"));
    }

    #[test]
    fn price_source_display() {
        assert_eq!(format!("{}", PriceSource::Ohlcv), "ohlcv");
        assert_eq!(format!("{}", PriceSource::Orderbook), "orderbook");
    }
}
