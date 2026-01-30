//! Liquidation data collector for Binance Futures.
//!
//! Collects liquidation events and maintains rolling window aggregates
//! for cascade detection signals.

use crate::collector::types::{CollectorEvent, CollectorStats};
use crate::common::BINANCE_LIQUIDATION_WS;
use algo_trade_data::{LiquidationAggregateRecord, LiquidationRecord, LiquidationSide};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::VecDeque;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Binance liquidation event (forceOrder).
#[derive(Debug, Deserialize)]
pub struct ForceOrderEvent {
    /// Event type
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Order data
    #[serde(rename = "o")]
    pub order: ForceOrder,
}

/// Liquidation order details.
#[derive(Debug, Deserialize)]
pub struct ForceOrder {
    /// Symbol
    #[serde(rename = "s")]
    pub symbol: String,
    /// Side (SELL = long liquidation, BUY = short liquidation)
    #[serde(rename = "S")]
    pub side: String,
    /// Order type
    #[serde(rename = "o")]
    pub order_type: String,
    /// Time in force
    #[serde(rename = "f")]
    pub time_in_force: String,
    /// Original quantity
    #[serde(rename = "q")]
    pub original_quantity: String,
    /// Price
    #[serde(rename = "p")]
    pub price: String,
    /// Average price
    #[serde(rename = "ap")]
    pub average_price: String,
    /// Order status
    #[serde(rename = "X")]
    pub order_status: String,
    /// Last filled quantity
    #[serde(rename = "l")]
    pub last_filled_quantity: String,
    /// Filled accumulated quantity
    #[serde(rename = "z")]
    pub filled_accumulated_quantity: String,
    /// Trade time
    #[serde(rename = "T")]
    pub trade_time: i64,
}

impl ForceOrder {
    /// Returns the liquidation side.
    /// SELL means a long position was liquidated.
    /// BUY means a short position was liquidated.
    pub fn liquidation_side(&self) -> LiquidationSide {
        if self.side == "SELL" {
            LiquidationSide::Long
        } else {
            LiquidationSide::Short
        }
    }

    /// Calculates USD value of the liquidation.
    pub fn usd_value(&self) -> Option<Decimal> {
        let qty = Decimal::from_str(&self.filled_accumulated_quantity).ok()?;
        let price = Decimal::from_str(&self.price).ok()?;
        Some(qty * price)
    }
}

/// Rolling windows for liquidation aggregation.
#[derive(Debug)]
pub struct RollingWindows {
    /// 5-minute window liquidations
    window_5m: VecDeque<LiquidationRecord>,
    /// 1-hour window liquidations
    window_1h: VecDeque<LiquidationRecord>,
    /// 4-hour window liquidations
    window_4h: VecDeque<LiquidationRecord>,
    /// 24-hour window liquidations
    window_24h: VecDeque<LiquidationRecord>,
}

impl Default for RollingWindows {
    fn default() -> Self {
        Self::new()
    }
}

impl RollingWindows {
    /// Creates new empty rolling windows.
    pub fn new() -> Self {
        Self {
            window_5m: VecDeque::new(),
            window_1h: VecDeque::new(),
            window_4h: VecDeque::new(),
            window_24h: VecDeque::new(),
        }
    }

    /// Adds a liquidation and prunes expired entries.
    pub fn add(&mut self, liq: LiquidationRecord) {
        let now = liq.timestamp;

        // Add to all windows
        self.window_5m.push_back(liq.clone());
        self.window_1h.push_back(liq.clone());
        self.window_4h.push_back(liq.clone());
        self.window_24h.push_back(liq);

        // Prune expired entries
        self.prune(now);
    }

    /// Prunes entries older than window duration.
    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff_5m = now - ChronoDuration::minutes(5);
        let cutoff_1h = now - ChronoDuration::hours(1);
        let cutoff_4h = now - ChronoDuration::hours(4);
        let cutoff_24h = now - ChronoDuration::hours(24);

        while self
            .window_5m
            .front()
            .map(|l| l.timestamp < cutoff_5m)
            .unwrap_or(false)
        {
            self.window_5m.pop_front();
        }

        while self
            .window_1h
            .front()
            .map(|l| l.timestamp < cutoff_1h)
            .unwrap_or(false)
        {
            self.window_1h.pop_front();
        }

        while self
            .window_4h
            .front()
            .map(|l| l.timestamp < cutoff_4h)
            .unwrap_or(false)
        {
            self.window_4h.pop_front();
        }

        while self
            .window_24h
            .front()
            .map(|l| l.timestamp < cutoff_24h)
            .unwrap_or(false)
        {
            self.window_24h.pop_front();
        }
    }

    /// Gets the 5-minute aggregate.
    pub fn aggregate_5m(&self, symbol: &str, exchange: &str) -> LiquidationAggregateRecord {
        Self::create_aggregate(&self.window_5m, symbol, exchange, 5)
    }

    /// Gets the 1-hour aggregate.
    pub fn aggregate_1h(&self, symbol: &str, exchange: &str) -> LiquidationAggregateRecord {
        Self::create_aggregate(&self.window_1h, symbol, exchange, 60)
    }

    /// Gets the 4-hour aggregate.
    pub fn aggregate_4h(&self, symbol: &str, exchange: &str) -> LiquidationAggregateRecord {
        Self::create_aggregate(&self.window_4h, symbol, exchange, 240)
    }

    /// Gets the 24-hour aggregate.
    pub fn aggregate_24h(&self, symbol: &str, exchange: &str) -> LiquidationAggregateRecord {
        Self::create_aggregate(&self.window_24h, symbol, exchange, 1440)
    }

    /// Creates an aggregate from a window.
    fn create_aggregate(
        window: &VecDeque<LiquidationRecord>,
        symbol: &str,
        exchange: &str,
        window_minutes: i32,
    ) -> LiquidationAggregateRecord {
        let liqs: Vec<LiquidationRecord> = window.iter().cloned().collect();
        LiquidationAggregateRecord::from_liquidations(
            Utc::now(),
            symbol.to_string(),
            exchange.to_string(),
            window_minutes,
            &liqs,
        )
    }

    /// Returns the count of liquidations in each window.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.window_5m.len(),
            self.window_1h.len(),
            self.window_4h.len(),
            self.window_24h.len(),
        )
    }

    /// Calculates net delta (long - short volume) for 1h window.
    pub fn net_delta_1h(&self) -> Decimal {
        let agg = self.aggregate_1h("", "");
        agg.net_delta
    }
}

/// Configuration for liquidation collector.
#[derive(Debug, Clone)]
pub struct LiquidationCollectorConfig {
    /// Minimum USD value to record individual liquidations
    pub min_usd_threshold: Decimal,
    /// Symbol filter (empty = all symbols)
    pub symbol_filter: Option<String>,
    /// Exchange name
    pub exchange: String,
    /// Reconnect delay
    pub reconnect_delay: Duration,
    /// Max reconnect attempts (0 = unlimited)
    pub max_reconnect_attempts: u32,
    /// Interval for emitting aggregates
    pub aggregate_interval: Duration,
}

impl Default for LiquidationCollectorConfig {
    fn default() -> Self {
        Self {
            min_usd_threshold: Decimal::from(3000),
            symbol_filter: None,
            exchange: "binance".to_string(),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
            aggregate_interval: Duration::from_secs(60),
        }
    }
}

impl LiquidationCollectorConfig {
    /// Creates a config for a specific symbol with default thresholds.
    pub fn for_symbol(symbol: impl Into<String>) -> Self {
        Self {
            symbol_filter: Some(symbol.into().to_uppercase()),
            ..Default::default()
        }
    }

    /// Sets the minimum USD threshold.
    #[must_use]
    pub fn with_min_usd_threshold(mut self, threshold: Decimal) -> Self {
        self.min_usd_threshold = threshold;
        self
    }

    /// Sets the aggregate emission interval.
    #[must_use]
    pub fn with_aggregate_interval(mut self, interval: Duration) -> Self {
        self.aggregate_interval = interval;
        self
    }
}

/// Liquidation data collector.
///
/// Collects individual liquidation events and emits rolling aggregates.
pub struct LiquidationCollector {
    /// Configuration
    config: LiquidationCollectorConfig,
    /// Channel for individual liquidations
    tx: mpsc::Sender<LiquidationRecord>,
    /// Channel for aggregates
    agg_tx: Option<mpsc::Sender<LiquidationAggregateRecord>>,
    /// Event channel for monitoring
    event_tx: Option<mpsc::Sender<CollectorEvent>>,
    /// Rolling windows
    windows: RollingWindows,
    /// Statistics
    stats: CollectorStats,
}

impl LiquidationCollector {
    /// Creates a new liquidation collector.
    pub fn new(config: LiquidationCollectorConfig, tx: mpsc::Sender<LiquidationRecord>) -> Self {
        Self {
            config,
            tx,
            agg_tx: None,
            event_tx: None,
            windows: RollingWindows::new(),
            stats: CollectorStats::default(),
        }
    }

    /// Sets the aggregate channel.
    #[must_use]
    pub fn with_aggregate_channel(mut self, tx: mpsc::Sender<LiquidationAggregateRecord>) -> Self {
        self.agg_tx = Some(tx);
        self
    }

    /// Sets the event channel for monitoring.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<CollectorEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Returns a reference to the rolling windows.
    pub fn windows(&self) -> &RollingWindows {
        &self.windows
    }

    /// Returns a reference to the current statistics.
    pub fn stats(&self) -> &CollectorStats {
        &self.stats
    }

    /// Checks if a liquidation passes the symbol filter.
    fn passes_filter(&self, symbol: &str) -> bool {
        match &self.config.symbol_filter {
            Some(filter) => symbol.to_uppercase().contains(filter),
            None => true,
        }
    }

    /// Runs the collector with reconnection handling.
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
                    tracing::info!("Liquidation collector exiting cleanly");
                    break;
                }
                Err(e) => {
                    self.stats.error_occurred();
                    tracing::error!("Liquidation stream error: {}", e);

                    self.emit_event(CollectorEvent::Error {
                        source: self.source_name(),
                        error: e.to_string(),
                    })
                    .await;

                    reconnect_attempts += 1;

                    if self.config.max_reconnect_attempts > 0
                        && reconnect_attempts >= self.config.max_reconnect_attempts
                    {
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
        tracing::info!(
            "Connecting to liquidation stream: {}",
            BINANCE_LIQUIDATION_WS
        );

        let ws_stream = crate::common::connect_websocket(BINANCE_LIQUIDATION_WS)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        self.emit_event(CollectorEvent::Connected {
            source: self.source_name(),
        })
        .await;

        let mut stream = ws_stream;
        let mut last_heartbeat = Instant::now();
        let mut last_aggregate = Instant::now();

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<ForceOrderEvent>(&text) {
                    // Check symbol filter
                    if !self.passes_filter(&event.order.symbol) {
                        continue;
                    }

                    // Calculate USD value
                    let usd_value = match event.order.usd_value() {
                        Some(v) => v,
                        None => continue,
                    };

                    // Create liquidation record
                    let timestamp = DateTime::from_timestamp_millis(event.order.trade_time)
                        .unwrap_or_else(Utc::now);

                    let quantity = Decimal::from_str(&event.order.filled_accumulated_quantity)
                        .unwrap_or(Decimal::ZERO);
                    let price = Decimal::from_str(&event.order.price).unwrap_or(Decimal::ZERO);

                    let record = LiquidationRecord::new(
                        timestamp,
                        event.order.symbol.clone(),
                        self.config.exchange.clone(),
                        event.order.liquidation_side(),
                        quantity,
                        price,
                    );

                    // Always add to windows for aggregation
                    self.windows.add(record.clone());

                    // Only send individual records above threshold
                    if usd_value >= self.config.min_usd_threshold {
                        if self.tx.send(record).await.is_err() {
                            tracing::info!("Liquidation channel closed");
                            break;
                        }
                        self.stats.record_collected();
                    }

                    // Emit aggregates periodically
                    if last_aggregate.elapsed() > self.config.aggregate_interval {
                        self.emit_aggregates().await;
                        last_aggregate = Instant::now();
                    }
                }

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

    /// Emits current aggregates.
    async fn emit_aggregates(&self) {
        if let Some(ref tx) = self.agg_tx {
            let symbol = self.config.symbol_filter.as_deref().unwrap_or("ALL");
            let exchange = &self.config.exchange;

            // Emit all window aggregates
            let _ = tx.send(self.windows.aggregate_5m(symbol, exchange)).await;
            let _ = tx.send(self.windows.aggregate_1h(symbol, exchange)).await;
            let _ = tx.send(self.windows.aggregate_4h(symbol, exchange)).await;
            let _ = tx.send(self.windows.aggregate_24h(symbol, exchange)).await;
        }
    }

    /// Helper to emit events.
    async fn emit_event(&self, event: CollectorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    /// Returns the source name for logging/events.
    fn source_name(&self) -> String {
        match &self.config.symbol_filter {
            Some(s) => format!("liquidation:{}", s.to_lowercase()),
            None => "liquidation:all".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // ========== Helper Functions ==========

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    fn sample_liquidation(
        timestamp: DateTime<Utc>,
        side: LiquidationSide,
        usd_value: Decimal,
    ) -> LiquidationRecord {
        let price = dec!(50000);
        let quantity = usd_value / price;
        LiquidationRecord::new(
            timestamp,
            "BTCUSDT".to_string(),
            "binance".to_string(),
            side,
            quantity,
            price,
        )
    }

    // ========== ForceOrder Tests ==========

    #[test]
    fn test_parse_force_order_event() {
        let json = r#"{
            "e": "forceOrder",
            "E": 1699999999999,
            "o": {
                "s": "BTCUSDT",
                "S": "SELL",
                "o": "LIMIT",
                "f": "IOC",
                "q": "0.500",
                "p": "42750.00",
                "ap": "42749.00",
                "X": "FILLED",
                "l": "0.500",
                "z": "0.500",
                "T": 1699999999998
            }
        }"#;

        let event: ForceOrderEvent = serde_json::from_str(json).unwrap();

        assert_eq!(event.event_type, "forceOrder");
        assert_eq!(event.order.symbol, "BTCUSDT");
        assert_eq!(event.order.side, "SELL");
        assert_eq!(event.order.price, "42750.00");
        assert_eq!(event.order.filled_accumulated_quantity, "0.500");
    }

    #[test]
    fn test_force_order_liquidation_side_long() {
        let order = ForceOrder {
            symbol: "BTCUSDT".to_string(),
            side: "SELL".to_string(),
            order_type: "LIMIT".to_string(),
            time_in_force: "IOC".to_string(),
            original_quantity: "1.0".to_string(),
            price: "50000".to_string(),
            average_price: "49999".to_string(),
            order_status: "FILLED".to_string(),
            last_filled_quantity: "1.0".to_string(),
            filled_accumulated_quantity: "1.0".to_string(),
            trade_time: 1700000000000,
        };

        // SELL side = long liquidation
        assert_eq!(order.liquidation_side(), LiquidationSide::Long);
    }

    #[test]
    fn test_force_order_liquidation_side_short() {
        let order = ForceOrder {
            symbol: "BTCUSDT".to_string(),
            side: "BUY".to_string(),
            order_type: "LIMIT".to_string(),
            time_in_force: "IOC".to_string(),
            original_quantity: "1.0".to_string(),
            price: "50000".to_string(),
            average_price: "50001".to_string(),
            order_status: "FILLED".to_string(),
            last_filled_quantity: "1.0".to_string(),
            filled_accumulated_quantity: "1.0".to_string(),
            trade_time: 1700000000000,
        };

        // BUY side = short liquidation
        assert_eq!(order.liquidation_side(), LiquidationSide::Short);
    }

    #[test]
    fn test_force_order_usd_value() {
        let order = ForceOrder {
            symbol: "BTCUSDT".to_string(),
            side: "SELL".to_string(),
            order_type: "LIMIT".to_string(),
            time_in_force: "IOC".to_string(),
            original_quantity: "1.5".to_string(),
            price: "50000".to_string(),
            average_price: "50000".to_string(),
            order_status: "FILLED".to_string(),
            last_filled_quantity: "1.5".to_string(),
            filled_accumulated_quantity: "1.5".to_string(),
            trade_time: 1700000000000,
        };

        let usd = order.usd_value().unwrap();
        assert_eq!(usd, dec!(75000));
    }

    // ========== Rolling Windows Tests ==========

    #[test]
    fn test_rolling_windows_new() {
        let windows = RollingWindows::new();
        assert_eq!(windows.counts(), (0, 0, 0, 0));
    }

    #[test]
    fn test_rolling_windows_add() {
        let mut windows = RollingWindows::new();

        let liq = sample_liquidation(sample_timestamp(), LiquidationSide::Long, dec!(10000));
        windows.add(liq);

        assert_eq!(windows.counts(), (1, 1, 1, 1));
    }

    #[test]
    fn test_rolling_windows_prune_5m() {
        let mut windows = RollingWindows::new();

        // Add old liquidation (6 minutes ago)
        let old_time = Utc::now() - ChronoDuration::minutes(6);
        let old_liq = sample_liquidation(old_time, LiquidationSide::Long, dec!(10000));
        windows.add(old_liq);

        // Add new liquidation
        let new_liq = sample_liquidation(Utc::now(), LiquidationSide::Short, dec!(5000));
        windows.add(new_liq);

        // 5m window should only have new one, others have both
        let (w5m, w1h, w4h, w24h) = windows.counts();
        assert_eq!(w5m, 1); // Old one pruned
        assert_eq!(w1h, 2);
        assert_eq!(w4h, 2);
        assert_eq!(w24h, 2);
    }

    #[test]
    fn test_rolling_windows_prune_1h() {
        let mut windows = RollingWindows::new();

        // Add old liquidation (61 minutes ago)
        let old_time = Utc::now() - ChronoDuration::minutes(61);
        let old_liq = sample_liquidation(old_time, LiquidationSide::Long, dec!(10000));
        windows.add(old_liq);

        // Add new liquidation
        let new_liq = sample_liquidation(Utc::now(), LiquidationSide::Short, dec!(5000));
        windows.add(new_liq);

        let (w5m, w1h, w4h, w24h) = windows.counts();
        assert_eq!(w5m, 1);
        assert_eq!(w1h, 1); // Old one pruned
        assert_eq!(w4h, 2);
        assert_eq!(w24h, 2);
    }

    #[test]
    fn test_rolling_windows_aggregate_5m() {
        let mut windows = RollingWindows::new();

        // Add long liquidation
        let liq1 = sample_liquidation(Utc::now(), LiquidationSide::Long, dec!(50000));
        windows.add(liq1);

        // Add short liquidation
        let liq2 = sample_liquidation(Utc::now(), LiquidationSide::Short, dec!(30000));
        windows.add(liq2);

        let agg = windows.aggregate_5m("BTCUSDT", "binance");

        assert_eq!(agg.long_volume, dec!(50000));
        assert_eq!(agg.short_volume, dec!(30000));
        assert_eq!(agg.net_delta, dec!(20000));
        assert_eq!(agg.count_long, 1);
        assert_eq!(agg.count_short, 1);
        assert_eq!(agg.window_minutes, 5);
    }

    #[test]
    fn test_rolling_windows_net_delta_1h() {
        let mut windows = RollingWindows::new();

        // More longs than shorts
        for _ in 0..5 {
            windows.add(sample_liquidation(
                Utc::now(),
                LiquidationSide::Long,
                dec!(10000),
            ));
        }
        for _ in 0..2 {
            windows.add(sample_liquidation(
                Utc::now(),
                LiquidationSide::Short,
                dec!(10000),
            ));
        }

        let net_delta = windows.net_delta_1h();
        // long = 50000, short = 20000, delta = 30000
        assert_eq!(net_delta, dec!(30000));
    }

    #[test]
    fn test_rolling_windows_empty_aggregate() {
        let windows = RollingWindows::new();
        let agg = windows.aggregate_1h("BTCUSDT", "binance");

        assert_eq!(agg.long_volume, Decimal::ZERO);
        assert_eq!(agg.short_volume, Decimal::ZERO);
        assert_eq!(agg.net_delta, Decimal::ZERO);
        assert_eq!(agg.count_long, 0);
        assert_eq!(agg.count_short, 0);
    }

    // ========== Liquidation Collector Config Tests ==========

    #[test]
    fn test_config_default() {
        let config = LiquidationCollectorConfig::default();

        assert_eq!(config.min_usd_threshold, dec!(3000));
        assert!(config.symbol_filter.is_none());
        assert_eq!(config.exchange, "binance");
    }

    #[test]
    fn test_config_for_symbol() {
        let config = LiquidationCollectorConfig::for_symbol("btcusdt");

        assert_eq!(config.symbol_filter, Some("BTCUSDT".to_string()));
    }

    #[test]
    fn test_config_builder() {
        let config = LiquidationCollectorConfig::default()
            .with_min_usd_threshold(dec!(10000))
            .with_aggregate_interval(Duration::from_secs(30));

        assert_eq!(config.min_usd_threshold, dec!(10000));
        assert_eq!(config.aggregate_interval, Duration::from_secs(30));
    }

    // ========== Liquidation Collector Tests ==========

    #[test]
    fn test_collector_creation() {
        let (tx, _rx) = mpsc::channel(100);
        let config = LiquidationCollectorConfig::default();
        let collector = LiquidationCollector::new(config, tx);

        assert_eq!(collector.stats().records_collected, 0);
    }

    #[test]
    fn test_collector_with_channels() {
        let (tx, _rx) = mpsc::channel(100);
        let (agg_tx, _agg_rx) = mpsc::channel(100);
        let (event_tx, _event_rx) = mpsc::channel(100);

        let config = LiquidationCollectorConfig::default();
        let collector = LiquidationCollector::new(config, tx)
            .with_aggregate_channel(agg_tx)
            .with_event_channel(event_tx);

        assert!(collector.agg_tx.is_some());
        assert!(collector.event_tx.is_some());
    }

    #[test]
    fn test_collector_passes_filter_all() {
        let (tx, _rx) = mpsc::channel(100);
        let config = LiquidationCollectorConfig::default(); // No filter
        let collector = LiquidationCollector::new(config, tx);

        assert!(collector.passes_filter("BTCUSDT"));
        assert!(collector.passes_filter("ETHUSDT"));
        assert!(collector.passes_filter("anything"));
    }

    #[test]
    fn test_collector_passes_filter_specific() {
        let (tx, _rx) = mpsc::channel(100);
        let config = LiquidationCollectorConfig::for_symbol("btc");
        let collector = LiquidationCollector::new(config, tx);

        assert!(collector.passes_filter("BTCUSDT"));
        assert!(collector.passes_filter("btcusdt"));
        assert!(!collector.passes_filter("ETHUSDT"));
    }

    #[test]
    fn test_collector_source_name_all() {
        let (tx, _rx) = mpsc::channel(100);
        let config = LiquidationCollectorConfig::default();
        let collector = LiquidationCollector::new(config, tx);

        assert_eq!(collector.source_name(), "liquidation:all");
    }

    #[test]
    fn test_collector_source_name_filtered() {
        let (tx, _rx) = mpsc::channel(100);
        let config = LiquidationCollectorConfig::for_symbol("BTCUSDT");
        let collector = LiquidationCollector::new(config, tx);

        assert_eq!(collector.source_name(), "liquidation:btcusdt");
    }

    // ========== LiquidationRecord Integration Tests ==========

    #[test]
    fn test_liquidation_record_creation() {
        let record = LiquidationRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            LiquidationSide::Long,
            dec!(1.5),
            dec!(50000),
        );

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.side, "long");
        assert_eq!(record.usd_value, dec!(75000));
        assert!(record.is_long());
        assert!(!record.is_short());
    }

    #[test]
    fn test_liquidation_aggregate_from_records() {
        let liquidations = vec![
            sample_liquidation(sample_timestamp(), LiquidationSide::Long, dec!(50000)),
            sample_liquidation(sample_timestamp(), LiquidationSide::Long, dec!(25000)),
            sample_liquidation(sample_timestamp(), LiquidationSide::Short, dec!(15000)),
        ];

        let agg = LiquidationAggregateRecord::from_liquidations(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            5,
            &liquidations,
        );

        assert_eq!(agg.long_volume, dec!(75000));
        assert_eq!(agg.short_volume, dec!(15000));
        assert_eq!(agg.net_delta, dec!(60000));
        assert_eq!(agg.count_long, 2);
        assert_eq!(agg.count_short, 1);
    }

    #[test]
    fn test_aggregate_cascade_detection() {
        let liquidations = vec![
            sample_liquidation(sample_timestamp(), LiquidationSide::Long, dec!(100000)),
            sample_liquidation(sample_timestamp(), LiquidationSide::Long, dec!(50000)),
            sample_liquidation(sample_timestamp(), LiquidationSide::Short, dec!(10000)),
        ];

        let agg = LiquidationAggregateRecord::from_liquidations(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            5,
            &liquidations,
        );

        // long_volume = 150000, short_volume = 10000
        // imbalance = (150000 - 10000) / 160000 = 0.875
        assert!(agg.is_long_cascade(dec!(100000), dec!(0.5)));
        assert!(!agg.is_short_cascade(dec!(50000), dec!(0.5)));
    }
}
