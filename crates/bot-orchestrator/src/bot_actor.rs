use crate::commands::{BotCommand, BotConfig, BotState, BotStatus, ExecutionMode};
use crate::events::{BotEvent, EnhancedBotStatus};
use crate::execution_wrapper::ExecutionHandlerWrapper;
use algo_trade_core::TradingSystem;
use algo_trade_hyperliquid::{LiveDataProvider, HyperliquidClient, PaperTradingExecutionHandler};
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
    system: Option<TradingSystem<LiveDataProvider, ExecutionHandlerWrapper>>,
    started_at: Option<chrono::DateTime<Utc>>, // Track when bot started

    // Event streaming
    event_tx: broadcast::Sender<BotEvent>,
    status_tx: watch::Sender<EnhancedBotStatus>,
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
            started_at: None,
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
        ).await.context(format!(
            "Failed to create live data provider (ws_url={}, symbol={}, interval={})",
            self.config.ws_url, self.config.symbol, self.config.interval
        ))?;

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

        // Create execution handler based on execution mode
        let execution_handler = match self.config.execution_mode {
            ExecutionMode::Live => {
                tracing::info!("Bot {} configured for LIVE TRADING", self.config.bot_id);

                // Create HTTP client (requires wallet for live trading)
                let client = if let Some(ref wallet_config) = self.config.wallet {
                    tracing::info!("Creating authenticated Hyperliquid client with wallet {}", wallet_config.account_address);
                    let private_key = wallet_config.api_wallet_private_key.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Live mode requires wallet private key"))?;
                    HyperliquidClient::with_wallet(
                        self.config.api_url.clone(),
                        private_key,
                        wallet_config.account_address.clone(),
                        wallet_config.nonce_counter.clone(),
                    ).context("Failed to create authenticated client")?
                } else {
                    anyhow::bail!("Live trading mode requires wallet configuration");
                };

                ExecutionHandlerWrapper::Live(Box::new(algo_trade_hyperliquid::LiveExecutionHandler::new(client)))
            }
            ExecutionMode::Paper => {
                tracing::info!("Bot {} configured for PAPER TRADING (simulated fills, no real money)", self.config.bot_id);

                if self.config.wallet.is_some() {
                    tracing::warn!("Wallet provided but paper trading mode active - wallet will NOT be used");
                }

                ExecutionHandlerWrapper::Paper(PaperTradingExecutionHandler::new(
                    self.config.paper_commission_rate,
                    self.config.paper_slippage_bps,
                ))
            }
        };

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
        loop {
            // Process next event from system
            let cycle_result = if let Some(ref mut system) = self.system {
                system.process_next_event().await
            } else {
                break;
            };

            // Handle the result
            match cycle_result {
                Ok(Some(cycle_events)) => {
                    self.emit_cycle_events(cycle_events);
                }
                Ok(None) => {
                    // No more events, stop trading
                    break;
                }
                Err(e) => {
                    tracing::error!("Bot {} trading error: {}", self.config.bot_id, e);
                    self.state = BotState::Error;
                    self.emit_error_event(format!("Trading error: {e}"));
                    break;
                }
            }

            // Check for stop command (non-blocking)
            if let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    BotCommand::Stop | BotCommand::Pause | BotCommand::Shutdown => {
                        break;
                    }
                    cmd => {
                        return Err(anyhow::anyhow!("Received command during trading: {cmd:?}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Emits events from a processing cycle to subscribers
    fn emit_cycle_events(&mut self, cycle: algo_trade_core::ProcessingCycleEvents) {
        // Emit market update
        let current_price = if let Some(close) = cycle.market_event.close_price() {
            let market_event = BotEvent::MarketUpdate {
                symbol: cycle.market_event.symbol().to_string(),
                price: close,
                volume: cycle.market_event.volume().unwrap_or(rust_decimal::Decimal::ZERO),
                timestamp: cycle.market_event.timestamp(),
            };
            self.add_event(market_event);
            Some(close)
        } else {
            None
        };

        // Emit signals
        for signal in cycle.signals {
            let signal_event = BotEvent::SignalGenerated(signal);
            self.add_event(signal_event);
        }

        // Emit orders
        for order in cycle.orders {
            let order_event = BotEvent::OrderPlaced(order);
            self.add_event(order_event);
        }

        // Emit fills
        for fill in cycle.fills {
            let fill_event = BotEvent::OrderFilled(fill);
            self.add_event(fill_event);
        }

        // Update enhanced status with latest metrics
        if let Some(price) = current_price {
            self.update_enhanced_status(cycle.market_event.symbol(), price);
        }
    }

    /// Updates status without market data (for state changes without price updates)
    fn update_status_without_market_data(&self) {
        let status = EnhancedBotStatus {
            bot_id: self.config.bot_id.clone(),
            state: self.state.clone(),
            execution_mode: self.config.execution_mode,
            last_heartbeat: Utc::now(),
            started_at: self.started_at,
            current_equity: self.system.as_ref().map_or(self.config.initial_capital, algo_trade_core::TradingSystem::current_equity),
            initial_capital: self.config.initial_capital,
            total_return_pct: self.system.as_ref().map_or(0.0, algo_trade_core::TradingSystem::total_return_pct),
            sharpe_ratio: self.system.as_ref().map_or(0.0, algo_trade_core::TradingSystem::sharpe_ratio),
            max_drawdown: self.system.as_ref().map_or(0.0, algo_trade_core::TradingSystem::max_drawdown),
            win_rate: self.system.as_ref().map_or(0.0, algo_trade_core::TradingSystem::win_rate),
            num_trades: self.system.as_ref().map_or(0, |s| s.open_positions().len()),
            open_positions: Vec::new(), // Skip position details without current price
            closed_trades: self.system.as_ref().map_or_else(Vec::new, |s| s.closed_trades().to_vec()),
            recent_events: self.recent_events.iter().cloned().collect(),
            error: if matches!(self.state, BotState::Error) {
                self.recent_events.iter().rev().find_map(|e| {
                    if let BotEvent::Error { message, .. } = e {
                        Some(message.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            },
        };

        let _ = self.status_tx.send(status);
    }

    /// Updates the enhanced bot status with latest metrics
    fn update_enhanced_status(&self, symbol: &str, current_price: rust_decimal::Decimal) {
        if let Some(ref system) = self.system {
            use crate::events::PositionInfo;

            // Get metrics from trading system
            let current_equity = system.current_equity();
            let total_return_pct = system.total_return_pct();
            let sharpe_ratio = system.sharpe_ratio();
            let max_drawdown = system.max_drawdown();
            let win_rate = system.win_rate();

            // Get open positions
            let open_positions: Vec<PositionInfo> = system.open_positions()
                .iter()
                .map(|(sym, pos)| {
                    let unrealized_pnl = if sym == symbol {
                        (current_price - pos.avg_price) * pos.quantity
                    } else {
                        rust_decimal::Decimal::ZERO
                    };

                    let unrealized_pnl_pct = if pos.avg_price > rust_decimal::Decimal::ZERO {
                        let pnl_f64: f64 = unrealized_pnl.try_into().unwrap_or(0.0);
                        let cost_f64: f64 = (pos.avg_price * pos.quantity).try_into().unwrap_or(1.0);
                        (pnl_f64 / cost_f64) * 100.0
                    } else {
                        0.0
                    };

                    PositionInfo {
                        symbol: sym.clone(),
                        quantity: pos.quantity,
                        avg_price: pos.avg_price,
                        current_price: if sym == symbol { current_price } else { pos.avg_price },
                        unrealized_pnl,
                        unrealized_pnl_pct,
                    }
                })
                .collect();

            // Count trades from trading system
            let num_trades = system.num_trades();

            let status = EnhancedBotStatus {
                bot_id: self.config.bot_id.clone(),
                state: self.state.clone(),
                execution_mode: self.config.execution_mode,
                last_heartbeat: Utc::now(),
                started_at: self.started_at,
                current_equity,
                initial_capital: self.config.initial_capital,
                total_return_pct,
                sharpe_ratio,
                max_drawdown,
                win_rate,
                num_trades,
                open_positions,
                closed_trades: system.closed_trades().to_vec(),
                recent_events: self.recent_events.iter().cloned().collect(),
                error: None,
            };

            // Broadcast updated status (ignore if no receivers)
            let _ = self.status_tx.send(status);
        }
    }

    /// Emits an error event
    fn emit_error_event(&mut self, message: String) {
        let error_event = BotEvent::Error {
            message,
            timestamp: Utc::now(),
        };
        self.add_event(error_event);
    }

    /// Adds an event to recent events and broadcasts it
    fn add_event(&mut self, event: BotEvent) {
        // Add to recent events (keep last 10)
        if self.recent_events.len() >= 10 {
            self.recent_events.pop_front();
        }
        self.recent_events.push_back(event.clone());

        // Broadcast to subscribers (ignore if no receivers)
        let _ = self.event_tx.send(event);
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
                    tracing::info!("Bot {} received start command", self.config.bot_id);

                    // Defensive: Only start if stopped or error (allow restart from error)
                    if matches!(self.state, BotState::Running | BotState::Paused) {
                        tracing::warn!("Bot {} in invalid state for start (state: {:?}), ignoring command", self.config.bot_id, self.state);
                        continue;
                    }

                    // Initialize trading system
                    if let Err(e) = self.initialize_system().await {
                        tracing::error!("Failed to initialize bot {}: {}", self.config.bot_id, e);
                        self.state = BotState::Error;
                        self.emit_error_event(format!("Initialization failed: {e}"));
                        self.update_status_without_market_data();
                        continue;
                    }

                    self.state = BotState::Running;
                    self.started_at = Some(Utc::now()); // Track start time
                    self.update_status_without_market_data();
                    tracing::info!("Bot {} is now running", self.config.bot_id);

                    // Start trading loop
                    if let Err(e) = self.trading_loop().await {
                        tracing::error!("Bot {} trading loop error: {}", self.config.bot_id, e);
                        self.state = BotState::Error;
                        self.emit_error_event(format!("Trading loop error: {e}"));
                    } else {
                        tracing::info!("Bot {} trading loop exited normally", self.config.bot_id);
                        self.state = BotState::Stopped;
                    }
                    self.update_status_without_market_data();
                }
                BotCommand::Stop => {
                    // Defensive: Only stop if running or paused
                    if matches!(self.state, BotState::Stopped | BotState::Error) {
                        tracing::warn!("Bot {} already stopped, ignoring stop command", self.config.bot_id);
                        continue;
                    }

                    tracing::info!("Bot {} stopped", self.config.bot_id);
                    self.state = BotState::Stopped;
                    self.update_status_without_market_data();
                }
                BotCommand::Pause => {
                    // Defensive: Only pause if running
                    if !matches!(self.state, BotState::Running) {
                        tracing::warn!("Bot {} not running (state: {:?}), cannot pause", self.config.bot_id, self.state);
                        continue;
                    }

                    tracing::info!("Bot {} paused", self.config.bot_id);
                    self.state = BotState::Paused;
                    self.update_status_without_market_data();
                }
                BotCommand::Resume => {
                    // Defensive: Only resume if paused
                    if !matches!(self.state, BotState::Paused) {
                        tracing::warn!("Bot {} not paused (state: {:?}), cannot resume", self.config.bot_id, self.state);
                        continue;
                    }

                    tracing::info!("Bot {} resumed", self.config.bot_id);
                    self.state = BotState::Running;
                    if self.started_at.is_none() {
                        self.started_at = Some(Utc::now()); // Set start time if never set
                    }
                    self.update_status_without_market_data();

                    // Resume trading loop
                    if let Err(e) = self.trading_loop().await {
                        tracing::error!("Bot {} trading loop error after resume: {}", self.config.bot_id, e);
                        self.state = BotState::Error;
                        self.emit_error_event(format!("Trading loop error: {e}"));
                    } else {
                        tracing::info!("Bot {} trading loop exited after resume", self.config.bot_id);
                        self.state = BotState::Stopped;
                    }
                    self.update_status_without_market_data();
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
