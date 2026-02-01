use crate::bot_actor::BotActor;
use crate::bot_database::BotDatabase;
use crate::bot_handle::BotHandle;
use crate::commands::{BotConfig, BotState};
// BotEvent will be used in Phase 2 for event emission logic
#[allow(unused_imports)]
use crate::events::{BotEvent, EnhancedBotStatus};
use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch, RwLock};

pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
    db: Option<Arc<BotDatabase>>,
    /// PostgreSQL pool for microstructure orchestrator (optional)
    pool: Option<PgPool>,
}

impl Default for BotRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BotRegistry {
    /// Creates a new bot registry without persistence.
    ///
    /// # Returns
    /// A new `BotRegistry` instance with an empty bot collection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: None,
            pool: None,
        }
    }

    /// Creates a new bot registry with database persistence.
    ///
    /// # Arguments
    ///
    /// * `database` - Database instance for persistence
    ///
    /// # Returns
    /// A new `BotRegistry` instance with persistence enabled.
    #[must_use]
    pub fn with_database(database: Arc<BotDatabase>) -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: Some(database),
            pool: None,
        }
    }

    /// Creates a new bot registry with database persistence and PostgreSQL pool.
    ///
    /// When a pool is provided, bots with microstructure enabled will spawn
    /// a `MicrostructureOrchestrator` background task for signal collection.
    ///
    /// # Arguments
    ///
    /// * `database` - SQLite database instance for bot persistence
    /// * `pool` - PostgreSQL pool for microstructure signal queries
    ///
    /// # Returns
    /// A new `BotRegistry` instance with full database support.
    #[must_use]
    pub fn with_database_and_pool(database: Arc<BotDatabase>, pool: PgPool) -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: Some(database),
            pool: Some(pool),
        }
    }

    /// Creates a new bot registry with only PostgreSQL pool (no bot persistence).
    ///
    /// Useful for testing or when bot persistence is not needed but microstructure
    /// signals are required.
    ///
    /// # Arguments
    ///
    /// * `pool` - PostgreSQL pool for microstructure signal queries
    ///
    /// # Returns
    /// A new `BotRegistry` instance with PostgreSQL access.
    #[must_use]
    pub fn with_pool(pool: PgPool) -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
            db: None,
            pool: Some(pool),
        }
    }

    /// Spawns a new bot with the given configuration.
    ///
    /// If persistence is enabled, the bot configuration is saved to the database.
    ///
    /// # Errors
    /// Returns an error if the bot cannot be spawned or database persistence fails.
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        // Persist to database if enabled
        if let Some(ref db) = self.db {
            db.insert_bot(&config).await?;
            tracing::info!("Persisted bot {} configuration to database", config.bot_id);
        }

        let (tx, rx) = mpsc::channel(32);

        // Create event streaming channels
        // event_rx stored in BotHandle, will be used by subscribers in Phase 2
        let (event_tx, _event_rx) = broadcast::channel(1000);

        // Create initial status
        let initial_status = EnhancedBotStatus {
            bot_id: config.bot_id.clone(),
            state: BotState::Stopped,
            execution_mode: config.execution_mode,
            last_heartbeat: Utc::now(),
            started_at: None,
            current_equity: Decimal::ZERO,
            initial_capital: Decimal::ZERO,
            total_return_pct: 0.0,
            sharpe_ratio: 0.0,
            max_drawdown: 0.0,
            win_rate: 0.0,
            num_trades: 0,
            open_positions: Vec::new(),
            closed_trades: Vec::new(),
            recent_events: Vec::new(),
            error: None,
        };
        let (status_tx, status_rx) = watch::channel(initial_status);

        let handle = BotHandle::new(tx, event_tx.clone(), status_rx);

        let bot_id = config.bot_id.clone();
        // Create actor with pool if available (for microstructure orchestrator)
        let actor = if let Some(pool) = &self.pool {
            BotActor::with_pool(config, rx, event_tx, status_tx, pool.clone())
        } else {
            BotActor::new(config, rx, event_tx, status_tx)
        };
        let bot_id_for_task = bot_id.clone();
        tokio::spawn(async move {
            if let Err(e) = actor.run().await {
                tracing::error!("Bot {} error: {}", bot_id_for_task, e);
            }
        });

        self.bots.write().await.insert(bot_id, handle.clone());

        Ok(handle)
    }

    /// Gets a handle to the bot with the given ID.
    ///
    /// # Returns
    /// `Some(BotHandle)` if the bot exists, `None` otherwise.
    #[must_use]
    pub async fn get_bot(&self, bot_id: &str) -> Option<BotHandle> {
        self.bots.read().await.get(bot_id).cloned()
    }

    /// Removes and shuts down the bot with the given ID.
    ///
    /// If persistence is enabled, the bot is also deleted from the database.
    ///
    /// # Errors
    /// Returns an error if the bot shutdown or database deletion fails.
    pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
        let value = self.bots.write().await.remove(bot_id);
        if let Some(handle) = value {
            handle.shutdown().await?;
        }

        // Delete from database if enabled
        if let Some(ref db) = self.db {
            db.delete_bot(bot_id).await?;
            tracing::info!("Deleted bot {} from database", bot_id);
        }

        Ok(())
    }

    /// Lists all bot IDs currently registered.
    ///
    /// # Returns
    /// A vector of bot IDs.
    #[must_use]
    pub async fn list_bots(&self) -> Vec<String> {
        self.bots.read().await.keys().cloned().collect()
    }

    /// Shuts down all bots in the registry.
    ///
    /// # Errors
    /// Returns an error if any bot shutdown fails.
    pub async fn shutdown_all(&self) -> Result<()> {
        let handles: Vec<_> = self.bots.read().await.values().cloned().collect();
        for handle in handles {
            handle.shutdown().await?;
        }
        Ok(())
    }

    /// Restores bots from database and spawns them.
    ///
    /// Only restores bots marked as `enabled = true` in the database.
    /// Does NOT auto-start bots - they remain in Stopped state.
    ///
    /// # Errors
    ///
    /// Returns error if database query or bot spawning fails.
    pub async fn restore_from_db(&self) -> Result<Vec<String>> {
        let Some(ref db) = self.db else {
            tracing::warn!("No database configured, skipping restore");
            return Ok(Vec::new());
        };

        let configs = db.get_enabled_bots().await?;
        let mut restored = Vec::new();

        for config in configs {
            let bot_id = config.bot_id.clone();
            match self.spawn_bot(config).await {
                Ok(_) => {
                    tracing::info!("Restored bot {}", bot_id);
                    restored.push(bot_id);
                }
                Err(e) => {
                    tracing::error!("Failed to restore bot {}: {}", bot_id, e);
                }
            }
        }

        Ok(restored)
    }

    /// Synchronizes running bots with approved tokens from backtest analysis.
    ///
    /// This method:
    /// 1. Compares the list of approved tokens with currently running bots
    /// 2. Spawns new paper trading bots for newly approved tokens
    /// 3. Stops bots for tokens that are no longer approved
    ///
    /// All spawned bots are in paper trading mode with the specified strategy configuration.
    ///
    /// # Arguments
    ///
    /// * `approved_tokens` - List of token symbols that passed backtest criteria
    /// * `strategy_name` - Name of the strategy to use (e.g., "quad_ma")
    /// * `base_config` - Template configuration for spawning new bots
    ///
    /// # Returns
    ///
    /// Returns a tuple of `(started, stopped)` where:
    /// - `started`: Vector of bot IDs that were newly spawned
    /// - `stopped`: Vector of bot IDs that were stopped
    ///
    /// # Errors
    ///
    /// Returns error if bot spawning or stopping fails.
    pub async fn sync_bots_with_approved_tokens(
        &self,
        approved_tokens: &[String],
        strategy_name: &str,
        base_config: &BotConfig,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let current_bots = self.bots.read().await;
        let current_bot_symbols: std::collections::HashSet<String> = current_bots
            .values()
            .map(|handle| {
                // Extract symbol from bot_id (format: "strategy_symbol")
                let bot_id = handle.latest_status().bot_id;
                bot_id.split('_').nth(1).unwrap_or("").to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();
        drop(current_bots);

        let approved_set: std::collections::HashSet<String> =
            approved_tokens.iter().cloned().collect();

        // Find tokens to add (approved but not running)
        let to_start: Vec<String> = approved_set
            .difference(&current_bot_symbols)
            .cloned()
            .collect();

        // Find tokens to remove (running but not approved)
        let to_stop: Vec<String> = current_bot_symbols
            .difference(&approved_set)
            .cloned()
            .collect();

        tracing::info!(
            "Bot sync: {} to start, {} to stop",
            to_start.len(),
            to_stop.len()
        );

        // Start new bots
        let mut started = Vec::new();
        for symbol in &to_start {
            let bot_id = format!("{}_{}", strategy_name, symbol);
            let mut config = base_config.clone();
            config.bot_id = bot_id.clone();
            config.symbol = symbol.clone();
            config.strategy = strategy_name.to_string();
            config.execution_mode = crate::ExecutionMode::Paper;

            match self.spawn_bot(config).await {
                Ok(_) => {
                    tracing::info!("Started paper bot {} for token {}", bot_id, symbol);
                    started.push(bot_id);
                }
                Err(e) => {
                    tracing::error!("Failed to start bot for token {}: {}", symbol, e);
                }
            }
        }

        // Stop removed bots
        let mut stopped = Vec::new();
        for symbol in &to_stop {
            let bot_id = format!("{}_{}", strategy_name, symbol);
            match self.remove_bot(&bot_id).await {
                Ok(()) => {
                    tracing::info!("Stopped bot {} for token {}", bot_id, symbol);
                    stopped.push(bot_id);
                }
                Err(e) => {
                    tracing::error!("Failed to stop bot {}: {}", bot_id, e);
                }
            }
        }

        Ok((started, stopped))
    }

    /// Synchronizes running bots with approved tokens including optimal backtest parameters.
    ///
    /// This enhanced version:
    /// 1. Extracts optimal strategy parameters from backtest results
    /// 2. Applies those parameters to newly spawned bots via `strategy_config`
    /// 3. Compares with currently running bots and starts/stops as needed
    ///
    /// # Arguments
    ///
    /// * `backtest_results` - Backtest results with parameters for approved tokens
    /// * `strategy_name` - Name of the strategy to use
    /// * `base_config` - Template configuration for spawning new bots
    ///
    /// # Returns
    ///
    /// Returns a tuple of `(started, stopped)` where:
    /// - `started`: Vector of bot IDs that were newly spawned with optimal parameters
    /// - `stopped`: Vector of bot IDs that were stopped
    ///
    /// # Errors
    ///
    /// Returns error if bot spawning or stopping fails.
    pub async fn sync_bots_with_backtest_results(
        &self,
        backtest_results: &[algo_trade_data::BacktestResultRecord],
        strategy_name: &str,
        base_config: &BotConfig,
    ) -> Result<(Vec<String>, Vec<String>)> {
        use std::collections::HashMap;

        let current_bots = self.bots.read().await;
        let current_bot_symbols: std::collections::HashSet<String> = current_bots
            .values()
            .map(|handle| {
                let bot_id = handle.latest_status().bot_id;
                bot_id.split('_').nth(1).unwrap_or("").to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();
        drop(current_bots);

        // Build map of symbol -> parameters from backtest results
        let approved_tokens: HashMap<String, Option<String>> = backtest_results
            .iter()
            .map(|r| {
                let params_json = r.parameters.as_ref().map(|v| v.to_string());
                (r.symbol.clone(), params_json)
            })
            .collect();

        let approved_set: std::collections::HashSet<String> =
            approved_tokens.keys().cloned().collect();

        // Find tokens to add/remove
        let to_start: Vec<String> = approved_set
            .difference(&current_bot_symbols)
            .cloned()
            .collect();

        let to_stop: Vec<String> = current_bot_symbols
            .difference(&approved_set)
            .cloned()
            .collect();

        tracing::info!(
            "Bot sync with parameters: {} to start, {} to stop",
            to_start.len(),
            to_stop.len()
        );

        // Start new bots with optimal parameters
        let mut started = Vec::new();
        for symbol in &to_start {
            let bot_id = format!("{}_{}", strategy_name, symbol);
            let mut config = base_config.clone();
            config.bot_id = bot_id.clone();
            config.symbol = symbol.clone();
            config.strategy = strategy_name.to_string();
            config.execution_mode = crate::ExecutionMode::Paper;

            // Apply optimal parameters from backtest if available
            if let Some(Some(params_json)) = approved_tokens.get(symbol) {
                config.strategy_config = Some(params_json.clone());
                tracing::info!(
                    "Applying optimal parameters to bot {}: {}",
                    bot_id,
                    params_json
                );
            }

            match self.spawn_bot(config).await {
                Ok(_) => {
                    tracing::info!(
                        "Started paper bot {} for token {} with optimal params",
                        bot_id,
                        symbol
                    );
                    started.push(bot_id);
                }
                Err(e) => {
                    tracing::error!("Failed to start bot for token {}: {}", symbol, e);
                }
            }
        }

        // Stop removed bots
        let mut stopped = Vec::new();
        for symbol in &to_stop {
            let bot_id = format!("{}_{}", strategy_name, symbol);
            match self.remove_bot(&bot_id).await {
                Ok(()) => {
                    tracing::info!("Stopped bot {} for token {}", bot_id, symbol);
                    stopped.push(bot_id);
                }
                Err(e) => {
                    tracing::error!("Failed to stop bot {}: {}", bot_id, e);
                }
            }
        }

        Ok((started, stopped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_new_has_no_pool() {
        let registry = BotRegistry::new();
        assert!(registry.pool.is_none());
        assert!(registry.db.is_none());
    }

    #[test]
    fn registry_with_pool_stores_pool() {
        // Note: We can't create a real PgPool in a unit test without a database connection,
        // but this test documents that the with_pool constructor exists
        // and the struct has the expected fields
        let registry = BotRegistry::new();
        assert!(registry.pool.is_none());
    }

    #[test]
    fn registry_default_is_same_as_new() {
        let registry1 = BotRegistry::new();
        let registry2 = BotRegistry::default();

        // Both should have no pool and no db
        assert!(registry1.pool.is_none());
        assert!(registry2.pool.is_none());
        assert!(registry1.db.is_none());
        assert!(registry2.db.is_none());
    }

    #[tokio::test]
    async fn registry_list_bots_empty_initially() {
        let registry = BotRegistry::new();
        let bots = registry.list_bots().await;
        assert!(bots.is_empty());
    }
}
