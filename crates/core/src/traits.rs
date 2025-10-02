use crate::events::{FillEvent, MarketEvent, OrderEvent, SignalEvent};
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DataProvider: Send + Sync {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>>;
}

#[async_trait]
pub trait Strategy: Send + Sync {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>>;
    fn name(&self) -> &str;
}

#[async_trait]
pub trait ExecutionHandler: Send + Sync {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent>;
}

#[async_trait]
pub trait RiskManager: Send + Sync {
    async fn evaluate_signal(&self, signal: &SignalEvent) -> Result<Option<OrderEvent>>;
}
