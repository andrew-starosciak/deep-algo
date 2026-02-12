//! Multi-coin directional runner for 15-minute binary options.
//!
//! Manages per-coin Binance spot feeds, Polymarket WebSocket order books,
//! reference trackers, and directional detectors. Emits the best signal
//! per check interval via an mpsc channel.
//!
//! # Architecture
//!
//! ```text
//! Binance WS (N coins) → SpotPriceTracker (per-coin Arc<RwLock>)
//!                               ↓
//!                     ReferenceTracker (window opening "price to beat")
//!                               ↓
//! Polymarket WS (2N tokens) → BookFeed (Up/Down asks for N coins)
//!                               ↓
//!                     DirectionalDetector.check() per coin
//!                               ↓
//!                     Rank by edge → best DirectionalSignal
//!                               ↓  mpsc channel
//!                     DirectionalExecutor
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::directional_runner::{
//!     DirectionalRunner, DirectionalRunnerConfig,
//! };
//!
//! let config = DirectionalRunnerConfig::default();
//! let (runner, mut signal_rx) = DirectionalRunner::new(config);
//!
//! tokio::spawn(async move {
//!     runner.run().await.expect("Runner failed");
//! });
//!
//! while let Some(signal) = signal_rx.recv().await {
//!     println!("Signal: {:?}", signal);
//! }
//! ```

use crate::arbitrage::data_service::DataServiceHandle;
use crate::arbitrage::directional_detector::{
    DirectionalConfig, DirectionalDetector, DirectionalSignal,
};
use crate::arbitrage::latency_detector::SpotPriceTracker;
use crate::arbitrage::reference_tracker::{ReferenceTracker, ReferenceTrackerConfig};
use crate::arbitrage::signal_aggregator::{SharedSignalMap, SignalAggregator};
use crate::arbitrage::spot_feed::{SpotPriceFeed, SpotPriceFeedConfig};
use crate::models::Coin;
use crate::websocket::{PolymarketWebSocket, WebSocketConfig};
use crate::GammaClient;
use chrono::Utc;
use nonzero_ext::nonzero;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// Errors from the directional runner.
#[derive(Error, Debug)]
pub enum DirectionalRunnerError {
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// API error.
    #[error("API error: {0}")]
    Api(String),

    /// Spot feed error.
    #[error("Spot feed error: {0}")]
    SpotFeed(String),

    /// Runner was stopped.
    #[error("Runner stopped")]
    Stopped,
}

/// Per-coin state managed by the runner.
struct CoinState {
    /// The coin this state tracks.
    coin: Coin,
    /// Shared spot price tracker (written by SpotPriceFeed, read by detection loop).
    spot_tracker: Arc<RwLock<SpotPriceTracker>>,
    /// Reference tracker for window opening prices.
    reference_tracker: ReferenceTracker,
    /// Directional detector instance.
    detector: DirectionalDetector,
    /// Current Up token ID (changes each window).
    up_token_id: String,
    /// Current Down token ID (changes each window).
    down_token_id: String,
}

/// Configuration for the directional runner.
#[derive(Debug, Clone)]
pub struct DirectionalRunnerConfig {
    /// Coins to monitor (default: btc, eth, sol, xrp).
    pub coins: Vec<Coin>,
    /// Directional detector configuration.
    pub detector_config: DirectionalConfig,
    /// Reference tracker configuration.
    pub reference_config: ReferenceTrackerConfig,
    /// How often to check for signals (milliseconds).
    pub check_interval_ms: u64,
    /// Signal channel buffer size.
    pub signal_buffer_size: usize,
    /// Gamma API rate limit (requests per minute).
    pub gamma_rate_limit: u32,
    /// Enable external signal aggregation (Binance order book, funding, liquidations).
    pub enable_signals: bool,
}

impl Default for DirectionalRunnerConfig {
    fn default() -> Self {
        Self {
            coins: vec![Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp],
            detector_config: DirectionalConfig::default(),
            reference_config: ReferenceTrackerConfig::default(),
            check_interval_ms: 200,
            signal_buffer_size: 100,
            gamma_rate_limit: 30,
            enable_signals: false,
        }
    }
}

/// Runner statistics exposed for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectionalRunnerStats {
    /// Number of detection loop iterations.
    pub checks_performed: u64,
    /// Total signals emitted.
    pub signals_emitted: u64,
    /// Signals by coin.
    pub signals_by_coin: HashMap<String, u64>,
    /// Current spot prices per coin.
    pub current_spot_prices: HashMap<String, f64>,
    /// Current reference prices per coin.
    pub current_reference_prices: HashMap<String, f64>,
    /// Current Up ask prices per coin.
    pub current_up_asks: HashMap<String, Decimal>,
    /// Current Down ask prices per coin.
    pub current_down_asks: HashMap<String, Decimal>,
    /// Windows seen.
    pub windows_seen: u64,
    /// Last signal time.
    pub last_signal_at: Option<chrono::DateTime<Utc>>,
    /// Runner start time.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Errors encountered.
    pub errors: u64,
}

/// Multi-coin directional trading runner.
pub struct DirectionalRunner {
    /// Configuration.
    config: DirectionalRunnerConfig,
    /// Gamma API client for market discovery.
    gamma_client: GammaClient,
    /// Signal sender.
    signal_tx: mpsc::Sender<DirectionalSignal>,
    /// Stop flag.
    should_stop: Arc<AtomicBool>,
    /// Statistics.
    stats: Arc<RwLock<DirectionalRunnerStats>>,
    /// Shared signal map from SignalAggregator (if enabled).
    signal_map: Option<SharedSignalMap>,
    /// External data service handle (if provided, skips creating own feeds).
    data_handle: Option<DataServiceHandle>,
}

impl DirectionalRunner {
    /// Creates a new directional runner.
    ///
    /// Returns the runner and a channel to receive signals.
    pub fn new(config: DirectionalRunnerConfig) -> (Self, mpsc::Receiver<DirectionalSignal>) {
        Self::build(config, None)
    }

    /// Creates a new directional runner backed by a shared [`DataServiceHandle`].
    ///
    /// When a data handle is provided, the runner uses shared spot trackers and
    /// signal map instead of spawning its own Binance feeds and signal aggregator.
    pub fn with_data_service(
        config: DirectionalRunnerConfig,
        data_handle: DataServiceHandle,
    ) -> (Self, mpsc::Receiver<DirectionalSignal>) {
        Self::build(config, Some(data_handle))
    }

    /// Shared constructor logic.
    fn build(
        config: DirectionalRunnerConfig,
        data_handle: Option<DataServiceHandle>,
    ) -> (Self, mpsc::Receiver<DirectionalSignal>) {
        let (signal_tx, signal_rx) = mpsc::channel(config.signal_buffer_size);

        let gamma_client = GammaClient::with_rate_limit(
            std::num::NonZeroU32::new(config.gamma_rate_limit).unwrap_or(nonzero!(30u32)),
        );

        let signal_map = data_handle.as_ref().map(|dh| dh.signal_map().clone());

        let runner = Self {
            config,
            gamma_client,
            signal_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(DirectionalRunnerStats::default())),
            signal_map,
            data_handle,
        };

        (runner, signal_rx)
    }

    /// Returns a handle to stop the runner.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.should_stop)
    }

    /// Returns access to statistics.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<DirectionalRunnerStats>> {
        Arc::clone(&self.stats)
    }

    /// Runs the directional trading scanner.
    pub async fn run(mut self) -> Result<(), DirectionalRunnerError> {
        if self.config.coins.is_empty() {
            return Err(DirectionalRunnerError::Config(
                "At least one coin is required".to_string(),
            ));
        }

        info!(
            coins = ?self.config.coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
            check_interval_ms = self.config.check_interval_ms,
            "Starting directional runner"
        );

        // Initialize stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Fetch initial markets from Gamma API
        let markets = self
            .gamma_client
            .get_15min_markets_for_coins(&self.config.coins)
            .await;

        if markets.is_empty() {
            return Err(DirectionalRunnerError::Api(
                "No 15-minute markets found for any coin".to_string(),
            ));
        }

        info!("Discovered {} markets from Gamma API", markets.len());

        // Build per-coin state
        let mut coin_states: Vec<CoinState> = Vec::new();
        let mut all_token_ids: Vec<String> = Vec::new();
        let mut spot_feeds: Vec<(SpotPriceFeed, Arc<AtomicBool>)> = Vec::new();

        for market in &markets {
            let coin = self.detect_coin_from_market_question(&market.question);
            let coin = match coin {
                Some(c) => c,
                None => {
                    warn!(question = %market.question, "Could not detect coin from market");
                    continue;
                }
            };

            let up_token = match market.up_token() {
                Some(t) => t,
                None => {
                    warn!(coin = coin.slug_prefix(), "No Up token for market");
                    continue;
                }
            };
            let down_token = match market.down_token() {
                Some(t) => t,
                None => {
                    warn!(coin = coin.slug_prefix(), "No Down token for market");
                    continue;
                }
            };

            info!(
                coin = coin.slug_prefix(),
                up_token = %up_token.token_id,
                down_token = %down_token.token_id,
                up_price = %up_token.price,
                down_price = %down_token.price,
                "Initialized coin state"
            );

            // Use shared tracker from DataService, or create own + feed
            let spot_tracker = if let Some(ref dh) = self.data_handle {
                match dh.spot_tracker(coin.slug_prefix()) {
                    Some(t) => Arc::clone(t),
                    None => {
                        warn!(coin = coin.slug_prefix(), "DataService has no tracker for coin, skipping");
                        continue;
                    }
                }
            } else {
                let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
                let spot_config = SpotPriceFeedConfig {
                    symbol: format!("{}usdt", coin.slug_prefix()),
                    max_reconnect_attempts: 0, // Unlimited reconnects
                    ..Default::default()
                };
                let feed = SpotPriceFeed::new(spot_config, Arc::clone(&tracker));
                let feed_stop = feed.stop_handle();
                spot_feeds.push((feed, feed_stop));
                tracker
            };

            all_token_ids.push(up_token.token_id.clone());
            all_token_ids.push(down_token.token_id.clone());

            coin_states.push(CoinState {
                coin,
                spot_tracker,
                reference_tracker: ReferenceTracker::new(self.config.reference_config.clone()),
                detector: DirectionalDetector::new(self.config.detector_config.clone()),
                up_token_id: up_token.token_id.clone(),
                down_token_id: down_token.token_id.clone(),
            });
        }

        if coin_states.is_empty() {
            return Err(DirectionalRunnerError::Config(
                "No valid coin states after market discovery".to_string(),
            ));
        }

        // Spawn spot feeds
        let should_stop = Arc::clone(&self.should_stop);
        let mut feed_handles = Vec::new();
        for (mut feed, _) in spot_feeds {
            let handle = tokio::spawn(async move {
                if let Err(e) = feed.run().await {
                    error!("Spot feed error: {}", e);
                }
            });
            feed_handles.push(handle);
        }

        // Spawn signal aggregator if enabled AND no DataService is providing signals
        if self.config.enable_signals && self.data_handle.is_none() {
            let signal_map: SharedSignalMap =
                Arc::new(tokio::sync::RwLock::new(HashMap::new()));
            let aggregator = SignalAggregator::new(
                self.config.coins.clone(),
                Arc::clone(&signal_map),
                Arc::clone(&self.should_stop),
            );
            info!("Spawning signal aggregator for composite signals");
            tokio::spawn(async move {
                aggregator.run().await;
            });
            self.signal_map = Some(signal_map);
        }

        // Connect Polymarket WebSocket for order books
        info!(
            "Connecting Polymarket WebSocket for {} tokens",
            all_token_ids.len()
        );
        let ws_config = WebSocketConfig {
            max_reconnect_attempts: 0, // Unlimited reconnects
            ..Default::default()
        };
        let (ws, mut _ws_rx) = match PolymarketWebSocket::connect(all_token_ids, ws_config).await {
            Ok(result) => result,
            Err(e) => {
                should_stop.store(true, Ordering::SeqCst);
                return Err(DirectionalRunnerError::Api(format!(
                    "WebSocket connect failed: {}",
                    e
                )));
            }
        };

        info!("Polymarket WebSocket connected, waiting for initial snapshots...");
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Run detection loop
        let result = self
            .detection_loop(&mut coin_states, &ws)
            .await;

        // Cleanup
        should_stop.store(true, Ordering::SeqCst);
        ws.shutdown().await;
        for handle in feed_handles {
            handle.abort();
        }

        result
    }

    /// Main detection loop: checks all coins every interval, emits best signal.
    async fn detection_loop(
        &self,
        coin_states: &mut [CoinState],
        ws: &PolymarketWebSocket,
    ) -> Result<(), DirectionalRunnerError> {
        // Read signal map reference (if signals enabled)
        let signal_map = self.signal_map.clone();
        let check_interval = Duration::from_millis(self.config.check_interval_ms);
        let mut last_window_start_ms: Option<i64> = None;
        let mut first_check_logged = false;

        info!("Detection loop started");

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("Detection loop stopping");
                return Ok(());
            }

            let now_ms = Utc::now().timestamp_millis();
            let mut best_signal: Option<DirectionalSignal> = None;
            let mut new_window_detected = false;

            for state in coin_states.iter_mut() {
                // Read spot price
                let tracker = state.spot_tracker.read().await;
                let spot_price = match tracker.current_price() {
                    Some(p) => p,
                    None => continue,
                };
                let spot_timestamp_ms = tracker.current_timestamp_ms().unwrap_or(now_ms);
                drop(tracker);

                // Update reference tracker with latest spot
                state
                    .reference_tracker
                    .update_price(spot_timestamp_ms, spot_price);

                // Get reference
                let reference = match state.reference_tracker.current_reference() {
                    Some(r) => r,
                    None => continue,
                };

                // Detect new window for stats
                if last_window_start_ms != Some(reference.window_start_ms) {
                    last_window_start_ms = Some(reference.window_start_ms);
                    let mut stats = self.stats.write().await;
                    stats.windows_seen += 1;
                    new_window_detected = true;

                    info!(
                        window_start = reference.window_start_ms,
                        "New 15-minute window detected"
                    );
                    break; // Will re-enter loop after resetting cooldowns
                }

                let reference_price = reference.reference_price;
                let time_remaining_secs = reference.time_remaining_ms(now_ms) / 1000;

                // Get order book prices from WebSocket
                let up_book = ws.get_book(&state.up_token_id);
                let down_book = ws.get_book(&state.down_token_id);

                let up_ask = up_book
                    .as_ref()
                    .and_then(|b| b.best_ask());
                let down_ask = down_book
                    .as_ref()
                    .and_then(|b| b.best_ask());

                let (up_ask, down_ask) = match (up_ask, down_ask) {
                    (Some(u), Some(d)) => (u, d),
                    _ => continue, // No book data yet
                };

                // Log first successful check
                if !first_check_logged {
                    info!(
                        coin = state.coin.slug_prefix(),
                        spot = format!("${:.2}", spot_price),
                        reference = format!("${:.2}", reference_price),
                        up_ask = %up_ask,
                        down_ask = %down_ask,
                        time_remaining = format!("{}s", time_remaining_secs),
                        "Data feeds connected - first check running"
                    );
                    first_check_logged = true;
                }

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    let key = state.coin.slug_prefix().to_uppercase();
                    stats.current_spot_prices.insert(key.clone(), spot_price);
                    stats
                        .current_reference_prices
                        .insert(key.clone(), reference_price);
                    stats.current_up_asks.insert(key.clone(), up_ask);
                    stats.current_down_asks.insert(key, down_ask);
                }

                // Read external signal for this coin (if available)
                let external_signal = if let Some(ref map) = signal_map {
                    let map_read = map.read().await;
                    map_read.get(state.coin.slug_prefix()).cloned()
                } else {
                    None
                };

                // Run detector
                let signal = state.detector.check(
                    state.coin.slug_prefix(),
                    spot_price,
                    reference_price,
                    up_ask,
                    down_ask,
                    &state.up_token_id,
                    &state.down_token_id,
                    time_remaining_secs,
                    now_ms,
                    external_signal.as_ref(),
                );

                if let Some(sig) = signal {
                    debug!(
                        coin = sig.coin,
                        direction = %sig.direction,
                        edge = format!("{:.4}", sig.estimated_edge),
                        confidence = format!("{:.3}", sig.confidence),
                        entry_price = %sig.entry_price,
                        "Signal detected"
                    );

                    // Keep best signal by edge
                    let is_better = best_signal
                        .as_ref()
                        .map_or(true, |best| sig.estimated_edge > best.estimated_edge);
                    if is_better {
                        best_signal = Some(sig);
                    }
                }
            }

            // Reset cooldowns on window transition (outside the per-coin loop)
            if new_window_detected {
                for state in coin_states.iter_mut() {
                    state.detector.reset_cooldown();
                }
                continue; // Re-enter detection loop with fresh cooldowns
            }

            // Emit best signal
            if let Some(signal) = best_signal {
                info!(
                    coin = signal.coin,
                    direction = %signal.direction,
                    edge = format!("{:.4}", signal.estimated_edge),
                    confidence = format!("{:.3}", signal.confidence),
                    entry_price = %signal.entry_price,
                    delta_pct = format!("{:+.4}%", signal.delta_pct * 100.0),
                    "DIRECTIONAL SIGNAL"
                );

                {
                    let mut stats = self.stats.write().await;
                    stats.signals_emitted += 1;
                    *stats
                        .signals_by_coin
                        .entry(signal.coin.clone())
                        .or_insert(0) += 1;
                    stats.last_signal_at = Some(signal.timestamp);
                }

                if self.signal_tx.send(signal).await.is_err() {
                    warn!("Signal channel closed");
                    return Err(DirectionalRunnerError::Stopped);
                }
            }

            // Update check counter
            {
                let mut stats = self.stats.write().await;
                stats.checks_performed += 1;
            }

            tokio::time::sleep(check_interval).await;
        }
    }

    /// Detects which coin a market belongs to from its question text.
    fn detect_coin_from_market_question(&self, question: &str) -> Option<Coin> {
        let q = question.to_lowercase();
        if q.contains("btc") || q.contains("bitcoin") {
            Some(Coin::Btc)
        } else if q.contains("eth") || q.contains("ethereum") {
            Some(Coin::Eth)
        } else if q.contains("sol") || q.contains("solana") {
            Some(Coin::Sol)
        } else if q.contains("xrp") || q.contains("ripple") {
            Some(Coin::Xrp)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = DirectionalRunnerConfig::default();
        assert_eq!(config.coins.len(), 4);
        assert_eq!(config.check_interval_ms, 200);
        assert_eq!(config.signal_buffer_size, 100);
    }

    #[test]
    fn test_runner_creation() {
        let config = DirectionalRunnerConfig::default();
        let (runner, _rx) = DirectionalRunner::new(config);
        assert!(!runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stop_handle() {
        let config = DirectionalRunnerConfig::default();
        let (runner, _rx) = DirectionalRunner::new(config);
        let stop = runner.stop_handle();
        stop.store(true, Ordering::SeqCst);
        assert!(runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stats_default() {
        let stats = DirectionalRunnerStats::default();
        assert_eq!(stats.checks_performed, 0);
        assert_eq!(stats.signals_emitted, 0);
        assert_eq!(stats.windows_seen, 0);
    }

    #[test]
    fn test_detect_coin_from_question() {
        let config = DirectionalRunnerConfig::default();
        let (runner, _rx) = DirectionalRunner::new(config);

        assert_eq!(
            runner.detect_coin_from_market_question("Will BTC go up?"),
            Some(Coin::Btc)
        );
        assert_eq!(
            runner.detect_coin_from_market_question("ETH price movement"),
            Some(Coin::Eth)
        );
        assert_eq!(
            runner.detect_coin_from_market_question("Solana next 15m"),
            Some(Coin::Sol)
        );
        assert_eq!(
            runner.detect_coin_from_market_question("XRP updown"),
            Some(Coin::Xrp)
        );
        assert_eq!(
            runner.detect_coin_from_market_question("Random market"),
            None
        );
    }
}
