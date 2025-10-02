use crate::traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    data_provider: D,
    execution_handler: E,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
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
        Self {
            data_provider,
            execution_handler,
            strategies,
            risk_manager,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while let Some(market_event) = self.data_provider.next_event().await? {
            // Generate signals from all strategies
            for strategy in &self.strategies {
                let mut strategy = strategy.lock().await;
                if let Some(signal) = strategy.on_market_event(&market_event).await? {
                    // Risk management evaluation
                    if let Some(order) = self.risk_manager.evaluate_signal(&signal).await? {
                        // Execute order
                        let fill = self.execution_handler.execute_order(order).await?;
                        tracing::info!("Order filled: {:?}", fill);
                    }
                }
            }
        }
        Ok(())
    }
}
