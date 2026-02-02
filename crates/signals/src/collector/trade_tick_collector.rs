//! Trade tick data collector for Binance Futures.
//!
//! Connects to Binance WebSocket aggTrade stream and collects trade executions
//! for CVD (Cumulative Volume Delta) signal generation.

use crate::collector::types::{CollectorConfig, CollectorEvent, CollectorStats};
use crate::common::BINANCE_FUTURES_WS;
use algo_trade_data::{CvdAggregateRecord, TradeSide, TradeTickRecord};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Binance aggTrade WebSocket message.
///
/// JSON format:
/// ```json
/// {
///   "e": "aggTrade",
///   "E": 1699999999999,
///   "s": "BTCUSDT",
///   "a": 123456789,
///   "p": "42750.50",
///   "q": "0.150",
///   "f": 100,
///   "l": 102,
///   "T": 1699999999998,
///   "m": true
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct AggTradeEvent {
    /// Event type ("aggTrade")
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time (milliseconds)
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,
    /// Aggregate trade ID
    #[serde(rename = "a")]
    pub agg_trade_id: i64,
    /// Price
    #[serde(rename = "p")]
    pub price: String,
    /// Quantity
    #[serde(rename = "q")]
    pub quantity: String,
    /// First trade ID
    #[serde(rename = "f")]
    pub first_trade_id: i64,
    /// Last trade ID
    #[serde(rename = "l")]
    pub last_trade_id: i64,
    /// Trade time (milliseconds)
    #[serde(rename = "T")]
    pub trade_time: i64,
    /// Is the buyer the market maker?
    /// true = buyer is maker (seller aggressor) = SELL
    /// false = buyer is taker (buyer aggressor) = BUY
    #[serde(rename = "m")]
    pub buyer_is_maker: bool,
}

impl AggTradeEvent {
    /// Converts to a TradeTickRecord.
    ///
    /// # Errors
    /// Returns None if price or quantity parsing fails.
    pub fn to_trade_tick(&self) -> Option<TradeTickRecord> {
        let price = Decimal::from_str(&self.price).ok()?;
        let quantity = Decimal::from_str(&self.quantity).ok()?;

        let timestamp = DateTime::from_timestamp_millis(self.trade_time)?;

        Some(TradeTickRecord::from_binance_agg_trade(
            timestamp,
            self.symbol.clone(),
            self.agg_trade_id,
            price,
            quantity,
            self.buyer_is_maker,
        ))
    }

    /// Returns the trade side based on the maker flag.
    #[must_use]
    pub fn trade_side(&self) -> TradeSide {
        TradeSide::from_binance_maker_flag(self.buyer_is_maker)
    }
}

/// Wrapper for streamed aggTrade data.
#[derive(Debug, Deserialize)]
pub struct AggTradeStreamWrapper {
    /// Stream name
    pub stream: String,
    /// Data payload
    pub data: AggTradeEvent,
}

/// Maximum trades per window to prevent memory exhaustion.
const MAX_TRADES_PER_WINDOW: usize = 100_000;

/// Aggregates trade ticks into CVD windows.
///
/// Buffers trades and emits CVD aggregates at configurable window boundaries.
/// Enforces a maximum of `MAX_TRADES_PER_WINDOW` trades per window to prevent
/// memory exhaustion from malicious or malfunctioning data feeds.
pub struct CvdAggregator {
    /// Window duration in seconds
    window_seconds: i32,
    /// Symbol being aggregated
    symbol: String,
    /// Exchange name
    exchange: String,
    /// Current window start timestamp (truncated to window)
    current_window_start: Option<i64>,
    /// Trades in the current window
    window_trades: Vec<TradeTickRecord>,
    /// Count of dropped trades due to window limit
    dropped_trades: u64,
}

impl CvdAggregator {
    /// Creates a new CVD aggregator.
    pub fn new(symbol: String, exchange: String, window_seconds: i32) -> Self {
        Self {
            window_seconds,
            symbol,
            exchange,
            current_window_start: None,
            window_trades: Vec::with_capacity(1000),
            dropped_trades: 0,
        }
    }

    /// Processes a trade tick, returning a CVD aggregate if window boundary crossed.
    pub fn process(&mut self, trade: TradeTickRecord) -> Option<CvdAggregateRecord> {
        let trade_ms = trade.timestamp.timestamp_millis();
        let window_ms = (self.window_seconds as i64) * 1000;
        let trade_window_start = (trade_ms / window_ms) * window_ms;

        let result = if let Some(current_start) = self.current_window_start {
            if trade_window_start > current_start {
                // Window boundary crossed - emit aggregate for previous window
                let aggregate = self.emit_aggregate(current_start);
                self.current_window_start = Some(trade_window_start);
                Some(aggregate)
            } else {
                None
            }
        } else {
            // First trade - initialize window
            self.current_window_start = Some(trade_window_start);
            None
        };

        // Add trade to current window (with bounds check to prevent memory exhaustion)
        if self.window_trades.len() < MAX_TRADES_PER_WINDOW {
            self.window_trades.push(trade);
        } else {
            self.dropped_trades += 1;
            if self.dropped_trades == 1 || self.dropped_trades.is_multiple_of(1000) {
                tracing::warn!(
                    "CVD aggregator dropping trades: {} dropped (limit: {})",
                    self.dropped_trades,
                    MAX_TRADES_PER_WINDOW
                );
            }
        }

        result
    }

    /// Returns the count of trades dropped due to window limit.
    #[must_use]
    pub fn dropped_trades(&self) -> u64 {
        self.dropped_trades
    }

    /// Emits an aggregate for the specified window start.
    fn emit_aggregate(&mut self, window_start_ms: i64) -> CvdAggregateRecord {
        // Calculate window end timestamp
        let window_end_ms = window_start_ms + (self.window_seconds as i64) * 1000;
        let timestamp = DateTime::from_timestamp_millis(window_end_ms).unwrap_or_else(Utc::now);

        // Create aggregate from accumulated trades
        let aggregate = CvdAggregateRecord::from_trades(
            timestamp,
            self.symbol.clone(),
            self.exchange.clone(),
            self.window_seconds,
            &self.window_trades,
        );

        // Clear trades for next window
        self.window_trades.clear();

        aggregate
    }

    /// Flushes any remaining trades as a partial window.
    pub fn flush(&mut self) -> Option<CvdAggregateRecord> {
        if self.window_trades.is_empty() {
            return None;
        }

        let window_start = self.current_window_start?;
        Some(self.emit_aggregate(window_start))
    }

    /// Returns the number of trades in the current window.
    #[must_use]
    pub fn current_window_trade_count(&self) -> usize {
        self.window_trades.len()
    }
}

/// Trade tick data collector.
///
/// Connects to Binance Futures WebSocket and emits trade ticks
/// and optional CVD aggregates.
pub struct TradeTickCollector {
    /// Configuration
    config: CollectorConfig,
    /// Channel sender for trade ticks
    tx: mpsc::Sender<TradeTickRecord>,
    /// Optional channel for CVD aggregates
    cvd_tx: Option<mpsc::Sender<CvdAggregateRecord>>,
    /// CVD aggregation window in seconds (None = no aggregation)
    cvd_window_seconds: Option<i32>,
    /// Optional event channel for monitoring
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    /// Statistics
    stats: CollectorStats,
}

impl TradeTickCollector {
    /// Creates a new trade tick collector.
    pub fn new(config: CollectorConfig, tx: mpsc::Sender<TradeTickRecord>) -> Self {
        Self {
            config,
            tx,
            cvd_tx: None,
            cvd_window_seconds: None,
            event_tx: None,
            stats: CollectorStats::default(),
        }
    }

    /// Sets the CVD aggregate channel with the specified window.
    #[must_use]
    pub fn with_cvd_channel(
        mut self,
        tx: mpsc::Sender<CvdAggregateRecord>,
        window_seconds: i32,
    ) -> Self {
        self.cvd_tx = Some(tx);
        self.cvd_window_seconds = Some(window_seconds);
        self
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

    /// Builds the WebSocket URL for aggTrade stream.
    pub fn build_ws_url(&self) -> String {
        format!(
            "{}stream?streams={}@aggTrade",
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
                    tracing::info!("Trade tick collector exiting cleanly");
                    break;
                }
                Err(e) => {
                    self.stats.error_occurred();
                    tracing::error!("Trade tick stream error: {}", e);

                    self.emit_event(CollectorEvent::Error {
                        source: self.source_name(),
                        error: e.to_string(),
                    })
                    .await;

                    reconnect_attempts += 1;

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
        tracing::info!("Connecting to trade tick stream: {}", url);

        let ws_stream = crate::common::connect_websocket(&url)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        self.emit_event(CollectorEvent::Connected {
            source: self.source_name(),
        })
        .await;

        // Create CVD aggregator if enabled
        let mut cvd_aggregator = self.cvd_window_seconds.map(|window_secs| {
            CvdAggregator::new(
                self.config.symbol.to_uppercase(),
                self.config.exchange.clone(),
                window_secs,
            )
        });

        let mut stream = ws_stream;
        let mut last_heartbeat = Instant::now();

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                // Parse the wrapped stream message
                let wrapper: AggTradeStreamWrapper = serde_json::from_str(&text)?;
                let event = wrapper.data;

                // Convert to trade tick
                if let Some(trade) = event.to_trade_tick() {
                    // Process through CVD aggregator if enabled
                    if let Some(ref mut aggregator) = cvd_aggregator {
                        if let Some(cvd_agg) = aggregator.process(trade.clone()) {
                            if let Some(ref cvd_tx) = self.cvd_tx {
                                if cvd_tx.send(cvd_agg).await.is_err() {
                                    tracing::debug!("CVD channel closed");
                                }
                            }
                        }
                    }

                    // Send trade tick
                    if self.tx.send(trade).await.is_err() {
                        tracing::info!("Trade tick channel closed, exiting");
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

        // Flush any remaining CVD data
        if let Some(ref mut aggregator) = cvd_aggregator {
            if let Some(cvd_agg) = aggregator.flush() {
                if let Some(ref cvd_tx) = self.cvd_tx {
                    let _ = cvd_tx.send(cvd_agg).await;
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
        format!("tradeticks:{}", self.config.symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // =========================================================================
    // AggTradeEvent Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_agg_trade_buy_aggressor() {
        // buyer_is_maker = false means buyer was taker -> BUY
        let json = r#"{
            "e": "aggTrade",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "a": 123456789,
            "p": "42750.50",
            "q": "0.150",
            "f": 100,
            "l": 102,
            "T": 1699999999998,
            "m": false
        }"#;

        let event: AggTradeEvent = serde_json::from_str(json).expect("parse failed");

        assert_eq!(event.event_type, "aggTrade");
        assert_eq!(event.symbol, "BTCUSDT");
        assert_eq!(event.agg_trade_id, 123456789);
        assert_eq!(event.price, "42750.50");
        assert_eq!(event.quantity, "0.150");
        assert!(!event.buyer_is_maker);
        assert_eq!(event.trade_side(), TradeSide::Buy);
    }

    #[test]
    fn test_parse_agg_trade_sell_aggressor() {
        // buyer_is_maker = true means buyer was maker -> SELL (seller aggressor)
        let json = r#"{
            "e": "aggTrade",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "a": 123456790,
            "p": "42749.00",
            "q": "1.500",
            "f": 103,
            "l": 105,
            "T": 1699999999999,
            "m": true
        }"#;

        let event: AggTradeEvent = serde_json::from_str(json).expect("parse failed");

        assert!(event.buyer_is_maker);
        assert_eq!(event.trade_side(), TradeSide::Sell);
    }

    #[test]
    fn test_agg_trade_to_trade_tick() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "a": 123456789,
            "p": "42750.50",
            "q": "0.150",
            "f": 100,
            "l": 102,
            "T": 1699999999998,
            "m": false
        }"#;

        let event: AggTradeEvent = serde_json::from_str(json).unwrap();
        let trade = event.to_trade_tick().expect("conversion failed");

        assert_eq!(trade.symbol, "BTCUSDT");
        assert_eq!(trade.exchange, "binance");
        assert_eq!(trade.trade_id, 123456789);
        assert_eq!(trade.price, dec!(42750.50));
        assert_eq!(trade.quantity, dec!(0.150));
        assert!(trade.is_buy());
        // USD value = 42750.50 * 0.150 = 6412.575
        assert_eq!(trade.usd_value, dec!(6412.575));
    }

    #[test]
    fn test_agg_trade_invalid_price() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "a": 123456789,
            "p": "invalid",
            "q": "0.150",
            "f": 100,
            "l": 102,
            "T": 1699999999998,
            "m": false
        }"#;

        let event: AggTradeEvent = serde_json::from_str(json).unwrap();
        let trade = event.to_trade_tick();

        assert!(trade.is_none());
    }

    #[test]
    fn test_parse_stream_wrapper() {
        let json = r#"{
            "stream": "btcusdt@aggTrade",
            "data": {
                "e": "aggTrade",
                "E": 1699999999999,
                "s": "BTCUSDT",
                "a": 123456789,
                "p": "42750.50",
                "q": "0.150",
                "f": 100,
                "l": 102,
                "T": 1699999999998,
                "m": false
            }
        }"#;

        let wrapper: AggTradeStreamWrapper = serde_json::from_str(json).unwrap();

        assert_eq!(wrapper.stream, "btcusdt@aggTrade");
        assert_eq!(wrapper.data.symbol, "BTCUSDT");
    }

    // =========================================================================
    // CvdAggregator Tests
    // =========================================================================

    fn make_trade_at(timestamp_ms: i64, quantity: Decimal, side: TradeSide) -> TradeTickRecord {
        let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap();
        TradeTickRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            timestamp_ms, // Use timestamp as trade_id for uniqueness
            dec!(50000),
            quantity,
            side,
        )
    }

    #[test]
    fn test_aggregator_first_trade_no_emit() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // First trade should not emit (no previous window to complete)
        let trade = make_trade_at(1700000000000, dec!(1.0), TradeSide::Buy);
        let result = agg.process(trade);

        assert!(result.is_none());
        assert_eq!(agg.current_window_trade_count(), 1);
    }

    #[test]
    fn test_aggregator_same_window_no_emit() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // Both trades in same 60-second window
        let trade1 = make_trade_at(1700000000000, dec!(1.0), TradeSide::Buy);
        let trade2 = make_trade_at(1700000030000, dec!(0.5), TradeSide::Sell); // 30s later

        let result1 = agg.process(trade1);
        let result2 = agg.process(trade2);

        assert!(result1.is_none());
        assert!(result2.is_none());
        assert_eq!(agg.current_window_trade_count(), 2);
    }

    #[test]
    fn test_aggregator_emits_at_window_boundary() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // Trade in first window (second 0)
        let trade1 = make_trade_at(1700000000000, dec!(1.0), TradeSide::Buy);
        let _ = agg.process(trade1);

        // Trade in second window (second 60) - should emit aggregate for first window
        let trade2 = make_trade_at(1700000060000, dec!(0.5), TradeSide::Sell);
        let result = agg.process(trade2);

        assert!(result.is_some());
        let cvd_agg = result.unwrap();

        assert_eq!(cvd_agg.symbol, "BTCUSDT");
        assert_eq!(cvd_agg.exchange, "binance");
        assert_eq!(cvd_agg.window_seconds, 60);
        assert_eq!(cvd_agg.buy_volume, dec!(1.0));
        assert_eq!(cvd_agg.sell_volume, Decimal::ZERO);
        assert_eq!(cvd_agg.cvd, dec!(1.0));
        assert_eq!(cvd_agg.trade_count, 1);
    }

    #[test]
    fn test_aggregator_multiple_window_boundaries() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // Window 1: Buy 2.0
        let trade1 = make_trade_at(1700000000000, dec!(2.0), TradeSide::Buy);
        let _ = agg.process(trade1);

        // Window 2: Sell 1.0 (emits window 1)
        let trade2 = make_trade_at(1700000060000, dec!(1.0), TradeSide::Sell);
        let result2 = agg.process(trade2);

        assert!(result2.is_some());
        let agg1 = result2.unwrap();
        assert_eq!(agg1.cvd, dec!(2.0)); // Window 1 CVD

        // Window 3: Buy 0.5 (emits window 2)
        let trade3 = make_trade_at(1700000120000, dec!(0.5), TradeSide::Buy);
        let result3 = agg.process(trade3);

        assert!(result3.is_some());
        let agg2 = result3.unwrap();
        assert_eq!(agg2.cvd, dec!(-1.0)); // Window 2 CVD (sell)
    }

    #[test]
    fn test_aggregator_flush_partial_window() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // Add trades to current window
        let trade1 = make_trade_at(1700000000000, dec!(1.5), TradeSide::Buy);
        let trade2 = make_trade_at(1700000030000, dec!(0.5), TradeSide::Sell);
        let _ = agg.process(trade1);
        let _ = agg.process(trade2);

        // Flush without crossing window boundary
        let result = agg.flush();

        assert!(result.is_some());
        let cvd_agg = result.unwrap();
        assert_eq!(cvd_agg.buy_volume, dec!(1.5));
        assert_eq!(cvd_agg.sell_volume, dec!(0.5));
        assert_eq!(cvd_agg.cvd, dec!(1.0));
        assert_eq!(cvd_agg.trade_count, 2);
    }

    #[test]
    fn test_aggregator_flush_empty_window() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // No trades processed, flush should return None
        let result = agg.flush();

        assert!(result.is_none());
    }

    #[test]
    fn test_aggregator_cvd_calculation() {
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 60);

        // Use a base timestamp that aligns to window boundary
        // 1700000040000 / 60000 = 28333334 -> window starts at 1700000040000
        let base = 1700000040000_i64;

        // Window 1: 3 buys, 2 sells (all within 60 seconds of base)
        let trades = vec![
            make_trade_at(base, dec!(1.0), TradeSide::Buy),
            make_trade_at(base + 10000, dec!(0.5), TradeSide::Sell),
            make_trade_at(base + 20000, dec!(2.0), TradeSide::Buy),
            make_trade_at(base + 30000, dec!(1.5), TradeSide::Sell),
            make_trade_at(base + 50000, dec!(0.5), TradeSide::Buy),
        ];

        for trade in trades {
            let _ = agg.process(trade);
        }

        // Cross window boundary to get aggregate (next window starts at base + 60000)
        let trigger = make_trade_at(base + 60000, dec!(0.1), TradeSide::Buy);
        let result = agg.process(trigger);

        let cvd_agg = result.unwrap();
        // Buy: 1.0 + 2.0 + 0.5 = 3.5
        // Sell: 0.5 + 1.5 = 2.0
        // CVD: 3.5 - 2.0 = 1.5
        assert_eq!(cvd_agg.buy_volume, dec!(3.5));
        assert_eq!(cvd_agg.sell_volume, dec!(2.0));
        assert_eq!(cvd_agg.cvd, dec!(1.5));
        assert_eq!(cvd_agg.trade_count, 5);
    }

    // =========================================================================
    // TradeTickCollector Tests
    // =========================================================================

    #[test]
    fn test_collector_build_ws_url() {
        let config = CollectorConfig::new("btcusdt");
        let (tx, _rx) = mpsc::channel(1);
        let collector = TradeTickCollector::new(config, tx);

        let url = collector.build_ws_url();
        assert!(url.contains("btcusdt@aggTrade"));
        assert!(url.contains("stream?streams="));
    }

    #[test]
    fn test_collector_stats_default() {
        let config = CollectorConfig::new("ethusdt");
        let (tx, _rx) = mpsc::channel(1);
        let collector = TradeTickCollector::new(config, tx);

        let stats = collector.stats();
        assert_eq!(stats.records_collected, 0);
        assert_eq!(stats.errors_encountered, 0);
    }

    #[test]
    fn test_collector_with_cvd_channel() {
        let config = CollectorConfig::new("btcusdt");
        let (tx, _rx) = mpsc::channel::<TradeTickRecord>(1);
        let (cvd_tx, _cvd_rx) = mpsc::channel::<CvdAggregateRecord>(1);

        let collector = TradeTickCollector::new(config, tx).with_cvd_channel(cvd_tx, 60);

        // Verify CVD is configured
        assert!(collector.cvd_tx.is_some());
        assert_eq!(collector.cvd_window_seconds, Some(60));
    }

    #[test]
    fn test_collector_source_name() {
        let config = CollectorConfig::new("solusdt");
        let (tx, _rx) = mpsc::channel(1);
        let collector = TradeTickCollector::new(config, tx);

        assert_eq!(collector.source_name(), "tradeticks:solusdt");
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_agg_trade_large_values() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1699999999999,
            "s": "BTCUSDT",
            "a": 9223372036854775807,
            "p": "100000.12345678",
            "q": "999.99999999",
            "f": 100,
            "l": 102,
            "T": 1699999999998,
            "m": false
        }"#;

        let event: AggTradeEvent = serde_json::from_str(json).unwrap();
        let trade = event.to_trade_tick().unwrap();

        assert_eq!(trade.trade_id, i64::MAX);
        assert_eq!(trade.price, dec!(100000.12345678));
        assert_eq!(trade.quantity, dec!(999.99999999));
    }

    #[test]
    fn test_aggregator_different_window_sizes() {
        // Test 5-second windows
        let mut agg = CvdAggregator::new("BTCUSDT".to_string(), "binance".to_string(), 5);

        let trade1 = make_trade_at(1700000000000, dec!(1.0), TradeSide::Buy);
        let _ = agg.process(trade1);

        // 5 seconds later = new window
        let trade2 = make_trade_at(1700000005000, dec!(0.5), TradeSide::Sell);
        let result = agg.process(trade2);

        assert!(result.is_some());
        assert_eq!(result.unwrap().window_seconds, 5);
    }
}
