use crate::bot_actor::BotActor;
use crate::bot_handle::BotHandle;
use crate::commands::BotConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
}

impl Default for BotRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BotRegistry {
    /// Creates a new bot registry.
    ///
    /// # Returns
    /// A new `BotRegistry` instance with an empty bot collection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawns a new bot with the given configuration.
    ///
    /// # Errors
    /// Returns an error if the bot cannot be spawned.
    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        let (tx, rx) = mpsc::channel(32);
        let handle = BotHandle::new(tx);

        let bot_id = config.bot_id.clone();
        let actor = BotActor::new(config, rx);
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
    /// # Errors
    /// Returns an error if the bot shutdown fails.
    pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
        let value = self.bots.write().await.remove(bot_id);
        if let Some(handle) = value {
            handle.shutdown().await?;
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
}
