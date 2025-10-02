use crate::commands::{BotCommand, BotConfig, BotState, BotStatus};
use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc;

pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
}

impl BotActor {
    /// Creates a new bot actor with the given configuration and command receiver.
    ///
    /// # Returns
    /// A new `BotActor` instance in the stopped state.
    #[must_use]
    pub const fn new(config: BotConfig, rx: mpsc::Receiver<BotCommand>) -> Self {
        Self {
            config,
            state: BotState::Stopped,
            rx,
        }
    }

    /// Runs the bot actor's main event loop, processing commands from the channel.
    ///
    /// # Errors
    /// Returns an error if command processing fails.
    // Allow cognitive_complexity: This is a simple event loop with a match statement.
    // The complexity calculation is inflated by the match arms, but the logic is straightforward.
    #[allow(clippy::cognitive_complexity)]
    pub async fn run(mut self) -> Result<()> {
        tracing::info!("Bot {} starting", self.config.bot_id);

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                BotCommand::Start => {
                    tracing::info!("Bot {} started", self.config.bot_id);
                    self.state = BotState::Running;
                }
                BotCommand::Stop => {
                    tracing::info!("Bot {} stopped", self.config.bot_id);
                    self.state = BotState::Stopped;
                }
                BotCommand::Pause => {
                    tracing::info!("Bot {} paused", self.config.bot_id);
                    self.state = BotState::Paused;
                }
                BotCommand::Resume => {
                    tracing::info!("Bot {} resumed", self.config.bot_id);
                    self.state = BotState::Running;
                }
                BotCommand::UpdateConfig(new_config) => {
                    tracing::info!("Bot {} config updated", self.config.bot_id);
                    self.config = new_config;
                }
                BotCommand::GetStatus(tx) => {
                    let status = BotStatus {
                        bot_id: self.config.bot_id.clone(),
                        state: self.state.clone(),
                        last_heartbeat: Utc::now(),
                        error: None,
                    };
                    let _ = tx.send(status);
                }
                BotCommand::Shutdown => {
                    tracing::info!("Bot {} shutting down", self.config.bot_id);
                    break;
                }
            }
        }

        tracing::info!("Bot {} stopped", self.config.bot_id);
        Ok(())
    }
}
