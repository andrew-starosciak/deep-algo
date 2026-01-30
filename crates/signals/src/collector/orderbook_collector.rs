//! Order book data collector for Binance Futures.
//!
//! Connects to Binance WebSocket and collects 20-level order book snapshots
//! at 1-second intervals for order book imbalance signal generation.

use crate::collector::types::{CollectorConfig, CollectorEvent, CollectorStats};
use crate::common::BINANCE_FUTURES_WS;
use algo_trade_data::OrderBookSnapshotRecord;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Binance order book depth update message.
#[derive(Debug, Deserialize)]
pub struct DepthUpdate {
    /// Event type
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Transaction time
    #[serde(rename = "T")]
    pub transaction_time: i64,
    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,
    /// First update ID
    #[serde(rename = "U")]
    pub first_update_id: u64,
    /// Final update ID
    #[serde(rename = "u")]
    pub last_update_id: u64,
    /// Previous final update ID
    #[serde(rename = "pu")]
    pub prev_update_id: u64,
    /// Bids (price, quantity)
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    /// Asks (price, quantity)
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

/// Wrapper for streamed data
#[derive(Debug, Deserialize)]
pub struct StreamWrapper {
    /// Stream name
    pub stream: String,
    /// Data payload
    pub data: DepthUpdate,
}

/// Parses raw bid/ask levels into JSON format for storage.
pub fn parse_levels_to_json(levels: &[[String; 2]]) -> JsonValue {
    let parsed: Vec<JsonValue> = levels
        .iter()
        .map(|[price, qty]| json!([price, qty]))
        .collect();
    JsonValue::Array(parsed)
}

/// Calculates order book imbalance from bid and ask levels.
///
/// Imbalance = (bid_volume - ask_volume) / (bid_volume + ask_volume)
/// Returns value in range [-1.0, 1.0]
pub fn calculate_imbalance(bids: &[[String; 2]], asks: &[[String; 2]]) -> Decimal {
    let bid_volume = calculate_total_volume(bids);
    let ask_volume = calculate_total_volume(asks);

    let total = bid_volume + ask_volume;
    if total > Decimal::ZERO {
        (bid_volume - ask_volume) / total
    } else {
        Decimal::ZERO
    }
}

/// Calculates total volume from price levels.
pub fn calculate_total_volume(levels: &[[String; 2]]) -> Decimal {
    levels
        .iter()
        .filter_map(|[_price, qty]| Decimal::from_str(qty).ok())
        .sum()
}

/// Calculates mid price from best bid and ask.
pub fn calculate_mid_price(bids: &[[String; 2]], asks: &[[String; 2]]) -> Option<Decimal> {
    let best_bid = bids.first().and_then(|[p, _]| Decimal::from_str(p).ok())?;
    let best_ask = asks.first().and_then(|[p, _]| Decimal::from_str(p).ok())?;

    if best_bid > Decimal::ZERO && best_ask > Decimal::ZERO {
        Some((best_bid + best_ask) / Decimal::TWO)
    } else {
        None
    }
}

/// Calculates spread in basis points.
pub fn calculate_spread_bps(bids: &[[String; 2]], asks: &[[String; 2]]) -> Option<Decimal> {
    let best_bid = bids.first().and_then(|[p, _]| Decimal::from_str(p).ok())?;
    let best_ask = asks.first().and_then(|[p, _]| Decimal::from_str(p).ok())?;
    let mid = calculate_mid_price(bids, asks)?;

    if mid > Decimal::ZERO {
        Some(((best_ask - best_bid) / mid) * Decimal::from(10000))
    } else {
        None
    }
}

/// Aggregator for 1-second order book snapshots.
///
/// Binance sends depth updates at 100ms intervals. This aggregates
/// multiple updates into 1-second snapshots, using the latest state.
pub struct OrderBookAggregator {
    /// Last snapshot time (truncated to second)
    last_snapshot_second: Option<i64>,
    /// Latest bids (accumulator for current second)
    latest_bids: Vec<[String; 2]>,
    /// Latest asks (accumulator for current second)
    latest_asks: Vec<[String; 2]>,
    /// Latest event time
    latest_event_time: i64,
    /// Symbol
    symbol: String,
    /// Exchange
    exchange: String,
}

impl OrderBookAggregator {
    /// Creates a new aggregator.
    pub fn new(symbol: String, exchange: String) -> Self {
        Self {
            last_snapshot_second: None,
            latest_bids: Vec::new(),
            latest_asks: Vec::new(),
            latest_event_time: 0,
            symbol,
            exchange,
        }
    }

    /// Processes a depth update, returning a snapshot if a second boundary is crossed.
    pub fn process(&mut self, update: &DepthUpdate) -> Option<OrderBookSnapshotRecord> {
        let event_second = update.event_time / 1000;

        // Check if we crossed a second boundary
        let result = if let Some(last_second) = self.last_snapshot_second {
            if event_second > last_second && !self.latest_bids.is_empty() {
                // Create snapshot from previous second's data
                Some(self.create_snapshot())
            } else {
                None
            }
        } else {
            None
        };

        // Update latest state
        self.last_snapshot_second = Some(event_second);
        self.latest_bids = update.bids.clone();
        self.latest_asks = update.asks.clone();
        self.latest_event_time = update.event_time;

        result
    }

    /// Creates a snapshot from current state.
    fn create_snapshot(&self) -> OrderBookSnapshotRecord {
        let timestamp =
            DateTime::from_timestamp_millis(self.latest_event_time).unwrap_or_else(Utc::now);
        let bid_levels = parse_levels_to_json(&self.latest_bids);
        let ask_levels = parse_levels_to_json(&self.latest_asks);

        OrderBookSnapshotRecord::new(
            timestamp,
            self.symbol.clone(),
            self.exchange.clone(),
            bid_levels,
            ask_levels,
        )
    }

    /// Forces creation of a snapshot from current state (for shutdown).
    pub fn flush(&self) -> Option<OrderBookSnapshotRecord> {
        if !self.latest_bids.is_empty() {
            Some(self.create_snapshot())
        } else {
            None
        }
    }
}

/// Order book data collector.
///
/// Connects to Binance Futures WebSocket and emits order book snapshots
/// at 1-second intervals.
pub struct OrderBookCollector {
    /// Configuration
    config: CollectorConfig,
    /// Channel sender for snapshots
    tx: mpsc::Sender<OrderBookSnapshotRecord>,
    /// Optional event channel for monitoring
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    /// Statistics
    stats: CollectorStats,
}

impl OrderBookCollector {
    /// Creates a new order book collector.
    pub fn new(config: CollectorConfig, tx: mpsc::Sender<OrderBookSnapshotRecord>) -> Self {
        Self {
            config,
            tx,
            event_tx: None,
            stats: CollectorStats::default(),
        }
    }

    /// Sets the event channel for monitoring.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<CollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Returns a reference to the current statistics.
    pub fn stats(&self) -> &CollectorStats {
        &self.stats
    }

    /// Builds the WebSocket URL for order book depth stream.
    pub fn build_ws_url(&self) -> String {
        // Use combined stream format for depth20@100ms
        format!(
            "{}stream?streams={}@depth20@100ms",
            BINANCE_FUTURES_WS.replace("/ws/", "/"),
            self.config.symbol
        )
    }

    /// Runs the collector, reconnecting on failures.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let mut reconnect_attempts = 0u32;

        loop {
            self.emit_event(CollectorEvent::Reconnecting {
                source: self.source_name(),
                attempt: reconnect_attempts,
            })
            .await;

            match self.collect_stream().await {
                Ok(()) => {
                    // Clean exit (channel closed)
                    tracing::info!("Order book collector exiting cleanly");
                    break;
                }
                Err(e) => {
                    self.stats.error_occurred();
                    tracing::error!("Order book stream error: {}", e);

                    self.emit_event(CollectorEvent::Error {
                        source: self.source_name(),
                        error: e.to_string(),
                    })
                    .await;

                    reconnect_attempts += 1;

                    // Check max reconnect attempts
                    if self.config.max_reconnect_attempts > 0
                        && reconnect_attempts >= self.config.max_reconnect_attempts
                    {
                        tracing::error!(
                            "Max reconnect attempts ({}) reached, stopping collector",
                            self.config.max_reconnect_attempts
                        );
                        return Err(anyhow::anyhow!("Max reconnect attempts reached"));
                    }

                    self.stats.reconnected();
                    tokio::time::sleep(self.config.reconnect_delay).await;
                }
            }
        }

        Ok(())
    }

    /// Collects data from the WebSocket stream.
    async fn collect_stream(&mut self) -> anyhow::Result<()> {
        let url = self.build_ws_url();
        tracing::info!("Connecting to order book stream: {}", url);

        let ws_stream = crate::common::connect_websocket(&url)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        self.emit_event(CollectorEvent::Connected {
            source: self.source_name(),
        })
        .await;

        let mut aggregator = OrderBookAggregator::new(
            self.config.symbol.to_uppercase(),
            self.config.exchange.clone(),
        );

        let mut stream = ws_stream;
        let mut last_heartbeat = Instant::now();

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                // Parse the wrapped stream message
                let wrapper: StreamWrapper = serde_json::from_str(&text)?;
                let update = wrapper.data;

                // Process through aggregator
                if let Some(snapshot) = aggregator.process(&update) {
                    // Try to send, exit if channel closed
                    if self.tx.send(snapshot).await.is_err() {
                        tracing::info!("Snapshot channel closed, exiting");
                        break;
                    }
                    self.stats.record_collected();
                }

                // Emit heartbeat every 30 seconds
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

    /// Helper to emit events.
    async fn emit_event(&self, event: CollectorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    /// Returns the source name for logging/events.
    fn source_name(&self) -> String {
        format!("orderbook:{}", self.config.symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ========== Helper Functions ==========

    fn sample_bids() -> Vec<[String; 2]> {
        vec![
            ["50000.00".to_string(), "1.500".to_string()],
            ["49999.50".to_string(), "2.300".to_string()],
            ["49999.00".to_string(), "3.200".to_string()],
        ]
    }

    fn sample_asks() -> Vec<[String; 2]> {
        vec![
            ["50000.50".to_string(), "0.800".to_string()],
            ["50001.00".to_string(), "1.200".to_string()],
            ["50001.50".to_string(), "1.500".to_string()],
        ]
    }

    fn sample_depth_update(event_time: i64) -> DepthUpdate {
        DepthUpdate {
            event_type: "depthUpdate".to_string(),
            event_time,
            transaction_time: event_time - 1,
            symbol: "BTCUSDT".to_string(),
            first_update_id: 1000,
            last_update_id: 1001,
            prev_update_id: 999,
            bids: sample_bids(),
            asks: sample_asks(),
        }
    }

    // ========== Unit Tests for Parsing Functions ==========

    #[test]
    fn test_parse_levels_to_json() {
        let levels = vec![
            ["50000.00".to_string(), "1.5".to_string()],
            ["49999.50".to_string(), "2.3".to_string()],
        ];

        let json = parse_levels_to_json(&levels);

        assert!(json.is_array());
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        let first = arr[0].as_array().unwrap();
        assert_eq!(first[0].as_str().unwrap(), "50000.00");
        assert_eq!(first[1].as_str().unwrap(), "1.5");
    }

    #[test]
    fn test_parse_levels_empty() {
        let levels: Vec<[String; 2]> = vec![];
        let json = parse_levels_to_json(&levels);

        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // ========== Unit Tests for Imbalance Calculation ==========

    #[test]
    fn test_calculate_imbalance_balanced() {
        let bids = vec![["50000".to_string(), "10.0".to_string()]];
        let asks = vec![["50001".to_string(), "10.0".to_string()]];

        let imbalance = calculate_imbalance(&bids, &asks);
        assert_eq!(imbalance, Decimal::ZERO);
    }

    #[test]
    fn test_calculate_imbalance_bullish() {
        // More bids than asks -> positive imbalance
        let bids = vec![
            ["50000".to_string(), "10.0".to_string()],
            ["49999".to_string(), "5.0".to_string()],
        ];
        let asks = vec![["50001".to_string(), "5.0".to_string()]];

        let imbalance = calculate_imbalance(&bids, &asks);
        // bid_vol = 15, ask_vol = 5, imbalance = (15-5)/(15+5) = 0.5
        assert_eq!(imbalance, dec!(0.5));
    }

    #[test]
    fn test_calculate_imbalance_bearish() {
        // More asks than bids -> negative imbalance
        let bids = vec![["50000".to_string(), "5.0".to_string()]];
        let asks = vec![
            ["50001".to_string(), "10.0".to_string()],
            ["50002".to_string(), "5.0".to_string()],
        ];

        let imbalance = calculate_imbalance(&bids, &asks);
        // bid_vol = 5, ask_vol = 15, imbalance = (5-15)/(5+15) = -0.5
        assert_eq!(imbalance, dec!(-0.5));
    }

    #[test]
    fn test_calculate_imbalance_empty() {
        let bids: Vec<[String; 2]> = vec![];
        let asks: Vec<[String; 2]> = vec![];

        let imbalance = calculate_imbalance(&bids, &asks);
        assert_eq!(imbalance, Decimal::ZERO);
    }

    #[test]
    fn test_calculate_imbalance_invalid_quantities() {
        // Invalid quantities should be skipped
        let bids = vec![
            ["50000".to_string(), "10.0".to_string()],
            ["49999".to_string(), "invalid".to_string()],
        ];
        let asks = vec![["50001".to_string(), "10.0".to_string()]];

        let imbalance = calculate_imbalance(&bids, &asks);
        // bid_vol = 10 (invalid skipped), ask_vol = 10, imbalance = 0
        assert_eq!(imbalance, Decimal::ZERO);
    }

    // ========== Unit Tests for Volume Calculation ==========

    #[test]
    fn test_calculate_total_volume() {
        let levels = vec![
            ["50000".to_string(), "1.5".to_string()],
            ["49999".to_string(), "2.5".to_string()],
            ["49998".to_string(), "3.0".to_string()],
        ];

        let volume = calculate_total_volume(&levels);
        assert_eq!(volume, dec!(7.0));
    }

    #[test]
    fn test_calculate_total_volume_empty() {
        let levels: Vec<[String; 2]> = vec![];
        let volume = calculate_total_volume(&levels);
        assert_eq!(volume, Decimal::ZERO);
    }

    // ========== Unit Tests for Mid Price ==========

    #[test]
    fn test_calculate_mid_price() {
        let bids = vec![["50000".to_string(), "1.0".to_string()]];
        let asks = vec![["50010".to_string(), "1.0".to_string()]];

        let mid = calculate_mid_price(&bids, &asks);
        assert_eq!(mid, Some(dec!(50005)));
    }

    #[test]
    fn test_calculate_mid_price_empty_bids() {
        let bids: Vec<[String; 2]> = vec![];
        let asks = vec![["50010".to_string(), "1.0".to_string()]];

        let mid = calculate_mid_price(&bids, &asks);
        assert_eq!(mid, None);
    }

    #[test]
    fn test_calculate_mid_price_empty_asks() {
        let bids = vec![["50000".to_string(), "1.0".to_string()]];
        let asks: Vec<[String; 2]> = vec![];

        let mid = calculate_mid_price(&bids, &asks);
        assert_eq!(mid, None);
    }

    // ========== Unit Tests for Spread ==========

    #[test]
    fn test_calculate_spread_bps() {
        let bids = vec![["50000".to_string(), "1.0".to_string()]];
        let asks = vec![["50010".to_string(), "1.0".to_string()]];

        let spread = calculate_spread_bps(&bids, &asks);
        // spread = (50010 - 50000) / 50005 * 10000 = 10/50005*10000 ~= 2.0
        let spread_val = spread.unwrap();
        assert!(spread_val > dec!(1.9) && spread_val < dec!(2.1));
    }

    #[test]
    fn test_calculate_spread_bps_empty() {
        let bids: Vec<[String; 2]> = vec![];
        let asks: Vec<[String; 2]> = vec![];

        let spread = calculate_spread_bps(&bids, &asks);
        assert_eq!(spread, None);
    }

    // ========== Unit Tests for Aggregator ==========

    #[test]
    fn test_aggregator_first_update_no_snapshot() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        let update = sample_depth_update(1700000000000); // First update

        let snapshot = aggregator.process(&update);
        // First update should not produce a snapshot
        assert!(snapshot.is_none());
    }

    #[test]
    fn test_aggregator_same_second_no_snapshot() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        // Two updates in the same second (100ms apart)
        let update1 = sample_depth_update(1700000000000);
        let update2 = sample_depth_update(1700000000100);

        let _ = aggregator.process(&update1);
        let snapshot = aggregator.process(&update2);

        // Same second, no snapshot
        assert!(snapshot.is_none());
    }

    #[test]
    fn test_aggregator_new_second_produces_snapshot() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        // First update at second 0
        let update1 = sample_depth_update(1700000000000);
        let _ = aggregator.process(&update1);

        // Second update at second 1 (crosses boundary)
        let update2 = sample_depth_update(1700000001000);
        let snapshot = aggregator.process(&update2);

        assert!(snapshot.is_some());
        let snap = snapshot.unwrap();
        assert_eq!(snap.symbol, "BTCUSDT");
        assert_eq!(snap.exchange, "binance");
    }

    #[test]
    fn test_aggregator_snapshot_uses_latest_data() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        // First update
        let update1 = sample_depth_update(1700000000000);
        let _ = aggregator.process(&update1);

        // Update within same second with different data
        let mut update2 = sample_depth_update(1700000000500);
        update2.bids = vec![["51000".to_string(), "5.0".to_string()]];
        let _ = aggregator.process(&update2);

        // Cross second boundary
        let update3 = sample_depth_update(1700000001000);
        let snapshot = aggregator.process(&update3);

        let snap = snapshot.unwrap();
        // Should use update2's data (the latest from previous second)
        let bids = snap.bid_levels.as_array().unwrap();
        assert_eq!(bids[0][0].as_str().unwrap(), "51000");
    }

    #[test]
    fn test_aggregator_flush() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        let update = sample_depth_update(1700000000000);
        let _ = aggregator.process(&update);

        // Flush should return current state
        let snapshot = aggregator.flush();
        assert!(snapshot.is_some());
    }

    #[test]
    fn test_aggregator_flush_empty() {
        let aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        // No data processed, flush should return None
        let snapshot = aggregator.flush();
        assert!(snapshot.is_none());
    }

    // ========== Unit Tests for Collector ==========

    #[test]
    fn test_collector_build_ws_url() {
        let config = CollectorConfig::new("btcusdt");
        let (tx, _rx) = mpsc::channel(1);
        let collector = OrderBookCollector::new(config, tx);

        let url = collector.build_ws_url();
        assert!(url.contains("btcusdt@depth20@100ms"));
        assert!(url.contains("stream?streams="));
    }

    #[test]
    fn test_collector_stats_default() {
        let config = CollectorConfig::new("ethusdt");
        let (tx, _rx) = mpsc::channel(1);
        let collector = OrderBookCollector::new(config, tx);

        let stats = collector.stats();
        assert_eq!(stats.records_collected, 0);
        assert_eq!(stats.errors_encountered, 0);
    }

    // ========== Parsing Tests for Binance Messages ==========

    #[test]
    fn test_parse_depth_update_message() {
        let json = r#"{
            "e": "depthUpdate",
            "E": 1699999999999,
            "T": 1699999999998,
            "s": "BTCUSDT",
            "U": 1234567890,
            "u": 1234567891,
            "pu": 1234567889,
            "b": [["42750.00", "1.500"], ["42749.50", "2.300"]],
            "a": [["42750.50", "0.800"], ["42751.00", "1.200"]]
        }"#;

        let update: DepthUpdate = serde_json::from_str(json).unwrap();

        assert_eq!(update.event_type, "depthUpdate");
        assert_eq!(update.event_time, 1699999999999);
        assert_eq!(update.symbol, "BTCUSDT");
        assert_eq!(update.bids.len(), 2);
        assert_eq!(update.asks.len(), 2);
        assert_eq!(update.bids[0][0], "42750.00");
        assert_eq!(update.bids[0][1], "1.500");
    }

    #[test]
    fn test_parse_stream_wrapper() {
        let json = r#"{
            "stream": "btcusdt@depth20@100ms",
            "data": {
                "e": "depthUpdate",
                "E": 1699999999999,
                "T": 1699999999998,
                "s": "BTCUSDT",
                "U": 1234567890,
                "u": 1234567891,
                "pu": 1234567889,
                "b": [["42750.00", "1.500"]],
                "a": [["42750.50", "0.800"]]
            }
        }"#;

        let wrapper: StreamWrapper = serde_json::from_str(json).unwrap();

        assert_eq!(wrapper.stream, "btcusdt@depth20@100ms");
        assert_eq!(wrapper.data.symbol, "BTCUSDT");
    }

    // ========== Integration Test for Snapshot Record ==========

    #[test]
    fn test_aggregator_creates_valid_snapshot_record() {
        let mut aggregator = OrderBookAggregator::new("BTCUSDT".to_string(), "binance".to_string());

        // Process update and cross second boundary
        let update1 = sample_depth_update(1700000000000);
        let _ = aggregator.process(&update1);

        let update2 = sample_depth_update(1700000001000);
        let snapshot = aggregator.process(&update2).unwrap();

        // Verify the snapshot record is properly constructed
        assert_eq!(snapshot.symbol, "BTCUSDT");
        assert_eq!(snapshot.exchange, "binance");

        // Verify calculated fields
        assert!(snapshot.bid_volume > Decimal::ZERO);
        assert!(snapshot.ask_volume > Decimal::ZERO);
        assert!(snapshot.mid_price.is_some());
        assert!(snapshot.spread_bps.is_some());

        // Bid volume > ask volume from sample data, so imbalance should be positive
        // sample_bids: 1.5 + 2.3 + 3.2 = 7.0
        // sample_asks: 0.8 + 1.2 + 1.5 = 3.5
        assert!(snapshot.imbalance > Decimal::ZERO);
    }
}
