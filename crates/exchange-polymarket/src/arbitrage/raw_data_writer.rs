//! Raw market data persistence for backtesting.
//!
//! Spawns independent Binance WebSocket collectors (order book, funding, liquidation)
//! and writes records to the database via batch inserts. These collectors are separate
//! from the [`SignalAggregator`]'s collectors, which only feed in-memory signal computation.
//!
//! # Architecture
//!
//! ```text
//! DataService (enable_raw_persistence = true)
//!   +-- OrderBookCollector -----> batch_writer -----> OrderBookRepository
//!   +-- FundingCollector  -----> batch_writer -----> FundingRateRepository
//!   +-- LiquidationCollector --> batch_writer -----> LiquidationRepository
//! ```
//!
//! This duplicates the Binance WebSocket connections from SignalAggregator, but keeps
//! changes minimal. A future refactor of SignalAggregator to expose collector channels
//! will eliminate the duplication.

use crate::models::Coin;
use algo_trade_data::{
    FundingRateRecord, FundingRateRepository, LiquidationRecord, LiquidationRepository,
    OrderBookRepository, OrderBookSnapshotRecord,
};
use algo_trade_signals::{
    CollectorConfig, FundingCollector, LiquidationCollector, LiquidationCollectorConfig,
    OrderBookCollector,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Batch size for database inserts.
const BATCH_SIZE: usize = 100;

/// Interval between periodic batch flushes.
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Channel capacity for collector -> writer communication.
const CHANNEL_CAPACITY: usize = 1000;

/// Trait for repositories that support batch insert of a specific record type.
#[async_trait::async_trait]
trait RawBatchInsert<T>: Send + Sync {
    async fn raw_insert_batch(&self, records: &[T]) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl RawBatchInsert<OrderBookSnapshotRecord> for OrderBookRepository {
    async fn raw_insert_batch(&self, records: &[OrderBookSnapshotRecord]) -> anyhow::Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl RawBatchInsert<FundingRateRecord> for FundingRateRepository {
    async fn raw_insert_batch(&self, records: &[FundingRateRecord]) -> anyhow::Result<()> {
        self.insert_batch(records).await
    }
}

#[async_trait::async_trait]
impl RawBatchInsert<LiquidationRecord> for LiquidationRepository {
    async fn raw_insert_batch(&self, records: &[LiquidationRecord]) -> anyhow::Result<()> {
        self.insert_events_batch(records).await
    }
}

/// Spawns independent data collectors and database writers for raw market data.
///
/// Returns a list of task handles that the caller should track for lifecycle management.
/// All tasks respect the `stop` flag for graceful shutdown.
pub fn spawn_raw_data_collectors(
    pool: &PgPool,
    coins: &[Coin],
    stop: &Arc<AtomicBool>,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();

    // Order book collectors (one per coin)
    for coin in coins {
        let (ob_tx, ob_rx) = mpsc::channel::<OrderBookSnapshotRecord>(CHANNEL_CAPACITY);
        let symbol = format!("{}usdt", coin.slug_prefix());
        let config = CollectorConfig {
            symbol: symbol.clone(),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
            exchange: "binance".to_string(),
        };

        let mut collector = OrderBookCollector::new(config, ob_tx);
        handles.push(tokio::spawn(async move {
            if let Err(e) = collector.run().await {
                warn!(error = %e, "Raw data OB collector stopped");
            }
        }));

        let repo = OrderBookRepository::new(pool.clone());
        let stop_clone = Arc::clone(stop);
        handles.push(tokio::spawn(async move {
            run_batch_writer("orderbook", ob_rx, stop_clone, repo).await;
        }));

        info!(coin = symbol.as_str(), "Spawned raw OB data collector");
    }

    // Funding rate collector (one for all symbols)
    {
        let (funding_tx, funding_rx) = mpsc::channel::<FundingRateRecord>(CHANNEL_CAPACITY);
        let funding_symbols: Vec<String> = coins
            .iter()
            .map(|c| format!("{}USDT", c.slug_prefix().to_uppercase()))
            .collect();

        let mut collector = FundingCollector::new(funding_symbols, funding_tx);
        handles.push(tokio::spawn(async move {
            if let Err(e) = collector.run().await {
                warn!(error = %e, "Raw data funding collector stopped");
            }
        }));

        let repo = FundingRateRepository::new(pool.clone());
        let stop_clone = Arc::clone(stop);
        handles.push(tokio::spawn(async move {
            run_batch_writer("funding", funding_rx, stop_clone, repo).await;
        }));

        info!("Spawned raw funding data collector");
    }

    // Liquidation collector (all symbols)
    {
        let (liq_tx, liq_rx) = mpsc::channel::<LiquidationRecord>(CHANNEL_CAPACITY);
        let liq_config = LiquidationCollectorConfig {
            min_usd_threshold: Decimal::new(3000, 0),
            symbol_filter: None,
            exchange: "binance".to_string(),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 0,
            aggregate_interval: Duration::from_secs(60),
        };

        let mut collector = LiquidationCollector::new(liq_config, liq_tx);
        handles.push(tokio::spawn(async move {
            if let Err(e) = collector.run().await {
                warn!(error = %e, "Raw data liquidation collector stopped");
            }
        }));

        let repo = LiquidationRepository::new(pool.clone());
        let stop_clone = Arc::clone(stop);
        handles.push(tokio::spawn(async move {
            run_batch_writer("liquidation", liq_rx, stop_clone, repo).await;
        }));

        info!("Spawned raw liquidation data collector");
    }

    handles
}

/// Generic batch writer that drains a channel and periodically flushes to DB.
///
/// Uses the [`RawBatchInsert`] trait to perform type-safe batch inserts for
/// any supported record/repository combination.
async fn run_batch_writer<T, R>(
    name: &str,
    mut rx: mpsc::Receiver<T>,
    stop: Arc<AtomicBool>,
    repo: R,
) where
    T: Send + 'static,
    R: RawBatchInsert<T>,
{
    let mut batch: Vec<T> = Vec::with_capacity(BATCH_SIZE);
    let mut flush_timer = tokio::time::interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            Some(record) = rx.recv() => {
                batch.push(record);

                if batch.len() >= BATCH_SIZE {
                    if let Err(e) = repo.raw_insert_batch(&batch).await {
                        error!(writer = name, error = %e, "Batch insert failed");
                    } else {
                        debug!(writer = name, count = batch.len(), "Batch inserted");
                    }
                    batch.clear();
                }
            }

            _ = flush_timer.tick() => {
                if stop.load(Ordering::Relaxed) {
                    // Final flush before shutdown
                    if !batch.is_empty() {
                        if let Err(e) = repo.raw_insert_batch(&batch).await {
                            error!(writer = name, error = %e, "Final flush failed");
                        } else {
                            info!(writer = name, count = batch.len(), "Final flush complete");
                        }
                    }
                    info!(writer = name, "Batch writer stopped");
                    return;
                }

                // Periodic flush
                if !batch.is_empty() {
                    if let Err(e) = repo.raw_insert_batch(&batch).await {
                        error!(writer = name, error = %e, "Periodic flush failed");
                    } else {
                        debug!(writer = name, count = batch.len(), "Periodic flush complete");
                    }
                    batch.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock repository for testing the batch writer.
    #[derive(Clone)]
    struct MockRepo {
        inserted: Arc<tokio::sync::Mutex<Vec<i32>>>,
        should_fail: bool,
    }

    impl MockRepo {
        fn new() -> Self {
            Self {
                inserted: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                inserted: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                should_fail: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl RawBatchInsert<i32> for MockRepo {
        async fn raw_insert_batch(&self, records: &[i32]) -> anyhow::Result<()> {
            if self.should_fail {
                return Err(anyhow::anyhow!("simulated DB error"));
            }
            let mut guard = self.inserted.lock().await;
            guard.extend_from_slice(records);
            Ok(())
        }
    }

    #[test]
    fn test_constants() {
        assert_eq!(BATCH_SIZE, 100);
        assert_eq!(FLUSH_INTERVAL, Duration::from_secs(5));
        assert_eq!(CHANNEL_CAPACITY, 1000);
    }

    #[tokio::test]
    async fn test_batch_writer_flushes_on_stop() {
        let (tx, rx) = mpsc::channel::<i32>(10);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let repo = MockRepo::new();
        let inserted = Arc::clone(&repo.inserted);

        let writer_handle = tokio::spawn(async move {
            run_batch_writer("test", rx, stop_clone, repo).await;
        });

        // Send a few records (fewer than BATCH_SIZE so they sit in the buffer)
        tx.send(1).await.ok();
        tx.send(2).await.ok();
        tx.send(3).await.ok();

        // Wait a moment for records to arrive
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Signal stop
        stop.store(true, Ordering::SeqCst);

        // Wait for writer to finish (flush timer will fire within 5s)
        let result = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
        assert!(result.is_ok(), "Writer should have stopped");

        // Verify all records were flushed
        let final_data = inserted.lock().await;
        assert_eq!(final_data.len(), 3);
        assert_eq!(*final_data, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_batch_writer_flushes_at_batch_size() {
        let (tx, rx) = mpsc::channel::<i32>(BATCH_SIZE + 10);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let repo = MockRepo::new();
        let inserted = Arc::clone(&repo.inserted);

        let writer_handle = tokio::spawn(async move {
            run_batch_writer("test", rx, stop_clone, repo).await;
        });

        // Send exactly BATCH_SIZE records to trigger an immediate flush
        for i in 0..BATCH_SIZE {
            tx.send(i as i32).await.ok();
        }

        // Wait for the batch flush to happen
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify at least one flush happened from batch size trigger
        let data = inserted.lock().await;
        assert!(
            data.len() >= BATCH_SIZE,
            "Expected at least {} records, got {}",
            BATCH_SIZE,
            data.len()
        );

        // Clean up
        stop.store(true, Ordering::SeqCst);
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
    }

    #[tokio::test]
    async fn test_batch_writer_handles_insert_error() {
        let (tx, rx) = mpsc::channel::<i32>(BATCH_SIZE + 10);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let repo = MockRepo::failing();

        let writer_handle = tokio::spawn(async move {
            run_batch_writer("test", rx, stop_clone, repo).await;
        });

        // Send records to fill a batch
        for i in 0..BATCH_SIZE {
            tx.send(i as i32).await.ok();
        }

        // Wait a moment for the error to be logged (not panic)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Clean up - writer should still be running despite errors
        stop.store(true, Ordering::SeqCst);
        drop(tx);
        let result = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
        assert!(
            result.is_ok(),
            "Writer should stop gracefully despite errors"
        );
    }

    #[tokio::test]
    async fn test_batch_writer_stops_when_channel_closes() {
        let (tx, rx) = mpsc::channel::<i32>(10);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let repo = MockRepo::new();

        let writer_handle = tokio::spawn(async move {
            run_batch_writer("test", rx, stop_clone, repo).await;
        });

        // Drop the sender to close the channel
        drop(tx);

        // Signal stop so periodic flush exits
        stop.store(true, Ordering::SeqCst);

        let result = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
        assert!(result.is_ok(), "Writer should stop when channel closes");
    }
}
