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
    /// Fetch historical OHLCV data from Hyperliquid
    FetchData {
        /// Symbol/coin to fetch (e.g., "BTC", "ETH")
        #[arg(long)]
        symbol: String,
        /// Candle interval (1m, 5m, 15m, 1h, 4h, 1d, etc.)
        #[arg(long)]
        interval: String,
        /// Start time in ISO 8601 format (e.g., "2025-01-01T00:00:00Z")
        #[arg(long)]
        start: String,
        /// End time in ISO 8601 format (e.g., "2025-02-01T00:00:00Z")
        #[arg(long)]
        end: String,
        /// Output CSV file path
        #[arg(short, long)]
        output: String,
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
        Commands::FetchData { symbol, interval, start, end, output } => {
            run_fetch_data(&symbol, &interval, &start, &end, &output).await?;
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

async fn run_backtest(data_path: &str, strategy_name: &str) -> anyhow::Result<()> {
    use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
    use algo_trade_core::{MetricsFormatter, TradingSystem};
    use algo_trade_strategy::{MaCrossoverStrategy, QuadMaStrategy, SimpleRiskManager};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    tracing::info!("Running backtest with data: {}, strategy: {}", data_path, strategy_name);

    // Extract symbol from CSV first row
    let symbol = extract_symbol_from_csv(data_path)?;
    tracing::info!("Detected symbol from CSV: {}", symbol);

    // Load historical data
    let data_provider = HistoricalDataProvider::from_csv(data_path)?;

    // Create simulated execution handler
    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0); // 0.1% commission, 5 bps slippage

    // Create strategy based on user selection
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> = match strategy_name {
        "ma_crossover" => {
            tracing::info!("Using MA Crossover strategy (10/30 periods)");
            let ma_strategy = MaCrossoverStrategy::new(symbol, 10, 30);
            vec![Arc::new(Mutex::new(ma_strategy))]
        }
        "quad_ma" => {
            tracing::info!("Using Quad MA strategy (5/8/13/21 Fibonacci periods)");
            let quad_strategy = QuadMaStrategy::new(symbol);
            vec![Arc::new(Mutex::new(quad_strategy))]
        }
        _ => anyhow::bail!("Unknown strategy: '{}'. Available: ma_crossover, quad_ma", strategy_name),
    };

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

    // Run backtest and get metrics
    let metrics = system.run().await?;

    // Display formatted metrics
    println!("{}", MetricsFormatter::format(&metrics));

    Ok(())
}

fn extract_symbol_from_csv(path: &str) -> anyhow::Result<String> {
    use anyhow::Context;

    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("Failed to open CSV file: {path}"))?;

    let mut records = reader.records();
    if let Some(result) = records.next() {
        let record = result.context("Failed to read first CSV record")?;
        if record.len() >= 2 {
            return Ok(record[1].to_string()); // symbol is column index 1
        }
    }

    anyhow::bail!("CSV file is empty or missing symbol column")
}

async fn run_server(addr: &str) -> anyhow::Result<()> {
    tracing::info!("Starting web API server on {}", addr);

    let registry = std::sync::Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());
    let server = algo_trade_web_api::ApiServer::new(registry);

    server.serve(addr).await?;

    Ok(())
}

async fn run_fetch_data(
    symbol: &str,
    interval: &str,
    start_str: &str,
    end_str: &str,
    output_path: &str,
) -> anyhow::Result<()> {
    use algo_trade_hyperliquid::HyperliquidClient;
    use algo_trade_data::CsvStorage;
    use chrono::{DateTime, Utc};
    use anyhow::Context;

    tracing::info!("Fetching OHLCV data for {} ({} interval)", symbol, interval);

    // Parse timestamps
    let start: DateTime<Utc> = start_str.parse()
        .context("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?;
    let end: DateTime<Utc> = end_str.parse()
        .context("Invalid end time. Use ISO 8601 format (e.g., 2025-02-01T00:00:00Z)")?;

    if start >= end {
        anyhow::bail!("Start time must be before end time");
    }

    // Create client (no auth needed for public candle data)
    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());

    let client = HyperliquidClient::new(api_url);

    // Fetch candles
    let records = client.fetch_candles(symbol, interval, start, end).await?;

    if records.is_empty() {
        tracing::warn!("No candle data returned. Symbol may not exist or date range may be invalid.");
        anyhow::bail!("No data fetched for {} {}", symbol, interval);
    }

    tracing::info!("Fetched {} candles, writing to {}", records.len(), output_path);

    // Write to CSV
    CsvStorage::write_ohlcv(output_path, &records)?;

    tracing::info!("✅ Successfully wrote {} candles to {}", records.len(), output_path);
    tracing::info!("You can now run: algo-trade backtest --data {} --strategy <strategy>", output_path);

    Ok(())
}
