//! Integrated Gabagool-style hybrid arbitrage runner.
//!
//! This module combines all components for the gabagool hybrid strategy:
//! - Binance spot price feed (for BTC price tracking)
//! - Polymarket order book feed (for YES/NO prices)
//! - Reference tracker (for accurate "price to beat" capture)
//! - Gabagool detector (for Entry/Hedge/Scratch signals)
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
//! │  ReferenceTracker    │   │  BookFeed (YES/NO)   │
//! │  (window references) │   │  (L2 order books)    │
//! └──────────┬───────────┘   └──────────┬───────────┘
//!            │                          │
//!            └──────────┬───────────────┘
//!                       ▼
//!            ┌──────────────────────┐
//!            │   GabagoolDetector   │
//!            │   check() → Signal   │
//!            └──────────┬───────────┘
//!                       ▼
//!            ┌──────────────────────┐
//!            │  Signal Channel      │
//!            │  + History Export    │
//!            └──────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::gabagool_runner::{
//!     GabagoolRunner, GabagoolRunnerConfig,
//! };
//!
//! let config = GabagoolRunnerConfig {
//!     yes_token_id: "yes-token-123".to_string(),
//!     no_token_id: "no-token-456".to_string(),
//!     ..Default::default()
//! };
//!
//! let (runner, mut signal_rx) = GabagoolRunner::new(config);
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
use crate::arbitrage::data_service::DataServiceHandle;
use crate::arbitrage::gabagool_detector::{
    GabagoolConfig, GabagoolDetector, GabagoolDirection, GabagoolSignal, GabagoolSignalType,
    MarketSnapshot, OpenPosition,
};
use crate::arbitrage::latency_detector::SpotPriceTracker;
use crate::arbitrage::reference_tracker::{
    ReferenceTracker, ReferenceTrackerConfig, WindowReference,
};
use crate::arbitrage::spot_feed::{SpotFeedError, SpotPriceFeedConfig};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// Errors from the gabagool runner.
#[derive(Error, Debug)]
pub enum GabagoolRunnerError {
    /// Spot feed error.
    #[error("Spot feed error: {0}")]
    SpotFeed(#[from] SpotFeedError),

    /// Book feed error.
    #[error("Book feed error: {0}")]
    BookFeed(#[from] BookFeedError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Runner was stopped.
    #[error("Runner stopped")]
    Stopped,
}

/// Configuration for the gabagool runner.
#[derive(Debug, Clone)]
pub struct GabagoolRunnerConfig {
    /// YES token ID for Polymarket.
    pub yes_token_id: String,
    /// NO token ID for Polymarket.
    pub no_token_id: String,
    /// Gabagool detector configuration.
    pub detector_config: GabagoolConfig,
    /// Reference tracker configuration.
    pub reference_config: ReferenceTrackerConfig,
    /// Spot feed configuration.
    pub spot_config: SpotPriceFeedConfig,
    /// Book feed configuration.
    pub book_config: BookFeedConfig,
    /// How often to check for signals (milliseconds).
    pub check_interval_ms: u64,
    /// Signal channel buffer size.
    pub signal_buffer_size: usize,
    /// Path to export signal history (optional).
    pub history_export_path: Option<PathBuf>,
    /// Maximum signal history to keep in memory.
    pub max_history: usize,
}

impl Default for GabagoolRunnerConfig {
    fn default() -> Self {
        Self {
            yes_token_id: String::new(),
            no_token_id: String::new(),
            detector_config: GabagoolConfig::default(),
            reference_config: ReferenceTrackerConfig::default(),
            spot_config: SpotPriceFeedConfig::default(),
            book_config: BookFeedConfig::default(),
            check_interval_ms: 100, // Check every 100ms
            signal_buffer_size: 100,
            history_export_path: None,
            max_history: 10_000,
        }
    }
}

/// A recorded signal with full context for backtesting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRecord {
    /// The signal itself.
    pub signal: GabagoolSignal,
    /// Window reference at time of signal.
    pub window_start_ms: i64,
    /// Window reference price.
    pub reference_price: f64,
    /// YES ask at time of signal.
    pub yes_ask: Decimal,
    /// YES bid at time of signal.
    pub yes_bid: Decimal,
    /// NO ask at time of signal.
    pub no_ask: Decimal,
    /// NO bid at time of signal.
    pub no_bid: Decimal,
    /// Pair cost at time of signal.
    pub pair_cost: Decimal,
}

/// Statistics for the gabagool runner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GabagoolRunnerStats {
    /// Number of signal checks performed.
    pub checks_performed: u64,
    /// Number of Entry signals generated.
    pub entry_signals: u64,
    /// Number of Hedge signals generated.
    pub hedge_signals: u64,
    /// Number of Scratch signals generated.
    pub scratch_signals: u64,
    /// Entry signals by direction.
    pub yes_entries: u64,
    /// Entry signals by direction.
    pub no_entries: u64,
    /// Last signal timestamp.
    pub last_signal_time: Option<DateTime<Utc>>,
    /// Current spot price.
    pub current_spot_price: Option<f64>,
    /// Current YES ask price.
    pub current_yes_ask: Option<Decimal>,
    /// Current NO ask price.
    pub current_no_ask: Option<Decimal>,
    /// Current pair cost.
    pub current_pair_cost: Option<Decimal>,
    /// Current window reference price.
    pub current_reference_price: Option<f64>,
    /// Runner start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Number of windows processed.
    pub windows_processed: u64,
}

impl GabagoolRunnerStats {
    /// Returns total signals generated.
    #[must_use]
    pub fn total_signals(&self) -> u64 {
        self.entry_signals + self.hedge_signals + self.scratch_signals
    }
}

/// Internal state shared between spot feed and main loop.
struct SharedState {
    /// Reference tracker for window prices.
    reference_tracker: ReferenceTracker,
    /// Current spot price.
    spot_price: Option<f64>,
    /// Last spot update timestamp.
    last_spot_update_ms: Option<i64>,
}

/// Integrated gabagool-style hybrid arbitrage runner.
pub struct GabagoolRunner {
    /// Configuration.
    config: GabagoolRunnerConfig,
    /// Shared state with reference tracker.
    state: Arc<RwLock<SharedState>>,
    /// Gabagool detector.
    detector: GabagoolDetector,
    /// Signal sender.
    signal_tx: mpsc::Sender<GabagoolSignal>,
    /// Stop flag.
    should_stop: Arc<AtomicBool>,
    /// Statistics.
    stats: Arc<RwLock<GabagoolRunnerStats>>,
    /// Signal history for backtesting.
    history: Arc<RwLock<VecDeque<SignalRecord>>>,
    /// External data service handle (if provided, skips creating own spot feed).
    data_handle: Option<DataServiceHandle>,
}

impl GabagoolRunner {
    /// Creates a new gabagool runner with its own Binance spot feed.
    ///
    /// Returns the runner and a channel to receive signals.
    pub fn new(config: GabagoolRunnerConfig) -> (Self, mpsc::Receiver<GabagoolSignal>) {
        Self::build(config, None)
    }

    /// Creates a new gabagool runner backed by a shared [`DataServiceHandle`].
    ///
    /// When a data handle is provided, the runner reads spot prices from the
    /// shared `SpotPriceTracker` instead of spawning its own Binance feed.
    pub fn with_data_service(
        config: GabagoolRunnerConfig,
        data_handle: DataServiceHandle,
    ) -> (Self, mpsc::Receiver<GabagoolSignal>) {
        Self::build(config, Some(data_handle))
    }

    /// Shared constructor logic.
    fn build(
        config: GabagoolRunnerConfig,
        data_handle: Option<DataServiceHandle>,
    ) -> (Self, mpsc::Receiver<GabagoolSignal>) {
        let (signal_tx, signal_rx) = mpsc::channel(config.signal_buffer_size);

        let state = SharedState {
            reference_tracker: ReferenceTracker::new(config.reference_config.clone()),
            spot_price: None,
            last_spot_update_ms: None,
        };

        let detector = GabagoolDetector::new(config.detector_config.clone());

        let runner = Self {
            config,
            state: Arc::new(RwLock::new(state)),
            detector,
            signal_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(GabagoolRunnerStats::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            data_handle,
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
    pub fn stats(&self) -> Arc<RwLock<GabagoolRunnerStats>> {
        self.stats.clone()
    }

    /// Returns the signal history.
    #[must_use]
    pub fn history(&self) -> Arc<RwLock<VecDeque<SignalRecord>>> {
        self.history.clone()
    }

    /// Exports signal history to a file.
    pub async fn export_history(&self, path: &PathBuf) -> Result<usize, GabagoolRunnerError> {
        let history = self.history.read().await;
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write as JSONL (one JSON object per line)
        for record in history.iter() {
            serde_json::to_writer(&mut writer, record).map_err(|e| {
                GabagoolRunnerError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(history.len())
    }

    /// Runs the gabagool hybrid arbitrage system.
    pub async fn run(mut self) -> Result<(), GabagoolRunnerError> {
        // Validate config
        if self.config.yes_token_id.is_empty() || self.config.no_token_id.is_empty() {
            return Err(GabagoolRunnerError::Config(
                "YES and NO token IDs are required".to_string(),
            ));
        }

        info!(
            yes_token = %self.config.yes_token_id,
            no_token = %self.config.no_token_id,
            cheap_threshold = %self.config.detector_config.cheap_threshold,
            pair_cost_threshold = %self.config.detector_config.pair_cost_threshold,
            "Starting Gabagool hybrid arbitrage runner"
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Spawn spot price data source: either poll DataService tracker or own WebSocket
        let spot_handle = if let Some(ref dh) = self.data_handle {
            // Resolve the coin slug from the spot config symbol (e.g. "btcusdt" → "btc")
            let slug = self
                .config
                .spot_config
                .symbol
                .to_lowercase()
                .replace("usdt", "");
            let tracker = match dh.spot_tracker(&slug) {
                Some(t) => Arc::clone(t),
                None => {
                    warn!(
                        coin = slug.as_str(),
                        "DataService has no tracker for coin, cannot run gabagool"
                    );
                    return Err(GabagoolRunnerError::Config(format!(
                        "DataService has no spot tracker for '{slug}'"
                    )));
                }
            };
            let state = self.state.clone();
            let stop = self.should_stop.clone();
            let stats_clone = self.stats.clone();
            tokio::spawn(async move {
                info!("Spot feed task starting (DataService shared tracker)...");
                Self::run_spot_from_tracker(state, tracker, stop, stats_clone).await;
                info!("Spot feed task finished (DataService)");
            })
        } else {
            let state = self.state.clone();
            let spot_config = self.config.spot_config.clone();
            let spot_stop = self.should_stop.clone();
            let stats_clone = self.stats.clone();
            tokio::spawn(async move {
                info!("Spot feed task starting...");
                match Self::run_spot_feed(state, spot_config, spot_stop, stats_clone).await {
                    Ok(()) => info!("Spot feed task finished normally"),
                    Err(e) => error!("Spot feed task failed: {}", e),
                }
            })
        };

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

        // Export history if configured
        if let Some(ref path) = self.config.history_export_path {
            match self.export_history(path).await {
                Ok(count) => info!(path = %path.display(), count, "Exported signal history"),
                Err(e) => warn!(error = %e, "Failed to export signal history"),
            }
        }

        result
    }

    /// Runs the spot feed and updates the reference tracker.
    async fn run_spot_feed(
        state: Arc<RwLock<SharedState>>,
        config: SpotPriceFeedConfig,
        should_stop: Arc<AtomicBool>,
        stats: Arc<RwLock<GabagoolRunnerStats>>,
    ) -> Result<(), SpotFeedError> {
        use futures_util::StreamExt;
        use tokio_tungstenite::connect_async;

        // Try spot API first, fall back to futures if geo-blocked
        let spot_url = format!(
            "wss://stream.binance.com:9443/ws/{}@aggTrade",
            config.symbol.to_lowercase()
        );
        let futures_url = format!(
            "wss://fstream.binance.com/ws/{}@aggTrade",
            config.symbol.to_lowercase()
        );

        info!(url = %spot_url, "Connecting to Binance spot feed...");

        let ws_stream = match connect_async(&spot_url).await {
            Ok((stream, _)) => {
                info!("Binance spot feed connected successfully");
                stream
            }
            Err(e) => {
                warn!("Spot feed failed ({}), trying futures feed...", e);
                match connect_async(&futures_url).await {
                    Ok((stream, _)) => {
                        info!("Binance futures feed connected successfully");
                        stream
                    }
                    Err(e2) => {
                        error!("Both Binance feeds failed. Spot: {}, Futures: {}", e, e2);
                        return Err(e.into());
                    }
                }
            }
        };
        let (_, mut read) = ws_stream.split();
        let mut last_window_start: Option<i64> = None;
        let mut trade_count: u64 = 0;

        while !should_stop.load(Ordering::SeqCst) {
            tokio::select! {
                msg = read.next() => {
                    match msg {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            // Parse aggTrade message
                            if let Ok(trade) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let (Some(price_str), Some(time)) = (
                                    trade.get("p").and_then(|p| p.as_str()),
                                    trade.get("T").and_then(|t| t.as_i64()),
                                ) {
                                    if let Ok(price) = price_str.parse::<f64>() {
                                        trade_count += 1;
                                        if trade_count == 1 {
                                            info!(price = format!("${:.2}", price), "First BTC trade received");
                                        }

                                        // Update state
                                        let mut s = state.write().await;
                                        s.reference_tracker.update_price(time, price);
                                        s.spot_price = Some(price);
                                        s.last_spot_update_ms = Some(time);

                                        // Check for new window
                                        if let Some(ref_) = s.reference_tracker.current_reference() {
                                            if last_window_start != Some(ref_.window_start_ms) {
                                                last_window_start = Some(ref_.window_start_ms);
                                                let mut st = stats.write().await;
                                                st.windows_processed += 1;
                                                st.current_reference_price = Some(ref_.reference_price);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                            debug!("Received ping");
                            // Pong is handled automatically by tungstenite
                            let _ = data;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                            info!("Spot feed WebSocket closed");
                            break;
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "Spot feed error");
                            break;
                        }
                        None => {
                            info!("Spot feed stream ended");
                            break;
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    // Heartbeat - check if we're still receiving data
                    let s = state.read().await;
                    if let Some(last) = s.last_spot_update_ms {
                        let now = Utc::now().timestamp_millis();
                        if now - last > 30_000 {
                            warn!("No spot updates in 30 seconds");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Polls a shared `SpotPriceTracker` (from DataService) and mirrors updates
    /// into the runner's `SharedState`, keeping the reference tracker and stats
    /// in sync just like the self-owned WebSocket feed does.
    async fn run_spot_from_tracker(
        state: Arc<RwLock<SharedState>>,
        tracker: Arc<RwLock<SpotPriceTracker>>,
        should_stop: Arc<AtomicBool>,
        stats: Arc<RwLock<GabagoolRunnerStats>>,
    ) {
        let poll_interval = Duration::from_millis(50);
        let mut last_seen_ts: Option<i64> = None;
        let mut last_window_start: Option<i64> = None;
        let mut first_logged = false;

        while !should_stop.load(Ordering::SeqCst) {
            // Read latest price from the shared tracker
            let (price, ts_ms) = {
                let t = tracker.read().await;
                (t.current_price(), t.current_timestamp_ms())
            };

            if let (Some(price), Some(ts)) = (price, ts_ms) {
                // Only update if the timestamp is new
                if last_seen_ts != Some(ts) {
                    last_seen_ts = Some(ts);

                    if !first_logged {
                        info!(price = format!("${:.2}", price), "First spot price from DataService tracker");
                        first_logged = true;
                    }

                    // Mirror into SharedState (same writes as run_spot_feed)
                    let mut s = state.write().await;
                    s.reference_tracker.update_price(ts, price);
                    s.spot_price = Some(price);
                    s.last_spot_update_ms = Some(ts);

                    // Detect new window
                    if let Some(ref_) = s.reference_tracker.current_reference() {
                        if last_window_start != Some(ref_.window_start_ms) {
                            last_window_start = Some(ref_.window_start_ms);
                            let mut st = stats.write().await;
                            st.windows_processed += 1;
                            st.current_reference_price = Some(ref_.reference_price);
                        }
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Main detection loop.
    async fn detection_loop(&mut self, book_feed: &BookFeed) -> Result<(), GabagoolRunnerError> {
        let check_interval = Duration::from_millis(self.config.check_interval_ms);
        let mut last_warning_time: Option<tokio::time::Instant> = None;
        let warning_interval = Duration::from_secs(5); // Rate-limit warnings
        let mut first_check_logged = false;

        info!("Detection loop started - waiting for data feeds...");

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("Detection loop stopping");
                return Ok(());
            }

            // Get current books
            let (yes_book, no_book) = match book_feed.get_books() {
                Ok(books) => books,
                Err(e) => {
                    warn!("Failed to get books: {}", e);
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            // Helper to check if we should log a warning (rate limited)
            let should_warn =
                || last_warning_time.map_or(true, |t| t.elapsed() >= warning_interval);

            // Get best prices
            let yes_ask = match yes_book.best_ask() {
                Some(price) => price,
                None => {
                    if should_warn() {
                        warn!("No YES ask available - book may be empty");
                        last_warning_time = Some(tokio::time::Instant::now());
                    }
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            let yes_bid = yes_book.best_bid().unwrap_or(yes_ask - Decimal::new(1, 2));

            let no_ask = match no_book.best_ask() {
                Some(price) => price,
                None => {
                    if should_warn() {
                        warn!("No NO ask available - book may be empty");
                        last_warning_time = Some(tokio::time::Instant::now());
                    }
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };

            let no_bid = no_book.best_bid().unwrap_or(no_ask - Decimal::new(1, 2));

            // Create market snapshot
            let now_ms = Utc::now().timestamp_millis();
            let market = MarketSnapshot::new(yes_ask, yes_bid, no_ask, no_bid, now_ms);

            // Get reference and spot price
            let state = self.state.read().await;
            let reference = match state.reference_tracker.current_reference() {
                Some(r) => r.clone(),
                None => {
                    if should_warn() {
                        warn!("No window reference yet - waiting for Binance spot feed");
                        last_warning_time = Some(tokio::time::Instant::now());
                    }
                    drop(state);
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };
            let spot_price = match state.spot_price {
                Some(p) => p,
                None => {
                    if should_warn() {
                        warn!("No spot price yet - Binance feed not connected?");
                        last_warning_time = Some(tokio::time::Instant::now());
                    }
                    drop(state);
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            };
            drop(state);

            // Log first successful check
            if !first_check_logged {
                info!(
                    spot = format!("${:.2}", spot_price),
                    reference = format!("${:.2}", reference.reference_price),
                    yes_ask = %yes_ask,
                    no_ask = %no_ask,
                    pair_cost = %market.pair_cost(),
                    "Data feeds connected - first check running"
                );
                first_check_logged = true;
            }

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.checks_performed += 1;
                stats.current_spot_price = Some(spot_price);
                stats.current_yes_ask = Some(yes_ask);
                stats.current_no_ask = Some(no_ask);
                stats.current_pair_cost = Some(market.pair_cost());
            }

            // Check for signal
            let signal = self.detector.check(&reference, spot_price, &market);

            // Handle signal
            if let Some(sig) = signal {
                self.handle_signal(sig.clone(), &reference, &market).await?;
            }

            tokio::time::sleep(check_interval).await;
        }
    }

    /// Handles a detected signal.
    async fn handle_signal(
        &mut self,
        signal: GabagoolSignal,
        reference: &WindowReference,
        market: &MarketSnapshot,
    ) -> Result<(), GabagoolRunnerError> {
        // Log the signal
        match signal.signal_type {
            GabagoolSignalType::Entry => {
                info!(
                    signal_type = "ENTRY",
                    direction = ?signal.direction,
                    entry_price = %signal.entry_price,
                    spot_vs_ref = format!("{:+.4}%", signal.spot_delta_pct * 100.0),
                    spot = format!("${:.2}", signal.spot_price),
                    reference = format!("${:.2}", signal.reference_price),
                    pair_cost = %signal.current_pair_cost,
                    time_left = format!("{}s", signal.time_remaining_secs),
                    confidence = ?signal.confidence,
                    "GABAGOOL ENTRY SIGNAL"
                );
            }
            GabagoolSignalType::Hedge => {
                let existing = signal.existing_entry_price.unwrap_or_default();
                let total_cost = existing + signal.entry_price;
                let profit = Decimal::ONE - total_cost;
                info!(
                    signal_type = "HEDGE",
                    direction = ?signal.direction,
                    hedge_price = %signal.entry_price,
                    existing_price = %existing,
                    total_cost = %total_cost,
                    locked_profit = %profit,
                    time_left = format!("{}s", signal.time_remaining_secs),
                    "GABAGOOL HEDGE SIGNAL"
                );
            }
            GabagoolSignalType::Scratch => {
                let existing = signal.existing_entry_price.unwrap_or_default();
                let loss = existing - signal.entry_price;
                info!(
                    signal_type = "SCRATCH",
                    direction = ?signal.direction,
                    exit_price = %signal.entry_price,
                    entry_price = %existing,
                    loss = %loss,
                    time_left = format!("{}s", signal.time_remaining_secs),
                    "GABAGOOL SCRATCH SIGNAL"
                );
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.last_signal_time = Some(signal.timestamp);

            match signal.signal_type {
                GabagoolSignalType::Entry => {
                    stats.entry_signals += 1;
                    match signal.direction {
                        GabagoolDirection::Yes => stats.yes_entries += 1,
                        GabagoolDirection::No => stats.no_entries += 1,
                    }
                }
                GabagoolSignalType::Hedge => stats.hedge_signals += 1,
                GabagoolSignalType::Scratch => stats.scratch_signals += 1,
            }
        }

        // Record for history
        let record = SignalRecord {
            signal: signal.clone(),
            window_start_ms: reference.window_start_ms,
            reference_price: reference.reference_price,
            yes_ask: market.yes_ask,
            yes_bid: market.yes_bid,
            no_ask: market.no_ask,
            no_bid: market.no_bid,
            pair_cost: market.pair_cost(),
        };

        {
            let mut history = self.history.write().await;
            history.push_back(record);
            while history.len() > self.config.max_history {
                history.pop_front();
            }
        }

        // Send to channel
        if self.signal_tx.send(signal).await.is_err() {
            warn!("Signal channel closed");
            return Err(GabagoolRunnerError::Stopped);
        }

        Ok(())
    }

    /// Manually records a position entry (for simulating fills).
    pub async fn record_position_entry(
        &mut self,
        direction: GabagoolDirection,
        entry_price: Decimal,
        quantity: Decimal,
    ) {
        let state = self.state.read().await;
        let window_start_ms = state
            .reference_tracker
            .current_reference()
            .map(|r| r.window_start_ms)
            .unwrap_or(0);
        let entry_time_ms = Utc::now().timestamp_millis();
        drop(state);

        self.detector.record_entry(OpenPosition {
            direction,
            entry_price,
            quantity,
            entry_time_ms,
            window_start_ms,
        });
    }

    /// Manually records a position exit.
    pub fn record_position_exit(&mut self) {
        self.detector.record_exit();
    }
}

/// Simple runner that monitors for signals without execution.
///
/// Useful for paper trading validation and data collection.
pub async fn run_gabagool_monitor(
    yes_token_id: String,
    no_token_id: String,
    duration: Duration,
    config: GabagoolConfig,
    history_path: Option<PathBuf>,
) -> Result<(Vec<GabagoolSignal>, GabagoolRunnerStats), GabagoolRunnerError> {
    let runner_config = GabagoolRunnerConfig {
        yes_token_id,
        no_token_id,
        detector_config: config,
        history_export_path: history_path,
        ..Default::default()
    };

    let (runner, mut signal_rx) = GabagoolRunner::new(runner_config);
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

    // Get final stats
    let final_stats = stats.read().await.clone();

    info!(
        checks = final_stats.checks_performed,
        entries = final_stats.entry_signals,
        hedges = final_stats.hedge_signals,
        scratches = final_stats.scratch_signals,
        windows = final_stats.windows_processed,
        "Gabagool monitor completed"
    );

    Ok((signals, final_stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = GabagoolRunnerConfig::default();
        assert!(config.yes_token_id.is_empty());
        assert!(config.no_token_id.is_empty());
        assert_eq!(config.check_interval_ms, 100);
        assert_eq!(config.signal_buffer_size, 100);
        assert!(config.history_export_path.is_none());
    }

    #[test]
    fn test_stats_default() {
        let stats = GabagoolRunnerStats::default();
        assert_eq!(stats.checks_performed, 0);
        assert_eq!(stats.entry_signals, 0);
        assert_eq!(stats.hedge_signals, 0);
        assert_eq!(stats.scratch_signals, 0);
        assert_eq!(stats.total_signals(), 0);
    }

    #[test]
    fn test_stats_total_signals() {
        let mut stats = GabagoolRunnerStats::default();
        stats.entry_signals = 5;
        stats.hedge_signals = 3;
        stats.scratch_signals = 2;
        assert_eq!(stats.total_signals(), 10);
    }

    #[tokio::test]
    async fn test_runner_creation() {
        let config = GabagoolRunnerConfig {
            yes_token_id: "yes-123".to_string(),
            no_token_id: "no-456".to_string(),
            ..Default::default()
        };

        let (runner, _rx) = GabagoolRunner::new(config);

        assert!(!runner.should_stop.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_runner_requires_token_ids() {
        let config = GabagoolRunnerConfig::default(); // Empty token IDs

        let (runner, _rx) = GabagoolRunner::new(config);
        let result = runner.run().await;

        assert!(matches!(result, Err(GabagoolRunnerError::Config(_))));
    }

    #[tokio::test]
    async fn test_stop_handle() {
        let config = GabagoolRunnerConfig {
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            ..Default::default()
        };

        let (runner, _rx) = GabagoolRunner::new(config);
        let stop = runner.stop_handle();

        assert!(!stop.load(Ordering::SeqCst));
        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn test_signal_record_serialization() {
        use rust_decimal_macros::dec;

        let signal = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: Utc::now(),
            estimated_edge: 0.05,
            existing_entry_price: None,
            confidence: crate::arbitrage::gabagool_detector::SignalConfidence::High,
        };

        let record = SignalRecord {
            signal,
            window_start_ms: 0,
            reference_price: 78000.0,
            yes_ask: dec!(0.35),
            yes_bid: dec!(0.34),
            no_ask: dec!(0.67),
            no_bid: dec!(0.66),
            pair_cost: dec!(1.02),
        };

        // Should serialize/deserialize without error
        let json = serde_json::to_string(&record).unwrap();
        let _: SignalRecord = serde_json::from_str(&json).unwrap();
    }
}
