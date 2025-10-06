use crate::events::FillEvent;
use crate::position::PositionTracker;
use crate::traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
use anyhow::Result;
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
    pub initial_capital: Decimal,
    pub final_capital: Decimal,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub duration: chrono::Duration,
    pub equity_peak: Decimal,
    pub buy_hold_return: Decimal,
    pub exposure_time: f64,
    pub fills: Vec<FillEvent>,
}

pub struct TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    data_provider: D,
    execution_handler: E,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
    position_tracker: PositionTracker,
    initial_capital: Decimal,
    returns: Vec<Decimal>,
    equity_curve: Vec<Decimal>,
    wins: usize,
    losses: usize,
    start_time: Option<chrono::DateTime<chrono::Utc>>,
    end_time: Option<chrono::DateTime<chrono::Utc>>,
    first_price: Option<Decimal>,
    last_price: Option<Decimal>,
    bars_in_position: usize,
    total_bars: usize,
    equity_peak: Decimal,
    fills: Vec<FillEvent>,
}

impl<D, E> TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    pub fn new(
        data_provider: D,
        execution_handler: E,
        strategies: Vec<Arc<Mutex<dyn Strategy>>>,
        risk_manager: Arc<dyn RiskManager>,
    ) -> Self {
        let initial_capital = Decimal::from(10000); // Default $10k
        Self {
            data_provider,
            execution_handler,
            strategies,
            risk_manager,
            position_tracker: PositionTracker::new(),
            initial_capital,
            returns: Vec::new(),
            equity_curve: vec![initial_capital],
            wins: 0,
            losses: 0,
            start_time: None,
            end_time: None,
            first_price: None,
            last_price: None,
            bars_in_position: 0,
            total_bars: 0,
            equity_peak: initial_capital,
            fills: Vec::new(),
        }
    }

    pub fn with_capital(
        data_provider: D,
        execution_handler: E,
        strategies: Vec<Arc<Mutex<dyn Strategy>>>,
        risk_manager: Arc<dyn RiskManager>,
        initial_capital: Decimal,
    ) -> Self {
        Self {
            data_provider,
            execution_handler,
            strategies,
            risk_manager,
            position_tracker: PositionTracker::new(),
            initial_capital,
            returns: Vec::new(),
            equity_curve: vec![initial_capital],
            wins: 0,
            losses: 0,
            start_time: None,
            end_time: None,
            first_price: None,
            last_price: None,
            bars_in_position: 0,
            total_bars: 0,
            equity_peak: initial_capital,
            fills: Vec::new(),
        }
    }

    /// Runs the trading system event loop and returns performance metrics.
    ///
    /// Processes market events from the data provider, generates signals from strategies,
    /// evaluates them through risk management, and executes orders.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Data provider fails to fetch next event
    /// - Strategy signal generation fails
    /// - Risk manager evaluation fails
    /// - Order execution fails
    ///
    /// # Panics
    ///
    /// Panics if the equity curve is empty (should never happen as it's initialized with initial capital).
    pub async fn run(&mut self) -> Result<PerformanceMetrics> {
        while let Some(market_event) = self.data_provider.next_event().await? {
            // Track timestamps
            if self.start_time.is_none() {
                self.start_time = Some(market_event.timestamp());
            }
            self.end_time = Some(market_event.timestamp());

            // Track prices for buy & hold
            if let Some(close) = market_event.close_price() {
                if self.first_price.is_none() {
                    self.first_price = Some(close);
                }
                self.last_price = Some(close);
            }

            // Track total bars
            self.total_bars += 1;

            // Track exposure time
            if !self.position_tracker.all_positions().is_empty() {
                self.bars_in_position += 1;
            }

            // Collect PnLs from trades (separate iteration from mutation)
            let mut pnls_to_record = Vec::new();

            // Generate signals from all strategies
            for strategy in &self.strategies {
                let mut strategy = strategy.lock().await;
                if let Some(signal) = strategy.on_market_event(&market_event).await? {
                    // Get current account equity for position sizing
                    let current_equity = *self.equity_curve.last().unwrap();

                    // Get current position for the signal's symbol (for position flipping)
                    let current_position = self.position_tracker.get_position(&signal.symbol)
                        .map(|p| p.quantity);

                    // Risk management evaluation (returns Vec of orders)
                    let orders = self.risk_manager.evaluate_signal(&signal, current_equity, current_position).await?;

                    // Execute all orders sequentially
                    for order in orders {
                        let fill = self.execution_handler.execute_order(order).await?;
                        tracing::info!("Order filled: {:?}", fill);

                        // Store fill for trade history
                        self.fills.push(fill.clone());

                        // Track position and calculate PnL if closing
                        if let Some(pnl) = self.position_tracker.process_fill(&fill) {
                            pnls_to_record.push(pnl);
                        }
                    }
                }
            }

            // Process PnLs after iteration completes
            for pnl in pnls_to_record {
                self.add_trade(pnl);
            }
        }

        Ok(self.calculate_metrics())
    }

    fn add_trade(&mut self, pnl: Decimal) {
        let current_equity = *self.equity_curve.last().unwrap();
        let new_equity = current_equity + pnl;

        self.equity_curve.push(new_equity);

        // Track equity peak
        if new_equity > self.equity_peak {
            self.equity_peak = new_equity;
        }

        if pnl > Decimal::ZERO {
            self.wins += 1;
        } else if pnl < Decimal::ZERO {
            self.losses += 1;
        }

        let return_pct = pnl / current_equity;
        self.returns.push(return_pct);
    }

    fn calculate_metrics(&self) -> PerformanceMetrics {
        let final_capital = *self.equity_curve.last().unwrap();
        let total_return = (final_capital - self.initial_capital) / self.initial_capital;

        #[allow(clippy::cast_precision_loss)]
        let returns_len_f64 = self.returns.len() as f64;

        let sharpe_ratio = if self.returns.is_empty() {
            0.0
        } else {
            let mean_return: f64 = self
                .returns
                .iter()
                .map(|r| r.to_string().parse::<f64>().unwrap_or(0.0))
                .sum::<f64>()
                / returns_len_f64;

            let variance: f64 = self
                .returns
                .iter()
                .map(|r| {
                    let val = r.to_string().parse::<f64>().unwrap_or(0.0);
                    (val - mean_return).powi(2)
                })
                .sum::<f64>()
                / returns_len_f64;

            let std_dev = variance.sqrt();
            if std_dev > 0.0 {
                mean_return / std_dev * (252.0_f64).sqrt() // Annualized
            } else {
                0.0
            }
        };

        let max_drawdown = self.calculate_max_drawdown();

        let total_trades = self.wins + self.losses;
        #[allow(clippy::cast_precision_loss)]
        let win_rate = if total_trades > 0 {
            self.wins as f64 / total_trades as f64
        } else {
            0.0
        };

        // Calculate buy & hold return
        let buy_hold_return = if let (Some(first), Some(last)) = (self.first_price, self.last_price) {
            (last - first) / first
        } else {
            Decimal::ZERO
        };

        // Calculate exposure time percentage
        #[allow(clippy::cast_precision_loss)]
        let exposure_time = if self.total_bars > 0 {
            self.bars_in_position as f64 / self.total_bars as f64
        } else {
            0.0
        };

        // Calculate duration
        let duration = if let (Some(start), Some(end)) = (self.start_time, self.end_time) {
            end - start
        } else {
            chrono::Duration::zero()
        };

        PerformanceMetrics {
            total_return,
            sharpe_ratio,
            max_drawdown,
            num_trades: total_trades,
            win_rate,
            initial_capital: self.initial_capital,
            final_capital,
            start_time: self.start_time.unwrap_or_else(chrono::Utc::now),
            end_time: self.end_time.unwrap_or_else(chrono::Utc::now),
            duration,
            equity_peak: self.equity_peak,
            buy_hold_return,
            exposure_time,
            fills: self.fills.clone(),
        }
    }

    /// Processes a single market event (for live trading loop)
    ///
    /// # Errors
    /// Returns error if data provider, strategy, risk manager, or execution fails
    ///
    /// # Panics
    /// Panics if `equity_curve` is empty (should never happen as it's initialized with `initial_capital`)
    pub async fn process_next_event(&mut self) -> Result<bool> {
        if let Some(market_event) = self.data_provider.next_event().await? {
            // Track timestamps
            if self.start_time.is_none() {
                self.start_time = Some(market_event.timestamp());
            }
            self.end_time = Some(market_event.timestamp());

            // Track prices for buy & hold
            if let Some(close) = market_event.close_price() {
                if self.first_price.is_none() {
                    self.first_price = Some(close);
                }
                self.last_price = Some(close);
            }

            // Track total bars
            self.total_bars += 1;

            // Track exposure time
            if !self.position_tracker.all_positions().is_empty() {
                self.bars_in_position += 1;
            }

            // Collect PnLs from trades
            let mut pnls_to_record = Vec::new();

            // Generate signals from all strategies
            for strategy in &self.strategies {
                let mut strategy = strategy.lock().await;
                if let Some(signal) = strategy.on_market_event(&market_event).await? {
                    let current_equity = *self.equity_curve.last().unwrap();
                    let current_position = self.position_tracker.get_position(&signal.symbol)
                        .map(|p| p.quantity);

                    let orders = self.risk_manager.evaluate_signal(
                        &signal,
                        current_equity,
                        current_position,
                    ).await?;

                    for order in orders {
                        let fill = self.execution_handler.execute_order(order).await?;

                        if let Some(pnl) = self.position_tracker.process_fill(&fill) {
                            pnls_to_record.push(pnl);
                        }

                        self.fills.push(fill);
                    }
                }
            }

            // Record PnLs to returns after all strategies processed
            for pnl in pnls_to_record {
                let return_pct = pnl / self.initial_capital;
                self.returns.push(return_pct);

                if pnl > Decimal::ZERO {
                    self.wins += 1;
                } else if pnl < Decimal::ZERO {
                    self.losses += 1;
                }
            }

            // Update equity curve
            let current_equity = *self.equity_curve.last().unwrap();
            // TODO: Calculate unrealized PnL from open positions
            // Need current market price for each symbol to calculate unrealized PnL
            // For now, just track realized PnL
            let new_equity = current_equity;
            self.equity_curve.push(new_equity);

            if new_equity > self.equity_peak {
                self.equity_peak = new_equity;
            }

            Ok(true) // Event processed
        } else {
            Ok(false) // No more events
        }
    }

    fn calculate_max_drawdown(&self) -> Decimal {
        let mut max_drawdown = Decimal::ZERO;
        let mut peak = self.equity_curve[0];

        for &equity in &self.equity_curve {
            if equity > peak {
                peak = equity;
            }
            let drawdown = (peak - equity) / peak;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        max_drawdown
    }

    // Public accessor methods for live monitoring
    /// Get current equity
    #[must_use]
    pub fn current_equity(&self) -> Decimal {
        self.equity_curve.last().copied().unwrap_or(self.initial_capital)
    }

    /// Get initial capital
    #[must_use]
    pub const fn initial_capital(&self) -> Decimal {
        self.initial_capital
    }

    /// Get total return percentage
    #[must_use]
    pub fn total_return_pct(&self) -> f64 {
        let current = self.current_equity();
        ((current - self.initial_capital) / self.initial_capital)
            .to_string()
            .parse()
            .unwrap_or(0.0)
    }

    /// Get Sharpe ratio
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Acceptable for statistics over small sample sizes
    pub fn sharpe_ratio(&self) -> f64 {
        if self.returns.is_empty() {
            return 0.0;
        }

        let mean_return: f64 = self
            .returns
            .iter()
            .map(|r| r.to_string().parse::<f64>().unwrap_or(0.0))
            .sum::<f64>()
            / self.returns.len() as f64;

        let variance: f64 = self
            .returns
            .iter()
            .map(|r| {
                let val = r.to_string().parse::<f64>().unwrap_or(0.0);
                (val - mean_return).powi(2)
            })
            .sum::<f64>()
            / self.returns.len() as f64;

        let std_dev = variance.sqrt();
        if std_dev > 0.0 {
            mean_return / std_dev * f64::sqrt(252.0)
        } else {
            0.0
        }
    }

    /// Get maximum drawdown
    #[must_use]
    pub fn max_drawdown(&self) -> f64 {
        self.calculate_max_drawdown()
            .to_string()
            .parse()
            .unwrap_or(0.0)
    }

    /// Get win rate
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Acceptable for statistics over small sample sizes
    pub fn win_rate(&self) -> f64 {
        let total = self.wins + self.losses;
        if total > 0 {
            self.wins as f64 / total as f64
        } else {
            0.0
        }
    }

    /// Get number of trades
    #[must_use]
    pub const fn num_trades(&self) -> usize {
        self.wins + self.losses
    }

    /// Get open positions
    #[must_use]
    pub const fn open_positions(&self) -> &std::collections::HashMap<String, crate::position::Position> {
        self.position_tracker.all_positions()
    }

    /// Calculate unrealized `PnL` for a position
    #[must_use]
    pub fn unrealized_pnl(&self, symbol: &str, current_price: Decimal) -> Option<Decimal> {
        self.position_tracker.get_position(symbol).map(|pos| {
            (current_price - pos.avg_price) * pos.quantity
        })
    }
}
