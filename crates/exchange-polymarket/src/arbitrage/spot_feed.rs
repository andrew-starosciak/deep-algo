//! Real-time BTC spot price feed from Binance for latency detection.
//!
//! This module connects to Binance Futures WebSocket to stream BTC price
//! updates and feed them into the `SpotPriceTracker` for latency arbitrage.
//!
//! # Architecture
//!
//! ```text
//! Binance WebSocket (aggTrade)
//!         │
//!         ▼
//! SpotPriceFeed::run()
//!         │
//!         ▼
//! SpotPriceTracker (shared Arc<RwLock>)
//!         │
//!         ▼
//! LatencyDetector::check()
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::spot_feed::{SpotPriceFeed, SpotPriceFeedConfig};
//! use algo_trade_polymarket::arbitrage::latency_detector::SpotPriceTracker;
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//!
//! let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
//! let feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker.clone());
//!
//! // Spawn feed in background
//! tokio::spawn(async move {
//!     feed.run().await.expect("Feed failed");
//! });
//!
//! // Read prices from tracker
//! let price = tracker.read().await.current_price();
//! ```

use crate::arbitrage::latency_detector::SpotPriceTracker;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// Binance Futures WebSocket endpoint.
const BINANCE_FUTURES_WS: &str = "wss://fstream.binance.com/ws/";

/// Errors from the spot price feed.
#[derive(Error, Debug)]
pub enum SpotFeedError {
    /// WebSocket connection failed.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// JSON parsing failed.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    /// Connection was closed.
    #[error("Connection closed: {0}")]
    Closed(String),

    /// Max reconnect attempts exceeded.
    #[error("Max reconnect attempts ({0}) exceeded")]
    MaxReconnects(u32),
}

/// Configuration for the spot price feed.
#[derive(Debug, Clone)]
pub struct SpotPriceFeedConfig {
    /// Symbol to track (default: "btcusdt").
    pub symbol: String,
    /// Reconnect delay after disconnect.
    pub reconnect_delay: Duration,
    /// Maximum reconnect attempts (0 = unlimited).
    pub max_reconnect_attempts: u32,
}

impl Default for SpotPriceFeedConfig {
    fn default() -> Self {
        Self {
            symbol: "btcusdt".to_string(),
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_attempts: 10,
        }
    }
}

/// Binance aggTrade WebSocket message.
#[derive(Debug, Deserialize)]
struct AggTradeEvent {
    /// Event type ("aggTrade").
    #[serde(rename = "e")]
    event_type: String,
    /// Price as string.
    #[serde(rename = "p")]
    price: String,
    /// Trade time in milliseconds.
    #[serde(rename = "T")]
    trade_time: i64,
}

/// Statistics for the spot price feed.
#[derive(Debug, Clone, Default)]
pub struct SpotFeedStats {
    /// Total messages received.
    pub messages_received: u64,
    /// Parse errors encountered.
    pub parse_errors: u64,
    /// Reconnect count.
    pub reconnects: u32,
    /// Last update timestamp.
    pub last_update: Option<DateTime<Utc>>,
}

/// Real-time BTC spot price feed from Binance.
pub struct SpotPriceFeed {
    /// Configuration.
    config: SpotPriceFeedConfig,
    /// Shared price tracker.
    tracker: Arc<RwLock<SpotPriceTracker>>,
    /// Feed statistics.
    stats: SpotFeedStats,
    /// Whether the feed should stop.
    should_stop: Arc<std::sync::atomic::AtomicBool>,
}

impl SpotPriceFeed {
    /// Creates a new spot price feed.
    pub fn new(config: SpotPriceFeedConfig, tracker: Arc<RwLock<SpotPriceTracker>>) -> Self {
        Self {
            config,
            tracker,
            stats: SpotFeedStats::default(),
            should_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Returns a handle to signal the feed to stop.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.should_stop.clone()
    }

    /// Returns the current statistics.
    #[must_use]
    pub fn stats(&self) -> &SpotFeedStats {
        &self.stats
    }

    /// Builds the WebSocket URL.
    fn build_url(&self) -> String {
        format!("{}{}@aggTrade", BINANCE_FUTURES_WS, self.config.symbol)
    }

    /// Runs the feed with automatic reconnection.
    pub async fn run(&mut self) -> Result<(), SpotFeedError> {
        let mut reconnect_attempts = 0u32;

        loop {
            if self.should_stop.load(std::sync::atomic::Ordering::SeqCst) {
                info!("Spot price feed stopping on request");
                return Ok(());
            }

            match self.connect_and_stream().await {
                Ok(()) => {
                    info!("Spot price feed exiting cleanly");
                    return Ok(());
                }
                Err(e) => {
                    error!("Spot price feed error: {}", e);
                    self.stats.reconnects += 1;
                    reconnect_attempts += 1;

                    if self.config.max_reconnect_attempts > 0
                        && reconnect_attempts >= self.config.max_reconnect_attempts
                    {
                        return Err(SpotFeedError::MaxReconnects(reconnect_attempts));
                    }

                    warn!(
                        "Reconnecting in {:?} (attempt {})",
                        self.config.reconnect_delay, reconnect_attempts
                    );
                    tokio::time::sleep(self.config.reconnect_delay).await;
                }
            }
        }
    }

    /// Connects to WebSocket and streams price updates.
    async fn connect_and_stream(&mut self) -> Result<(), SpotFeedError> {
        let url = self.build_url();
        info!("Connecting to Binance spot feed: {}", url);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await?;
        info!("Connected to Binance spot feed");

        let (_, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            if self.should_stop.load(std::sync::atomic::Ordering::SeqCst) {
                info!("Spot price feed stopping on request");
                return Ok(());
            }

            match msg {
                Ok(Message::Text(text)) => {
                    self.handle_message(&text).await;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping");
                    // Pong is handled automatically by tungstenite
                    let _ = data;
                }
                Ok(Message::Close(frame)) => {
                    let reason = frame
                        .map(|f| f.reason.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    warn!("WebSocket closed: {}", reason);
                    return Err(SpotFeedError::Closed(reason));
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    return Err(SpotFeedError::WebSocket(e));
                }
                _ => {}
            }
        }

        Err(SpotFeedError::Closed("Stream ended".to_string()))
    }

    /// Handles a single WebSocket message.
    async fn handle_message(&mut self, text: &str) {
        self.stats.messages_received += 1;

        match serde_json::from_str::<AggTradeEvent>(text) {
            Ok(event) => {
                if event.event_type != "aggTrade" {
                    return;
                }

                // Parse price
                if let Ok(price) = Decimal::from_str(&event.price) {
                    let price_f64 = price.to_string().parse::<f64>().unwrap_or(0.0);

                    // Update tracker
                    let mut tracker = self.tracker.write().await;
                    tracker.update(price_f64, event.trade_time);

                    self.stats.last_update = DateTime::from_timestamp_millis(event.trade_time);

                    debug!(
                        price = %price,
                        timestamp_ms = event.trade_time,
                        "Updated spot price"
                    );
                }
            }
            Err(e) => {
                self.stats.parse_errors += 1;
                if self.stats.parse_errors <= 5 {
                    warn!("Failed to parse spot price message: {}", e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = SpotPriceFeedConfig::default();
        assert_eq!(config.symbol, "btcusdt");
        assert_eq!(config.reconnect_delay, Duration::from_secs(1));
        assert_eq!(config.max_reconnect_attempts, 10);
    }

    #[test]
    fn test_build_url() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker);

        let url = feed.build_url();
        assert!(url.contains("btcusdt@aggTrade"));
        assert!(url.starts_with("wss://fstream.binance.com"));
    }

    #[test]
    fn test_stats_default() {
        let stats = SpotFeedStats::default();
        assert_eq!(stats.messages_received, 0);
        assert_eq!(stats.parse_errors, 0);
        assert_eq!(stats.reconnects, 0);
        assert!(stats.last_update.is_none());
    }

    #[tokio::test]
    async fn test_feed_stop_handle() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker);

        let stop = feed.stop_handle();
        assert!(!stop.load(std::sync::atomic::Ordering::SeqCst));

        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(stop.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_handle_message_valid() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let mut feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker.clone());

        let msg = r#"{"e":"aggTrade","E":1234567890123,"s":"BTCUSDT","a":123,"p":"105000.50","q":"0.1","f":1,"l":1,"T":1234567890000,"m":false}"#;

        feed.handle_message(msg).await;

        let t = tracker.read().await;
        assert_eq!(t.current_price(), Some(105000.50));
        assert_eq!(t.current_timestamp_ms(), Some(1234567890000));
    }

    #[tokio::test]
    async fn test_handle_message_invalid() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let mut feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker.clone());

        feed.handle_message("invalid json").await;

        assert_eq!(feed.stats.parse_errors, 1);
        assert!(tracker.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_price_updates() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let mut feed = SpotPriceFeed::new(SpotPriceFeedConfig::default(), tracker.clone());

        let messages = vec![
            r#"{"e":"aggTrade","E":1,"s":"BTCUSDT","a":1,"p":"100000","q":"0.1","f":1,"l":1,"T":0,"m":false}"#,
            r#"{"e":"aggTrade","E":2,"s":"BTCUSDT","a":2,"p":"100500","q":"0.1","f":1,"l":1,"T":60000,"m":false}"#,
            r#"{"e":"aggTrade","E":3,"s":"BTCUSDT","a":3,"p":"101000","q":"0.1","f":1,"l":1,"T":120000,"m":false}"#,
        ];

        for msg in messages {
            feed.handle_message(msg).await;
        }

        let t = tracker.read().await;
        assert_eq!(t.len(), 3);
        assert_eq!(t.current_price(), Some(101000.0));

        // Check 2-minute change: from 100000 to 101000 = 1%
        let change = t.change_5min().unwrap();
        assert!((change - 0.01).abs() < 0.001);
    }

    // Integration test - requires network (ignored by default)
    #[tokio::test]
    #[ignore = "requires network connection to Binance"]
    async fn test_feed_connects_to_binance() {
        let tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let mut feed = SpotPriceFeed::new(
            SpotPriceFeedConfig {
                max_reconnect_attempts: 1,
                ..Default::default()
            },
            tracker.clone(),
        );

        let stop = feed.stop_handle();

        // Stop after 2 seconds
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let _ = feed.run().await;

        // Should have received some prices
        let t = tracker.read().await;
        assert!(!t.is_empty(), "Should have received price updates");
        assert!(t.current_price().is_some());
    }
}
