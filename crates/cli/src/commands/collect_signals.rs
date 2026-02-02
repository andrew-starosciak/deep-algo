//! collect-signals CLI command for orchestrating data collectors.
//!
//! Runs multiple data collectors (orderbook, funding, liquidations, polymarket, news)
//! concurrently with database persistence and graceful shutdown handling.

use anyhow::{anyhow, Result};
use clap::Args;
use std::time::Duration;

/// Data source types for collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Order book snapshots (Binance Futures)
    OrderBook,
    /// Funding rates (Binance Futures)
    Funding,
    /// Liquidation events (Binance Futures)
    Liquidations,
    /// Trade ticks for CVD (Binance Futures)
    TradeTicks,
    /// Polymarket odds (CLOB API)
    Polymarket,
    /// News events (CryptoPanic)
    News,
}

impl Source {
    /// Returns all available sources.
    pub fn all() -> Vec<Source> {
        vec![
            Source::OrderBook,
            Source::Funding,
            Source::Liquidations,
            Source::TradeTicks,
            Source::Polymarket,
            Source::News,
        ]
    }

    /// Returns the source name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::OrderBook => "orderbook",
            Source::Funding => "funding",
            Source::Liquidations => "liquidations",
            Source::TradeTicks => "tradeticks",
            Source::Polymarket => "polymarket",
            Source::News => "news",
        }
    }
}

/// Arguments for the collect-signals command.
#[derive(Args, Debug, Clone)]
pub struct CollectSignalsArgs {
    /// Duration to collect (e.g., "1h", "24h", "7d")
    #[arg(long, default_value = "24h")]
    pub duration: String,

    /// Comma-separated sources (orderbook,funding,liquidations,polymarket,news)
    #[arg(long, default_value = "orderbook,funding,liquidations,polymarket,news")]
    pub sources: String,

    /// Trading symbol (default: btcusdt)
    #[arg(long, default_value = "btcusdt")]
    pub symbol: String,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// CryptoPanic API key for news collection
    #[arg(long, env = "CRYPTOPANIC_API_KEY")]
    pub news_api_key: Option<String>,
}

/// Parses a duration string like "1h", "24h", "7d" into a Duration.
///
/// Supported formats:
/// - `<n>s` - seconds
/// - `<n>m` - minutes
/// - `<n>h` - hours
/// - `<n>d` - days
///
/// # Errors
/// Returns an error if the format is invalid or the number cannot be parsed.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();

    if s.is_empty() {
        return Err(anyhow!("Duration string cannot be empty"));
    }

    // Find where the numeric part ends and the unit begins
    let (num_str, unit) = if s.ends_with("ms") {
        // Handle milliseconds specially
        let num_str = &s[..s.len() - 2];
        (num_str, "ms")
    } else {
        let split_idx = s
            .chars()
            .position(|c| !c.is_ascii_digit())
            .ok_or_else(|| anyhow!("Duration must have a unit (s, m, h, d)"))?;
        let num_str = &s[..split_idx];
        let unit = &s[split_idx..];
        (num_str, unit)
    };

    if num_str.is_empty() {
        return Err(anyhow!("Duration must start with a number"));
    }

    let value: u64 = num_str
        .parse()
        .map_err(|_| anyhow!("Invalid number in duration: {}", num_str))?;

    if value == 0 {
        return Err(anyhow!("Duration cannot be zero"));
    }

    let duration = match unit {
        "ms" => Duration::from_millis(value),
        "s" => Duration::from_secs(value),
        "m" => Duration::from_secs(value * 60),
        "h" => Duration::from_secs(value * 3600),
        "d" => Duration::from_secs(value * 86400),
        _ => {
            return Err(anyhow!(
                "Unknown duration unit: {}. Use s, m, h, or d",
                unit
            ))
        }
    };

    Ok(duration)
}

/// Parses a comma-separated source string into a vector of Sources.
///
/// # Errors
/// Returns an error if any source name is invalid.
pub fn parse_sources(s: &str) -> Result<Vec<Source>> {
    let s = s.trim();

    if s.is_empty() || s.to_lowercase() == "all" {
        return Ok(Source::all());
    }

    let mut sources = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for part in s.split(',') {
        let source_name = part.trim().to_lowercase();

        if source_name.is_empty() {
            continue;
        }

        let source = match source_name.as_str() {
            "orderbook" => Source::OrderBook,
            "funding" => Source::Funding,
            "liquidations" => Source::Liquidations,
            "tradeticks" => Source::TradeTicks,
            "polymarket" => Source::Polymarket,
            "news" => Source::News,
            _ => {
                return Err(anyhow!(
                    "Unknown source: '{}'. Valid sources: orderbook, funding, liquidations, tradeticks, polymarket, news",
                    source_name
                ))
            }
        };

        // Deduplicate
        if seen.insert(source) {
            sources.push(source);
        }
    }

    if sources.is_empty() {
        return Err(anyhow!("At least one source must be specified"));
    }

    Ok(sources)
}

/// Statistics for a running collector.
#[derive(Debug, Clone, Default)]
pub struct SourceStats {
    /// Records collected
    pub records: u64,
    /// Errors encountered
    pub errors: u64,
    /// Last record timestamp
    pub last_record: Option<chrono::DateTime<chrono::Utc>>,
}

/// Aggregated health statistics for all collectors.
#[derive(Debug, Clone, Default)]
pub struct HealthStats {
    /// Per-source statistics
    pub sources: std::collections::HashMap<Source, SourceStats>,
    /// Database insert lag (time since last successful write)
    pub db_lag: Option<Duration>,
    /// Start time of collection
    pub start_time: Option<std::time::Instant>,
}

impl HealthStats {
    /// Creates new health stats.
    pub fn new() -> Self {
        Self {
            sources: std::collections::HashMap::new(),
            db_lag: None,
            start_time: Some(std::time::Instant::now()),
        }
    }

    /// Records a successful collection.
    pub fn record_success(&mut self, source: Source) {
        let stats = self.sources.entry(source).or_default();
        stats.records += 1;
        stats.last_record = Some(chrono::Utc::now());
    }

    /// Records an error.
    pub fn record_error(&mut self, source: Source) {
        let stats = self.sources.entry(source).or_default();
        stats.errors += 1;
    }

    /// Returns total records collected across all sources.
    pub fn total_records(&self) -> u64 {
        self.sources.values().map(|s| s.records).sum()
    }

    /// Returns total errors across all sources.
    pub fn total_errors(&self) -> u64 {
        self.sources.values().map(|s| s.errors).sum()
    }

    /// Formats a summary for logging.
    pub fn format_summary(&self) -> String {
        let mut summary = String::new();
        summary.push_str("Health Stats:\n");

        if let Some(start) = self.start_time {
            let elapsed = start.elapsed();
            summary.push_str(&format!("  Uptime: {:?}\n", elapsed));
        }

        summary.push_str(&format!(
            "  Total: {} records, {} errors\n",
            self.total_records(),
            self.total_errors()
        ));

        for (source, stats) in &self.sources {
            summary.push_str(&format!(
                "  {}: {} records, {} errors\n",
                source.as_str(),
                stats.records,
                stats.errors
            ));
        }

        if let Some(lag) = self.db_lag {
            summary.push_str(&format!("  DB Lag: {:?}\n", lag));
        }

        summary
    }
}

/// Runs the collect-signals command.
///
/// # Errors
/// Returns an error if database connection fails or collectors cannot start.
pub async fn run_collect_signals(args: CollectSignalsArgs) -> Result<()> {
    use algo_trade_data::{
        CvdAggregateRecord, CvdRepository, FundingRateRecord, FundingRateRepository,
        LiquidationAggregateRecord, LiquidationRecord, LiquidationRepository, NewsEventRecord,
        NewsEventRepository, OrderBookRepository, OrderBookSnapshotRecord, PolymarketOddsRecord,
        PolymarketOddsRepository, TradeTickRecord, TradeTickRepository,
    };
    use algo_trade_polymarket::{OddsCollector, OddsCollectorConfig, PolymarketClient};
    use algo_trade_signals::{
        CollectorConfig, FundingCollector, LiquidationCollector, LiquidationCollectorConfig,
        NewsCollector, NewsCollectorConfig, OrderBookCollector, TradeTickCollector,
    };
    use sqlx::postgres::PgPoolOptions;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    // Parse arguments
    let duration = parse_duration(&args.duration)?;
    let sources = parse_sources(&args.sources)?;

    tracing::info!(
        "Starting signal collection for {:?} duration with sources: {:?}",
        duration,
        sources.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    );

    // Get database URL
    let db_url = args
        .db_url
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Create repositories
    let orderbook_repo = OrderBookRepository::new(pool.clone());
    let funding_repo = FundingRateRepository::new(pool.clone());
    let liquidation_repo = LiquidationRepository::new(pool.clone());
    let trade_tick_repo = TradeTickRepository::new(pool.clone());
    let cvd_repo = CvdRepository::new(pool.clone());
    let polymarket_repo = PolymarketOddsRepository::new(pool.clone());
    let news_repo = NewsEventRepository::new(pool.clone());

    // Shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Health stats
    let health_stats = Arc::new(tokio::sync::Mutex::new(HealthStats::new()));

    // Channel capacity
    const CHANNEL_SIZE: usize = 1000;

    // Task handles
    let mut handles = Vec::new();

    // Start collectors based on requested sources
    for source in &sources {
        match source {
            Source::OrderBook => {
                let (tx, rx) = mpsc::channel::<OrderBookSnapshotRecord>(CHANNEL_SIZE);
                let config = CollectorConfig::new(&args.symbol);
                let shutdown = shutdown.clone();
                let health = health_stats.clone();

                // Spawn collector
                let mut collector = OrderBookCollector::new(config, tx);
                handles.push(tokio::spawn(async move {
                    if let Err(e) = collector.run().await {
                        tracing::error!("OrderBook collector error: {}", e);
                    }
                }));

                // Spawn database writer
                let repo = orderbook_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::OrderBook).await;
                }));

                tracing::info!("Started OrderBook collector for {}", args.symbol);
            }

            Source::Funding => {
                let (tx, rx) = mpsc::channel::<FundingRateRecord>(CHANNEL_SIZE);
                let shutdown = shutdown.clone();
                let health = health_stats.clone();

                // Spawn collector for default symbols
                let mut collector = FundingCollector::new(FundingCollector::default_symbols(), tx);
                handles.push(tokio::spawn(async move {
                    if let Err(e) = collector.run().await {
                        tracing::error!("Funding collector error: {}", e);
                    }
                }));

                // Spawn database writer
                let repo = funding_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::Funding).await;
                }));

                tracing::info!("Started Funding collector");
            }

            Source::Liquidations => {
                let (tx, rx) = mpsc::channel::<LiquidationRecord>(CHANNEL_SIZE);
                let (agg_tx, agg_rx) = mpsc::channel::<LiquidationAggregateRecord>(CHANNEL_SIZE);
                let config = LiquidationCollectorConfig::for_symbol(&args.symbol)
                    .with_aggregate_interval(Duration::from_secs(60)); // Emit aggregates every 60s
                let shutdown = shutdown.clone();
                let shutdown_agg = shutdown.clone();
                let health = health_stats.clone();
                let health_agg = health_stats.clone();

                // Spawn collector with aggregate channel
                let mut collector =
                    LiquidationCollector::new(config, tx).with_aggregate_channel(agg_tx);
                handles.push(tokio::spawn(async move {
                    if let Err(e) = collector.run().await {
                        tracing::error!("Liquidation collector error: {}", e);
                    }
                }));

                // Spawn database writer for individual liquidations
                let repo = liquidation_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::Liquidations).await;
                }));

                // Spawn database writer for aggregates
                let repo_agg = liquidation_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(
                        agg_rx,
                        repo_agg,
                        shutdown_agg,
                        health_agg,
                        Source::Liquidations,
                    )
                    .await;
                }));

                tracing::info!(
                    "Started Liquidation collector for {} (with aggregate persistence)",
                    args.symbol
                );
            }

            Source::TradeTicks => {
                let (tx, rx) = mpsc::channel::<TradeTickRecord>(CHANNEL_SIZE);
                let (cvd_tx, cvd_rx) = mpsc::channel::<CvdAggregateRecord>(CHANNEL_SIZE);
                let config = CollectorConfig::new(&args.symbol);
                let shutdown = shutdown.clone();
                let shutdown_cvd = shutdown.clone();
                let health = health_stats.clone();
                let health_cvd = health_stats.clone();

                // Spawn collector with CVD aggregation (60-second windows)
                let mut collector =
                    TradeTickCollector::new(config, tx).with_cvd_channel(cvd_tx, 60);
                handles.push(tokio::spawn(async move {
                    if let Err(e) = collector.run().await {
                        tracing::error!("TradeTick collector error: {}", e);
                    }
                }));

                // Spawn database writer for individual trade ticks
                let repo = trade_tick_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::TradeTicks).await;
                }));

                // Spawn database writer for CVD aggregates
                let repo_cvd = cvd_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(
                        cvd_rx,
                        repo_cvd,
                        shutdown_cvd,
                        health_cvd,
                        Source::TradeTicks,
                    )
                    .await;
                }));

                tracing::info!(
                    "Started TradeTick collector for {} (with CVD aggregation)",
                    args.symbol
                );
            }

            Source::Polymarket => {
                let (tx, rx) = mpsc::channel::<PolymarketOddsRecord>(CHANNEL_SIZE);
                let client = PolymarketClient::new();
                let config = OddsCollectorConfig::default();
                let shutdown = shutdown.clone();
                let health = health_stats.clone();

                // Spawn collector
                let mut collector = OddsCollector::new(client, config, tx);
                handles.push(tokio::spawn(async move {
                    // run() already calls discover_markets() internally
                    if let Err(e) = collector.run().await {
                        tracing::error!("Polymarket collector error: {}", e);
                    }
                }));

                // Spawn database writer
                let repo = polymarket_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::Polymarket).await;
                }));

                tracing::info!("Started Polymarket odds collector");
            }

            Source::News => {
                let api_key = match &args.news_api_key {
                    Some(key) => key.clone(),
                    None => {
                        tracing::warn!("Skipping News collector: CRYPTOPANIC_API_KEY not provided");
                        continue;
                    }
                };

                let (tx, rx) = mpsc::channel::<NewsEventRecord>(CHANNEL_SIZE);
                let config = NewsCollectorConfig::new(&api_key);
                let shutdown = shutdown.clone();
                let health = health_stats.clone();

                // Spawn collector
                let mut collector = NewsCollector::new(config, tx);
                handles.push(tokio::spawn(async move {
                    if let Err(e) = collector.run().await {
                        tracing::error!("News collector error: {}", e);
                    }
                }));

                // Spawn database writer
                let repo = news_repo.clone();
                handles.push(tokio::spawn(async move {
                    run_database_writer(rx, repo, shutdown, health, Source::News).await;
                }));

                tracing::info!("Started News collector");
            }
        }
    }

    // Spawn health logger
    let health_clone = health_stats.clone();
    let shutdown_health = shutdown.clone();
    handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
        loop {
            interval.tick().await;
            if shutdown_health.load(Ordering::Relaxed) {
                break;
            }
            let stats = health_clone.lock().await;
            tracing::info!("{}", stats.format_summary());
        }
    }));

    // Wait for duration or shutdown signal
    tracing::info!(
        "Collection running. Will stop in {:?} or on Ctrl+C",
        duration
    );

    let timeout = tokio::time::sleep(duration);
    tokio::pin!(timeout);

    tokio::select! {
        _ = &mut timeout => {
            tracing::info!("Duration elapsed, initiating shutdown");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, initiating shutdown");
        }
    }

    // Signal shutdown
    shutdown_clone.store(true, Ordering::Relaxed);

    // Give tasks time to flush
    tracing::info!("Waiting for collectors to flush...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Abort remaining tasks
    for handle in handles {
        handle.abort();
    }

    // Print final stats
    let final_stats = health_stats.lock().await;
    tracing::info!("Final {}", final_stats.format_summary());

    tracing::info!("Signal collection complete");
    Ok(())
}

/// Trait for repositories that support batch insert.
#[async_trait::async_trait]
pub trait BatchInsert<T>: Send + Sync {
    /// Inserts a batch of records.
    async fn insert_batch(&self, records: &[T]) -> Result<()>;
}

// Implement BatchInsert for each repository type
#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::OrderBookSnapshotRecord>
    for algo_trade_data::OrderBookRepository
{
    async fn insert_batch(
        &self,
        records: &[algo_trade_data::OrderBookSnapshotRecord],
    ) -> Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::FundingRateRecord> for algo_trade_data::FundingRateRepository {
    async fn insert_batch(&self, records: &[algo_trade_data::FundingRateRecord]) -> Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::LiquidationRecord> for algo_trade_data::LiquidationRepository {
    async fn insert_batch(&self, records: &[algo_trade_data::LiquidationRecord]) -> Result<()> {
        self.insert_events_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::LiquidationAggregateRecord>
    for algo_trade_data::LiquidationRepository
{
    async fn insert_batch(
        &self,
        records: &[algo_trade_data::LiquidationAggregateRecord],
    ) -> Result<()> {
        self.insert_aggregates_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::PolymarketOddsRecord>
    for algo_trade_data::PolymarketOddsRepository
{
    async fn insert_batch(&self, records: &[algo_trade_data::PolymarketOddsRecord]) -> Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::NewsEventRecord> for algo_trade_data::NewsEventRepository {
    async fn insert_batch(&self, records: &[algo_trade_data::NewsEventRecord]) -> Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::TradeTickRecord> for algo_trade_data::TradeTickRepository {
    async fn insert_batch(&self, records: &[algo_trade_data::TradeTickRecord]) -> Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl BatchInsert<algo_trade_data::CvdAggregateRecord> for algo_trade_data::CvdRepository {
    async fn insert_batch(&self, records: &[algo_trade_data::CvdAggregateRecord]) -> Result<()> {
        self.insert_batch(records).await
    }
}

/// Runs a database writer actor that batches records and inserts them.
async fn run_database_writer<T, R>(
    mut rx: tokio::sync::mpsc::Receiver<T>,
    repo: R,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    health_stats: std::sync::Arc<tokio::sync::Mutex<HealthStats>>,
    source: Source,
) where
    T: Clone + Send + 'static,
    R: BatchInsert<T>,
{
    use std::sync::atomic::Ordering;

    const BATCH_SIZE: usize = 100;
    const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    let mut batch: Vec<T> = Vec::with_capacity(BATCH_SIZE);
    let mut flush_timer = tokio::time::interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            Some(record) = rx.recv() => {
                batch.push(record);

                if batch.len() >= BATCH_SIZE {
                    if let Err(e) = repo.insert_batch(&batch).await {
                        tracing::error!("Failed to insert {} batch: {}", source.as_str(), e);
                        let mut stats = health_stats.lock().await;
                        stats.record_error(source);
                    } else {
                        let mut stats = health_stats.lock().await;
                        for _ in &batch {
                            stats.record_success(source);
                        }
                    }
                    batch.clear();
                }
            }

            _ = flush_timer.tick() => {
                if shutdown.load(Ordering::Relaxed) {
                    // Final flush
                    if !batch.is_empty() {
                        if let Err(e) = repo.insert_batch(&batch).await {
                            tracing::error!("Failed to flush {} batch on shutdown: {}", source.as_str(), e);
                        } else {
                            tracing::info!("Flushed {} {} records on shutdown", batch.len(), source.as_str());
                        }
                    }
                    break;
                }

                // Periodic flush
                if !batch.is_empty() {
                    if let Err(e) = repo.insert_batch(&batch).await {
                        tracing::error!("Failed to flush {} batch: {}", source.as_str(), e);
                        let mut stats = health_stats.lock().await;
                        stats.record_error(source);
                    } else {
                        let mut stats = health_stats.lock().await;
                        for _ in &batch {
                            stats.record_success(source);
                        }
                    }
                    batch.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    // Duration Parsing Tests (TDD: Write tests first)
    // =====================================================================

    #[test]
    fn test_parse_duration_seconds() {
        let duration = parse_duration("30s").unwrap();
        assert_eq!(duration, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_minutes() {
        let duration = parse_duration("15m").unwrap();
        assert_eq!(duration, Duration::from_secs(15 * 60));
    }

    #[test]
    fn test_parse_duration_hours() {
        let duration = parse_duration("24h").unwrap();
        assert_eq!(duration, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_parse_duration_days() {
        let duration = parse_duration("7d").unwrap();
        assert_eq!(duration, Duration::from_secs(7 * 86400));
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        let duration = parse_duration("  12h  ").unwrap();
        assert_eq!(duration, Duration::from_secs(12 * 3600));
    }

    #[test]
    fn test_parse_duration_case_insensitive() {
        let duration_lower = parse_duration("1h").unwrap();
        let duration_upper = parse_duration("1H").unwrap();
        assert_eq!(duration_lower, duration_upper);
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        let duration = parse_duration("500ms").unwrap();
        assert_eq!(duration, Duration::from_millis(500));
    }

    #[test]
    fn test_parse_duration_large_value() {
        let duration = parse_duration("365d").unwrap();
        assert_eq!(duration, Duration::from_secs(365 * 86400));
    }

    #[test]
    fn test_parse_duration_empty_string() {
        let result = parse_duration("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_parse_duration_no_unit() {
        let result = parse_duration("100");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unit"));
    }

    #[test]
    fn test_parse_duration_invalid_unit() {
        let result = parse_duration("5x");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown duration unit"));
    }

    #[test]
    fn test_parse_duration_zero() {
        let result = parse_duration("0h");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("zero"));
    }

    #[test]
    fn test_parse_duration_negative() {
        // Negative numbers will fail to parse as u64
        let result = parse_duration("-5h");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_duration_no_number() {
        let result = parse_duration("h");
        assert!(result.is_err());
    }

    // =====================================================================
    // Source Parsing Tests
    // =====================================================================

    #[test]
    fn test_parse_sources_single() {
        let sources = parse_sources("orderbook").unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0], Source::OrderBook);
    }

    #[test]
    fn test_parse_sources_multiple() {
        let sources = parse_sources("orderbook,funding,liquidations").unwrap();
        assert_eq!(sources.len(), 3);
        assert!(sources.contains(&Source::OrderBook));
        assert!(sources.contains(&Source::Funding));
        assert!(sources.contains(&Source::Liquidations));
    }

    #[test]
    fn test_parse_sources_all() {
        let sources =
            parse_sources("orderbook,funding,liquidations,tradeticks,polymarket,news").unwrap();
        assert_eq!(sources.len(), 6);
    }

    #[test]
    fn test_parse_sources_empty_returns_all() {
        let sources = parse_sources("").unwrap();
        assert_eq!(sources.len(), 6);
    }

    #[test]
    fn test_parse_sources_all_keyword() {
        let sources = parse_sources("all").unwrap();
        assert_eq!(sources.len(), 6);
    }

    #[test]
    fn test_parse_sources_tradeticks() {
        let sources = parse_sources("tradeticks").unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0], Source::TradeTicks);
    }

    #[test]
    fn test_parse_sources_with_whitespace() {
        let sources = parse_sources("  orderbook , funding , liquidations  ").unwrap();
        assert_eq!(sources.len(), 3);
    }

    #[test]
    fn test_parse_sources_case_insensitive() {
        let sources = parse_sources("OrderBook,FUNDING,Liquidations").unwrap();
        assert_eq!(sources.len(), 3);
    }

    #[test]
    fn test_parse_sources_deduplicates() {
        let sources = parse_sources("orderbook,orderbook,funding,funding").unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn test_parse_sources_invalid() {
        let result = parse_sources("orderbook,invalid_source");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown source"));
    }

    #[test]
    fn test_parse_sources_only_commas() {
        let result = parse_sources(",,,");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("At least one"));
    }

    // =====================================================================
    // Source Enum Tests
    // =====================================================================

    #[test]
    fn test_source_all() {
        let all = Source::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&Source::OrderBook));
        assert!(all.contains(&Source::Funding));
        assert!(all.contains(&Source::Liquidations));
        assert!(all.contains(&Source::TradeTicks));
        assert!(all.contains(&Source::Polymarket));
        assert!(all.contains(&Source::News));
    }

    #[test]
    fn test_source_as_str() {
        assert_eq!(Source::OrderBook.as_str(), "orderbook");
        assert_eq!(Source::Funding.as_str(), "funding");
        assert_eq!(Source::Liquidations.as_str(), "liquidations");
        assert_eq!(Source::TradeTicks.as_str(), "tradeticks");
        assert_eq!(Source::Polymarket.as_str(), "polymarket");
        assert_eq!(Source::News.as_str(), "news");
    }

    #[test]
    fn test_source_all_includes_tradeticks() {
        let all = Source::all();
        assert!(all.contains(&Source::TradeTicks));
    }

    // =====================================================================
    // Health Stats Tests
    // =====================================================================

    #[test]
    fn test_health_stats_new() {
        let stats = HealthStats::new();
        assert_eq!(stats.total_records(), 0);
        assert_eq!(stats.total_errors(), 0);
        assert!(stats.start_time.is_some());
    }

    #[test]
    fn test_health_stats_record_success() {
        let mut stats = HealthStats::new();

        stats.record_success(Source::OrderBook);
        stats.record_success(Source::OrderBook);
        stats.record_success(Source::Funding);

        assert_eq!(stats.total_records(), 3);
        assert_eq!(stats.sources.get(&Source::OrderBook).unwrap().records, 2);
        assert_eq!(stats.sources.get(&Source::Funding).unwrap().records, 1);
    }

    #[test]
    fn test_health_stats_record_error() {
        let mut stats = HealthStats::new();

        stats.record_error(Source::News);
        stats.record_error(Source::News);

        assert_eq!(stats.total_errors(), 2);
        assert_eq!(stats.sources.get(&Source::News).unwrap().errors, 2);
    }

    #[test]
    fn test_health_stats_format_summary() {
        let mut stats = HealthStats::new();
        stats.record_success(Source::OrderBook);
        stats.record_error(Source::Funding);

        let summary = stats.format_summary();

        assert!(summary.contains("Health Stats"));
        assert!(summary.contains("Total: 1 records"));
        assert!(summary.contains("1 errors"));
    }

    // =====================================================================
    // CollectSignalsArgs Tests
    // =====================================================================

    #[test]
    fn test_collect_signals_args_defaults() {
        // Verify default values match expected behavior
        let args = CollectSignalsArgs {
            duration: "24h".to_string(),
            sources: "orderbook,funding,liquidations,polymarket,news".to_string(),
            symbol: "btcusdt".to_string(),
            db_url: None,
            news_api_key: None,
        };

        let duration = parse_duration(&args.duration).unwrap();
        let sources = parse_sources(&args.sources).unwrap();

        assert_eq!(duration, Duration::from_secs(24 * 3600));
        assert_eq!(sources.len(), 5);
        assert_eq!(args.symbol, "btcusdt");
    }
}
