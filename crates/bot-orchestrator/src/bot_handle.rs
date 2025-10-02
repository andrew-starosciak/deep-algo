use crate::commands::{BotCommand, BotConfig, BotStatus};
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
}

impl BotHandle {
    /// Creates a new bot handle with the given command sender.
    ///
    /// # Returns
    /// A new `BotHandle` instance that can be cloned and shared.
    #[must_use]
    pub const fn new(tx: mpsc::Sender<BotCommand>) -> Self {
        Self { tx }
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
        self.tx.send(BotCommand::UpdateConfig(config)).await?;
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
}
