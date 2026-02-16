//! CLOB first-move timing runner for 15-minute binary options.
//!
//! Observes CLOB prices via Gamma API polling and generates directional signals
//! when price displacement from midpoint exceeds a threshold within an observation
//! window (default: 2.5-5 min into each 15-min window).
//!
//! # Strategy
//!
//! Data analysis shows that when CLOB prices deviate >= 15c from 0.50 within the
//! first 2.5-5 minutes of a window, the direction is predictive of the final
//! outcome with ~82% accuracy. BTC+ETH at >= 20c: 93-96% accuracy.
//!
//! # Architecture
//!
//! ```text
//! GammaClient (5s poll)
//!        |
//! ClobTimingRunner (observe 2.5min, check displacement)
//!        |  DirectionalSignal via mpsc
//! DirectionalExecutor (Kelly -> FOK order -> settlement)
//! ```

use crate::arbitrage::directional_detector::{Direction, DirectionalSignal};
use crate::models::Coin;
use crate::GammaClient;
use chrono::{Timelike, Utc};
use nonzero_ext::nonzero;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

/// Errors from the CLOB timing runner.
#[derive(Error, Debug)]
pub enum ClobTimingRunnerError {
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

/// Configuration for the CLOB timing runner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobTimingConfig {
    /// Coins to monitor.
    pub coins: Vec<Coin>,
    /// Seconds into window to START checking for displacement (default: 150 = 2.5 min).
    pub observation_start_secs: u64,
    /// Seconds into window to STOP checking (default: 300 = 5 min).
    pub observation_end_secs: u64,
    /// Minimum displacement from 0.50 to trigger signal (default: 0.15).
    pub min_displacement: Decimal,
    /// Maximum entry price (don't buy above this, default: 0.65).
    pub max_entry_price: Decimal,
    /// Poll interval in seconds (default: 5).
    pub poll_interval_secs: u64,
    /// Gamma API rate limit (requests per minute, default: 30).
    pub gamma_rate_limit: u32,
    /// Signal channel buffer size.
    pub signal_buffer_size: usize,
    /// Hours (0-23 UTC) to skip signal generation entirely.
    pub excluded_hours_utc: Vec<u8>,
}

impl Default for ClobTimingConfig {
    fn default() -> Self {
        Self {
            coins: vec![Coin::Btc, Coin::Eth],
            observation_start_secs: 150,
            observation_end_secs: 300,
            min_displacement: dec!(0.15),
            max_entry_price: dec!(0.85),
            poll_interval_secs: 5,
            gamma_rate_limit: 30,
            signal_buffer_size: 100,
            excluded_hours_utc: vec![4, 9, 21, 22, 23],
        }
    }
}

/// Per-coin window state.
struct CoinWindowState {
    /// The coin.
    coin: Coin,
    /// Current Up token ID.
    up_token_id: String,
    /// Current Down token ID.
    down_token_id: String,
    /// Whether a signal has been emitted this window for this coin.
    signal_emitted: bool,
    /// Price history: (timestamp_ms, up_price).
    price_history: Vec<(i64, Decimal)>,
}

/// Runner statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClobTimingRunnerStats {
    /// Number of poll iterations.
    pub checks_performed: u64,
    /// Total signals emitted.
    pub signals_emitted: u64,
    /// Signals by coin.
    pub signals_by_coin: HashMap<String, u64>,
    /// Current CLOB Up prices per coin.
    pub current_up_prices: HashMap<String, Decimal>,
    /// Current CLOB Down prices per coin.
    pub current_down_prices: HashMap<String, Decimal>,
    /// Windows seen.
    pub windows_seen: u64,
    /// Last signal time.
    pub last_signal_at: Option<chrono::DateTime<Utc>>,
    /// Runner start time.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Windows skipped due to excluded hours.
    pub hours_skipped: u64,
    /// Errors encountered.
    pub errors: u64,
}

/// CLOB first-move timing runner.
///
/// Polls Gamma API for 15-minute market prices, detects displacement from
/// midpoint within an observation window, and emits `DirectionalSignal`s.
pub struct ClobTimingRunner {
    config: ClobTimingConfig,
    gamma_client: GammaClient,
    signal_tx: mpsc::Sender<DirectionalSignal>,
    should_stop: Arc<AtomicBool>,
    stats: Arc<RwLock<ClobTimingRunnerStats>>,
}

impl ClobTimingRunner {
    /// Creates a new CLOB timing runner.
    ///
    /// Returns the runner and a channel to receive signals.
    pub fn new(config: ClobTimingConfig) -> (Self, mpsc::Receiver<DirectionalSignal>) {
        let (signal_tx, signal_rx) = mpsc::channel(config.signal_buffer_size);

        let gamma_client = GammaClient::with_rate_limit(
            std::num::NonZeroU32::new(config.gamma_rate_limit).unwrap_or(nonzero!(30u32)),
        );

        let runner = Self {
            config,
            gamma_client,
            signal_tx,
            should_stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(ClobTimingRunnerStats::default())),
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
    pub fn stats(&self) -> Arc<RwLock<ClobTimingRunnerStats>> {
        Arc::clone(&self.stats)
    }

    /// Runs the CLOB timing scanner.
    pub async fn run(self) -> Result<(), ClobTimingRunnerError> {
        if self.config.coins.is_empty() {
            return Err(ClobTimingRunnerError::Config(
                "At least one coin is required".to_string(),
            ));
        }

        info!(
            coins = ?self.config.coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
            obs_start = self.config.observation_start_secs,
            obs_end = self.config.observation_end_secs,
            min_displacement = %self.config.min_displacement,
            "Starting CLOB timing runner"
        );

        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        // Main loop: discover markets and run observation
        self.main_loop().await
    }

    /// Main loop: discovers markets at window boundaries and polls within windows.
    async fn main_loop(&self) -> Result<(), ClobTimingRunnerError> {
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let mut coin_states: Vec<CoinWindowState> = Vec::new();
        let mut current_window_start_ms: Option<i64> = None;

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("CLOB timing runner stopping");
                return Ok(());
            }

            let now = Utc::now();
            let now_ms = now.timestamp_millis();
            let window_start_ms = window_start_for_timestamp(now_ms);

            // Detect new window
            if current_window_start_ms != Some(window_start_ms) {
                info!(
                    window_start = window_start_ms,
                    "New 15-minute window, discovering markets"
                );

                match self.discover_markets().await {
                    Ok(states) => {
                        coin_states = states;
                        current_window_start_ms = Some(window_start_ms);

                        let mut stats = self.stats.write().await;
                        stats.windows_seen += 1;
                    }
                    Err(e) => {
                        warn!("Market discovery failed: {}", e);
                        let mut stats = self.stats.write().await;
                        stats.errors += 1;
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                }
            }

            // Skip excluded hours
            let current_hour = now.hour() as u8;
            if self.config.excluded_hours_utc.contains(&current_hour) {
                debug!(hour = current_hour, "Skipping excluded hour");
                {
                    let mut stats = self.stats.write().await;
                    stats.hours_skipped += 1;
                    stats.checks_performed += 1;
                }
                tokio::time::sleep(poll_interval).await;
                continue;
            }

            // Calculate seconds into the current window
            let secs_into_window = ((now_ms - window_start_ms) / 1000) as u64;

            // Poll and check displacement within observation window
            if secs_into_window >= self.config.observation_start_secs
                && secs_into_window <= self.config.observation_end_secs
            {
                if let Err(e) = self.poll_and_check(&mut coin_states, now_ms).await {
                    debug!("Poll error: {}", e);
                    let mut stats = self.stats.write().await;
                    stats.errors += 1;
                }
            } else if secs_into_window < self.config.observation_start_secs {
                debug!(
                    secs_into_window,
                    obs_start = self.config.observation_start_secs,
                    "Waiting for observation window"
                );
            }

            {
                let mut stats = self.stats.write().await;
                stats.checks_performed += 1;
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Discovers current 15-minute markets for configured coins.
    async fn discover_markets(&self) -> Result<Vec<CoinWindowState>, ClobTimingRunnerError> {
        let markets = self
            .gamma_client
            .get_15min_markets_for_coins(&self.config.coins)
            .await;

        if markets.is_empty() {
            return Err(ClobTimingRunnerError::Api(
                "No 15-minute markets found".to_string(),
            ));
        }

        let mut states = Vec::new();
        for market in &markets {
            let coin = detect_coin_from_question(&market.question);
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
                    warn!(coin = coin.slug_prefix(), "No Up token");
                    continue;
                }
            };
            let down_token = match market.down_token() {
                Some(t) => t,
                None => {
                    warn!(coin = coin.slug_prefix(), "No Down token");
                    continue;
                }
            };

            info!(
                coin = coin.slug_prefix(),
                up_token = %up_token.token_id,
                down_token = %down_token.token_id,
                up_price = %up_token.price,
                down_price = %down_token.price,
                "Market discovered"
            );

            states.push(CoinWindowState {
                coin,
                up_token_id: up_token.token_id.clone(),
                down_token_id: down_token.token_id.clone(),
                signal_emitted: false,
                price_history: Vec::new(),
            });
        }

        if states.is_empty() {
            return Err(ClobTimingRunnerError::Api(
                "No valid coin states after market discovery".to_string(),
            ));
        }

        Ok(states)
    }

    /// Polls Gamma for current prices and checks for displacement signals.
    async fn poll_and_check(
        &self,
        coin_states: &mut [CoinWindowState],
        now_ms: i64,
    ) -> Result<(), ClobTimingRunnerError> {
        // Re-fetch markets to get fresh CLOB prices
        let coins: Vec<Coin> = coin_states.iter().map(|s| s.coin).collect();
        let markets = self
            .gamma_client
            .get_15min_markets_for_coins(&coins)
            .await;

        // Build a lookup from coin -> (up_price, down_price)
        let mut price_map: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        for market in &markets {
            if let Some(coin) = detect_coin_from_question(&market.question) {
                if let (Some(up), Some(down)) = (market.up_price(), market.down_price()) {
                    price_map.insert(coin.slug_prefix().to_string(), (up, down));
                }
            }
        }

        for state in coin_states.iter_mut() {
            let key = state.coin.slug_prefix().to_string();
            let (up_price, down_price) = match price_map.get(&key) {
                Some(p) => *p,
                None => continue,
            };

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats
                    .current_up_prices
                    .insert(key.to_uppercase(), up_price);
                stats
                    .current_down_prices
                    .insert(key.to_uppercase(), down_price);
            }

            // Record price history (bounded to ~15 min at 5s intervals)
            state.price_history.push((now_ms, up_price));
            if state.price_history.len() > 200 {
                state.price_history.remove(0);
            }

            // Skip if already emitted this window
            if state.signal_emitted {
                continue;
            }

            // Check displacement
            let midpoint = dec!(0.50);
            let displacement = if up_price > midpoint {
                up_price - midpoint
            } else {
                midpoint - up_price
            };

            if displacement < self.config.min_displacement {
                debug!(
                    coin = state.coin.slug_prefix(),
                    up_price = %up_price,
                    displacement = %displacement,
                    min = %self.config.min_displacement,
                    "Displacement below threshold"
                );
                continue;
            }

            // Determine direction
            let (direction, entry_price, entry_token_id) = if up_price > midpoint {
                // Market leans Up — buy Up token
                (Direction::Up, up_price, state.up_token_id.clone())
            } else {
                // Market leans Down — buy Down token
                (Direction::Down, down_price, state.down_token_id.clone())
            };

            // Check max entry price
            if entry_price > self.config.max_entry_price {
                debug!(
                    coin = state.coin.slug_prefix(),
                    entry_price = %entry_price,
                    max = %self.config.max_entry_price,
                    "Entry price too high"
                );
                continue;
            }

            // Calculate win probability from calibrated model
            let displacement_f64 = displacement.to_f64().unwrap_or(0.0);
            let win_probability = displacement_to_win_prob(displacement_f64);
            let entry_price_f64 = entry_price.to_f64().unwrap_or(0.5);
            let estimated_edge = win_probability - entry_price_f64;

            if estimated_edge <= 0.0 {
                debug!(
                    coin = state.coin.slug_prefix(),
                    win_prob = format!("{:.3}", win_probability),
                    entry_price = %entry_price,
                    edge = format!("{:.4}", estimated_edge),
                    "No positive edge"
                );
                continue;
            }

            let timestamp = Utc::now();

            let signal = DirectionalSignal {
                coin: state.coin.slug_prefix().to_string(),
                direction,
                entry_token_id,
                entry_price,
                spot_price: 0.0, // Not applicable (no Binance feed)
                reference_price: 0.50, // Midpoint
                delta_pct: displacement_f64, // Use displacement as delta
                confidence: win_probability, // High confidence from calibrated model
                win_probability,
                estimated_edge,
                time_remaining_secs: 0, // Not used by executor for sizing
                timestamp,
            };

            info!(
                coin = signal.coin,
                direction = %signal.direction,
                displacement = %displacement,
                win_prob = format!("{:.3}", win_probability),
                edge = format!("{:.4}", estimated_edge),
                entry_price = %entry_price,
                "CLOB TIMING SIGNAL"
            );

            state.signal_emitted = true;

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
                return Err(ClobTimingRunnerError::Stopped);
            }
        }

        Ok(())
    }
}

/// Calculates the start of the current 15-minute window for a given timestamp.
fn window_start_for_timestamp(timestamp_ms: i64) -> i64 {
    let window_duration_ms: i64 = 15 * 60 * 1000;
    (timestamp_ms / window_duration_ms) * window_duration_ms
}

/// Calibrated win probability model based on CLOB displacement (ETH-tuned).
///
/// Data points from ETH analysis:
/// - 5c displacement -> 72% win rate
/// - 10c displacement -> 77% win rate
/// - 15c displacement -> 80% win rate
/// - 20c displacement -> 84% win rate
///
/// Linear fit: wp = 0.70 + displacement * 0.56, clamped to [0.55, 0.95].
fn displacement_to_win_prob(displacement: f64) -> f64 {
    let wp = 0.70 + displacement * 0.56;
    wp.clamp(0.55, 0.95)
}

/// Detects which coin a market belongs to from its question text.
///
/// Matches case-insensitively against ticker symbols and full names.
fn detect_coin_from_question(question: &str) -> Option<Coin> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = ClobTimingConfig::default();
        assert_eq!(config.coins.len(), 2);
        assert_eq!(config.observation_start_secs, 150);
        assert_eq!(config.observation_end_secs, 300);
        assert_eq!(config.min_displacement, dec!(0.15));
        assert_eq!(config.max_entry_price, dec!(0.85));
        assert_eq!(config.poll_interval_secs, 5);
        assert_eq!(config.excluded_hours_utc, vec![4, 9, 21, 22, 23]);
    }

    #[test]
    fn test_excluded_hours_config() {
        let config = ClobTimingConfig {
            excluded_hours_utc: vec![0, 6, 12],
            ..Default::default()
        };
        assert_eq!(config.excluded_hours_utc, vec![0, 6, 12]);
        assert!(!config.excluded_hours_utc.contains(&4));

        let empty_config = ClobTimingConfig {
            excluded_hours_utc: vec![],
            ..Default::default()
        };
        assert!(empty_config.excluded_hours_utc.is_empty());
    }

    #[test]
    fn test_runner_creation() {
        let config = ClobTimingConfig::default();
        let (runner, _rx) = ClobTimingRunner::new(config);
        assert!(!runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stop_handle() {
        let config = ClobTimingConfig::default();
        let (runner, _rx) = ClobTimingRunner::new(config);
        let stop = runner.stop_handle();
        stop.store(true, Ordering::SeqCst);
        assert!(runner.should_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stats_default() {
        let stats = ClobTimingRunnerStats::default();
        assert_eq!(stats.checks_performed, 0);
        assert_eq!(stats.signals_emitted, 0);
        assert_eq!(stats.windows_seen, 0);
    }

    // =========================================================================
    // Window Start Calculation
    // =========================================================================

    #[test]
    fn test_window_start_at_boundary() {
        // Exactly at a 15-min boundary
        let ts = 15 * 60 * 1000; // 15 min in ms
        assert_eq!(window_start_for_timestamp(ts), ts);
    }

    #[test]
    fn test_window_start_mid_window() {
        // 7.5 minutes into a window starting at 15 min
        let window_start = 15 * 60 * 1000_i64;
        let ts = window_start + 7 * 60 * 1000 + 30 * 1000;
        assert_eq!(window_start_for_timestamp(ts), window_start);
    }

    #[test]
    fn test_window_start_just_before_boundary() {
        // 1ms before the 30-min boundary
        let ts = 30 * 60 * 1000 - 1;
        assert_eq!(window_start_for_timestamp(ts), 15 * 60 * 1000);
    }

    // =========================================================================
    // Win Probability Model
    // =========================================================================

    #[test]
    fn test_win_prob_at_5c() {
        let wp = displacement_to_win_prob(0.05);
        // 0.70 + 0.05 * 0.56 = 0.728
        assert!((wp - 0.728).abs() < 0.01, "Expected ~0.728, got {}", wp);
    }

    #[test]
    fn test_win_prob_at_10c() {
        let wp = displacement_to_win_prob(0.10);
        // 0.70 + 0.10 * 0.56 = 0.756
        assert!((wp - 0.756).abs() < 0.01, "Expected ~0.756, got {}", wp);
    }

    #[test]
    fn test_win_prob_at_15c() {
        let wp = displacement_to_win_prob(0.15);
        // 0.70 + 0.15 * 0.56 = 0.784
        assert!((wp - 0.784).abs() < 0.01, "Expected ~0.784, got {}", wp);
    }

    #[test]
    fn test_win_prob_at_20c() {
        let wp = displacement_to_win_prob(0.20);
        // 0.70 + 0.20 * 0.56 = 0.812
        assert!((wp - 0.812).abs() < 0.01, "Expected ~0.812, got {}", wp);
    }

    #[test]
    fn test_win_prob_clamped_low() {
        let wp = displacement_to_win_prob(0.0);
        assert!(wp >= 0.55, "Should clamp at 0.55, got {}", wp);
    }

    #[test]
    fn test_win_prob_clamped_high() {
        let wp = displacement_to_win_prob(0.50);
        assert!(wp <= 0.95, "Should clamp at 0.95, got {}", wp);
    }

    // =========================================================================
    // Coin Detection
    // =========================================================================

    #[test]
    fn test_detect_coin_from_question() {
        assert_eq!(
            detect_coin_from_question("Will BTC go up in the next 15 minutes?"),
            Some(Coin::Btc)
        );
        assert_eq!(
            detect_coin_from_question("ETH price movement"),
            Some(Coin::Eth)
        );
        assert_eq!(
            detect_coin_from_question("Solana next 15m"),
            Some(Coin::Sol)
        );
        assert_eq!(
            detect_coin_from_question("XRP updown"),
            Some(Coin::Xrp)
        );
        assert_eq!(detect_coin_from_question("Random market"), None);
    }

    // =========================================================================
    // Once-Per-Window Enforcement
    // =========================================================================

    #[test]
    fn test_signal_emitted_flag() {
        let mut state = CoinWindowState {
            coin: Coin::Btc,
            up_token_id: "up-123".to_string(),
            down_token_id: "down-456".to_string(),
            signal_emitted: false,
            price_history: Vec::new(),
        };

        assert!(!state.signal_emitted);
        state.signal_emitted = true;
        assert!(state.signal_emitted);
    }
}
