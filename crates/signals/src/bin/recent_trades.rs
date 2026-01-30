use algo_trade_signals::trades::RecentTradesMonitor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let symbols = RecentTradesMonitor::default_symbols();
    let monitor = RecentTradesMonitor::new(symbols, "binance_trades.csv", 15_000.0);
    monitor.run().await
}
