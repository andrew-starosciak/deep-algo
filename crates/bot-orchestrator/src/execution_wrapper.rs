use algo_trade_core::events::{FillEvent, OrderEvent};
use algo_trade_core::traits::ExecutionHandler;
use algo_trade_hyperliquid::{LiveExecutionHandler, PaperTradingExecutionHandler};
use anyhow::Result;
use async_trait::async_trait;

/// Type-safe wrapper for execution handlers supporting both live and paper trading modes.
///
/// This enum provides compile-time safety for execution mode switching without trait object
/// overhead. Each variant wraps a concrete handler type, enabling zero-cost abstraction.
///
/// # Modes
///
/// - **Live**: Real trading with `LiveExecutionHandler` (requires wallet, makes API calls)
/// - **Paper**: Simulated trading with `PaperTradingExecutionHandler` (no API calls, safe testing)
///
/// # Safety
///
/// Physical type separation prevents accidental live trading when configured for paper mode.
/// The enum cannot be constructed incorrectly - you either have a Live variant with real
/// authentication or a Paper variant with no auth.
pub enum ExecutionHandlerWrapper {
    /// Live trading mode - executes real orders on exchange
    Live(Box<LiveExecutionHandler>),
    /// Paper trading mode - simulates fills locally (zero API calls)
    Paper(PaperTradingExecutionHandler),
}

#[async_trait]
impl ExecutionHandler for ExecutionHandlerWrapper {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        match self {
            Self::Live(handler) => handler.execute_order(order).await,
            Self::Paper(handler) => handler.execute_order(order).await,
        }
    }
}
