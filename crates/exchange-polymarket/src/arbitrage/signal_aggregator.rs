//! Real-time signal aggregator for directional trading.
//!
//! Spawns Binance WebSocket collectors (order book, trades, funding, liquidations)
//! and maintains per-coin `SignalContext` objects. Every ~5s, runs a weighted
//! `CompositeSignal` combining all 7 signal generators and stores results in a
//! shared map.
//!
//! The directional detection loop reads these signals to blend with its
//! spot-delta confidence.
//!
//! # Error Handling
//!
//! Collector failures are non-fatal. If a collector goes down, the aggregator
//! continues with remaining data. If all collectors fail, the directional
//! detector falls back to delta-only confidence.

use algo_trade_core::{
    LiquidationAggregate, OhlcvCandle, OrderBookSnapshot, PriceLevel, SignalContext,
    SignalGenerator, SignalValue,
};
use algo_trade_data::{FundingRateRecord, LiquidationRecord, OrderBookSnapshotRecord};
use algo_trade_signals::{
    ClobVelocitySignal, CollectorConfig, CompositeSignal, FundingCollector, FundingRateSignal,
    LiquidationCascadeSignal, LiquidationCollector, LiquidationCollectorConfig,
    LiquidationRatioConfig, LiquidationRatioSignal, MomentumExhaustionConfig,
    MomentumExhaustionSignal, NewsSignal, OrderBookCollector, OrderBookImbalanceSignal,
};
use algo_trade_signals::generator::{CvdDivergenceConfig, CvdDivergenceSignal};
use crate::gamma::GammaClient;
use crate::models::Coin;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Shared signal map: coin slug (e.g., "btc") → latest composite signal.
pub type SharedSignalMap = Arc<RwLock<HashMap<String, SignalValue>>>;

/// Per-coin collector state maintained by the aggregator.
struct CoinCollectorState {
    /// Latest Binance order book snapshot.
    orderbook: Option<OrderBookSnapshot>,
    /// Rolling order book imbalance history for z-score.
    imbalance_history: Vec<f64>,
    /// Latest funding rate.
    funding_rate: Option<f64>,
    /// Latest 5-min liquidation aggregate.
    liquidation_aggregate: Option<LiquidationAggregate>,
    /// Latest mid price.
    mid_price: Option<Decimal>,
    /// Rolling CLOB price history for velocity signal: (timestamp, yes_price).
    clob_price_history: Vec<(chrono::DateTime<chrono::Utc>, f64)>,
    /// Recent OHLCV candles for momentum exhaustion signal.
    historical_ohlcv: Vec<OhlcvCandle>,
}

impl CoinCollectorState {
    fn new() -> Self {
        Self {
            orderbook: None,
            imbalance_history: Vec::new(),
            funding_rate: None,
            liquidation_aggregate: None,
            mid_price: None,
            clob_price_history: Vec::new(),
            historical_ohlcv: Vec::new(),
        }
    }

    /// Builds a `SignalContext` from the current collector state.
    fn build_context(&self, coin: &Coin) -> SignalContext {
        let symbol = format!("{}USDT", coin.slug_prefix().to_uppercase());
        let mut ctx = SignalContext::new(Utc::now(), symbol).with_exchange("binance");

        if let Some(ref ob) = self.orderbook {
            ctx = ctx.with_orderbook(ob.clone());
        }
        if let Some(mid) = self.mid_price {
            ctx = ctx.with_mid_price(mid);
        }
        if let Some(rate) = self.funding_rate {
            ctx = ctx.with_funding_rate(rate);
        }
        if !self.imbalance_history.is_empty() {
            ctx = ctx.with_historical_imbalances(self.imbalance_history.clone());
        }
        if let Some(ref agg) = self.liquidation_aggregate {
            ctx = ctx.with_liquidation_aggregates(agg.clone());
        }
        if !self.clob_price_history.is_empty() {
            ctx = ctx.with_clob_price_history(self.clob_price_history.clone());
        }
        if !self.historical_ohlcv.is_empty() {
            ctx = ctx.with_historical_ohlcv(self.historical_ohlcv.clone());
        }

        ctx
    }
}

/// Configuration for the signal aggregator.
#[derive(Debug, Clone)]
pub struct SignalAggregatorConfig {
    /// Interval between composite signal computations.
    pub compute_interval: Duration,
    /// Maximum imbalance history length for z-score.
    pub max_imbalance_history: usize,
}

impl Default for SignalAggregatorConfig {
    fn default() -> Self {
        Self {
            compute_interval: Duration::from_secs(5),
            max_imbalance_history: 200,
        }
    }
}

/// Orchestrates real-time signal collection and composite signal computation.
pub struct SignalAggregator {
    /// Coins to monitor.
    coins: Vec<Coin>,
    /// Shared output map.
    signal_map: SharedSignalMap,
    /// Stop flag.
    stop: Arc<AtomicBool>,
    /// Configuration.
    config: SignalAggregatorConfig,
}

impl SignalAggregator {
    /// Creates a new signal aggregator.
    pub fn new(coins: Vec<Coin>, signal_map: SharedSignalMap, stop: Arc<AtomicBool>) -> Self {
        Self {
            coins,
            signal_map,
            stop,
            config: SignalAggregatorConfig::default(),
        }
    }

    /// Creates a new signal aggregator with custom config.
    pub fn with_config(
        coins: Vec<Coin>,
        signal_map: SharedSignalMap,
        stop: Arc<AtomicBool>,
        config: SignalAggregatorConfig,
    ) -> Self {
        Self {
            coins,
            signal_map,
            stop,
            config,
        }
    }

    /// Builds a composite signal with all 7 generators.
    ///
    /// Uses RequireN(2) combination: at least 2 active signals must agree on a
    /// direction before producing a directional output. This prevents any single
    /// signal (e.g. liquidation cascade at strength ~1.0) from dominating the
    /// composite.
    fn build_composite() -> CompositeSignal {
        CompositeSignal::weighted_average("directional_composite")
            .require_agreement(2)
            .with_generator(Box::new(OrderBookImbalanceSignal::new(0.1, 1.5, 50)))
            .with_generator(Box::new(LiquidationCascadeSignal::new(
                Decimal::new(5000, 0),
                3.0,
                1.2,
                100,
            )))
            .with_generator(Box::new(FundingRateSignal::new(2.0, 1.0, 30)))
            .with_generator(Box::new(CvdDivergenceSignal::new(
                CvdDivergenceConfig::default(),
            )))
            .with_generator(Box::new(MomentumExhaustionSignal::new(
                MomentumExhaustionConfig {
                    big_move_threshold: 0.005, // 0.5% — tuned for 15-min windows (~$500 BTC move)
                    big_move_lookback: 5,      // 5 one-minute candles
                    stall_ratio: 0.3,          // stall if range < 30% of big move
                    stall_lookback: 3,         // check last 3 candles for stall
                    min_candles: 8,            // need 8 minutes of data
                },
                0.8,
            )))
            .with_generator(Box::new(
                NewsSignal::new("directional_news").with_weight(0.7),
            ))
            .with_generator(Box::new(LiquidationRatioSignal::new(
                LiquidationRatioConfig {
                    weight: 0.5,
                    ..LiquidationRatioConfig::default()
                },
            )))
            .with_generator(Box::new(ClobVelocitySignal::new()))
    }

    /// Parses a JSON `[[price, qty], ...]` array into `Vec<PriceLevel>`.
    fn parse_price_levels(json: &serde_json::Value) -> Vec<PriceLevel> {
        json.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|level| {
                        let pair = level.as_array()?;
                        if pair.len() < 2 {
                            return None;
                        }
                        let price_str = pair[0].as_str().or_else(|| {
                            pair[0].as_f64().map(|_| "").and(None)
                        }).unwrap_or_default();
                        let qty_str = pair[1].as_str().or_else(|| {
                            pair[1].as_f64().map(|_| "").and(None)
                        }).unwrap_or_default();

                        let price = if price_str.is_empty() {
                            Decimal::from_str(&pair[0].to_string().replace('"', "")).ok()?
                        } else {
                            Decimal::from_str(price_str).ok()?
                        };
                        let quantity = if qty_str.is_empty() {
                            Decimal::from_str(&pair[1].to_string().replace('"', "")).ok()?
                        } else {
                            Decimal::from_str(qty_str).ok()?
                        };

                        Some(PriceLevel { price, quantity })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs the signal aggregator.
    ///
    /// Spawns Binance collectors for each coin, maintains per-coin context,
    /// and periodically computes composite signals.
    pub async fn run(self) {
        info!(
            coins = ?self.coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
            interval = ?self.config.compute_interval,
            "Starting signal aggregator"
        );

        // Shared per-coin state
        let coin_states: Arc<RwLock<HashMap<String, CoinCollectorState>>> =
            Arc::new(RwLock::new(HashMap::new()));
        {
            let mut states = coin_states.write().await;
            for coin in &self.coins {
                states.insert(coin.slug_prefix().to_string(), CoinCollectorState::new());
            }
        }

        let mut collector_handles = Vec::new();

        // Spawn order book collectors per coin
        for coin in &self.coins {
            let symbol = format!("{}usdt", coin.slug_prefix());
            let (ob_tx, mut ob_rx) =
                tokio::sync::mpsc::channel::<OrderBookSnapshotRecord>(100);
            let ob_config = CollectorConfig {
                symbol: symbol.clone(),
                reconnect_delay: Duration::from_secs(5),
                max_reconnect_attempts: 0,
                exchange: "binance".to_string(),
            };
            let mut ob_collector = OrderBookCollector::new(ob_config, ob_tx);
            let handle = tokio::spawn(async move {
                if let Err(e) = ob_collector.run().await {
                    warn!(error = %e, "Order book collector stopped");
                }
            });
            collector_handles.push(handle);

            // Drainer task for this coin's order book
            let states = Arc::clone(&coin_states);
            let max_history = self.config.max_imbalance_history;
            let slug = coin.slug_prefix().to_string();
            let stop = Arc::clone(&self.stop);
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    match ob_rx.recv().await {
                        Some(record) => {
                            let bids = Self::parse_price_levels(&record.bid_levels);
                            let asks = Self::parse_price_levels(&record.ask_levels);

                            let snapshot = OrderBookSnapshot {
                                bids,
                                asks,
                                timestamp: record.timestamp,
                            };

                            let imbalance = snapshot.calculate_imbalance();

                            let mut states = states.write().await;
                            if let Some(state) = states.get_mut(&slug) {
                                state.mid_price = snapshot.mid_price();
                                state.orderbook = Some(snapshot);
                                state.imbalance_history.push(imbalance);
                                if state.imbalance_history.len() > max_history {
                                    state.imbalance_history.remove(0);
                                }
                            }
                        }
                        None => break,
                    }
                }
            });
        }

        // Spawn funding rate collector (one for all symbols)
        {
            let funding_symbols: Vec<String> = self
                .coins
                .iter()
                .map(|c| format!("{}USDT", c.slug_prefix().to_uppercase()))
                .collect();
            let (funding_tx, mut funding_rx) =
                tokio::sync::mpsc::channel::<FundingRateRecord>(100);
            let mut funding_collector = FundingCollector::new(funding_symbols, funding_tx);
            let handle = tokio::spawn(async move {
                if let Err(e) = funding_collector.run().await {
                    warn!(error = %e, "Funding collector stopped");
                }
            });
            collector_handles.push(handle);

            // Drainer for funding rates
            let states = Arc::clone(&coin_states);
            let stop = Arc::clone(&self.stop);
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    match funding_rx.recv().await {
                        Some(record) => {
                            let symbol_lower = record.symbol.to_lowercase();
                            let mut states = states.write().await;
                            for (slug, state) in states.iter_mut() {
                                if symbol_lower == format!("{}usdt", slug) {
                                    // Convert Decimal funding rate to f64
                                    state.funding_rate = record
                                        .funding_rate
                                        .to_string()
                                        .parse::<f64>()
                                        .ok();
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
            });
        }

        // Spawn liquidation collector
        {
            let (liq_tx, mut liq_rx) =
                tokio::sync::mpsc::channel::<LiquidationRecord>(100);
            let liq_config = LiquidationCollectorConfig {
                min_usd_threshold: Decimal::new(3000, 0),
                symbol_filter: None,
                exchange: "binance".to_string(),
                reconnect_delay: Duration::from_secs(5),
                max_reconnect_attempts: 0,
                aggregate_interval: Duration::from_secs(60),
            };
            let mut liq_collector = LiquidationCollector::new(liq_config, liq_tx);
            let handle = tokio::spawn(async move {
                if let Err(e) = liq_collector.run().await {
                    warn!(error = %e, "Liquidation collector stopped");
                }
            });
            collector_handles.push(handle);

            // Drainer for liquidations
            let states = Arc::clone(&coin_states);
            let stop = Arc::clone(&self.stop);
            let coins = self.coins.clone();
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    match liq_rx.recv().await {
                        Some(record) => {
                            let symbol_lower = record.symbol.to_lowercase();
                            let mut states = states.write().await;
                            for coin in &coins {
                                let slug = coin.slug_prefix();
                                if symbol_lower == format!("{}usdt", slug) {
                                    if let Some(state) = states.get_mut(slug) {
                                        let agg = state
                                            .liquidation_aggregate
                                            .get_or_insert_with(|| LiquidationAggregate {
                                                timestamp: Utc::now(),
                                                window_minutes: 5,
                                                long_volume_usd: Decimal::ZERO,
                                                short_volume_usd: Decimal::ZERO,
                                                net_delta_usd: Decimal::ZERO,
                                                count_long: 0,
                                                count_short: 0,
                                            });
                                        if record.side == "long" {
                                            agg.long_volume_usd += record.usd_value;
                                            agg.count_long += 1;
                                        } else {
                                            agg.short_volume_usd += record.usd_value;
                                            agg.count_short += 1;
                                        }
                                        agg.net_delta_usd =
                                            agg.long_volume_usd - agg.short_volume_usd;
                                        agg.timestamp = Utc::now();
                                    }
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
            });
        }

        // Spawn Polymarket CLOB price poller (for velocity signal)
        {
            let states = Arc::clone(&coin_states);
            let stop = Arc::clone(&self.stop);
            let coins = self.coins.clone();
            let max_clob_history = 120; // ~10 minutes at 5s intervals
            tokio::spawn(async move {
                let gamma = GammaClient::new();
                let mut poll_interval = tokio::time::interval(Duration::from_secs(5));
                while !stop.load(Ordering::Relaxed) {
                    poll_interval.tick().await;
                    let markets = gamma.get_15min_markets_for_coins(&coins).await;
                    let now = Utc::now();
                    let mut states = states.write().await;
                    for market in &markets {
                        if let Some(up_price) = market.up_price() {
                            // Match market to coin by checking the question text
                            for coin in &coins {
                                let slug = coin.slug_prefix();
                                let prefix = slug.to_uppercase();
                                if market.question.contains(&prefix) {
                                    if let Some(state) = states.get_mut(slug) {
                                        let price_f64 = up_price
                                            .to_string()
                                            .parse::<f64>()
                                            .unwrap_or(0.5);
                                        state.clob_price_history.push((now, price_f64));
                                        if state.clob_price_history.len() > max_clob_history {
                                            state.clob_price_history.remove(0);
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }

        // Spawn OHLCV poller — fetches last 20 one-minute candles per coin from
        // Binance Futures REST API every 60 seconds. Feeds momentum_exhaustion signal.
        {
            let states = Arc::clone(&coin_states);
            let stop = Arc::clone(&self.stop);
            let coins = self.coins.clone();
            tokio::spawn(async move {
                use algo_trade_signals::collector::{Interval, OhlcvCollector};
                let collector = OhlcvCollector::new();
                let mut poll_interval = tokio::time::interval(Duration::from_secs(60));
                // Skip the first tick so we don't fire immediately at startup
                poll_interval.tick().await;
                while !stop.load(Ordering::Relaxed) {
                    poll_interval.tick().await;
                    let now = Utc::now();
                    let start = now - chrono::Duration::minutes(20);
                    for coin in &coins {
                        let symbol = format!("{}USDT", coin.slug_prefix().to_uppercase());
                        match collector.fetch(&symbol, Interval::OneMinute, start, now).await {
                            Ok((records, _stats)) => {
                                let candles: Vec<OhlcvCandle> = records
                                    .into_iter()
                                    .map(|r| OhlcvCandle {
                                        timestamp: r.timestamp,
                                        open: r.open,
                                        high: r.high,
                                        low: r.low,
                                        close: r.close,
                                        volume: r.volume,
                                    })
                                    .collect();
                                if !candles.is_empty() {
                                    let mut states = states.write().await;
                                    if let Some(state) =
                                        states.get_mut(coin.slug_prefix())
                                    {
                                        state.historical_ohlcv = candles;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    symbol = %symbol,
                                    error = %e,
                                    "Failed to fetch OHLCV candles"
                                );
                            }
                        }
                    }
                }
            });
        }

        // Build composite signal per coin
        let mut composites: HashMap<String, CompositeSignal> = self
            .coins
            .iter()
            .map(|c| (c.slug_prefix().to_string(), Self::build_composite()))
            .collect();

        // Main compute loop
        let mut interval = tokio::time::interval(self.config.compute_interval);
        loop {
            interval.tick().await;

            if self.stop.load(Ordering::Relaxed) {
                info!("Signal aggregator stopping");
                break;
            }

            let states = coin_states.read().await;
            for coin in &self.coins {
                let slug = coin.slug_prefix();
                if let Some(state) = states.get(slug) {
                    let ctx = state.build_context(coin);

                    if let Some(composite) = composites.get_mut(slug) {
                        match composite.compute(&ctx).await {
                            Ok(signal) => {
                                debug!(
                                    coin = slug,
                                    direction = ?signal.direction,
                                    strength = format!("{:.3}", signal.strength),
                                    "Composite signal computed"
                                );
                                let mut map = self.signal_map.write().await;
                                map.insert(slug.to_string(), signal);
                            }
                            Err(e) => {
                                warn!(coin = slug, error = %e, "Composite signal error");
                            }
                        }
                    }
                }
            }
        }

        // Cleanup
        for handle in collector_handles {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_aggregator_config_default() {
        let config = SignalAggregatorConfig::default();
        assert_eq!(config.compute_interval, Duration::from_secs(5));
        assert_eq!(config.max_imbalance_history, 200);
    }

    #[test]
    fn test_coin_collector_state_new() {
        let state = CoinCollectorState::new();
        assert!(state.orderbook.is_none());
        assert!(state.funding_rate.is_none());
        assert!(state.liquidation_aggregate.is_none());
        assert!(state.mid_price.is_none());
        assert!(state.imbalance_history.is_empty());
        assert!(state.clob_price_history.is_empty());
    }

    #[test]
    fn test_coin_collector_state_build_context_empty() {
        let state = CoinCollectorState::new();
        let ctx = state.build_context(&Coin::Btc);
        assert_eq!(ctx.symbol, "BTCUSDT");
        assert_eq!(ctx.exchange, "binance");
        assert!(ctx.orderbook.is_none());
        assert!(ctx.funding_rate.is_none());
    }

    #[test]
    fn test_coin_collector_state_build_context_with_data() {
        let mut state = CoinCollectorState::new();
        state.funding_rate = Some(0.0001);
        state.mid_price = Some(Decimal::new(2500, 0));
        state.imbalance_history = vec![0.1, 0.2, 0.3];

        let ctx = state.build_context(&Coin::Eth);
        assert_eq!(ctx.symbol, "ETHUSDT");
        assert_eq!(ctx.funding_rate, Some(0.0001));
        assert_eq!(ctx.mid_price, Some(Decimal::new(2500, 0)));
        assert_eq!(ctx.historical_imbalances.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn test_build_composite_has_8_generators() {
        let composite = SignalAggregator::build_composite();
        assert_eq!(composite.generator_count(), 8);
    }

    #[test]
    fn test_parse_price_levels_string_format() {
        let json: serde_json::Value =
            serde_json::json!([["100.50", "10.5"], ["99.00", "20.0"]]);
        let levels = SignalAggregator::parse_price_levels(&json);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].price, Decimal::from_str("100.50").unwrap());
        assert_eq!(levels[0].quantity, Decimal::from_str("10.5").unwrap());
    }

    #[test]
    fn test_parse_price_levels_empty() {
        let json: serde_json::Value = serde_json::json!([]);
        let levels = SignalAggregator::parse_price_levels(&json);
        assert!(levels.is_empty());
    }

    #[test]
    fn test_parse_price_levels_null() {
        let json = serde_json::Value::Null;
        let levels = SignalAggregator::parse_price_levels(&json);
        assert!(levels.is_empty());
    }

    #[test]
    fn test_shared_signal_map_creation() {
        let map: SharedSignalMap = Arc::new(RwLock::new(HashMap::new()));
        assert_eq!(Arc::strong_count(&map), 1);
    }
}
