//! Unified data collection service for Binance market data.
//!
//! Owns all Binance data connections (spot feeds, signal aggregator) and provides
//! shared in-memory state to consumers via [`DataServiceHandle`]. Optionally
//! persists composite signal snapshots to the database for backtesting.
//!
//! # Architecture
//!
//! ```text
//! DataService
//!   ├── SpotPriceFeed per coin → shared Arc<RwLock<SpotPriceTracker>>
//!   ├── SignalAggregator → shared SharedSignalMap
//!   └── SignalSnapshotWriter → signal_snapshots DB table (every 15s)
//!
//! DataServiceHandle (cloneable, given to consumers)
//!   ├── spot_tracker(coin) → Arc<RwLock<SpotPriceTracker>>
//!   └── signal_map() → SharedSignalMap
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::data_service::{DataService, DataServiceConfig};
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicBool;
//!
//! let config = DataServiceConfig::for_coins(vec![Coin::Btc, Coin::Eth]);
//! let stop = Arc::new(AtomicBool::new(false));
//! let service = DataService::new(config, None, stop);
//! let handle = service.handle();
//!
//! tokio::spawn(async move { service.run().await; });
//!
//! // Use handle in consumers
//! if let Some(tracker) = handle.spot_tracker("btc") {
//!     let price = tracker.read().await.current_price();
//! }
//! ```

use crate::arbitrage::latency_detector::SpotPriceTracker;
use crate::arbitrage::raw_data_writer::spawn_raw_data_collectors;
use crate::arbitrage::signal_aggregator::{SharedSignalMap, SignalAggregator, SignalAggregatorConfig};
use crate::arbitrage::spot_feed::{SpotPriceFeed, SpotPriceFeedConfig};
use crate::models::Coin;
use algo_trade_core::{Direction, SignalValue};
use algo_trade_data::models::signal_snapshot::SignalDirection;
use algo_trade_data::models::SignalSnapshotRecord;
use algo_trade_data::repositories::SignalSnapshotRepository;
use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Configuration for the data service.
#[derive(Debug, Clone)]
pub struct DataServiceConfig {
    /// Coins to collect data for.
    pub coins: Vec<Coin>,
    /// Enable the signal aggregator (order book, funding, liquidation collectors).
    pub enable_signals: bool,
    /// Interval between composite signal computations.
    pub signal_compute_interval: Duration,
    /// Enable writing signal snapshots to the database.
    pub enable_signal_persistence: bool,
    /// Interval between signal snapshot DB flushes.
    pub signal_flush_interval: Duration,
    /// Enable writing raw market data (order book, funding, liquidation) to the database.
    ///
    /// When true and a `db_pool` is provided, independent Binance WebSocket collectors
    /// are spawned alongside the signal aggregator's collectors. These write raw records
    /// to the database for backtesting. This duplicates the Binance connections but
    /// avoids modifying the signal aggregator's internal channel ownership.
    pub enable_raw_persistence: bool,
}

impl DataServiceConfig {
    /// Creates a config for the given coins with signals and raw persistence disabled.
    pub fn for_coins(coins: Vec<Coin>) -> Self {
        Self {
            coins,
            enable_signals: false,
            signal_compute_interval: Duration::from_secs(5),
            enable_signal_persistence: false,
            signal_flush_interval: Duration::from_secs(15),
            enable_raw_persistence: false,
        }
    }
}

/// Cloneable handle providing read access to shared data state.
///
/// Consumers (e.g., `DirectionalRunner`) receive this handle instead of
/// managing their own data connections.
#[derive(Clone)]
pub struct DataServiceHandle {
    spot_trackers: HashMap<String, Arc<RwLock<SpotPriceTracker>>>,
    signal_map: SharedSignalMap,
}

impl DataServiceHandle {
    /// Returns the spot price tracker for a coin (by slug, e.g., "btc").
    pub fn spot_tracker(&self, coin: &str) -> Option<&Arc<RwLock<SpotPriceTracker>>> {
        self.spot_trackers.get(coin)
    }

    /// Returns all spot price trackers.
    pub fn spot_trackers(&self) -> &HashMap<String, Arc<RwLock<SpotPriceTracker>>> {
        &self.spot_trackers
    }

    /// Returns the shared signal map.
    pub fn signal_map(&self) -> &SharedSignalMap {
        &self.signal_map
    }
}

/// Unified data collection service.
///
/// Owns Binance spot feeds and signal aggregator. Provides shared state via
/// [`DataServiceHandle`] and optionally persists signal snapshots to the database.
pub struct DataService {
    config: DataServiceConfig,
    db_pool: Option<PgPool>,
    stop: Arc<AtomicBool>,
    handle: DataServiceHandle,
}

impl DataService {
    /// Creates a new data service.
    ///
    /// Pre-creates spot trackers and signal map so the handle is available
    /// before `run()` is called.
    pub fn new(config: DataServiceConfig, db_pool: Option<PgPool>, stop: Arc<AtomicBool>) -> Self {
        // Pre-create spot trackers
        let mut spot_trackers = HashMap::new();
        for coin in &config.coins {
            let slug = coin.slug_prefix().to_string();
            spot_trackers.insert(slug, Arc::new(RwLock::new(SpotPriceTracker::new())));
        }

        let signal_map: SharedSignalMap = Arc::new(RwLock::new(HashMap::new()));

        let handle = DataServiceHandle {
            spot_trackers,
            signal_map,
        };

        Self {
            config,
            db_pool,
            stop,
            handle,
        }
    }

    /// Returns a cloneable handle for consumers.
    pub fn handle(&self) -> DataServiceHandle {
        self.handle.clone()
    }

    /// Returns the stop flag.
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Runs the data service, spawning all collection tasks.
    ///
    /// Blocks until the stop flag is set. Spawns:
    /// 1. Per-coin Binance aggTrade WebSocket feeds
    /// 2. Signal aggregator (if enabled)
    /// 3. Signal snapshot writer (if enabled + DB available)
    /// 4. Raw data collectors + writers (if enabled + DB available)
    pub async fn run(self) {
        info!(
            coins = ?self.config.coins.iter().map(|c| c.slug_prefix()).collect::<Vec<_>>(),
            signals = self.config.enable_signals,
            signal_persistence = self.config.enable_signal_persistence,
            raw_persistence = self.config.enable_raw_persistence,
            "Starting DataService"
        );

        let mut handles = Vec::new();

        // 1. Spawn per-coin spot price feeds
        for coin in &self.config.coins {
            let slug = coin.slug_prefix().to_string();
            let tracker = match self.handle.spot_trackers.get(&slug) {
                Some(t) => Arc::clone(t),
                None => continue,
            };

            let spot_config = SpotPriceFeedConfig {
                symbol: format!("{}usdt", slug),
                max_reconnect_attempts: 0, // Unlimited reconnects
                ..Default::default()
            };

            let stop = Arc::clone(&self.stop);
            let handle = tokio::spawn(async move {
                let mut feed = SpotPriceFeed::new(spot_config, tracker);
                // Link feed stop to service stop
                let feed_stop = feed.stop_handle();
                let stop_watcher = tokio::spawn(async move {
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            feed_stop.store(true, Ordering::SeqCst);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
                if let Err(e) = feed.run().await {
                    error!(error = %e, "Spot feed error");
                }
                stop_watcher.abort();
            });
            handles.push(handle);

            info!(coin = slug.as_str(), "Spawned spot price feed");
        }

        // 2. Spawn signal aggregator (if enabled)
        if self.config.enable_signals {
            let aggregator_config = SignalAggregatorConfig {
                compute_interval: self.config.signal_compute_interval,
                ..Default::default()
            };
            let aggregator = SignalAggregator::with_config(
                self.config.coins.clone(),
                Arc::clone(&self.handle.signal_map),
                Arc::clone(&self.stop),
                aggregator_config,
            );
            let handle = tokio::spawn(async move {
                aggregator.run().await;
            });
            handles.push(handle);

            info!("Spawned signal aggregator");
        }

        // 3. Spawn signal snapshot writer (if enabled + DB available)
        if self.config.enable_signal_persistence {
            if let Some(ref pool) = self.db_pool {
                let writer = SignalSnapshotWriter::new(
                    pool.clone(),
                    Arc::clone(&self.handle.signal_map),
                    self.config.coins.clone(),
                    self.config.signal_flush_interval,
                    Arc::clone(&self.stop),
                );
                let handle = tokio::spawn(async move {
                    writer.run().await;
                });
                handles.push(handle);

                info!(
                    interval = ?self.config.signal_flush_interval,
                    "Spawned signal snapshot writer"
                );
            } else {
                warn!("Signal persistence enabled but no database pool provided");
            }
        }

        // 4. Spawn raw data collectors + writers (if enabled + DB available)
        if self.config.enable_raw_persistence {
            if let Some(ref pool) = self.db_pool {
                let raw_handles = spawn_raw_data_collectors(
                    pool,
                    &self.config.coins,
                    &self.stop,
                );
                handles.extend(raw_handles);

                info!(
                    coins = self.config.coins.len(),
                    "Spawned raw data collectors (OB, funding, liquidation)"
                );
            } else {
                warn!("Raw data persistence enabled but no database pool provided");
            }
        }

        // Wait for stop signal
        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        info!("DataService shutting down");

        // Give tasks time to notice stop flag and flush
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Abort remaining tasks
        for handle in handles {
            handle.abort();
        }
    }
}

/// Periodically reads the shared signal map and writes snapshots to the database.
pub struct SignalSnapshotWriter {
    repo: SignalSnapshotRepository,
    signal_map: SharedSignalMap,
    coins: Vec<Coin>,
    flush_interval: Duration,
    stop: Arc<AtomicBool>,
}

impl SignalSnapshotWriter {
    /// Creates a new signal snapshot writer.
    pub fn new(
        pool: PgPool,
        signal_map: SharedSignalMap,
        coins: Vec<Coin>,
        flush_interval: Duration,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            repo: SignalSnapshotRepository::new(pool),
            signal_map,
            coins,
            flush_interval,
            stop,
        }
    }

    /// Runs the snapshot writer loop, flushing to the database at the configured interval.
    pub async fn run(self) {
        info!("Signal snapshot writer started");
        let mut interval = tokio::time::interval(self.flush_interval);

        loop {
            interval.tick().await;

            if self.stop.load(Ordering::Relaxed) {
                // Final flush before shutdown
                self.flush().await;
                info!("Signal snapshot writer stopped");
                return;
            }

            self.flush().await;
        }
    }

    async fn flush(&self) {
        let map = self.signal_map.read().await;
        if map.is_empty() {
            return;
        }

        let now = Utc::now();
        let mut records = Vec::new();

        for coin in &self.coins {
            let slug = coin.slug_prefix();
            if let Some(signal) = map.get(slug) {
                let record = signal_to_snapshot(coin, signal, now);
                records.push(record);
            }
        }

        drop(map); // Release read lock before DB write

        if records.is_empty() {
            return;
        }

        match self.repo.insert_batch(&records).await {
            Ok(ids) => {
                info!(count = ids.len(), "Flushed signal snapshots to DB");
            }
            Err(e) => {
                error!(error = %e, "Failed to flush signal snapshots");
            }
        }
    }
}

/// Converts a core `SignalValue` into a `SignalSnapshotRecord` for DB persistence.
pub fn signal_to_snapshot(
    coin: &Coin,
    signal: &SignalValue,
    timestamp: chrono::DateTime<Utc>,
) -> SignalSnapshotRecord {
    let direction = match signal.direction {
        Direction::Up => SignalDirection::Up,
        Direction::Down => SignalDirection::Down,
        Direction::Neutral => SignalDirection::Neutral,
    };

    let symbol = format!("{}USDT", coin.slug_prefix().to_uppercase());

    // Convert f64 strength/confidence to Decimal
    let strength = Decimal::try_from(signal.strength).unwrap_or(Decimal::ZERO);
    let confidence = Decimal::try_from(signal.confidence).unwrap_or(Decimal::ZERO);

    let mut metadata = serde_json::Map::new();
    for (k, v) in &signal.metadata {
        metadata.insert(k.clone(), serde_json::json!(v));
    }

    SignalSnapshotRecord::new(
        timestamp,
        "directional_composite",
        &symbol,
        "binance",
        direction,
        strength,
        confidence,
    )
    .with_metadata(serde_json::Value::Object(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_data_service_config_for_coins() {
        let config = DataServiceConfig::for_coins(vec![Coin::Btc, Coin::Eth]);
        assert_eq!(config.coins.len(), 2);
        assert!(!config.enable_signals);
        assert!(!config.enable_signal_persistence);
        assert!(!config.enable_raw_persistence);
    }

    #[test]
    fn test_data_service_config_raw_persistence_default_false() {
        let config = DataServiceConfig::for_coins(vec![Coin::Btc]);
        assert!(!config.enable_raw_persistence);
    }

    #[test]
    fn test_data_service_config_raw_persistence_can_be_enabled() {
        let mut config = DataServiceConfig::for_coins(vec![Coin::Btc, Coin::Eth]);
        config.enable_raw_persistence = true;
        assert!(config.enable_raw_persistence);
        // Other fields unaffected
        assert!(!config.enable_signals);
        assert!(!config.enable_signal_persistence);
    }

    #[test]
    fn test_data_service_creates_trackers() {
        let config = DataServiceConfig::for_coins(vec![Coin::Btc, Coin::Eth, Coin::Sol]);
        let stop = Arc::new(AtomicBool::new(false));
        let service = DataService::new(config, None, stop);
        let handle = service.handle();

        assert!(handle.spot_tracker("btc").is_some());
        assert!(handle.spot_tracker("eth").is_some());
        assert!(handle.spot_tracker("sol").is_some());
        assert!(handle.spot_tracker("xrp").is_none());
    }

    #[test]
    fn test_handle_is_cloneable() {
        let config = DataServiceConfig::for_coins(vec![Coin::Btc]);
        let stop = Arc::new(AtomicBool::new(false));
        let service = DataService::new(config, None, stop);

        let handle1 = service.handle();
        let handle2 = handle1.clone();

        // Both handles point to the same tracker
        assert!(Arc::ptr_eq(
            handle1.spot_tracker("btc").unwrap(),
            handle2.spot_tracker("btc").unwrap(),
        ));
    }

    #[test]
    fn test_signal_map_shared() {
        let config = DataServiceConfig::for_coins(vec![Coin::Btc]);
        let stop = Arc::new(AtomicBool::new(false));
        let service = DataService::new(config, None, stop);

        let handle1 = service.handle();
        let handle2 = handle1.clone();

        // Both point to same signal map
        assert!(Arc::ptr_eq(handle1.signal_map(), handle2.signal_map()));
    }

    #[test]
    fn test_signal_to_snapshot_up() {
        let signal = SignalValue {
            direction: Direction::Up,
            strength: 0.75,
            confidence: 0.85,
            metadata: {
                let mut m = HashMap::new();
                m.insert("ob_imbalance".to_string(), 0.15);
                m
            },
        };

        let record = signal_to_snapshot(&Coin::Btc, &signal, Utc::now());
        assert_eq!(record.signal_name, "directional_composite");
        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.direction, "up");
        assert!(record.metadata.get("ob_imbalance").is_some());
    }

    #[test]
    fn test_signal_to_snapshot_down() {
        let signal = SignalValue {
            direction: Direction::Down,
            strength: 0.6,
            confidence: 0.5,
            metadata: HashMap::new(),
        };

        let record = signal_to_snapshot(&Coin::Eth, &signal, Utc::now());
        assert_eq!(record.symbol, "ETHUSDT");
        assert_eq!(record.direction, "down");
    }

    #[test]
    fn test_signal_to_snapshot_neutral() {
        let signal = SignalValue {
            direction: Direction::Neutral,
            strength: 0.0,
            confidence: 0.0,
            metadata: HashMap::new(),
        };

        let record = signal_to_snapshot(&Coin::Sol, &signal, Utc::now());
        assert_eq!(record.direction, "neutral");
    }
}
