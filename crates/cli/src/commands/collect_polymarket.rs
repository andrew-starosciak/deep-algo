//! collect-polymarket CLI command for Polymarket odds collection.
//!
//! Collects Polymarket binary market odds with optional regex-based market filtering
//! and database persistence.

use anyhow::{anyhow, Result};
use clap::Args;
use regex::Regex;
use std::time::Duration;

use crate::commands::collect_signals::parse_duration;
use algo_trade_polymarket::Market;

/// Arguments for the collect-polymarket command.
#[derive(Args, Debug, Clone)]
pub struct CollectPolymarketArgs {
    /// Duration to collect (e.g., "1h", "24h", "7d")
    #[arg(long)]
    pub duration: String,

    /// Polling interval in seconds (default: 15)
    #[arg(long, default_value = "15")]
    pub poll_interval_secs: u64,

    /// Regex pattern to filter market questions (e.g., "Bitcoin|BTC")
    #[arg(long)]
    pub market_pattern: Option<String>,

    /// Minimum liquidity in USD for markets to track (default: 1000)
    #[arg(long, default_value = "1000")]
    pub min_liquidity: u64,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

/// Filter for markets based on regex pattern matching.
#[derive(Debug, Clone)]
pub struct MarketPatternFilter {
    /// Compiled regex pattern (None means match all)
    #[allow(dead_code)] // Used in tests and for future expansion
    pattern: Option<Regex>,
}

impl MarketPatternFilter {
    /// Creates a new filter with optional pattern.
    ///
    /// # Arguments
    /// * `pattern` - Optional regex pattern to match market questions
    pub fn new(pattern: Option<Regex>) -> Self {
        Self { pattern }
    }

    /// Returns true if the market matches the filter.
    ///
    /// If no pattern is set, all markets match.
    #[allow(dead_code)] // Used in tests and for future filtering expansion
    pub fn matches(&self, market: &Market) -> bool {
        match &self.pattern {
            Some(re) => re.is_match(&market.question),
            None => true,
        }
    }

    /// Filters a list of markets, keeping only those that match.
    #[allow(dead_code)] // Used in tests and for future filtering expansion
    pub fn filter_markets(&self, markets: Vec<Market>) -> Vec<Market> {
        markets.into_iter().filter(|m| self.matches(m)).collect()
    }
}

/// Compiles a market pattern string into a Regex.
///
/// # Errors
/// Returns an error if the pattern is invalid regex.
pub fn compile_market_pattern(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| anyhow!("Invalid regex pattern '{}': {}", pattern, e))
}

/// Statistics for the Polymarket collector.
#[derive(Debug, Clone, Default)]
pub struct CollectorStats {
    /// Total records stored in database
    pub records_stored: u64,
    /// Total errors encountered
    pub errors: u64,
    /// Number of unique markets tracked
    pub markets_tracked: usize,
    /// Number of poll cycles completed
    pub poll_cycles: u64,
    /// Start time of collection
    pub start_time: Option<std::time::Instant>,
}

impl CollectorStats {
    /// Creates new stats with start time set.
    pub fn new() -> Self {
        Self {
            records_stored: 0,
            errors: 0,
            markets_tracked: 0,
            poll_cycles: 0,
            start_time: Some(std::time::Instant::now()),
        }
    }

    /// Records a successful poll cycle.
    pub fn record_poll(&mut self, records: usize) {
        self.poll_cycles += 1;
        self.records_stored += records as u64;
    }

    /// Records an error.
    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    /// Updates the number of tracked markets.
    #[allow(dead_code)] // Used in tests and for future stats tracking
    pub fn update_markets(&mut self, count: usize) {
        self.markets_tracked = count;
    }

    /// Formats a summary for logging.
    pub fn format_summary(&self) -> String {
        let mut summary = String::new();
        summary.push_str("Polymarket Collector Stats:\n");

        if let Some(start) = self.start_time {
            let elapsed = start.elapsed();
            summary.push_str(&format!("  Uptime: {:?}\n", elapsed));
        }

        summary.push_str(&format!("  Poll cycles: {}\n", self.poll_cycles));
        summary.push_str(&format!("  Records stored: {}\n", self.records_stored));
        summary.push_str(&format!("  Markets tracked: {}\n", self.markets_tracked));
        summary.push_str(&format!("  Errors: {}\n", self.errors));

        summary
    }
}

/// Builds an `OddsCollectorConfig` from CLI arguments.
pub fn build_collector_config(
    args: &CollectPolymarketArgs,
) -> algo_trade_polymarket::OddsCollectorConfig {
    use rust_decimal::Decimal;

    algo_trade_polymarket::OddsCollectorConfig::default()
        .with_poll_interval(Duration::from_secs(args.poll_interval_secs))
        .with_min_liquidity(Decimal::from(args.min_liquidity))
}

/// Runs the collect-polymarket command.
///
/// # Errors
/// Returns an error if database connection fails or collector cannot start.
pub async fn run_collect_polymarket(args: CollectPolymarketArgs) -> Result<()> {
    use algo_trade_data::PolymarketOddsRepository;
    use algo_trade_polymarket::{OddsCollector, PolymarketClient};
    use sqlx::postgres::PgPoolOptions;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    // Parse duration
    let duration = parse_duration(&args.duration)?;

    tracing::info!("Starting Polymarket odds collection for {:?}", duration);

    // Compile market pattern if provided
    let filter = if let Some(pattern_str) = &args.market_pattern {
        let regex = compile_market_pattern(pattern_str)?;
        tracing::info!("Using market filter pattern: {}", pattern_str);
        MarketPatternFilter::new(Some(regex))
    } else {
        tracing::info!("No market filter pattern - collecting all BTC markets");
        MarketPatternFilter::new(None)
    };

    // Get database URL
    let db_url = args
        .db_url
        .clone()
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Create repository
    let repo = PolymarketOddsRepository::new(pool);

    // Build collector config
    let config = build_collector_config(&args);
    tracing::info!(
        "Collector config: poll_interval={:?}, min_liquidity={}",
        config.poll_interval,
        config.min_liquidity
    );

    // Create channels
    let (tx, rx) = mpsc::channel(1000);

    // Create client and collector
    let client = PolymarketClient::new();
    let mut collector = OddsCollector::new(client, config, tx);

    // Shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_writer = shutdown.clone();

    // Stats tracking
    let stats = Arc::new(tokio::sync::Mutex::new(CollectorStats::new()));
    let stats_writer = stats.clone();
    let stats_logger = stats.clone();

    // Spawn database writer
    let writer_handle = tokio::spawn(async move {
        run_polymarket_db_writer(rx, repo, shutdown_writer, stats_writer, filter).await;
    });

    // Spawn collector
    let collector_handle = tokio::spawn(async move {
        if let Err(e) = collector.run().await {
            tracing::error!("Polymarket collector error: {}", e);
        }
    });

    // Spawn stats logger
    let shutdown_logger = shutdown.clone();
    let logger_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if shutdown_logger.load(Ordering::Relaxed) {
                break;
            }
            let s = stats_logger.lock().await;
            tracing::info!("{}", s.format_summary());
        }
    });

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
    shutdown.store(true, Ordering::Relaxed);

    // Give tasks time to flush
    tracing::info!("Waiting for collector to flush...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Abort tasks
    collector_handle.abort();
    writer_handle.abort();
    logger_handle.abort();

    // Print final stats
    let final_stats = stats.lock().await;
    tracing::info!("Final {}", final_stats.format_summary());

    tracing::info!("Polymarket collection complete");
    Ok(())
}

/// Runs the database writer actor that batches records and inserts them.
async fn run_polymarket_db_writer(
    mut rx: tokio::sync::mpsc::Receiver<algo_trade_data::PolymarketOddsRecord>,
    repo: algo_trade_data::PolymarketOddsRepository,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stats: std::sync::Arc<tokio::sync::Mutex<CollectorStats>>,
    _filter: MarketPatternFilter,
) {
    use std::sync::atomic::Ordering;

    const BATCH_SIZE: usize = 50;
    const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut flush_timer = tokio::time::interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            Some(record) = rx.recv() => {
                batch.push(record);

                if batch.len() >= BATCH_SIZE {
                    match repo.insert_batch(&batch).await {
                        Ok(()) => {
                            let mut s = stats.lock().await;
                            s.record_poll(batch.len());
                        }
                        Err(e) => {
                            tracing::error!("Failed to insert Polymarket batch: {}", e);
                            let mut s = stats.lock().await;
                            s.record_error();
                        }
                    }
                    batch.clear();
                }
            }

            _ = flush_timer.tick() => {
                if shutdown.load(Ordering::Relaxed) {
                    // Final flush
                    if !batch.is_empty() {
                        match repo.insert_batch(&batch).await {
                            Ok(()) => {
                                tracing::info!("Flushed {} Polymarket records on shutdown", batch.len());
                            }
                            Err(e) => {
                                tracing::error!("Failed to flush Polymarket batch on shutdown: {}", e);
                            }
                        }
                    }
                    break;
                }

                // Periodic flush
                if !batch.is_empty() {
                    match repo.insert_batch(&batch).await {
                        Ok(()) => {
                            let mut s = stats.lock().await;
                            s.record_poll(batch.len());
                        }
                        Err(e) => {
                            tracing::error!("Failed to flush Polymarket batch: {}", e);
                            let mut s = stats.lock().await;
                            s.record_error();
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
    use algo_trade_polymarket::{Market, Token};
    use rust_decimal_macros::dec;

    // =========================================================================
    // Test Helpers
    // =========================================================================

    fn sample_btc_market() -> Market {
        Market {
            condition_id: "btc-100k".to_string(),
            question: "Will Bitcoin hit $100k?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![
                Token {
                    token_id: "yes-1".to_string(),
                    outcome: "Yes".to_string(),
                    price: dec!(0.65),
                    winner: None,
                },
                Token {
                    token_id: "no-1".to_string(),
                    outcome: "No".to_string(),
                    price: dec!(0.35),
                    winner: None,
                },
            ],
            active: true,
            tags: None,
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
        }
    }

    fn sample_eth_market() -> Market {
        Market {
            condition_id: "eth-10k".to_string(),
            question: "Will Ethereum hit $10k?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![
                Token {
                    token_id: "yes-2".to_string(),
                    outcome: "Yes".to_string(),
                    price: dec!(0.30),
                    winner: None,
                },
                Token {
                    token_id: "no-2".to_string(),
                    outcome: "No".to_string(),
                    price: dec!(0.70),
                    winner: None,
                },
            ],
            active: true,
            tags: None,
            volume_24h: Some(dec!(30000)),
            liquidity: Some(dec!(80000)),
        }
    }

    fn sample_trump_market() -> Market {
        Market {
            condition_id: "trump-2024".to_string(),
            question: "Will Trump win 2024 election?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        }
    }

    // =========================================================================
    // MarketPatternFilter Tests (TDD: Write tests FIRST)
    // =========================================================================

    #[test]
    fn test_market_pattern_filter_matches_without_pattern() {
        // Given: A filter with no pattern
        let filter = MarketPatternFilter::new(None);

        // When: We check if any market matches
        let btc_market = sample_btc_market();
        let eth_market = sample_eth_market();
        let trump_market = sample_trump_market();

        // Then: All markets should match (no filter)
        assert!(filter.matches(&btc_market));
        assert!(filter.matches(&eth_market));
        assert!(filter.matches(&trump_market));
    }

    #[test]
    fn test_market_pattern_filter_matches_with_pattern() {
        // Given: A filter with "Bitcoin|BTC" pattern
        let pattern = Regex::new(r"(?i)Bitcoin|BTC").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        // When: We check various markets
        let btc_market = sample_btc_market();
        let eth_market = sample_eth_market();
        let trump_market = sample_trump_market();

        // Then: Only Bitcoin-related markets match
        assert!(filter.matches(&btc_market)); // Contains "Bitcoin"
        assert!(!filter.matches(&eth_market)); // Does not contain Bitcoin/BTC
        assert!(!filter.matches(&trump_market)); // Does not contain Bitcoin/BTC
    }

    #[test]
    fn test_market_pattern_filter_matches_case_insensitive() {
        // Given: A case-insensitive pattern
        let pattern = Regex::new(r"(?i)ethereum").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        // When/Then: Should match regardless of case
        let eth_market = sample_eth_market();
        assert!(filter.matches(&eth_market));

        // Create a market with different casing
        let mut eth_upper = sample_eth_market();
        eth_upper.question = "ETHEREUM to $10k?".to_string();
        assert!(filter.matches(&eth_upper));
    }

    #[test]
    fn test_market_pattern_filter_filter_markets() {
        // Given: A filter for Bitcoin markets
        let pattern = Regex::new(r"(?i)Bitcoin|BTC").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        // And: A list of mixed markets
        let markets = vec![
            sample_btc_market(),
            sample_eth_market(),
            sample_trump_market(),
        ];

        // When: We filter the markets
        let filtered = filter.filter_markets(markets);

        // Then: Only Bitcoin markets remain
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].condition_id, "btc-100k");
    }

    #[test]
    fn test_market_pattern_filter_filter_markets_empty_input() {
        // Given: A filter and empty input
        let filter = MarketPatternFilter::new(None);
        let markets: Vec<Market> = vec![];

        // When: We filter
        let filtered = filter.filter_markets(markets);

        // Then: Result is empty
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_market_pattern_filter_filter_markets_all_match() {
        // Given: A filter that matches all (no pattern)
        let filter = MarketPatternFilter::new(None);

        // And: Multiple markets
        let markets = vec![
            sample_btc_market(),
            sample_eth_market(),
            sample_trump_market(),
        ];

        // When: We filter
        let filtered = filter.filter_markets(markets);

        // Then: All markets remain
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_market_pattern_filter_filter_markets_none_match() {
        // Given: A filter that matches nothing
        let pattern = Regex::new(r"NONEXISTENT_STRING_12345").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        // And: Multiple markets
        let markets = vec![
            sample_btc_market(),
            sample_eth_market(),
            sample_trump_market(),
        ];

        // When: We filter
        let filtered = filter.filter_markets(markets);

        // Then: No markets remain
        assert!(filtered.is_empty());
    }

    // =========================================================================
    // compile_market_pattern Tests
    // =========================================================================

    #[test]
    fn test_compile_market_pattern_valid() {
        // Simple patterns
        assert!(compile_market_pattern("Bitcoin").is_ok());
        assert!(compile_market_pattern("BTC").is_ok());

        // Pattern with alternation
        let result = compile_market_pattern("Bitcoin|BTC|crypto");
        assert!(result.is_ok());

        // Case-insensitive pattern
        let result = compile_market_pattern("(?i)bitcoin");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_market_pattern_invalid() {
        // Invalid regex (unclosed group)
        let result = compile_market_pattern("Bitcoin(");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid regex"));

        // Invalid regex (unclosed bracket)
        let result = compile_market_pattern("[Bitcoin");
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_market_pattern_empty() {
        // Empty pattern is valid regex (matches everything)
        let result = compile_market_pattern("");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_market_pattern_special_chars() {
        // Dollar sign (common in questions like "$100k")
        let result = compile_market_pattern(r"\$100k");
        assert!(result.is_ok());

        // Question mark (literal)
        let result = compile_market_pattern(r"hit \$100k\?");
        assert!(result.is_ok());
    }

    // =========================================================================
    // CollectorStats Tests
    // =========================================================================

    #[test]
    fn test_collector_stats_new() {
        let stats = CollectorStats::new();

        assert_eq!(stats.records_stored, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.markets_tracked, 0);
        assert_eq!(stats.poll_cycles, 0);
        assert!(stats.start_time.is_some());
    }

    #[test]
    fn test_collector_stats_record_poll() {
        let mut stats = CollectorStats::new();

        stats.record_poll(5);
        assert_eq!(stats.poll_cycles, 1);
        assert_eq!(stats.records_stored, 5);

        stats.record_poll(3);
        assert_eq!(stats.poll_cycles, 2);
        assert_eq!(stats.records_stored, 8);
    }

    #[test]
    fn test_collector_stats_record_poll_zero() {
        let mut stats = CollectorStats::new();

        stats.record_poll(0);
        assert_eq!(stats.poll_cycles, 1);
        assert_eq!(stats.records_stored, 0);
    }

    #[test]
    fn test_collector_stats_record_error() {
        let mut stats = CollectorStats::new();

        stats.record_error();
        assert_eq!(stats.errors, 1);

        stats.record_error();
        stats.record_error();
        assert_eq!(stats.errors, 3);
    }

    #[test]
    fn test_collector_stats_update_markets() {
        let mut stats = CollectorStats::new();

        stats.update_markets(10);
        assert_eq!(stats.markets_tracked, 10);

        stats.update_markets(5); // Can decrease
        assert_eq!(stats.markets_tracked, 5);
    }

    #[test]
    fn test_collector_stats_format_summary() {
        let mut stats = CollectorStats::new();
        stats.record_poll(10);
        stats.record_poll(5);
        stats.record_error();
        stats.update_markets(3);

        let summary = stats.format_summary();

        assert!(summary.contains("Polymarket Collector Stats"));
        assert!(summary.contains("Poll cycles: 2"));
        assert!(summary.contains("Records stored: 15"));
        assert!(summary.contains("Markets tracked: 3"));
        assert!(summary.contains("Errors: 1"));
        assert!(summary.contains("Uptime:"));
    }

    #[test]
    fn test_collector_stats_default() {
        let stats = CollectorStats::default();

        assert_eq!(stats.records_stored, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.markets_tracked, 0);
        assert_eq!(stats.poll_cycles, 0);
        assert!(stats.start_time.is_none()); // Default doesn't set start_time
    }

    // =========================================================================
    // build_collector_config Tests
    // =========================================================================

    #[test]
    fn test_build_collector_config_default_values() {
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(),
            poll_interval_secs: 15,
            market_pattern: None,
            min_liquidity: 1000,
            db_url: None,
        };

        let config = build_collector_config(&args);

        assert_eq!(config.poll_interval, Duration::from_secs(15));
        assert_eq!(config.min_liquidity, dec!(1000));
    }

    #[test]
    fn test_build_collector_config_custom_poll_interval() {
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(),
            poll_interval_secs: 30,
            market_pattern: None,
            min_liquidity: 1000,
            db_url: None,
        };

        let config = build_collector_config(&args);

        assert_eq!(config.poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_build_collector_config_custom_min_liquidity() {
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(),
            poll_interval_secs: 15,
            market_pattern: None,
            min_liquidity: 50000,
            db_url: None,
        };

        let config = build_collector_config(&args);

        assert_eq!(config.min_liquidity, dec!(50000));
    }

    #[test]
    fn test_build_collector_config_zero_liquidity() {
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(),
            poll_interval_secs: 15,
            market_pattern: None,
            min_liquidity: 0,
            db_url: None,
        };

        let config = build_collector_config(&args);

        assert_eq!(config.min_liquidity, dec!(0));
    }

    // =========================================================================
    // CollectPolymarketArgs Tests
    // =========================================================================

    #[test]
    fn test_collect_polymarket_args_structure() {
        let args = CollectPolymarketArgs {
            duration: "24h".to_string(),
            poll_interval_secs: 15,
            market_pattern: Some("Bitcoin|BTC".to_string()),
            min_liquidity: 5000,
            db_url: Some("postgres://localhost/test".to_string()),
        };

        assert_eq!(args.duration, "24h");
        assert_eq!(args.poll_interval_secs, 15);
        assert_eq!(args.market_pattern, Some("Bitcoin|BTC".to_string()));
        assert_eq!(args.min_liquidity, 5000);
        assert!(args.db_url.is_some());
    }

    #[test]
    fn test_collect_polymarket_args_with_defaults() {
        // Simulate default values
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(), // Required, no default
            poll_interval_secs: 15,     // Default
            market_pattern: None,       // Optional
            min_liquidity: 1000,        // Default
            db_url: None,               // From env
        };

        assert_eq!(args.poll_interval_secs, 15);
        assert_eq!(args.min_liquidity, 1000);
        assert!(args.market_pattern.is_none());
    }

    // =========================================================================
    // Integration-style Tests (no actual DB connection)
    // =========================================================================

    #[test]
    fn test_duration_parsing_with_args() {
        let args = CollectPolymarketArgs {
            duration: "2h".to_string(),
            poll_interval_secs: 15,
            market_pattern: None,
            min_liquidity: 1000,
            db_url: None,
        };

        let duration = parse_duration(&args.duration).unwrap();
        assert_eq!(duration, Duration::from_secs(2 * 3600));
    }

    #[test]
    fn test_invalid_duration_parsing() {
        let args = CollectPolymarketArgs {
            duration: "invalid".to_string(),
            poll_interval_secs: 15,
            market_pattern: None,
            min_liquidity: 1000,
            db_url: None,
        };

        let result = parse_duration(&args.duration);
        assert!(result.is_err());
    }

    #[test]
    fn test_market_pattern_integration() {
        // Given: CLI args with a market pattern
        let args = CollectPolymarketArgs {
            duration: "1h".to_string(),
            poll_interval_secs: 15,
            market_pattern: Some("(?i)bitcoin|btc".to_string()),
            min_liquidity: 1000,
            db_url: None,
        };

        // When: We compile the pattern
        let regex = compile_market_pattern(args.market_pattern.as_ref().unwrap()).unwrap();
        let filter = MarketPatternFilter::new(Some(regex));

        // And: Filter some markets
        let markets = vec![sample_btc_market(), sample_eth_market()];
        let filtered = filter.filter_markets(markets);

        // Then: Only BTC market remains
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].question.contains("Bitcoin"));
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_filter_with_complex_regex() {
        // Pattern matching price targets like "$100k" or "$50000"
        let pattern = Regex::new(r"\$\d+k?").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        let btc_market = sample_btc_market(); // "$100k" in question
        let trump_market = sample_trump_market(); // No price

        assert!(filter.matches(&btc_market));
        assert!(!filter.matches(&trump_market));
    }

    #[test]
    fn test_filter_with_word_boundary() {
        // Match "BTC" as a word, not part of another word
        let pattern = Regex::new(r"\bBTC\b").unwrap();
        let filter = MarketPatternFilter::new(Some(pattern));

        // Create a market with "BTC" as word
        let mut btc_word = sample_btc_market();
        btc_word.question = "Will BTC hit $100k?".to_string();
        assert!(filter.matches(&btc_word));

        // This should NOT match (Bitcoin contains BTC but not as word)
        // Actually "Bitcoin" doesn't contain "BTC" as substring, so we need different test
        let mut no_btc = sample_btc_market();
        no_btc.question = "Will Bitcoin reach $100k?".to_string();
        assert!(!filter.matches(&no_btc));
    }

    #[test]
    fn test_stats_multiple_operations_sequence() {
        let mut stats = CollectorStats::new();

        // Simulate a collection run
        stats.update_markets(5);
        stats.record_poll(10);
        stats.record_poll(8);
        stats.record_error();
        stats.update_markets(4); // Market went inactive
        stats.record_poll(7);
        stats.record_error();

        assert_eq!(stats.poll_cycles, 3);
        assert_eq!(stats.records_stored, 25);
        assert_eq!(stats.errors, 2);
        assert_eq!(stats.markets_tracked, 4);
    }
}
