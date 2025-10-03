use super::{BacktestResult, ParamConfig, StrategyType, TradeRecord};
use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
use algo_trade_core::{FillEvent, PerformanceMetrics, TradingSystem};
use algo_trade_data::CsvStorage;
use algo_trade_hyperliquid::HyperliquidClient;
use algo_trade_strategy::{MaCrossoverStrategy, QuadMaStrategy, SimpleRiskManager};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Run all backtests for given tokens and parameter configurations
///
/// `progress_callback`: (completed, total, `current_token`, `current_config`, `status_message`)
pub async fn run_all_backtests<F>(
    tokens: &[String],
    configs: &[ParamConfig],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: &str,
    mut progress_callback: F,
) -> Result<Vec<BacktestResult>>
where
    F: FnMut(usize, usize, &str, &str, Option<String>),
{
    let total = tokens.len() * configs.len();
    let mut results = Vec::with_capacity(total);
    let mut completed = 0;

    // Create cache directory
    std::fs::create_dir_all("cache")?;

    for token in tokens {
        for config in configs {
            progress_callback(completed, total, token, &config.name, None);

            // Fetch/load data
            let csv_path = get_cache_path(token, interval, start, end);
            if Path::new(&csv_path).exists() {
                progress_callback(completed, total, token, &config.name,
                    Some(format!("Using cached data for {token}")));
            } else {
                progress_callback(completed, total, token, &config.name,
                    Some(format!("Fetching data for {token}...")));
                fetch_and_cache_data(token, interval, start, end, &csv_path).await?;
                progress_callback(completed, total, token, &config.name,
                    Some(format!("Data cached for {token}")));
            }

            // Run backtest
            progress_callback(completed, total, token, &config.name,
                Some(format!("Running backtest: {token} - {}", config.name)));

            match run_single_backtest(token, config, &csv_path).await {
                Ok(metrics) => {
                    // Convert FillEvents to TradeRecords
                    let trades = metrics.fills.iter().map(convert_fill_to_trade).collect();

                    results.push(BacktestResult {
                        token: token.clone(),
                        config_name: config.name.clone(),
                        total_return: metrics.total_return,
                        sharpe_ratio: metrics.sharpe_ratio,
                        max_drawdown: metrics.max_drawdown,
                        num_trades: metrics.num_trades,
                        win_rate: metrics.win_rate,
                        trades,
                    });
                    progress_callback(completed, total, token, &config.name,
                        Some(format!("✓ Completed: {token} - {} ({} trades)",
                            config.name, metrics.num_trades)));
                }
                Err(e) => {
                    progress_callback(completed, total, token, &config.name,
                        Some(format!("✗ Failed: {token} - {}: {e}", config.name)));
                }
            }

            completed += 1;
        }
    }

    Ok(results)
}

fn get_cache_path(token: &str, interval: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    format!(
        "cache/{}_{}_{}_{}.csv",
        token,
        interval,
        start.format("%Y%m%d"),
        end.format("%Y%m%d")
    )
}

async fn fetch_and_cache_data(
    token: &str,
    interval: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    output_path: &str,
) -> Result<()> {
    tracing::info!("Fetching data for {} ({} interval)", token, interval);

    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());

    let client = HyperliquidClient::new(api_url);
    let records = client.fetch_candles(token, interval, start, end).await
        .with_context(|| format!("Failed to fetch candles for {token}"))?;

    if records.is_empty() {
        anyhow::bail!("No data returned for {token} in date range");
    }

    CsvStorage::write_ohlcv(output_path, &records)?;
    tracing::info!("Cached {} candles for {}", records.len(), token);

    Ok(())
}

async fn run_single_backtest(
    token: &str,
    config: &ParamConfig,
    csv_path: &str,
) -> Result<PerformanceMetrics> {
    // Load data
    let data_provider = HistoricalDataProvider::from_csv(csv_path)
        .with_context(|| format!("Failed to load CSV for {token}"))?;

    // Create execution handler
    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0);

    // Create strategy based on config
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> = match &config.strategy {
        StrategyType::MaCrossover { fast, slow } => {
            let strategy = MaCrossoverStrategy::new(token.to_string(), *fast, *slow);
            vec![Arc::new(Mutex::new(strategy))]
        }
        StrategyType::QuadMa { ma1, ma2, ma3, ma4 } => {
            let strategy = QuadMaStrategy::with_periods(
                token.to_string(),
                *ma1,
                *ma2,
                *ma3,
                *ma4,
            );
            vec![Arc::new(Mutex::new(strategy))]
        }
    };

    // Create risk manager
    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(10000.0, 0.1));

    // Create trading system
    let mut system = TradingSystem::new(
        data_provider,
        execution_handler,
        strategies,
        risk_manager,
    );

    // Run backtest
    system.run().await
}

/// Convert FillEvent to TradeRecord for UI display
fn convert_fill_to_trade(fill: &FillEvent) -> TradeRecord {
    use algo_trade_core::events::OrderDirection;

    let action = match fill.direction {
        OrderDirection::Buy => "BUY",
        OrderDirection::Sell => "SELL",
    };

    TradeRecord {
        timestamp: fill.timestamp,
        action: action.to_string(),
        price: fill.price,
        quantity: fill.quantity,
        commission: fill.commission,
    }
}
