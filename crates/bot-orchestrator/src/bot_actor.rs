use crate::commands::{BotCommand, BotConfig, BotState, BotStatus};
use crate::events::{BotEvent, EnhancedBotStatus};
use algo_trade_core::TradingSystem;
use algo_trade_hyperliquid::{LiveDataProvider, LiveExecutionHandler, HyperliquidClient};
use algo_trade_strategy::{SimpleRiskManager, create_strategy};
use anyhow::{Result, Context};
use chrono::Utc;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};

pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
    system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,

    // Event streaming (used in Phase 2 for event emission)
    #[allow(dead_code)]
    event_tx: broadcast::Sender<BotEvent>,
    #[allow(dead_code)]
    status_tx: watch::Sender<EnhancedBotStatus>,
    #[allow(dead_code)]
    recent_events: VecDeque<BotEvent>,
}

impl BotActor {
    /// Creates a new bot actor with the given configuration and command receiver.
    ///
    /// # Returns
    /// A new `BotActor` instance in the stopped state.
    #[must_use]
    pub fn new(
        config: BotConfig,
        rx: mpsc::Receiver<BotCommand>,
        event_tx: broadcast::Sender<BotEvent>,
        status_tx: watch::Sender<EnhancedBotStatus>,
    ) -> Self {
        Self {
            config,
            state: BotState::Stopped,
            rx,
            system: None,
            event_tx,
            status_tx,
            recent_events: VecDeque::with_capacity(10),
        }
    }

    /// Initializes the trading system with all components
    #[allow(clippy::cognitive_complexity)]
    async fn initialize_system(&mut self) -> Result<()> {
        tracing::info!("Initializing trading system for bot {}", self.config.bot_id);

        // Create live data provider with WebSocket
        let data_provider = LiveDataProvider::new(
            self.config.ws_url.clone(),
            self.config.symbol.clone(),
            self.config.interval.clone(),
        ).await.context("Failed to create live data provider")?;

        // Warmup with historical data
        let warmup_events = data_provider.warmup(
            self.config.api_url.clone(),
            self.config.warmup_periods,
        ).await.context("Failed to warmup with historical data")?;

        tracing::info!(
            "Warmed up bot {} with {} historical candles",
            self.config.bot_id,
            warmup_events.len()
        );

        // Create HTTP client (authenticated if wallet provided)
        let client = if let Some(ref wallet_config) = self.config.wallet {
            tracing::info!("Creating authenticated Hyperliquid client with wallet {}", wallet_config.account_address);
            let private_key = wallet_config.api_wallet_private_key.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Wallet private key not provided"))?;
            HyperliquidClient::with_wallet(
                self.config.api_url.clone(),
                private_key,
                wallet_config.account_address.clone(),
                wallet_config.nonce_counter.clone(),
            ).context("Failed to create authenticated client")?
        } else {
            tracing::warn!("Creating unauthenticated client - order execution will fail");
            HyperliquidClient::new(self.config.api_url.clone())
        };

        // Create execution handler
        let execution_handler = LiveExecutionHandler::new(client);

        // Create strategy
        let strategy = create_strategy(
            &self.config.strategy,
            self.config.symbol.clone(),
            self.config.strategy_config.clone(),
        ).context("Failed to create strategy")?;

        // Feed warmup events to strategy to initialize state
        for event in warmup_events {
            let _ = strategy.lock().await.on_market_event(&event).await?;
        }

        tracing::info!("Bot {} strategy initialized", self.config.bot_id);

        // Create risk manager with bot config parameters
        let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
            Arc::new(SimpleRiskManager::new(
                self.config.risk_per_trade_pct,
                self.config.max_position_pct,
                self.config.leverage,
            ));

        // Create trading system
        let system = TradingSystem::new(
            data_provider,
            execution_handler,
            vec![strategy],
            risk_manager,
        );

        self.system = Some(system);
        Ok(())
    }

    /// Runs trading system loop (processes market events)
    async fn trading_loop(&mut self) -> Result<()> {
        if let Some(ref mut system) = self.system {
            loop {
                tokio::select! {
                    // Check for stop command
                    cmd = self.rx.recv() => {
                        match cmd {
                            Some(BotCommand::Stop | BotCommand::Pause | BotCommand::Shutdown) => {
                                break;
                            }
                            Some(cmd) => {
                                // Re-process command in main loop
                                return Err(anyhow::anyhow!("Received command during trading: {cmd:?}"));
                            }
                            None => break,
                        }
                    }
                    // Process market events
                    result = system.process_next_event() => {
                        if let Err(e) = result {
                            tracing::error!("Bot {} trading error: {}", self.config.bot_id, e);
                            self.state = BotState::Error;
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
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

                    // Initialize trading system
                    if let Err(e) = self.initialize_system().await {
                        tracing::error!("Failed to initialize bot {}: {}", self.config.bot_id, e);
                        self.state = BotState::Error;
                        continue;
                    }

                    self.state = BotState::Running;
                    // Start trading loop
                    self.trading_loop().await?;
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
                    self.trading_loop().await?;
                }
                BotCommand::UpdateConfig(new_config) => {
                    tracing::info!("Bot {} config updated", self.config.bot_id);
                    self.config = *new_config;
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
