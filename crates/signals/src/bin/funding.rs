use algo_trade_signals::funding::FundingMonitor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let symbols = FundingMonitor::default_symbols();
    let monitor = FundingMonitor::new(symbols);
    monitor.run().await
}
