use algo_trade_signals::trades::HugeTradesMonitor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let symbols = HugeTradesMonitor::default_symbols();
    let monitor = HugeTradesMonitor::new(symbols, 500_000.0);
    monitor.run().await
}
