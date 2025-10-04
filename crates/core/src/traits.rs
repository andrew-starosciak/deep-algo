use crate::events::{FillEvent, MarketEvent, OrderEvent, SignalEvent};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

#[async_trait]
pub trait DataProvider: Send + Sync {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>>;
}

#[async_trait]
pub trait Strategy: Send + Sync {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>>;
    fn name(&self) -> &'static str;
}

#[async_trait]
pub trait ExecutionHandler: Send + Sync {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent>;
}

#[async_trait]
pub trait RiskManager: Send + Sync {
    /// Evaluates a trading signal and generates orders if risk criteria are met.
    ///
    /// # Parameters
    /// - `signal`: The trading signal to evaluate
    /// - `account_equity`: Current account equity in USDC for position sizing
    /// - `current_position`: Current position quantity (positive for long, negative for short, None for flat)
    ///
    /// # Returns
    /// - `Ok(Vec<OrderEvent>)`: Zero or more orders to execute
    ///   - Empty vec: Signal rejected by risk management
    ///   - Single order: Normal entry/add to position
    ///   - Two orders: Position flip (close existing, open new)
    /// - `Err`: Risk evaluation error
    ///
    /// # Position Flipping
    /// When flipping from long to short (or vice versa), returns TWO orders:
    /// 1. Close order: Opposite direction, quantity = |`current_position`|
    /// 2. Entry order: Signal direction, quantity = `target_position`
    ///
    /// # Exit Signals
    /// When signal direction is Exit and a position exists, returns close order.
    async fn evaluate_signal(&self, signal: &SignalEvent, account_equity: Decimal, current_position: Option<Decimal>) -> Result<Vec<OrderEvent>>;
}
