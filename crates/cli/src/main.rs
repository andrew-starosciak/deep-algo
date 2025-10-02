use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "algo-trade")]
#[command(about = "Algorithmic trading system for Hyperliquid", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the trading system with web API
    Run {
        /// Config file path
        #[arg(short, long, default_value = "config/Config.toml")]
        config: String,
    },
    /// Run a backtest
    Backtest {
        /// Historical data CSV file
        #[arg(short, long)]
        data: String,
        /// Strategy to use
        #[arg(short, long)]
        strategy: String,
    },
    /// Start the web API server
    Server {
        /// Server address
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        addr: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { config } => {
            run_trading_system(&config).await?;
        }
        Commands::Backtest { data, strategy } => {
            run_backtest(&data, &strategy).await?;
        }
        Commands::Server { addr } => {
            run_server(&addr).await?;
        }
    }

    Ok(())
}

async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    tracing::info!("Starting trading system with config: {}", config_path);

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    // Create bot registry
    let registry = std::sync::Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());

    // Start web API
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);

    tracing::info!("Web API listening on {}", addr);
    server.serve(&addr).await?;

    Ok(())
}

async fn run_backtest(data_path: &str, strategy: &str) -> anyhow::Result<()> {
    use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
    use algo_trade_core::TradingSystem;
    use algo_trade_strategy::{MaCrossoverStrategy, SimpleRiskManager};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    tracing::info!("Running backtest with data: {}, strategy: {}", data_path, strategy);

    // Load historical data
    let data_provider = HistoricalDataProvider::from_csv(data_path)?;

    // Create simulated execution handler
    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0); // 0.1% commission, 5 bps slippage

    // Create strategy
    let ma_strategy = MaCrossoverStrategy::new("BTC".to_string(), 10, 30);
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> = vec![
        Arc::new(Mutex::new(ma_strategy))
    ];

    // Create risk manager
    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(1000.0, 0.1));

    // Create trading system
    let mut system = TradingSystem::new(
        data_provider,
        execution_handler,
        strategies,
        risk_manager,
    );

    // Run backtest
    system.run().await?;

    tracing::info!("Backtest completed");

    Ok(())
}

async fn run_server(addr: &str) -> anyhow::Result<()> {
    tracing::info!("Starting web API server on {}", addr);

    let registry = std::sync::Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());
    let server = algo_trade_web_api::ApiServer::new(registry);

    server.serve(addr).await?;

    Ok(())
}
