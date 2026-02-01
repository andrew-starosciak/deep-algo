//! Background orchestrator for microstructure signal collection.
//!
//! The `MicrostructureOrchestrator` runs as a background task, periodically
//! updating cached signals from database queries and signal computations.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::mpsc;

use algo_trade_core::signal::SignalGenerator;

use super::SharedMicroSignals;
use crate::context_builder::SignalContextBuilder;
use crate::generator::{
    CompositeSignal, FundingRateSignal, LiquidationCascadeSignal, NewsSignal,
    OrderBookImbalanceSignal,
};

/// Commands to control the orchestrator.
#[derive(Debug, Clone)]
pub enum OrchestratorCommand {
    /// Force an immediate update of all signals
    UpdateNow,
    /// Gracefully shutdown the orchestrator
    Shutdown,
}

/// Configuration for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Interval between signal updates
    pub update_interval: Duration,
    /// Symbol to track (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance", "hyperliquid")
    pub exchange: String,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            update_interval: Duration::from_secs(5),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
        }
    }
}

/// Background task that periodically updates microstructure signals.
///
/// The orchestrator:
/// 1. Builds a `SignalContext` from database queries
/// 2. Computes all microstructure signals
/// 3. Updates the shared `CachedMicroSignals`
///
/// # Example
///
/// ```ignore
/// let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));
///
/// let orchestrator = MicrostructureOrchestrator::new(
///     pool,
///     OrchestratorConfig::default(),
///     signals.clone(),
/// );
///
/// let cmd_tx = orchestrator.spawn();
///
/// // Later: force update
/// cmd_tx.send(OrchestratorCommand::UpdateNow).await?;
///
/// // Later: shutdown
/// cmd_tx.send(OrchestratorCommand::Shutdown).await?;
/// ```
pub struct MicrostructureOrchestrator {
    pool: PgPool,
    config: OrchestratorConfig,
    signals: SharedMicroSignals,
    generators: MicrostructureGenerators,
}

/// Collection of configured signal generators.
pub struct MicrostructureGenerators {
    /// Order book imbalance signal generator
    pub order_book: OrderBookImbalanceSignal,
    /// Funding rate signal generator
    pub funding: FundingRateSignal,
    /// Liquidation cascade signal generator
    pub liquidation: LiquidationCascadeSignal,
    /// News/sentiment signal generator
    pub news: NewsSignal,
    /// Composite signal combining all inputs
    pub composite: CompositeSignal,
}

impl Default for MicrostructureGenerators {
    fn default() -> Self {
        Self {
            order_book: OrderBookImbalanceSignal::default(),
            funding: FundingRateSignal::default(),
            liquidation: LiquidationCascadeSignal::default(),
            news: NewsSignal::default(),
            composite: CompositeSignal::weighted_average("microstructure_composite"),
        }
    }
}

impl MicrostructureOrchestrator {
    /// Creates a new orchestrator.
    ///
    /// # Arguments
    ///
    /// * `pool` - Database connection pool for signal context queries
    /// * `config` - Orchestrator configuration
    /// * `signals` - Shared signal cache to update
    #[must_use]
    pub fn new(pool: PgPool, config: OrchestratorConfig, signals: SharedMicroSignals) -> Self {
        Self {
            pool,
            config,
            signals,
            generators: MicrostructureGenerators::default(),
        }
    }

    /// Creates a new orchestrator with custom signal generators.
    #[must_use]
    pub fn with_generators(
        pool: PgPool,
        config: OrchestratorConfig,
        signals: SharedMicroSignals,
        generators: MicrostructureGenerators,
    ) -> Self {
        Self {
            pool,
            config,
            signals,
            generators,
        }
    }

    /// Spawns the background collection task and returns a command channel.
    ///
    /// The task runs until a `Shutdown` command is received or the channel closes.
    ///
    /// # Returns
    ///
    /// A sender for sending commands to the orchestrator.
    pub fn spawn(mut self) -> mpsc::Sender<OrchestratorCommand> {
        let (tx, mut rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.config.update_interval);

            tracing::info!(
                "Microstructure orchestrator started (symbol: {}, interval: {:?})",
                self.config.symbol,
                self.config.update_interval
            );

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = self.update_signals().await {
                            tracing::warn!("Failed to update microstructure signals: {e}");
                        }
                    }
                    Some(cmd) = rx.recv() => {
                        match cmd {
                            OrchestratorCommand::UpdateNow => {
                                tracing::debug!("Received UpdateNow command");
                                if let Err(e) = self.update_signals().await {
                                    tracing::warn!("Failed to update microstructure signals: {e}");
                                }
                            }
                            OrchestratorCommand::Shutdown => {
                                tracing::info!("Microstructure orchestrator shutting down");
                                break;
                            }
                        }
                    }
                    else => {
                        tracing::info!("Command channel closed, orchestrator shutting down");
                        break;
                    }
                }
            }
        });

        tx
    }

    /// Updates all microstructure signals.
    ///
    /// This method:
    /// 1. Builds a `SignalContext` from the database
    /// 2. Computes each signal generator
    /// 3. Updates the shared cache
    async fn update_signals(&mut self) -> Result<()> {
        let now = Utc::now();

        // Build context from database at current timestamp
        let ctx = SignalContextBuilder::new(
            self.pool.clone(),
            &self.config.symbol,
            &self.config.exchange,
        )
        .with_max_orderbook_levels(20)
        .with_liquidation_window(5)
        .build_at(now)
        .await?;

        // Compute all signals
        let ob_signal = self.generators.order_book.compute(&ctx).await?;
        let funding_signal = self.generators.funding.compute(&ctx).await?;
        let liq_signal = self.generators.liquidation.compute(&ctx).await?;
        let news_signal = self.generators.news.compute(&ctx).await?;
        let composite_signal = self.generators.composite.compute(&ctx).await?;

        // Update cache under write lock
        {
            let mut cache = self.signals.write().await;
            cache.order_book_imbalance = ob_signal;
            cache.funding_rate = funding_signal;
            cache.liquidation_cascade = liq_signal;
            cache.news = news_signal;
            cache.composite = composite_signal;
            cache.last_updated = now;
        }

        tracing::trace!("Updated microstructure signals at {}", now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================
    // OrchestratorConfig Tests
    // ============================================

    #[test]
    fn config_default_has_expected_values() {
        let config = OrchestratorConfig::default();

        assert_eq!(config.update_interval, Duration::from_secs(5));
        assert_eq!(config.symbol, "BTCUSDT");
        assert_eq!(config.exchange, "binance");
    }

    #[test]
    fn config_can_be_customized() {
        let config = OrchestratorConfig {
            update_interval: Duration::from_secs(10),
            symbol: "ETHUSDT".to_string(),
            exchange: "hyperliquid".to_string(),
        };

        assert_eq!(config.update_interval, Duration::from_secs(10));
        assert_eq!(config.symbol, "ETHUSDT");
        assert_eq!(config.exchange, "hyperliquid");
    }

    // ============================================
    // OrchestratorCommand Tests
    // ============================================

    #[test]
    fn command_update_now_is_debug_printable() {
        let cmd = OrchestratorCommand::UpdateNow;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("UpdateNow"));
    }

    #[test]
    fn command_shutdown_is_debug_printable() {
        let cmd = OrchestratorCommand::Shutdown;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Shutdown"));
    }

    #[test]
    fn command_can_be_cloned() {
        let cmd = OrchestratorCommand::UpdateNow;
        let cloned = cmd.clone();
        assert!(matches!(cloned, OrchestratorCommand::UpdateNow));
    }

    // ============================================
    // MicrostructureGenerators Tests
    // ============================================

    #[test]
    fn generators_default_creates_all_generators() {
        let generators = MicrostructureGenerators::default();

        // Just verify they exist and have names
        assert!(!generators.order_book.name().is_empty());
        assert!(!generators.funding.name().is_empty());
        assert!(!generators.liquidation.name().is_empty());
        assert!(!generators.news.name().is_empty());
        assert!(!generators.composite.name().is_empty());
    }

    // Note: Full integration tests for MicrostructureOrchestrator require
    // a database connection and are better suited for integration test files.
    // The spawn() and update_signals() methods are tested via integration tests.
}
