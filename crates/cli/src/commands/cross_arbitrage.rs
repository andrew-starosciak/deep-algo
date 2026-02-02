//! Cross-exchange arbitrage CLI command for Kalshi/Polymarket.
//!
//! Implements Week 3 of the cross-exchange arbitrage system, providing CLI scaffolding
//! for detecting and executing arbitrage opportunities between Kalshi and Polymarket.
//!
//! ## Overview
//!
//! The cross-arbitrage bot monitors matched markets on both exchanges for opportunities
//! where buying opposing positions guarantees profit regardless of outcome.
//!
//! ## Trading Modes
//!
//! - **Monitor**: Only detects and logs opportunities (default)
//! - **Paper**: Simulates execution without real trades
//! - **Live**: Executes real trades on both exchanges
//!
//! ## Example Usage
//!
//! ```bash
//! # Monitor mode - detect and log opportunities
//! cargo run -p algo-trade-cli -- cross-arbitrage --mode monitor
//!
//! # Paper trading with conservative settings
//! cargo run -p algo-trade-cli -- cross-arbitrage --mode paper --max-position 50
//!
//! # Live trading (requires API credentials)
//! cargo run -p algo-trade-cli -- cross-arbitrage --mode live --duration 4h
//! ```

use anyhow::{anyhow, Result};
use chrono::Utc;
use clap::{Args, ValueEnum};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::commands::collect_signals::parse_duration;
use algo_trade_arbitrage_cross::{
    Comparison, CrossExchangeDetector, CrossExecutorConfig, DetectorConfig, MarketMatcher,
    MatchConfig, ParsedKalshiMarket, ParsedPolymarketMarket, SettlementReconciler,
};
use algo_trade_kalshi::{Orderbook as KalshiOrderbook, PriceLevel};
use algo_trade_polymarket::arbitrage::types::L2OrderBook;
use algo_trade_polymarket::gamma::GammaClient;
use algo_trade_polymarket::models::Coin;

/// Trading mode for the cross-arbitrage bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum CrossTradingMode {
    /// Monitor only - detect and log opportunities
    #[default]
    Monitor,
    /// Paper trading - simulate execution
    Paper,
    /// Live trading - execute real trades
    Live,
}

impl std::fmt::Display for CrossTradingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CrossTradingMode::Monitor => write!(f, "monitor"),
            CrossTradingMode::Paper => write!(f, "paper"),
            CrossTradingMode::Live => write!(f, "live"),
        }
    }
}

/// Arguments for the cross-arbitrage command.
#[derive(Args, Debug, Clone)]
pub struct CrossArbitrageArgs {
    /// Trading mode: monitor, paper, or live
    #[arg(long, default_value = "monitor", value_enum)]
    pub mode: CrossTradingMode,

    /// Duration to run the bot (e.g., "2h", "1d", "1w")
    ///
    /// If not specified, runs indefinitely until Ctrl+C.
    #[arg(long)]
    pub duration: Option<String>,

    /// Maximum position size per market (in shares/contracts)
    #[arg(long, default_value = "100")]
    pub max_position: Decimal,

    /// Maximum daily volume (in dollars)
    #[arg(long, default_value = "1000")]
    pub max_daily_volume: Decimal,

    /// Maximum daily loss before stopping (in dollars)
    #[arg(long, default_value = "50")]
    pub max_daily_loss: Decimal,

    /// Minimum edge percentage to consider an opportunity
    #[arg(long, default_value = "0.5")]
    pub min_edge_pct: Decimal,

    /// Minimum profit per pair (in dollars)
    #[arg(long, default_value = "0.01")]
    pub min_profit: Decimal,

    /// Settlement confidence threshold (0.0-1.0)
    ///
    /// Only execute if settlement verification confidence exceeds this.
    #[arg(long, default_value = "0.95")]
    pub settlement_confidence: f64,

    /// Maximum concurrent positions across all markets
    #[arg(long, default_value = "3")]
    pub max_concurrent_positions: u32,

    /// Polling interval in milliseconds
    #[arg(long, default_value = "1000")]
    pub poll_interval_ms: u64,

    /// Cooldown between executions in seconds
    #[arg(long, default_value = "30")]
    pub cooldown_secs: u64,
}

impl CrossArbitrageArgs {
    /// Creates a detector configuration from arguments.
    fn create_detector_config(&self) -> DetectorConfig {
        // Convert edge percentage to decimal (e.g., 0.5% -> 0.005)
        let min_net_edge = self.min_edge_pct / Decimal::from(100);

        DetectorConfig::default()
            .with_min_net_edge(min_net_edge)
            .with_max_size(self.max_position)
    }

    /// Creates an executor configuration from arguments.
    fn create_executor_config(&self) -> CrossExecutorConfig {
        CrossExecutorConfig::conservative()
            .with_settlement_confidence_threshold(self.settlement_confidence)
            .with_max_position_per_market(self.max_position)
            .with_max_daily_loss(self.max_daily_loss)
    }
}

/// Configuration summary for logging.
struct ConfigSummary<'a> {
    args: &'a CrossArbitrageArgs,
}

impl<'a> ConfigSummary<'a> {
    fn new(args: &'a CrossArbitrageArgs) -> Self {
        Self { args }
    }

    fn log(&self) {
        tracing::info!("========================================");
        tracing::info!("  CROSS-EXCHANGE ARBITRAGE CONFIG       ");
        tracing::info!("========================================");
        tracing::info!("Mode:                  {}", self.args.mode);
        tracing::info!("----------------------------------------");
        tracing::info!("Thresholds:");
        tracing::info!("  Min Edge:            {}%", self.args.min_edge_pct);
        tracing::info!("  Min Profit/Pair:     ${}", self.args.min_profit);
        tracing::info!("  Settlement Conf:     {:.0}%", self.args.settlement_confidence * 100.0);
        tracing::info!("----------------------------------------");
        tracing::info!("Sizing:");
        tracing::info!("  Max Position:        {} contracts", self.args.max_position);
        tracing::info!("  Max Daily Volume:    ${}", self.args.max_daily_volume);
        tracing::info!("  Max Daily Loss:      ${}", self.args.max_daily_loss);
        tracing::info!("  Max Concurrent Pos:  {}", self.args.max_concurrent_positions);
        tracing::info!("----------------------------------------");
        tracing::info!("Timing:");
        tracing::info!("  Poll Interval:       {}ms", self.args.poll_interval_ms);
        tracing::info!("  Cooldown:            {}s", self.args.cooldown_secs);
        if let Some(ref dur) = self.args.duration {
            tracing::info!("  Duration:            {}", dur);
        } else {
            tracing::info!("  Duration:            indefinite");
        }
        tracing::info!("========================================");
    }
}

/// Statistics for the cross-arbitrage session.
#[derive(Debug, Default)]
struct CrossArbitrageStats {
    kalshi_markets_matched: usize,
    polymarket_markets_matched: usize,
    polls_completed: u64,
    opportunities_detected: u64,
    opportunities_executed: u64,
    partial_fills: u64,
    settlement_mismatches: u64,
    circuit_breaker_trips: u64,
    total_invested: Decimal,
    total_profit: Decimal,
    daily_pnl: Decimal,
}

impl CrossArbitrageStats {
    fn log_summary(&self, elapsed: Duration) {
        let elapsed_mins = elapsed.as_secs_f64() / 60.0;
        tracing::info!("========================================");
        tracing::info!("         SESSION SUMMARY                ");
        tracing::info!("========================================");
        tracing::info!("Runtime:              {:.1} minutes", elapsed_mins);
        tracing::info!("Markets Matched:");
        tracing::info!("  Kalshi:             {}", self.kalshi_markets_matched);
        tracing::info!("  Polymarket:         {}", self.polymarket_markets_matched);
        tracing::info!("Polls Completed:      {}", self.polls_completed);
        tracing::info!("----------------------------------------");
        tracing::info!("Opportunities:");
        tracing::info!("  Detected:           {}", self.opportunities_detected);
        tracing::info!("  Executed:           {}", self.opportunities_executed);
        tracing::info!("  Partial Fills:      {}", self.partial_fills);
        tracing::info!("----------------------------------------");
        tracing::info!("Safety Events:");
        tracing::info!("  Settlement Mismatches: {}", self.settlement_mismatches);
        tracing::info!("  Circuit Breaker Trips: {}", self.circuit_breaker_trips);
        tracing::info!("----------------------------------------");
        tracing::info!("Financials:");
        tracing::info!("  Total Invested:     ${}", self.total_invested);
        tracing::info!("  Total Profit:       ${}", self.total_profit);
        tracing::info!("  Daily P&L:          ${}", self.daily_pnl);
        tracing::info!("========================================");
    }
}

/// Runs the cross-arbitrage bot command.
///
/// # Errors
///
/// Returns an error if:
/// - Duration parsing fails
/// - Market discovery fails
/// - API connection fails
pub async fn run_cross_arbitrage(args: CrossArbitrageArgs) -> Result<()> {
    // Validate live mode requirements
    if args.mode == CrossTradingMode::Live {
        // Check for required environment variables
        if std::env::var("KALSHI_EMAIL").is_err() || std::env::var("KALSHI_PASSWORD").is_err() {
            tracing::warn!("Live mode requires KALSHI_EMAIL and KALSHI_PASSWORD environment variables");
            return Err(anyhow!(
                "Missing Kalshi credentials. Set KALSHI_EMAIL and KALSHI_PASSWORD."
            ));
        }
        if std::env::var("POLYMARKET_PRIVATE_KEY").is_err() {
            tracing::warn!("Live mode requires POLYMARKET_PRIVATE_KEY environment variable");
            return Err(anyhow!(
                "Missing Polymarket credentials. Set POLYMARKET_PRIVATE_KEY."
            ));
        }
    }

    // Parse duration if provided
    let run_duration = if let Some(ref dur_str) = args.duration {
        let dur = parse_duration(dur_str)?;
        Some(std::time::Duration::from_secs(dur.as_secs() as u64))
    } else {
        None
    };

    // Create configurations
    let detector_config = args.create_detector_config();
    let _executor_config = args.create_executor_config();

    // Log configuration
    let config_summary = ConfigSummary::new(&args);
    config_summary.log();

    // Initialize detector
    let detector = CrossExchangeDetector::with_config(detector_config);
    let reconciler = SettlementReconciler::new();

    // Initialize stats
    let mut stats = CrossArbitrageStats::default();
    let start_time = std::time::Instant::now();

    // Set up shutdown signal
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("Received Ctrl+C, initiating graceful shutdown...");
            running_clone.store(false, Ordering::SeqCst);
        }
    });

    // Discover and match markets
    tracing::info!("Discovering markets on Kalshi and Polymarket...");

    // Discover Polymarket BTC markets via Gamma API
    let gamma_client = GammaClient::new();
    let poly_markets = discover_polymarket_markets(&gamma_client).await;
    stats.polymarket_markets_matched = poly_markets.len();
    tracing::info!("Found {} Polymarket BTC markets", poly_markets.len());

    // For Kalshi, we need API credentials - create mock markets for monitor mode
    let kalshi_markets = discover_kalshi_markets_mock();
    stats.kalshi_markets_matched = kalshi_markets.len();
    tracing::info!("Found {} Kalshi BTC markets (mock data for demo)", kalshi_markets.len());

    // Match markets across exchanges
    let matcher = MarketMatcher::with_config(MatchConfig::relaxed());
    let matched_markets = matcher.find_btc_matches(&kalshi_markets, &poly_markets);
    tracing::info!("Matched {} cross-exchange market pairs", matched_markets.len());

    if matched_markets.is_empty() {
        tracing::warn!("No matched markets found - detection loop will be idle");
        tracing::info!(
            "Note: Full market matching requires Kalshi API credentials"
        );
    } else {
        for matched in &matched_markets {
            tracing::info!(
                "  Matched: {} <-> {} (confidence: {:.2}%)",
                matched.kalshi_ticker,
                &matched.polymarket_condition_id[..20.min(matched.polymarket_condition_id.len())],
                matched.match_confidence * 100.0
            );
        }
    }

    // Create mock orderbooks for demonstration
    let mut poly_yes_books: HashMap<String, L2OrderBook> = HashMap::new();
    let mut poly_no_books: HashMap<String, L2OrderBook> = HashMap::new();

    // Initialize mock orderbooks for matched markets
    for matched in &matched_markets {
        let yes_book = create_mock_polymarket_book(&matched.polymarket_yes_token);
        let no_book = create_mock_polymarket_book(&matched.polymarket_no_token);
        poly_yes_books.insert(matched.polymarket_condition_id.clone(), yes_book);
        poly_no_books.insert(matched.polymarket_condition_id.clone(), no_book);
    }

    // Main loop
    let poll_interval = Duration::from_millis(args.poll_interval_ms);
    let cooldown_duration = Duration::from_secs(args.cooldown_secs);
    #[allow(unused_variables)]
    let last_execution: Option<std::time::Instant> = None;

    tracing::info!("Starting cross-arbitrage detection loop...");
    tracing::info!("Press Ctrl+C to stop");

    while running.load(Ordering::SeqCst) {
        // Check duration limit
        if let Some(max_duration) = run_duration {
            if start_time.elapsed() >= max_duration {
                tracing::info!("Duration limit reached, stopping...");
                break;
            }
        }

        // Check daily loss limit
        if stats.daily_pnl < -args.max_daily_loss {
            tracing::error!(
                "Daily loss limit of ${} exceeded (P&L: ${}), stopping...",
                args.max_daily_loss,
                stats.daily_pnl
            );
            stats.circuit_breaker_trips += 1;
            break;
        }

        // Check cooldown
        if let Some(last) = last_execution {
            if last.elapsed() < cooldown_duration {
                let remaining = cooldown_duration - last.elapsed();
                tokio::time::sleep(remaining).await;
                continue;
            }
        }

        // Poll markets
        stats.polls_completed += 1;

        if stats.polls_completed % 10 == 0 {
            tracing::debug!(
                "Poll #{}: Checking {} matched markets for opportunities...",
                stats.polls_completed,
                matched_markets.len()
            );
        }

        // Check each matched market for arbitrage opportunities
        for matched in &matched_markets {
            // Get orderbooks (in real implementation, these would be fetched live)
            let kalshi_book = create_mock_kalshi_book(&matched.kalshi_ticker);

            let poly_yes_book = poly_yes_books
                .get(&matched.polymarket_condition_id)
                .cloned()
                .unwrap_or_else(|| L2OrderBook::new(matched.polymarket_yes_token.clone()));

            let poly_no_book = poly_no_books
                .get(&matched.polymarket_condition_id)
                .cloned()
                .unwrap_or_else(|| L2OrderBook::new(matched.polymarket_no_token.clone()));

            // Run detection
            if let Some(opportunity) = detector.detect(
                matched,
                &kalshi_book,
                &poly_yes_book,
                &poly_no_book,
            ) {
                stats.opportunities_detected += 1;

                tracing::info!(
                    "OPPORTUNITY: {} | Kalshi {} @ {} + Poly {} @ {} = {} cost | Net edge: {}% | Profit: ${}",
                    matched.kalshi_ticker,
                    opportunity.kalshi_side,
                    opportunity.kalshi_price,
                    opportunity.polymarket_side,
                    opportunity.polymarket_price,
                    opportunity.combined_cost,
                    opportunity.net_edge_pct,
                    opportunity.expected_profit
                );

                // In paper/live mode, execute the opportunity
                match args.mode {
                    CrossTradingMode::Monitor => {
                        // Just log, don't execute
                    }
                    CrossTradingMode::Paper | CrossTradingMode::Live => {
                        // TODO: Execute via CrossExchangeExecutor
                        tracing::info!("Execution would happen here in {} mode", args.mode);
                        stats.opportunities_executed += 1;
                    }
                }
            }
        }

        // Randomize mock orderbooks slightly for next poll (simulate market movement)
        for book in poly_yes_books.values_mut() {
            simulate_book_movement(book);
        }
        for book in poly_no_books.values_mut() {
            simulate_book_movement(book);
        }

        // Sleep before next poll
        tokio::time::sleep(poll_interval).await;
    }

    // Log reconciler summary
    let summary = reconciler.summary();
    tracing::info!("Reconciler: {} open, {} reconciled, ${} P&L",
        summary.open_positions,
        summary.reconciled_positions,
        summary.total_pnl
    );

    // Log final stats
    let elapsed = start_time.elapsed();
    stats.log_summary(elapsed);

    tracing::info!("Cross-arbitrage bot stopped");
    Ok(())
}

// =============================================================================
// Market Discovery Helpers
// =============================================================================

/// Discovers Polymarket BTC markets via Gamma API.
async fn discover_polymarket_markets(gamma: &GammaClient) -> Vec<ParsedPolymarketMarket> {
    let mut markets = Vec::new();

    // Get current 15-minute BTC market
    match gamma.get_current_15min_market(Coin::Btc).await {
        Ok(market) => {
            let settlement_time = market.end_date;
            let yes_token_id = market
                .up_token()
                .map(|t| t.token_id.clone())
                .unwrap_or_default();
            let no_token_id = market
                .down_token()
                .map(|t| t.token_id.clone())
                .unwrap_or_default();

            let parsed = ParsedPolymarketMarket {
                condition_id: market.condition_id.clone(),
                yes_token_id,
                no_token_id,
                underlying: Some("BTC".to_string()),
                strike_price: None, // 15-min markets don't have fixed strike
                settlement_time,
            };
            tracing::debug!(
                "Discovered Polymarket BTC 15-min market: {} (settles {:?})",
                market.condition_id,
                settlement_time
            );
            markets.push(parsed);
        }
        Err(e) => {
            tracing::warn!("Failed to fetch BTC 15-min market: {}", e);
        }
    }

    markets
}

/// Creates mock Kalshi markets for demonstration.
///
/// In production, this would call KalshiClient::get_tradeable_btc_markets().
fn discover_kalshi_markets_mock() -> Vec<(ParsedKalshiMarket, chrono::DateTime<Utc>)> {
    let now = Utc::now();
    let settlement_time = now + chrono::Duration::minutes(15);

    vec![
        (
            ParsedKalshiMarket {
                ticker: "KXBTC-26FEB02-B100000".to_string(),
                underlying: "BTC".to_string(),
                strike_price: dec!(100000),
                direction: Comparison::Above,
                settlement_hint: Some(settlement_time),
            },
            settlement_time,
        ),
        (
            ParsedKalshiMarket {
                ticker: "KXBTC-26FEB02-B95000".to_string(),
                underlying: "BTC".to_string(),
                strike_price: dec!(95000),
                direction: Comparison::Above,
                settlement_hint: Some(settlement_time),
            },
            settlement_time,
        ),
        (
            ParsedKalshiMarket {
                ticker: "KXBTC-26FEB02-B105000".to_string(),
                underlying: "BTC".to_string(),
                strike_price: dec!(105000),
                direction: Comparison::Above,
                settlement_hint: Some(settlement_time),
            },
            settlement_time,
        ),
    ]
}

/// Creates a mock Polymarket L2 orderbook for testing.
fn create_mock_polymarket_book(token_id: &str) -> L2OrderBook {
    use std::cmp::Reverse;
    use std::collections::BTreeMap;

    let mut book = L2OrderBook::new(token_id.to_string());

    // Create realistic bid/ask spread
    let mut bids = BTreeMap::new();
    let mut asks = BTreeMap::new();

    // Bids: 0.48, 0.47, 0.46
    bids.insert(Reverse(dec!(0.48)), dec!(500));
    bids.insert(Reverse(dec!(0.47)), dec!(1000));
    bids.insert(Reverse(dec!(0.46)), dec!(1500));

    // Asks: 0.52, 0.53, 0.54
    asks.insert(dec!(0.52), dec!(500));
    asks.insert(dec!(0.53), dec!(1000));
    asks.insert(dec!(0.54), dec!(1500));

    book.bids = bids;
    book.asks = asks;
    book
}

/// Creates a mock Kalshi orderbook for testing.
fn create_mock_kalshi_book(ticker: &str) -> KalshiOrderbook {
    KalshiOrderbook {
        ticker: ticker.to_string(),
        yes_bids: vec![
            PriceLevel { price: 47, count: 500 },
            PriceLevel { price: 46, count: 1000 },
            PriceLevel { price: 45, count: 1500 },
        ],
        yes_asks: vec![
            PriceLevel { price: 53, count: 500 },
            PriceLevel { price: 54, count: 1000 },
            PriceLevel { price: 55, count: 1500 },
        ],
        timestamp: Utc::now(),
    }
}

/// Simulates market movement by slightly adjusting orderbook levels.
///
/// Uses a simple deterministic pattern based on timestamp to avoid external dependencies.
fn simulate_book_movement(book: &mut L2OrderBook) {
    use std::cmp::Reverse;

    // Use timestamp-based pseudo-randomness (deterministic but varies per call)
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i32)
        .unwrap_or(0);
    let delta = ((tick % 3) - 1) as i64; // -1, 0, or 1

    // Adjust best bid
    if let Some((&Reverse(best_bid), &size)) = book.bids.iter().next() {
        let new_bid = (best_bid + Decimal::from(delta) / dec!(100)).max(dec!(0.01)).min(dec!(0.98));
        book.bids.remove(&Reverse(best_bid));
        book.bids.insert(Reverse(new_bid), size);
    }

    // Adjust best ask
    if let Some((&best_ask, &size)) = book.asks.iter().next() {
        let new_ask = (best_ask + Decimal::from(delta) / dec!(100)).max(dec!(0.02)).min(dec!(0.99));
        book.asks.remove(&best_ask);
        book.asks.insert(new_ask, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trading_mode_display() {
        assert_eq!(CrossTradingMode::Monitor.to_string(), "monitor");
        assert_eq!(CrossTradingMode::Paper.to_string(), "paper");
        assert_eq!(CrossTradingMode::Live.to_string(), "live");
    }

    #[test]
    fn test_trading_mode_default() {
        assert_eq!(CrossTradingMode::default(), CrossTradingMode::Monitor);
    }

    #[test]
    fn test_args_create_detector_config() {
        use rust_decimal_macros::dec;

        let args = CrossArbitrageArgs {
            mode: CrossTradingMode::Paper,
            duration: None,
            max_position: dec!(200),
            max_daily_volume: dec!(2000),
            max_daily_loss: dec!(100),
            min_edge_pct: dec!(1.0),
            min_profit: dec!(0.02),
            settlement_confidence: 0.98,
            max_concurrent_positions: 5,
            poll_interval_ms: 500,
            cooldown_secs: 60,
        };

        let config = args.create_detector_config();
        // 1.0% -> 0.01 as decimal
        assert_eq!(config.min_net_edge, dec!(0.01));
        assert_eq!(config.max_size, dec!(200));
    }

    #[test]
    fn test_args_create_executor_config() {
        use rust_decimal_macros::dec;

        let args = CrossArbitrageArgs {
            mode: CrossTradingMode::Live,
            duration: Some("4h".to_string()),
            max_position: dec!(150),
            max_daily_volume: dec!(1500),
            max_daily_loss: dec!(75),
            min_edge_pct: dec!(0.8),
            min_profit: dec!(0.015),
            settlement_confidence: 0.99,
            max_concurrent_positions: 4,
            poll_interval_ms: 1000,
            cooldown_secs: 45,
        };

        let config = args.create_executor_config();
        assert!((config.settlement_confidence_threshold - 0.99).abs() < 0.001);
        assert_eq!(config.max_position_per_market, dec!(150));
        assert_eq!(config.max_daily_loss, dec!(75));
    }
}
