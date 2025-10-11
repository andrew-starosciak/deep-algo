use clap::{Parser, Subcommand};

mod tui_backtest;
mod tui_live_bot;

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
    /// Interactive TUI for multi-token parameter sweep backtests
    TuiBacktest {
        /// Start date for historical data (defaults to 60 days ago)
        #[arg(long)]
        start: Option<String>,
        /// End date for historical data (defaults to today)
        #[arg(long)]
        end: Option<String>,
        /// Candle interval (defaults to 1m)
        #[arg(long, default_value = "1m")]
        interval: String,
    },
    /// Run scheduled backtests (daemon mode)
    ScheduledBacktest {
        /// Config file path
        #[arg(short, long, default_value = "config/Config.toml")]
        config: String,
    },
    /// Run token selection once and display results
    TokenSelection {
        /// Config file path
        #[arg(short, long, default_value = "config/Config.toml")]
        config: String,
        /// Strategy name to filter results
        #[arg(short, long, default_value = "quad_ma")]
        strategy: String,
    },
    /// Interactive TUI for managing live trading bots
    LiveBotTui {
        /// Optional log file path (logs to file instead of stderr)
        #[arg(long)]
        log_file: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging (disabled for TUI to prevent screen corruption, unless log_file is provided)
    match &cli.command {
        Commands::LiveBotTui { log_file: Some(path) } => {
            // Log to file for TUI
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .with_writer(std::sync::Mutex::new(file))
                .init();
        }
        Commands::TuiBacktest { .. } | Commands::LiveBotTui { .. } => {
            // No logging for TUI (prevents screen corruption)
        }
        _ => {
            // Normal stderr logging for non-TUI commands
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();
        }
    }

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
        Commands::TuiBacktest { start, end, interval } => {
            run_tui_backtest(start.as_deref(), end.as_deref(), &interval).await?;
        }
        Commands::ScheduledBacktest { config } => {
            run_scheduled_backtest(&config).await?;
        }
        Commands::TokenSelection { config, strategy } => {
            run_token_selection(&config, &strategy).await?;
        }
        Commands::LiveBotTui { log_file: _ } => {
            tui_live_bot::run().await?;
        }
    }

    Ok(())
}

async fn run_trading_system(config_path: &str) -> anyhow::Result<()> {
    tracing::info!("Starting trading system daemon with config: {}", config_path);

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    // Initialize database for persistence
    let db_path = std::env::var("BOT_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/bots.db".to_string());

    tracing::info!("Initializing bot database at: {}", db_path);

    // Ensure parent directory exists for SQLite database
    if let Some(file_path) = db_path.strip_prefix("sqlite://") {
        let path = std::path::Path::new(file_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tracing::info!("Creating directory for SQLite database: {}", parent.display());
                std::fs::create_dir_all(parent)?;
                tracing::info!("Directory created successfully, checking permissions...");

                // Verify we can write to the directory
                let test_file = parent.join(".write_test");
                match std::fs::write(&test_file, "test") {
                    Ok(_) => {
                        std::fs::remove_file(&test_file)?;
                        tracing::info!("Directory is writable");
                    }
                    Err(e) => {
                        tracing::error!("Cannot write to directory {}: {}", parent.display(), e);
                        return Err(e.into());
                    }
                }
            }
        }
    }

    let database = std::sync::Arc::new(
        algo_trade_bot_orchestrator::BotDatabase::new(&db_path).await?
    );

    // Create bot registry with persistence
    let registry = std::sync::Arc::new(
        algo_trade_bot_orchestrator::BotRegistry::with_database(database)
    );

    // Restore bots from database
    match registry.restore_from_db().await {
        Ok(restored) => {
            if restored.is_empty() {
                tracing::info!("No bots to restore from database");
            } else {
                tracing::info!("Restored {} bot(s) from database: {:?}", restored.len(), restored);
            }
        }
        Err(e) => {
            tracing::error!("Failed to restore bots from database: {}", e);
        }
    }

    // Start web API server
    let server = algo_trade_web_api::ApiServer::new(registry.clone());
    let addr = format!("{}:{}", config.server.host, config.server.port);

    tracing::info!("Web API listening on {}", addr);

    // Spawn server in background task
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.serve(&addr).await {
            tracing::error!("Server error: {}", e);
        }
    });

    // Wait for shutdown signal (SIGINT or SIGTERM)
    let shutdown_signal = async {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        ).expect("Failed to create SIGTERM handler");

        let mut sigint = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::interrupt()
        ).expect("Failed to create SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, initiating graceful shutdown");
            }
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
            }
        }
    };

    // Wait for shutdown signal
    shutdown_signal.await;

    // Graceful shutdown
    tracing::info!("Shutting down all bots...");
    if let Err(e) = registry.shutdown_all().await {
        tracing::error!("Error during bot shutdown: {}", e);
    }

    // Abort server task
    server_handle.abort();

    tracing::info!("Trading system daemon stopped");
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
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
        _ => anyhow::bail!("Unknown strategy: '{strategy_name}'. Available: ma_crossover, quad_ma"),
    };

    // Create risk manager with leverage-aware position sizing
    // Risk 5% of equity per trade, max 20% in any single position, 1x leverage (conservative)
    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(0.05, 0.20, 1));

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

#[allow(clippy::cognitive_complexity)]
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
        anyhow::bail!("No data fetched for {symbol} {interval}");
    }

    tracing::info!("Fetched {} candles, writing to {}", records.len(), output_path);

    // Write to CSV
    CsvStorage::write_ohlcv(output_path, &records)?;

    tracing::info!("✅ Successfully wrote {} candles to {}", records.len(), output_path);
    tracing::info!("You can now run: algo-trade backtest --data {} --strategy <strategy>", output_path);

    Ok(())
}

async fn run_tui_backtest(
    start_opt: Option<&str>,
    end_opt: Option<&str>,
    interval: &str,
) -> anyhow::Result<()> {
    use chrono::{DateTime, Duration, Utc};
    use anyhow::Context;

    // Parse or default dates
    let end: DateTime<Utc> = if let Some(end_str) = end_opt {
        end_str.parse()
            .context("Invalid end time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?
    } else {
        Utc::now()
    };

    let start: DateTime<Utc> = if let Some(start_str) = start_opt {
        start_str.parse()
            .context("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?
    } else {
        end - Duration::days(3) // Default: 3 days before end
    };

    if start >= end {
        anyhow::bail!("Start time must be before end time");
    }

    tracing::info!("Starting TUI backtest wizard");
    tracing::info!("Date range: {} to {}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d"));
    tracing::info!("Interval: {}", interval);

    // Run TUI application
    tui_backtest::run(start, end, interval.to_string()).await
}

async fn run_scheduled_backtest(_config_path: &str) -> anyhow::Result<()> {
    use algo_trade_backtest_scheduler::BacktestScheduler;
    use algo_trade_data::DatabaseClient;
    use std::sync::Arc;

    tracing::info!("Starting scheduled backtest daemon");

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    // Create database client
    let db_client = Arc::new(DatabaseClient::new(&config.database.url).await?);

    let cron_schedule = config.backtest_scheduler.cron_schedule.clone();

    // Create and start scheduler
    let scheduler = BacktestScheduler::new(config.backtest_scheduler, db_client);

    tracing::info!("Scheduler started. Running according to cron schedule: {}", cron_schedule);
    tracing::info!("Press Ctrl+C to stop");

    // This will run forever according to the cron schedule
    scheduler.start().await?;

    Ok(())
}

async fn run_token_selection(_config_path: &str, strategy_name: &str) -> anyhow::Result<()> {
    use algo_trade_token_selector::TokenSelector;
    use algo_trade_data::DatabaseClient;
    use std::sync::Arc;

    tracing::info!("Running token selection for strategy: {}", strategy_name);

    // Load config
    let config = algo_trade_core::ConfigLoader::load()?;

    // Create database client
    let db_client = Arc::new(DatabaseClient::new(&config.database.url).await?);

    // Create token selector
    let selector = TokenSelector::new(config.token_selector.clone(), db_client);

    // Get selection details
    let results = selector.get_selection_details(strategy_name).await?;

    // Display results
    println!("\n{}", "=".repeat(100));
    println!("Token Selection Results - Strategy: {}", strategy_name);
    println!("{}", "=".repeat(100));
    println!("{:<10} {:>12} {:>10} {:>12} {:>10} {:>15} {:>10}",
             "Symbol", "Sharpe", "Win Rate", "Max DD", "Trades", "Total PnL", "Approved");
    println!("{}", "-".repeat(100));

    for result in &results {
        let approved_mark = if result.approved { "✓" } else { "✗" };
        println!("{:<10} {:>12.2} {:>9.1}% {:>11.1}% {:>10} {:>15} {:>10}",
                 result.symbol,
                 result.sharpe_ratio,
                 result.win_rate * 100.0,
                 result.max_drawdown.to_string().parse::<f64>().unwrap_or(0.0) * 100.0,
                 result.num_trades,
                 result.total_pnl,
                 approved_mark);
    }

    println!("{}", "=".repeat(100));

    let approved_count = results.iter().filter(|r| r.approved).count();
    println!("\nApproved: {}/{} tokens", approved_count, results.len());
    println!("\nCriteria:");
    println!("  - Min Sharpe Ratio: {}", config.token_selector.min_sharpe_ratio);
    println!("  - Min Win Rate: {}%", config.token_selector.min_win_rate * 100.0);
    println!("  - Max Drawdown: {}%", config.token_selector.max_drawdown * 100.0);
    println!("  - Min Trades: {}", config.token_selector.min_num_trades);
    println!();

    Ok(())
}
