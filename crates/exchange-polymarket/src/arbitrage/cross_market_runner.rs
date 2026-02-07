//! Cross-market correlation scanner runner.
//!
//! This module provides a continuous scanner that monitors all 4 coin markets
//! (BTC, ETH, SOL, XRP) for cross-market correlation opportunities.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │   Gamma API         │
//! │   (15-min markets)  │
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │ Market Snapshots    │
//! │ (BTC/ETH/SOL/XRP)   │
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │ CrossMarketDetector │
//! │ check() → Vec<Opp>  │
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │ Signal Channel +    │
//! │ Stats Tracking      │
//! └─────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::cross_market_runner::{
//!     CrossMarketRunner, CrossMarketRunnerConfig,
//! };
//!
//! let config = CrossMarketRunnerConfig::default();
//! let (runner, mut opp_rx) = CrossMarketRunner::new(config);
//!
//! // Spawn runner
//! tokio::spawn(async move {
//!     runner.run().await.expect("Runner failed");
//! });
//!
//! // Receive opportunities
//! while let Some(opp) = opp_rx.recv().await {
//!     println!("Opportunity: {}", opp.display_short());
//! }
//! ```

use crate::models::Coin;
use crate::websocket::{PolymarketWebSocket, WebSocketConfig};
use crate::{GammaClient, Market};
use chrono::Utc;
use nonzero_ext::nonzero;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::correlation_tracker::CorrelationTracker;
use super::cross_market_detector::CrossMarketDetector;
use super::cross_market_types::{
    CoinMarketSnapshot, CrossMarketConfig, CrossMarketOpportunity, TokenDepth,
};

/// Errors from the cross-market runner.
#[derive(Error, Debug)]
pub enum CrossMarketRunnerError {
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// API error.
    #[error("API error: {0}")]
    Api(String),

    /// Runner was stopped.
    #[error("Runner stopped")]
    Stopped,
}

/// Configuration for the cross-market runner.
#[derive(Debug, Clone)]
pub struct CrossMarketRunnerConfig {
    /// Cross-market detector configuration.
    pub detector_config: CrossMarketConfig,
    /// How often to scan markets (milliseconds).
    pub scan_interval_ms: u64,
    /// Opportunity channel buffer size.
    pub signal_buffer_size: usize,
    /// Rate limit for Gamma API (requests per minute).
    pub gamma_rate_limit: u32,
    /// Enable order book depth tracking via WebSocket.
    pub track_depth: bool,
}

impl Default for CrossMarketRunnerConfig {
    fn default() -> Self {
        Self {
            detector_config: CrossMarketConfig::default(),
            scan_interval_ms: 1_000, // Scan every 1 second
            signal_buffer_size: 100,
            gamma_rate_limit: 30, // 30 req/min for Gamma API
            track_depth: false,   // Disabled by default
        }
    }
}

impl CrossMarketRunnerConfig {
    /// Creates an aggressive config with faster scanning.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            detector_config: CrossMarketConfig::aggressive(),
            scan_interval_ms: 500, // Faster scanning
            signal_buffer_size: 200,
            gamma_rate_limit: 60, // Higher rate limit
            track_depth: false,
        }
    }

    /// Enables order book depth tracking.
    #[must_use]
    pub fn with_depth_tracking(mut self) -> Self {
        self.track_depth = true;
        self
    }
}

/// Runner statistics exposed for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrossMarketRunnerStats {
    /// Number of scan cycles performed.
    pub scans_performed: u64,
    /// Total opportunities detected.
    pub opportunities_detected: u64,
    /// Opportunities by coin pair.
    pub by_pair: std::collections::HashMap<String, u64>,
    /// Best spread seen.
    pub best_spread: Option<Decimal>,
    /// Lowest total cost seen.
    pub lowest_cost: Option<Decimal>,
    /// Last scan timestamp.
    pub last_scan_at: Option<chrono::DateTime<Utc>>,
    /// Runner start time.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Current market prices (for display).
    pub current_prices: std::collections::HashMap<String, (Decimal, Decimal)>,
    /// Latest market snapshots (for DB persistence of CLOB prices).
    pub current_snapshots: Vec<CoinMarketSnapshot>,
    /// Errors encountered.
    pub errors: u64,
}

impl CrossMarketRunnerStats {
    /// Records an opportunity.
    pub fn record_opportunity(&mut self, opp: &CrossMarketOpportunity) {
        self.opportunities_detected += 1;

        let pair_key = format!("{}/{}", opp.coin1, opp.coin2);
        *self.by_pair.entry(pair_key).or_insert(0) += 1;

        if self.best_spread.map_or(true, |best| opp.spread > best) {
            self.best_spread = Some(opp.spread);
        }

        if self.lowest_cost.map_or(true, |low| opp.total_cost < low) {
            self.lowest_cost = Some(opp.total_cost);
        }
    }
}

/// Cross-market correlation scanner runner.
pub struct CrossMarketRunner {
    /// Configuration.
    config: CrossMarketRunnerConfig,
    /// Gamma API client for fetching markets.
    gamma_client: GammaClient,
    /// Cross-market detector.
    detector: CrossMarketDetector,
    /// Opportunity sender.
    opp_tx: mpsc::Sender<CrossMarketOpportunity>,
    /// Stop flag.
    should_stop: Arc<AtomicBool>,
    /// Statistics.
    stats: Arc<RwLock<CrossMarketRunnerStats>>,
    /// WebSocket handle for order book depth (optional).
    ws_handle: Option<PolymarketWebSocket>,
    /// Current subscribed token IDs.
    subscribed_tokens: Arc<RwLock<Vec<String>>>,
}

impl CrossMarketRunner {
    /// Creates a new cross-market runner.
    ///
    /// Returns the runner and a channel to receive opportunities.
    pub fn new(config: CrossMarketRunnerConfig) -> (Self, mpsc::Receiver<CrossMarketOpportunity>) {
        let (opp_tx, opp_rx) = mpsc::channel(config.signal_buffer_size);

        let gamma_client = GammaClient::with_rate_limit(
            NonZeroU32::new(config.gamma_rate_limit).unwrap_or(nonzero!(30u32)),
        );
        let detector = CrossMarketDetector::new(config.detector_config.clone());

        let runner = Self {
            config,
            gamma_client,
            detector,
            opp_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(CrossMarketRunnerStats::default())),
            ws_handle: None,
            subscribed_tokens: Arc::new(RwLock::new(Vec::new())),
        };

        (runner, opp_rx)
    }

    /// Creates a new cross-market runner with a dynamic correlation tracker.
    ///
    /// The tracker provides real-time correlation estimates to the detector,
    /// replacing the static `assumed_correlation` config value.
    pub fn with_correlation_tracker(
        config: CrossMarketRunnerConfig,
        tracker: Arc<CorrelationTracker>,
    ) -> (Self, mpsc::Receiver<CrossMarketOpportunity>) {
        let (opp_tx, opp_rx) = mpsc::channel(config.signal_buffer_size);

        let gamma_client = GammaClient::with_rate_limit(
            NonZeroU32::new(config.gamma_rate_limit).unwrap_or(nonzero!(30u32)),
        );
        let detector =
            CrossMarketDetector::with_correlation_tracker(config.detector_config.clone(), tracker);

        let runner = Self {
            config,
            gamma_client,
            detector,
            opp_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(CrossMarketRunnerStats::default())),
            ws_handle: None,
            subscribed_tokens: Arc::new(RwLock::new(Vec::new())),
        };

        (runner, opp_rx)
    }

    /// Returns a handle to stop the runner.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.should_stop)
    }

    /// Returns access to statistics.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<CrossMarketRunnerStats>> {
        Arc::clone(&self.stats)
    }

    /// Runs the cross-market scanner.
    ///
    /// This will continuously scan for opportunities until stopped.
    pub async fn run(mut self) -> Result<(), CrossMarketRunnerError> {
        info!("Starting cross-market correlation scanner");
        info!(
            "Scanning {} coins: {:?}",
            self.config.detector_config.coins.len(),
            self.config.detector_config.coins
        );
        info!(
            "Max cost: ${}, Min spread: ${}, Depth tracking: {}",
            self.config.detector_config.max_total_cost,
            self.config.detector_config.min_spread,
            if self.config.track_depth { "ON" } else { "OFF" }
        );

        // Initialize stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        let scan_interval = Duration::from_millis(self.config.scan_interval_ms);

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("Cross-market runner stopped");
                // Clean up WebSocket
                if let Some(ws) = &self.ws_handle {
                    ws.shutdown().await;
                }
                return Ok(());
            }

            // Perform one scan cycle
            match self.scan_once().await {
                Ok(opportunities) => {
                    let mut stats = self.stats.write().await;
                    stats.scans_performed += 1;
                    stats.last_scan_at = Some(Utc::now());

                    for opp in opportunities {
                        stats.record_opportunity(&opp);

                        // Send to channel
                        if self.opp_tx.send(opp).await.is_err() {
                            warn!("Opportunity channel closed");
                            return Err(CrossMarketRunnerError::Stopped);
                        }
                    }
                }
                Err(e) => {
                    error!("Scan error: {}", e);
                    let mut stats = self.stats.write().await;
                    stats.errors += 1;
                }
            }

            tokio::time::sleep(scan_interval).await;
        }
    }

    /// Performs a single scan cycle.
    async fn scan_once(&mut self) -> Result<Vec<CrossMarketOpportunity>, CrossMarketRunnerError> {
        // Fetch all current 15-minute markets
        let markets = self
            .gamma_client
            .get_15min_markets_for_coins(&self.config.detector_config.coins)
            .await;

        if markets.is_empty() {
            debug!("No markets found");
            return Ok(Vec::new());
        }

        // If depth tracking is enabled, ensure we have WebSocket connected to these tokens
        if self.config.track_depth {
            self.ensure_websocket_connected(&markets).await;
        }

        // Convert markets to snapshots (with depth if available)
        let now_ms = Utc::now().timestamp_millis();
        let snapshots: Vec<CoinMarketSnapshot> = markets
            .iter()
            .filter_map(|m| self.market_to_snapshot_with_depth(m, now_ms))
            .collect();

        // Update current prices and snapshots in stats
        {
            let mut stats = self.stats.write().await;
            for snapshot in &snapshots {
                stats.current_prices.insert(
                    snapshot.coin.slug_prefix().to_uppercase(),
                    (snapshot.up_price, snapshot.down_price),
                );
            }
            stats.current_snapshots = snapshots.clone();
        }

        if snapshots.len() < 2 {
            debug!("Not enough markets for cross-market analysis");
            return Ok(Vec::new());
        }

        // Run detector
        let opportunities = self.detector.check(&snapshots, now_ms);

        if !opportunities.is_empty() {
            debug!("Found {} opportunities", opportunities.len());
        }

        Ok(opportunities)
    }

    /// Ensures WebSocket is connected to all market tokens.
    async fn ensure_websocket_connected(&mut self, markets: &[Market]) {
        // Collect all token IDs we need
        let mut token_ids: Vec<String> = Vec::new();
        for market in markets {
            if let Some(up) = market.up_token() {
                token_ids.push(up.token_id.clone());
            }
            if let Some(down) = market.down_token() {
                token_ids.push(down.token_id.clone());
            }
        }

        // Check if we need to reconnect (token list changed)
        let current_tokens = self.subscribed_tokens.read().await;
        let needs_reconnect = token_ids.len() != current_tokens.len()
            || !token_ids.iter().all(|t| current_tokens.contains(t));
        drop(current_tokens);

        if needs_reconnect && !token_ids.is_empty() {
            // Shutdown existing connection
            if let Some(ws) = &self.ws_handle {
                ws.shutdown().await;
            }

            // Connect to new tokens
            let config = WebSocketConfig {
                max_reconnect_attempts: 3,
                ..Default::default()
            };

            match PolymarketWebSocket::connect(token_ids.clone(), config).await {
                Ok((ws, mut rx)) => {
                    info!("WebSocket connected for {} tokens", token_ids.len());
                    self.ws_handle = Some(ws);

                    // Update subscribed tokens
                    let mut current = self.subscribed_tokens.write().await;
                    *current = token_ids;

                    // Spawn a task to drain events (we just need the book state)
                    tokio::spawn(async move {
                        while rx.recv().await.is_some() {
                            // Events are processed, book state is maintained in ws_handle
                        }
                    });

                    // Give WebSocket time to receive initial snapshots
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    warn!("Failed to connect WebSocket for depth tracking: {}", e);
                    self.ws_handle = None;
                }
            }
        }
    }

    /// Converts a Market to a CoinMarketSnapshot with optional depth data.
    ///
    /// Uses WebSocket order book prices when available (real-time CLOB prices),
    /// falling back to Gamma API prices if WebSocket is not connected.
    fn market_to_snapshot_with_depth(
        &self,
        market: &Market,
        timestamp_ms: i64,
    ) -> Option<CoinMarketSnapshot> {
        let coin = self.detect_coin_from_market(market)?;
        let up_token = market.up_token()?;
        let down_token = market.down_token()?;

        // Default prices from Gamma API (often stale at ~0.50)
        let mut up_price = up_token.price;
        let mut down_price = down_token.price;

        // Get depth and real-time prices from WebSocket if available
        let (up_depth, down_depth) = if let Some(ws) = &self.ws_handle {
            let up_book = ws.get_book(&up_token.token_id);
            let down_book = ws.get_book(&down_token.token_id);

            // Use WebSocket best ask price for buying (what we'd actually pay)
            // Fall back to mid price if no asks, then to Gamma API price
            if let Some(ref book) = up_book {
                if let Some(ask) = book.best_ask() {
                    up_price = ask;
                } else if let Some(mid) = book.mid_price() {
                    up_price = mid;
                }
            }
            if let Some(ref book) = down_book {
                if let Some(ask) = book.best_ask() {
                    down_price = ask;
                } else if let Some(mid) = book.mid_price() {
                    down_price = mid;
                }
            }

            let up_depth = up_book.map(|book| {
                let spread_bps = book
                    .spread()
                    .map(|s| {
                        let mid = book.mid_price().unwrap_or(dec!(0.5));
                        if mid > Decimal::ZERO {
                            s / mid * dec!(10000)
                        } else {
                            Decimal::ZERO
                        }
                    })
                    .unwrap_or(Decimal::ZERO);

                TokenDepth {
                    bid_depth: book.total_bid_depth(),
                    ask_depth: book.total_ask_depth(),
                    spread_bps,
                }
            });

            let down_depth = down_book.map(|book| {
                let spread_bps = book
                    .spread()
                    .map(|s| {
                        let mid = book.mid_price().unwrap_or(dec!(0.5));
                        if mid > Decimal::ZERO {
                            s / mid * dec!(10000)
                        } else {
                            Decimal::ZERO
                        }
                    })
                    .unwrap_or(Decimal::ZERO);

                TokenDepth {
                    bid_depth: book.total_bid_depth(),
                    ask_depth: book.total_ask_depth(),
                    spread_bps,
                }
            });

            (up_depth, down_depth)
        } else {
            (None, None)
        };

        if up_price <= Decimal::ZERO || down_price <= Decimal::ZERO {
            return None;
        }

        Some(CoinMarketSnapshot {
            coin,
            up_price,
            down_price,
            up_token_id: up_token.token_id.clone(),
            down_token_id: down_token.token_id.clone(),
            timestamp_ms,
            up_depth,
            down_depth,
        })
    }

    /// Detects which coin a market belongs to based on its question text.
    fn detect_coin_from_market(&self, market: &Market) -> Option<Coin> {
        let question_lower = market.question.to_lowercase();

        if question_lower.contains("btc") || question_lower.contains("bitcoin") {
            Some(Coin::Btc)
        } else if question_lower.contains("eth") || question_lower.contains("ethereum") {
            Some(Coin::Eth)
        } else if question_lower.contains("sol") || question_lower.contains("solana") {
            Some(Coin::Sol)
        } else if question_lower.contains("xrp") || question_lower.contains("ripple") {
            Some(Coin::Xrp)
        } else {
            None
        }
    }
}

/// Convenience function to run the scanner with default config.
pub async fn run_cross_market_scanner(
    duration: Duration,
) -> Result<CrossMarketRunnerStats, CrossMarketRunnerError> {
    let config = CrossMarketRunnerConfig::default();
    let (runner, mut opp_rx) = CrossMarketRunner::new(config);
    let stop_handle = runner.stop_handle();
    let stats = runner.stats();

    // Spawn runner
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // Set up timeout
    let timeout = tokio::time::sleep(duration);
    tokio::pin!(timeout);

    // Collect opportunities
    let mut opportunities = Vec::new();
    loop {
        tokio::select! {
            _ = &mut timeout => {
                info!("Duration elapsed");
                break;
            }
            opp = opp_rx.recv() => {
                match opp {
                    Some(o) => {
                        info!("Opportunity: {}", o.display_short());
                        opportunities.push(o);
                    }
                    None => break,
                }
            }
        }
    }

    // Stop runner
    stop_handle.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(Duration::from_secs(5), runner_handle).await;

    let final_stats = stats.read().await.clone();
    Ok(final_stats)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn config_default_values() {
        let config = CrossMarketRunnerConfig::default();
        assert_eq!(config.scan_interval_ms, 1_000);
        assert_eq!(config.signal_buffer_size, 100);
        assert_eq!(config.gamma_rate_limit, 30);
    }

    #[test]
    fn config_aggressive_faster() {
        let default = CrossMarketRunnerConfig::default();
        let aggressive = CrossMarketRunnerConfig::aggressive();
        assert!(aggressive.scan_interval_ms < default.scan_interval_ms);
    }

    #[test]
    fn runner_creation() {
        let config = CrossMarketRunnerConfig::default();
        let (runner, _rx) = CrossMarketRunner::new(config);
        assert!(!runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn stop_handle_works() {
        let config = CrossMarketRunnerConfig::default();
        let (runner, _rx) = CrossMarketRunner::new(config);
        let stop = runner.stop_handle();
        stop.store(true, Ordering::SeqCst);
        assert!(runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn stats_record_opportunity() {
        let mut stats = CrossMarketRunnerStats::default();
        let opp = CrossMarketOpportunity {
            coin1: "BTC".to_string(),
            coin2: "ETH".to_string(),
            combination: super::super::cross_market_types::CrossMarketCombination::BothUp,
            leg1_direction: "UP".to_string(),
            leg1_price: dec!(0.40),
            leg1_token_id: "t1".to_string(),
            leg2_direction: "UP".to_string(),
            leg2_price: dec!(0.35),
            leg2_token_id: "t2".to_string(),
            total_cost: dec!(0.75),
            spread: dec!(0.25),
            expected_value: dec!(0.05),
            assumed_correlation: 0.85,
            win_probability: 0.95,
            detected_at: Utc::now(),
            leg1_bid_depth: None,
            leg1_ask_depth: None,
            leg1_spread_bps: None,
            leg2_bid_depth: None,
            leg2_ask_depth: None,
            leg2_spread_bps: None,
        };

        stats.record_opportunity(&opp);

        assert_eq!(stats.opportunities_detected, 1);
        assert_eq!(stats.by_pair.get("BTC/ETH"), Some(&1));
        assert_eq!(stats.best_spread, Some(dec!(0.25)));
        assert_eq!(stats.lowest_cost, Some(dec!(0.75)));
    }

    #[test]
    fn detect_coin_from_question() {
        let config = CrossMarketRunnerConfig::default();
        let (runner, _rx) = CrossMarketRunner::new(config);

        // Create mock markets
        let btc_market = Market {
            condition_id: "btc".to_string(),
            question: "Will BTC go up?".to_string(),
            description: None,
            end_date: None,
            tokens: vec![],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: None,
        };

        let eth_market = Market {
            condition_id: "eth".to_string(),
            question: "ETH price movement".to_string(),
            ..btc_market.clone()
        };

        assert_eq!(runner.detect_coin_from_market(&btc_market), Some(Coin::Btc));
        assert_eq!(runner.detect_coin_from_market(&eth_market), Some(Coin::Eth));
    }
}
