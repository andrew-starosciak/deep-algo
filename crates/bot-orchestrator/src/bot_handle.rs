use crate::commands::{BotCommand, BotConfig, BotStatus};
use crate::events::{BotEvent, EnhancedBotStatus};
use anyhow::Result;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

#[derive(Clone)]
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
    event_tx: broadcast::Sender<BotEvent>,
    status_rx: watch::Receiver<EnhancedBotStatus>,
}

impl BotHandle {
    /// Creates a new bot handle with the given command sender and event channels.
    ///
    /// # Returns
    /// A new `BotHandle` instance that can be cloned and shared.
    #[must_use]
    pub const fn new(
        tx: mpsc::Sender<BotCommand>,
        event_tx: broadcast::Sender<BotEvent>,
        status_rx: watch::Receiver<EnhancedBotStatus>,
    ) -> Self {
        Self {
            tx,
            event_tx,
            status_rx,
        }
    }

    /// Starts the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn start(&self) -> Result<()> {
        self.tx.send(BotCommand::Start).await?;
        Ok(())
    }

    /// Stops the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn stop(&self) -> Result<()> {
        self.tx.send(BotCommand::Stop).await?;
        Ok(())
    }

    /// Pauses the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn pause(&self) -> Result<()> {
        self.tx.send(BotCommand::Pause).await?;
        Ok(())
    }

    /// Resumes the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn resume(&self) -> Result<()> {
        self.tx.send(BotCommand::Resume).await?;
        Ok(())
    }

    /// Updates the bot configuration.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn update_config(&self, config: BotConfig) -> Result<()> {
        self.tx
            .send(BotCommand::UpdateConfig(Box::new(config)))
            .await?;
        Ok(())
    }

    /// Gets the current status of the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent or the response cannot be received.
    pub async fn get_status(&self) -> Result<BotStatus> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(BotCommand::GetStatus(tx)).await?;
        let status = rx.await?;
        Ok(status)
    }

    /// Shuts down the bot.
    ///
    /// # Errors
    /// Returns an error if the command cannot be sent to the bot actor.
    pub async fn shutdown(&self) -> Result<()> {
        self.tx.send(BotCommand::Shutdown).await?;
        Ok(())
    }

    /// Subscribe to bot events (for TUI, web API, logging).
    ///
    /// # Returns
    /// A new receiver that will receive all future bot events.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<BotEvent> {
        self.event_tx.subscribe()
    }

    /// Get latest bot status (non-blocking).
    ///
    /// # Returns
    /// The most recent `EnhancedBotStatus`.
    #[must_use]
    pub fn latest_status(&self) -> EnhancedBotStatus {
        self.status_rx.borrow().clone()
    }

    /// Wait for status changes.
    ///
    /// # Errors
    /// Returns an error if the status channel is closed.
    pub async fn wait_for_status_change(&mut self) -> Result<EnhancedBotStatus> {
        self.status_rx.changed().await?;
        Ok(self.status_rx.borrow().clone())
    }
}
