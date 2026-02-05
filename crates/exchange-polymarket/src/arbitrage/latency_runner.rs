//! Integrated latency arbitrage runner.
//!
//! This module combines all components for latency arbitrage:
//! - Binance spot price feed
//! - Polymarket order book feed
//! - Latency detector for signal generation
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐    ┌─────────────────────┐
//! │  Binance WebSocket  │    │ Polymarket WebSocket│
//! │    (aggTrade)       │    │   (order books)     │
//! └──────────┬──────────┘    └──────────┬──────────┘
//!            │                          │
//!            ▼                          ▼
//! ┌──────────────────────┐   ┌──────────────────────┐
//! │  SpotPriceTracker    │   │  BookFeed (YES/NO)   │
//! │  (rolling 5min)      │   │  (L2 order books)    │
//! └──────────┬───────────┘   └──────────┬───────────┘
//!            │                          │
//!            └──────────┬───────────────┘
//!                       ▼
//!            ┌──────────────────────┐
//!            │   LatencyDetector    │
//!            │   check() → Signal   │
//!            └──────────┬───────────┘
//!                       ▼
//!            ┌──────────────────────┐
//!            │  Signal Channel      │
//!            │  (mpsc::Receiver)    │
//!            └──────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::latency_runner::{
//!     LatencyRunner, LatencyRunnerConfig,
//! };
//!
//! let config = LatencyRunnerConfig {
//!     yes_token_id: "yes-token-123".to_string(),
//!     no_token_id: "no-token-456".to_string(),
//!     ..Default::default()
//! };
//!
//! let (runner, mut signal_rx) = LatencyRunner::new(config);
//!
//! // Spawn runner
//! tokio::spawn(async move {
//!     runner.run().await.expect("Runner failed");
//! });
//!
//! // Receive signals
//! while let Some(signal) = signal_rx.recv().await {
//!     println!("Signal: {:?}", signal);
//! }
//! ```

use crate::arbitrage::book_feed::{BookFeed, BookFeedConfig, BookFeedError};
use crate::arbitrage::latency_detector::{
    LatencyConfig, LatencyDetector, LatencySignal, SpotPriceTracker,
};
use crate::arbitrage::spot_feed::{SpotFeedError, SpotPriceFeed, SpotPriceFeedConfig};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

/// Errors from the latency runner.
#[derive(Error, Debug)]
pub enum LatencyRunnerError {
    /// Spot feed error.
    #[error("Spot feed error: {0}")]
    SpotFeed(#[from] SpotFeedError),

    /// Book feed error.
    #[error("Book feed error: {0}")]
    BookFeed(#[from] BookFeedError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Runner was stopped.
    #[error("Runner stopped")]
    Stopped,
}

/// Configuration for the latency runner.
#[derive(Debug, Clone)]
pub struct LatencyRunnerConfig {
    /// YES token ID for Polymarket.
    pub yes_token_id: String,
    /// NO token ID for Polymarket.
    pub no_token_id: String,
    /// Latency detector configuration.
    pub latency_config: LatencyConfig,
    /// Spot feed configuration.
    pub spot_config: SpotPriceFeedConfig,
    /// Book feed configuration.
    pub book_config: BookFeedConfig,
    /// How often to check for signals (milliseconds).
    pub check_interval_ms: u64,
    /// Signal channel buffer size.
    pub signal_buffer_size: usize,
}

impl Default for LatencyRunnerConfig {
    fn default() -> Self {
        Self {
            yes_token_id: String::new(),
            no_token_id: String::new(),
            latency_config: LatencyConfig::default(),
            spot_config: SpotPriceFeedConfig::default(),
            book_config: BookFeedConfig::default(),
            check_interval_ms: 100, // Check every 100ms
            signal_buffer_size: 100,
        }
    }
}

/// Statistics for the latency runner.
#[derive(Debug, Clone, Default)]
pub struct LatencyRunnerStats {
    /// Number of signal checks performed.
    pub checks_performed: u64,
    /// Number of signals generated.
    pub signals_generated: u64,
    /// Number of BUY YES signals.
    pub buy_yes_signals: u64,
    /// Number of BUY NO signals.
    pub buy_no_signals: u64,
    /// Last signal timestamp.
    pub last_signal_time: Option<DateTime<Utc>>,
    /// Current spot price.
    pub current_spot_price: Option<f64>,
    /// Current YES ask price.
    pub current_yes_ask: Option<Decimal>,
    /// Current NO ask price.
    pub current_no_ask: Option<Decimal>,
    /// Runner start time.
    pub started_at: Option<DateTime<Utc>>,
}

/// Integrated latency arbitrage runner.
///
/// Combines Binance spot feed, Polymarket order books, and latency detection
/// into a single coordinated system.
pub struct LatencyRunner {
    /// Configuration.
    config: LatencyRunnerConfig,
    /// Shared spot price tracker.
    spot_tracker: Arc<RwLock<SpotPriceTracker>>,
    /// Latency detector.
    detector: LatencyDetector,
    /// Signal sender.
    signal_tx: mpsc::Sender<LatencySignal>,
    /// Stop flag.
    should_stop: Arc<AtomicBool>,
    /// Statistics.
    stats: Arc<RwLock<LatencyRunnerStats>>,
}

impl LatencyRunner {
    /// Creates a new latency runner.
    ///
    /// Returns the runner and a channel to receive signals.
    pub fn new(config: LatencyRunnerConfig) -> (Self, mpsc::Receiver<LatencySignal>) {
        let (signal_tx, signal_rx) = mpsc::channel(config.signal_buffer_size);
        let spot_tracker = Arc::new(RwLock::new(SpotPriceTracker::new()));
        let detector = LatencyDetector::new(config.latency_config.clone());

        let runner = Self {
            config,
            spot_tracker,
            detector,
            signal_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(LatencyRunnerStats::default())),
        };

        (runner, signal_rx)
    }

    /// Returns a handle to stop the runner.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.should_stop.clone()
    }

    /// Returns the shared stats.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<LatencyRunnerStats>> {
        self.stats.clone()
    }

    /// Returns the shared spot tracker (for external monitoring).
    #[must_use]
    pub fn spot_tracker(&self) -> Arc<RwLock<SpotPriceTracker>> {
        self.spot_tracker.clone()
    }

    /// Runs the latency arbitrage system.
    ///
    /// This spawns the spot feed in the background and runs the main
    /// detection loop.
    pub async fn run(mut self) -> Result<(), LatencyRunnerError> {
        // Validate config
        if self.config.yes_token_id.is_empty() || self.config.no_token_id.is_empty() {
            return Err(LatencyRunnerError::Config(
                "YES and NO token IDs are required".to_string(),
            ));
        }

        info!(
            yes_token = %self.config.yes_token_id,
            no_token = %self.config.no_token_id,
            "Starting latency arbitrage runner"
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Spawn spot price feed
        let spot_tracker = self.spot_tracker.clone();
        let spot_config = self.config.spot_config.clone();
        let spot_stop = self.should_stop.clone();

        let spot_handle = tokio::spawn(async move {
            let mut feed = SpotPriceFeed::new(spot_config, spot_tracker);
            let stop = feed.stop_handle();

            // Link stop handles
            tokio::spawn(async move {
                while !spot_stop.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                stop.store(true, Ordering::SeqCst);
            });

            feed.run().await
        });

        // Connect to Polymarket order books
        info!("Connecting to Polymarket order books...");
        let book_feed = BookFeed::connect(
            self.config.yes_token_id.clone(),
            self.config.no_token_id.clone(),
            self.config.book_config.clone(),
        )
        .await?;

        // Wait for book snapshots
        info!("Waiting for order book snapshots...");
        book_feed
            .wait_for_ready(self.config.book_config.ready_timeout)
            .await?;
        info!("Order books ready");

        // Run main detection loop
        let result = self.detection_loop(&book_feed).await;

        // Stop spot feed
        self.should_stop.store(true, Ordering::SeqCst);
        let _ = spot_handle.await;

        // Shutdown book feed
        book_feed.shutdown().await;

        result
    }

    /// Main detection loop.
    async fn detection_loop(&mut self, book_feed: &BookFeed) -> Result<(), LatencyRunnerError> {
        let check_interval = Duration::from_millis(self.config.check_interval_ms);

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("Detection loop stopping");
                return Ok(());
            }

            // Get current prices
            let (yes_book, no_book) = match book_feed.get_books() {
                Ok(books) => books,
                Err(e) => {
                    warn!("Failed to get books: {}", e);
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            let yes_ask = match yes_book.best_ask() {
                Some(price) => price,
                None => {
                    debug!("No YES ask available");
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            let no_ask = match no_book.best_ask() {
                Some(price) => price,
                None => {
                    debug!("No NO ask available");
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            // Get current time
            let now_ms = Utc::now().timestamp_millis();

            // Check for signal
            let tracker = self.spot_tracker.read().await;
            let signal = self.detector.check(&tracker, yes_ask, no_ask, now_ms);
            let spot_price = tracker.current_price();
            drop(tracker);

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.checks_performed += 1;
                stats.current_spot_price = spot_price;
                stats.current_yes_ask = Some(yes_ask);
                stats.current_no_ask = Some(no_ask);
            }

            // Handle signal
            if let Some(sig) = signal {
                info!(
                    direction = ?sig.direction,
                    entry_price = %sig.entry_price,
                    spot_vs_ref = format!("{:+.3}%", sig.spot_change_pct * 100.0),
                    spot_price = format!("${:.2}", sig.spot_price),
                    reference_price = format!("${:.2}", sig.reference_price),
                    time_remaining = format!("{}s", sig.time_remaining_secs),
                    strength = format!("{:.2}", sig.strength),
                    "🎯 LATENCY SIGNAL DETECTED"
                );

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.signals_generated += 1;
                    stats.last_signal_time = Some(sig.timestamp);
                    match sig.direction {
                        crate::arbitrage::latency_detector::LatencyDirection::BuyYes => {
                            stats.buy_yes_signals += 1;
                        }
                        crate::arbitrage::latency_detector::LatencyDirection::BuyNo => {
                            stats.buy_no_signals += 1;
                        }
                    }
                }

                // Send signal
                if self.signal_tx.send(sig).await.is_err() {
                    warn!("Signal channel closed");
                    return Err(LatencyRunnerError::Stopped);
                }
            }

            tokio::time::sleep(check_interval).await;
        }
    }
}

/// Simple runner that just monitors for signals without execution.
///
/// Useful for paper trading validation.
pub async fn run_latency_monitor(
    yes_token_id: String,
    no_token_id: String,
    duration: Duration,
    config: LatencyConfig,
) -> Result<Vec<LatencySignal>, LatencyRunnerError> {
    let runner_config = LatencyRunnerConfig {
        yes_token_id,
        no_token_id,
        latency_config: config,
        ..Default::default()
    };

    let (runner, mut signal_rx) = LatencyRunner::new(runner_config);
    let stop_handle = runner.stop_handle();
    let stats = runner.stats();

    // Spawn runner
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // Collect signals for duration
    let mut signals = Vec::new();
    let deadline = tokio::time::Instant::now() + duration;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                info!("Monitor duration elapsed");
                break;
            }
            signal = signal_rx.recv() => {
                match signal {
                    Some(s) => signals.push(s),
                    None => break,
                }
            }
        }
    }

    // Stop and wait
    stop_handle.store(true, Ordering::SeqCst);
    let _ = runner_handle.await;

    // Log final stats
    let final_stats = stats.read().await;
    info!(
        checks = final_stats.checks_performed,
        signals = final_stats.signals_generated,
        buy_yes = final_stats.buy_yes_signals,
        buy_no = final_stats.buy_no_signals,
        "Latency monitor completed"
    );

    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = LatencyRunnerConfig::default();
        assert!(config.yes_token_id.is_empty());
        assert!(config.no_token_id.is_empty());
        assert_eq!(config.check_interval_ms, 100);
        assert_eq!(config.signal_buffer_size, 100);
    }

    #[test]
    fn test_stats_default() {
        let stats = LatencyRunnerStats::default();
        assert_eq!(stats.checks_performed, 0);
        assert_eq!(stats.signals_generated, 0);
        assert!(stats.last_signal_time.is_none());
    }

    #[tokio::test]
    async fn test_runner_creation() {
        let config = LatencyRunnerConfig {
            yes_token_id: "yes-123".to_string(),
            no_token_id: "no-456".to_string(),
            ..Default::default()
        };

        let (runner, _rx) = LatencyRunner::new(config);

        assert!(!runner.should_stop.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_runner_requires_token_ids() {
        let config = LatencyRunnerConfig::default(); // Empty token IDs

        let (runner, _rx) = LatencyRunner::new(config);
        let result = runner.run().await;

        assert!(matches!(result, Err(LatencyRunnerError::Config(_))));
    }

    #[tokio::test]
    async fn test_stop_handle() {
        let config = LatencyRunnerConfig {
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            ..Default::default()
        };

        let (runner, _rx) = LatencyRunner::new(config);
        let stop = runner.stop_handle();

        assert!(!stop.load(Ordering::SeqCst));
        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }
}
