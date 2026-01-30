//! Funding rate data collector for Binance Futures.
//!
//! Connects to Binance WebSocket mark price stream to collect funding rates
//! with rolling statistical context (percentile, z-score).

use crate::collector::types::{CollectorEvent, CollectorStats};
use crate::common::BINANCE_FUTURES_WS;
use algo_trade_data::FundingRateRecord;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Binance mark price update message containing funding rate.
#[derive(Debug, Deserialize)]
pub struct MarkPriceUpdate {
    /// Event type
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,
    /// Mark price
    #[serde(rename = "p")]
    pub mark_price: String,
    /// Index price
    #[serde(rename = "i")]
    pub index_price: String,
    /// Estimated settle price (funding settlement)
    #[serde(rename = "P")]
    pub estimated_settle_price: String,
    /// Funding rate
    #[serde(rename = "r")]
    pub funding_rate: String,
    /// Next funding time
    #[serde(rename = "T")]
    pub next_funding_time: i64,
}

/// Rolling history for statistical calculations.
#[derive(Debug)]
pub struct RollingHistory {
    /// Historical funding rates
    values: VecDeque<f64>,
    /// Maximum history size (e.g., 30 days * 3 funding periods = 90)
    max_size: usize,
    /// Cached sorted values for percentile calculation
    sorted_cache: Option<Vec<f64>>,
    /// Cached mean
    cached_mean: Option<f64>,
    /// Cached std dev
    cached_std: Option<f64>,
}

impl RollingHistory {
    /// Creates a new rolling history with specified max size.
    pub fn new(max_size: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(max_size),
            max_size,
            sorted_cache: None,
            cached_mean: None,
            cached_std: None,
        }
    }

    /// Adds a new value to the history.
    pub fn push(&mut self, value: f64) {
        if self.values.len() >= self.max_size {
            self.values.pop_front();
        }
        self.values.push_back(value);

        // Invalidate caches
        self.sorted_cache = None;
        self.cached_mean = None;
        self.cached_std = None;
    }

    /// Returns the number of values in history.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns true if history is empty.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns true if we have enough data for statistical calculations.
    /// Requires at least 10 data points for meaningful statistics.
    pub fn has_sufficient_data(&self) -> bool {
        self.values.len() >= 10
    }

    /// Calculates the percentile rank of a value (0.0 to 1.0).
    pub fn percentile(&mut self, value: f64) -> Option<f64> {
        if !self.has_sufficient_data() {
            return None;
        }

        // Build or use cached sorted values
        if self.sorted_cache.is_none() {
            let mut sorted: Vec<f64> = self.values.iter().copied().collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            self.sorted_cache = Some(sorted);
        }

        let sorted = self.sorted_cache.as_ref().unwrap();
        let count_below = sorted.iter().filter(|&&v| v < value).count();
        let count_equal = sorted
            .iter()
            .filter(|&&v| (v - value).abs() < f64::EPSILON)
            .count();

        // Percentile = (count below + 0.5 * count equal) / total
        Some((count_below as f64 + 0.5 * count_equal as f64) / sorted.len() as f64)
    }

    /// Calculates the z-score of a value.
    pub fn zscore(&mut self, value: f64) -> Option<f64> {
        if !self.has_sufficient_data() {
            return None;
        }

        let mean = self.mean()?;
        let std = self.std_dev()?;

        if std < f64::EPSILON {
            return Some(0.0); // All values are the same
        }

        Some((value - mean) / std)
    }

    /// Calculates the mean of historical values.
    fn mean(&mut self) -> Option<f64> {
        if self.values.is_empty() {
            return None;
        }

        if self.cached_mean.is_none() {
            let sum: f64 = self.values.iter().sum();
            self.cached_mean = Some(sum / self.values.len() as f64);
        }

        self.cached_mean
    }

    /// Calculates the standard deviation of historical values.
    fn std_dev(&mut self) -> Option<f64> {
        if self.values.len() < 2 {
            return None;
        }

        if self.cached_std.is_none() {
            let mean = self.mean()?;
            let variance: f64 = self.values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                / (self.values.len() - 1) as f64; // Sample variance
            self.cached_std = Some(variance.sqrt());
        }

        self.cached_std
    }
}

/// Funding rate data collector.
///
/// Collects funding rates from multiple symbols with rolling statistical context.
pub struct FundingCollector {
    /// Symbols to track
    symbols: Vec<String>,
    /// Channel sender for records
    tx: mpsc::Sender<FundingRateRecord>,
    /// Optional event channel for monitoring
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    /// Rolling history per symbol for percentile/z-score
    history: HashMap<String, RollingHistory>,
    /// Statistics
    stats: CollectorStats,
    /// Exchange name
    exchange: String,
    /// Reconnect delay
    reconnect_delay: Duration,
    /// Max reconnect attempts
    max_reconnect_attempts: u32,
    /// History size for statistics (number of funding periods)
    history_size: usize,
}

impl FundingCollector {
    /// Creates a new funding collector.
    pub fn new(symbols: Vec<String>, tx: mpsc::Sender<FundingRateRecord>) -> Self {
        let symbols: Vec<String> = symbols.into_iter().map(|s| s.to_lowercase()).collect();
        let history = symbols
            .iter()
            .map(|s| (s.clone(), RollingHistory::new(90))) // 30 days * 3 = 90 funding periods
            .collect();

        Self {
            symbols,
            tx,
            event_tx: None,
            history,
            stats: CollectorStats::default(),
            exchange: "binance".to_string(),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
            history_size: 90,
        }
    }

    /// Default symbols for BTC-related trading.
    pub fn default_symbols() -> Vec<String> {
        vec![
            "btcusdt".to_string(),
            "ethusdt".to_string(),
            "solusdt".to_string(),
        ]
    }

    /// Sets the event channel for monitoring.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<CollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Sets the exchange name.
    #[must_use]
    pub fn with_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.exchange = exchange.into();
        self
    }

    /// Sets the history size for statistical calculations.
    #[must_use]
    pub fn with_history_size(mut self, size: usize) -> Self {
        self.history_size = size;
        // Recreate history with new size
        self.history = self
            .symbols
            .iter()
            .map(|s| (s.clone(), RollingHistory::new(size)))
            .collect();
        self
    }

    /// Sets reconnection parameters.
    #[must_use]
    pub fn with_reconnect_config(mut self, delay: Duration, max_attempts: u32) -> Self {
        self.reconnect_delay = delay;
        self.max_reconnect_attempts = max_attempts;
        self
    }

    /// Returns a reference to the current statistics.
    pub fn stats(&self) -> &CollectorStats {
        &self.stats
    }

    /// Builds the WebSocket URL for a symbol's mark price stream.
    pub fn build_ws_url(symbol: &str) -> String {
        format!("{}{}@markPrice", BINANCE_FUTURES_WS, symbol)
    }

    /// Calculates percentile for a symbol's funding rate.
    pub fn calculate_percentile(&mut self, symbol: &str, rate: f64) -> Option<Decimal> {
        self.history
            .get_mut(symbol)
            .and_then(|h| h.percentile(rate))
            .map(|p| Decimal::from_str(&format!("{:.6}", p)).unwrap_or(Decimal::ZERO))
    }

    /// Calculates z-score for a symbol's funding rate.
    pub fn calculate_zscore(&mut self, symbol: &str, rate: f64) -> Option<Decimal> {
        self.history
            .get_mut(symbol)
            .and_then(|h| h.zscore(rate))
            .map(|z| Decimal::from_str(&format!("{:.6}", z)).unwrap_or(Decimal::ZERO))
    }

    /// Updates the rolling history for a symbol.
    pub fn update_history(&mut self, symbol: &str, rate: f64) {
        if let Some(history) = self.history.get_mut(symbol) {
            history.push(rate);
        }
    }

    /// Runs the collector for all symbols concurrently.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let symbols = self.symbols.clone();
        let mut handles = Vec::new();

        for symbol in symbols {
            let tx = self.tx.clone();
            let event_tx = self.event_tx.clone();
            let exchange = self.exchange.clone();
            let reconnect_delay = self.reconnect_delay;
            let max_attempts = self.max_reconnect_attempts;
            let history_size = self.history_size;

            let handle = tokio::spawn(async move {
                let mut single_collector =
                    SingleSymbolFundingCollector::new(symbol, tx, exchange, history_size)
                        .with_reconnect_config(reconnect_delay, max_attempts);

                if let Some(evt_tx) = event_tx {
                    single_collector = single_collector.with_event_channel(evt_tx);
                }

                single_collector.run().await
            });

            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!("Funding collector task failed: {}", e);
            }
        }

        Ok(())
    }
}

/// Single-symbol funding collector for internal use.
struct SingleSymbolFundingCollector {
    symbol: String,
    tx: mpsc::Sender<FundingRateRecord>,
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    history: RollingHistory,
    stats: CollectorStats,
    exchange: String,
    reconnect_delay: Duration,
    max_reconnect_attempts: u32,
}

impl SingleSymbolFundingCollector {
    fn new(
        symbol: String,
        tx: mpsc::Sender<FundingRateRecord>,
        exchange: String,
        history_size: usize,
    ) -> Self {
        Self {
            symbol: symbol.to_lowercase(),
            tx,
            event_tx: None,
            history: RollingHistory::new(history_size),
            stats: CollectorStats::default(),
            exchange,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
        }
    }

    fn with_event_channel(mut self, tx: mpsc::Sender<CollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    fn with_reconnect_config(mut self, delay: Duration, max_attempts: u32) -> Self {
        self.reconnect_delay = delay;
        self.max_reconnect_attempts = max_attempts;
        self
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        let mut reconnect_attempts = 0u32;

        loop {
            self.emit_event(CollectorEvent::Reconnecting {
                source: self.source_name(),
                attempt: reconnect_attempts,
            })
            .await;

            match self.collect_stream().await {
                Ok(()) => {
                    tracing::info!("Funding collector for {} exiting cleanly", self.symbol);
                    break;
                }
                Err(e) => {
                    self.stats.error_occurred();
                    tracing::error!("Funding stream error for {}: {}", self.symbol, e);

                    self.emit_event(CollectorEvent::Error {
                        source: self.source_name(),
                        error: e.to_string(),
                    })
                    .await;

                    reconnect_attempts += 1;

                    if self.max_reconnect_attempts > 0
                        && reconnect_attempts >= self.max_reconnect_attempts
                    {
                        return Err(anyhow::anyhow!("Max reconnect attempts reached"));
                    }

                    self.stats.reconnected();
                    tokio::time::sleep(self.reconnect_delay).await;
                }
            }
        }

        Ok(())
    }

    async fn collect_stream(&mut self) -> anyhow::Result<()> {
        let url = FundingCollector::build_ws_url(&self.symbol);
        tracing::info!("Connecting to funding stream: {}", url);

        let ws_stream = crate::common::connect_websocket(&url)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        self.emit_event(CollectorEvent::Connected {
            source: self.source_name(),
        })
        .await;

        let mut stream = ws_stream;
        let mut last_heartbeat = Instant::now();

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let update: MarkPriceUpdate = serde_json::from_str(&text)?;

                // Parse funding rate
                let funding_rate_f64: f64 = update.funding_rate.parse()?;
                let funding_rate = Decimal::from_str(&update.funding_rate)?;

                // Update history and calculate statistics
                self.history.push(funding_rate_f64);

                let percentile = self
                    .history
                    .percentile(funding_rate_f64)
                    .map(|p| Decimal::from_str(&format!("{:.6}", p)).unwrap_or(Decimal::ZERO));

                let zscore = self
                    .history
                    .zscore(funding_rate_f64)
                    .map(|z| Decimal::from_str(&format!("{:.6}", z)).unwrap_or(Decimal::ZERO));

                // Create record
                let timestamp =
                    DateTime::from_timestamp_millis(update.event_time).unwrap_or_else(Utc::now);

                let mut record = FundingRateRecord::new(
                    timestamp,
                    update.symbol,
                    self.exchange.clone(),
                    funding_rate,
                );

                if let (Some(p), Some(z)) = (percentile, zscore) {
                    record = record.with_statistics(p, z);
                }

                // Send record
                if self.tx.send(record).await.is_err() {
                    tracing::info!("Funding channel closed for {}", self.symbol);
                    break;
                }
                self.stats.record_collected();

                // Heartbeat
                if last_heartbeat.elapsed() > Duration::from_secs(30) {
                    self.emit_event(CollectorEvent::Heartbeat {
                        source: self.source_name(),
                        timestamp: Utc::now(),
                        records_collected: self.stats.records_collected,
                    })
                    .await;
                    last_heartbeat = Instant::now();
                }
            }
        }

        self.emit_event(CollectorEvent::Disconnected {
            source: self.source_name(),
            reason: "Stream ended".to_string(),
        })
        .await;

        Ok(())
    }

    async fn emit_event(&self, event: CollectorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    fn source_name(&self) -> String {
        format!("funding:{}", self.symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ========== Rolling History Tests ==========

    #[test]
    fn test_rolling_history_new() {
        let history = RollingHistory::new(10);
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());
        assert!(!history.has_sufficient_data());
    }

    #[test]
    fn test_rolling_history_push() {
        let mut history = RollingHistory::new(10);

        history.push(0.0001);
        assert_eq!(history.len(), 1);
        assert!(!history.is_empty());
    }

    #[test]
    fn test_rolling_history_max_size() {
        let mut history = RollingHistory::new(5);

        for i in 0..10 {
            history.push(i as f64);
        }

        // Should only keep last 5 values
        assert_eq!(history.len(), 5);
    }

    #[test]
    fn test_rolling_history_has_sufficient_data() {
        let mut history = RollingHistory::new(20);

        // Need at least 10 for sufficient data
        for i in 0..9 {
            history.push(i as f64);
        }
        assert!(!history.has_sufficient_data());

        history.push(9.0);
        assert!(history.has_sufficient_data());
    }

    // ========== Percentile Calculation Tests ==========

    #[test]
    fn test_percentile_insufficient_data() {
        let mut history = RollingHistory::new(20);

        for i in 0..5 {
            history.push(i as f64);
        }

        // Not enough data
        assert!(history.percentile(2.5).is_none());
    }

    #[test]
    fn test_percentile_median_value() {
        let mut history = RollingHistory::new(20);

        // Push values 0-19
        for i in 0..20 {
            history.push(i as f64);
        }

        // Value 10 should be around 50th percentile
        let percentile = history.percentile(10.0).unwrap();
        assert!(percentile > 0.45 && percentile < 0.55);
    }

    #[test]
    fn test_percentile_extreme_values() {
        let mut history = RollingHistory::new(20);

        for i in 0..20 {
            history.push(i as f64);
        }

        // Minimum value should be near 0 percentile
        let low_percentile = history.percentile(0.0).unwrap();
        assert!(low_percentile < 0.1);

        // Maximum value should be near 100 percentile
        let high_percentile = history.percentile(19.0).unwrap();
        assert!(high_percentile > 0.9);
    }

    #[test]
    fn test_percentile_value_below_all() {
        let mut history = RollingHistory::new(20);

        for i in 10..30 {
            history.push(i as f64);
        }

        // Value below all should be 0 percentile
        let percentile = history.percentile(5.0).unwrap();
        assert_eq!(percentile, 0.0);
    }

    #[test]
    fn test_percentile_value_above_all() {
        let mut history = RollingHistory::new(20);

        for i in 0..20 {
            history.push(i as f64);
        }

        // Value above all should be 100 percentile
        let percentile = history.percentile(100.0).unwrap();
        assert_eq!(percentile, 1.0);
    }

    // ========== Z-Score Calculation Tests ==========

    #[test]
    fn test_zscore_insufficient_data() {
        let mut history = RollingHistory::new(20);

        for i in 0..5 {
            history.push(i as f64);
        }

        assert!(history.zscore(2.5).is_none());
    }

    #[test]
    fn test_zscore_mean_value() {
        let mut history = RollingHistory::new(20);

        // Push same value 20 times - mean = 100, std = 0
        for _ in 0..20 {
            history.push(100.0);
        }

        // Any value should have z-score of 0 when std is 0
        let zscore = history.zscore(100.0).unwrap();
        assert!((zscore - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_zscore_normal_distribution() {
        let mut history = RollingHistory::new(100);

        // Mean = 0, values from -10 to 10
        for i in -50..50 {
            history.push(i as f64);
        }

        // Value at mean should have z-score near 0
        let zscore_mean = history.zscore(0.0).unwrap();
        assert!(zscore_mean.abs() < 0.1);

        // Positive value should have positive z-score
        let zscore_high = history.zscore(30.0).unwrap();
        assert!(zscore_high > 0.5);

        // Negative value should have negative z-score
        let zscore_low = history.zscore(-30.0).unwrap();
        assert!(zscore_low < -0.5);
    }

    #[test]
    fn test_zscore_extreme_value() {
        let mut history = RollingHistory::new(20);

        // Normal funding rates around 0.0001
        for _ in 0..20 {
            history.push(0.0001);
        }

        // Extreme funding rate should have high z-score
        let zscore = history.zscore(0.001).unwrap();
        // Since all values are the same, std is near 0, but we handle that case
        // This tests the edge case handling
        assert!(zscore.is_finite());
    }

    // ========== Mark Price Update Parsing ==========

    #[test]
    fn test_parse_mark_price_update() {
        let json = r#"{
            "e": "markPriceUpdate",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "p": "42750.00",
            "i": "42749.50",
            "P": "42750.25",
            "r": "0.00010000",
            "T": 1700000000000
        }"#;

        let update: MarkPriceUpdate = serde_json::from_str(json).unwrap();

        assert_eq!(update.event_type, "markPriceUpdate");
        assert_eq!(update.event_time, 1699999999999);
        assert_eq!(update.symbol, "BTCUSDT");
        assert_eq!(update.mark_price, "42750.00");
        assert_eq!(update.funding_rate, "0.00010000");
        assert_eq!(update.next_funding_time, 1700000000000);
    }

    #[test]
    fn test_parse_negative_funding_rate() {
        let json = r#"{
            "e": "markPriceUpdate",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "p": "42750.00",
            "i": "42749.50",
            "P": "42750.25",
            "r": "-0.00050000",
            "T": 1700000000000
        }"#;

        let update: MarkPriceUpdate = serde_json::from_str(json).unwrap();

        assert_eq!(update.funding_rate, "-0.00050000");

        let rate: f64 = update.funding_rate.parse().unwrap();
        assert!(rate < 0.0);
    }

    // ========== Funding Collector Tests ==========

    #[test]
    fn test_funding_collector_default_symbols() {
        let symbols = FundingCollector::default_symbols();
        assert!(symbols.contains(&"btcusdt".to_string()));
        assert!(symbols.contains(&"ethusdt".to_string()));
    }

    #[test]
    fn test_funding_collector_build_ws_url() {
        let url = FundingCollector::build_ws_url("btcusdt");
        assert!(url.contains("btcusdt@markPrice"));
    }

    #[test]
    fn test_funding_collector_creation() {
        let (tx, _rx) = mpsc::channel(100);
        let collector = FundingCollector::new(vec!["btcusdt".to_string()], tx);

        assert_eq!(collector.symbols.len(), 1);
        assert_eq!(collector.symbols[0], "btcusdt");
    }

    #[test]
    fn test_funding_collector_with_config() {
        let (tx, _rx) = mpsc::channel(100);
        let collector = FundingCollector::new(vec!["btcusdt".to_string()], tx)
            .with_exchange("hyperliquid")
            .with_history_size(180)
            .with_reconnect_config(Duration::from_secs(10), 5);

        assert_eq!(collector.exchange, "hyperliquid");
        assert_eq!(collector.history_size, 180);
        assert_eq!(collector.reconnect_delay, Duration::from_secs(10));
        assert_eq!(collector.max_reconnect_attempts, 5);
    }

    #[test]
    fn test_funding_collector_calculate_percentile() {
        let (tx, _rx) = mpsc::channel(100);
        let mut collector = FundingCollector::new(vec!["btcusdt".to_string()], tx);

        // Not enough data
        assert!(collector.calculate_percentile("btcusdt", 0.0001).is_none());

        // Add enough data
        for i in 0..20 {
            collector.update_history("btcusdt", i as f64 * 0.0001);
        }

        let percentile = collector.calculate_percentile("btcusdt", 0.001);
        assert!(percentile.is_some());
    }

    #[test]
    fn test_funding_collector_calculate_zscore() {
        let (tx, _rx) = mpsc::channel(100);
        let mut collector = FundingCollector::new(vec!["btcusdt".to_string()], tx);

        // Add enough data
        for i in 0..20 {
            collector.update_history("btcusdt", (i as f64 - 10.0) * 0.0001);
        }

        let zscore = collector.calculate_zscore("btcusdt", 0.0005);
        assert!(zscore.is_some());
    }

    #[test]
    fn test_funding_collector_unknown_symbol() {
        let (tx, _rx) = mpsc::channel(100);
        let mut collector = FundingCollector::new(vec!["btcusdt".to_string()], tx);

        // Unknown symbol should return None
        assert!(collector.calculate_percentile("unknown", 0.0001).is_none());
        assert!(collector.calculate_zscore("unknown", 0.0001).is_none());
    }

    // ========== FundingRateRecord Integration ==========

    #[test]
    fn test_funding_record_creation() {
        let timestamp = Utc::now();
        let record = FundingRateRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        );

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.funding_rate, dec!(0.0001));
        // Annual rate = 0.0001 * 3 * 365 = 0.1095
        assert_eq!(record.annual_rate, Some(dec!(0.1095)));
    }

    #[test]
    fn test_funding_record_with_statistics() {
        let timestamp = Utc::now();
        let record = FundingRateRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        )
        .with_statistics(dec!(0.75), dec!(1.5));

        assert_eq!(record.rate_percentile, Some(dec!(0.75)));
        assert_eq!(record.rate_zscore, Some(dec!(1.5)));
    }
}
