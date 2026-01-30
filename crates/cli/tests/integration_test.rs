use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
use algo_trade_core::TradingSystem;
use algo_trade_strategy::{MaCrossoverStrategy, SimpleRiskManager};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn test_backtest_ma_crossover() {
    // This test requires a sample CSV file
    // Skip if file doesn't exist
    if !std::path::Path::new("tests/data/sample.csv").exists() {
        return;
    }

    let data_provider = HistoricalDataProvider::from_csv("tests/data/sample.csv")
        .expect("Failed to load test data");

    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0);

    let strategy = MaCrossoverStrategy::new("BTC".to_string(), 5, 15);
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> =
        vec![Arc::new(Mutex::new(strategy))];

    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(0.05, 0.20, 1));

    let mut system = TradingSystem::new(data_provider, execution_handler, strategies, risk_manager);

    // Should run without errors
    system.run().await.expect("Backtest failed");
}
