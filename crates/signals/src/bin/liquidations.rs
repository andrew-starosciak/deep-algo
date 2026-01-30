use algo_trade_signals::liquidations::LiquidationMonitor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let monitor = LiquidationMonitor::default_all();
    monitor.run().await
}
