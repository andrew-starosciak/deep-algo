use algo_trade_core::config::BacktestSchedulerConfig;
use algo_trade_data::{BacktestResultRecord, DatabaseClient};
use algo_trade_hyperliquid::HyperliquidClient;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};

pub struct BacktestScheduler {
    config: BacktestSchedulerConfig,
    db_client: Arc<DatabaseClient>,
}

impl BacktestScheduler {
    /// Creates a new backtest scheduler.
    #[must_use]
    pub fn new(config: BacktestSchedulerConfig, db_client: Arc<DatabaseClient>) -> Self {
        Self { config, db_client }
    }

    /// Starts the scheduler and runs according to the cron schedule.
    ///
    /// # Errors
    /// Returns an error if the scheduler fails to start or if job scheduling fails.
    pub async fn start(self) -> Result<()> {
        if !self.config.enabled {
            info!("Backtest scheduler is disabled");
            return Ok(());
        }

        info!(
            "Starting backtest scheduler with cron: {}",
            self.config.cron_schedule
        );

        let scheduler = JobScheduler::new().await?;
        let config = self.config.clone();
        let db_client = self.db_client.clone();
        let cron_schedule = config.cron_schedule.clone();

        let job = Job::new_async(cron_schedule.as_str(), move |_uuid, _lock| {
            let config = config.clone();
            let db_client = db_client.clone();
            Box::pin(async move {
                if let Err(e) = run_backtest_batch(config, db_client).await {
                    error!("Backtest batch failed: {}", e);
                }
            })
        })?;

        scheduler.add(job).await?;
        scheduler.start().await?;

        info!("Backtest scheduler started successfully");

        // Keep scheduler running
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    }

    /// Runs backtests manually (one-time execution).
    ///
    /// # Errors
    /// Returns an error if backtest execution or database storage fails.
    pub async fn run_once(&self) -> Result<Vec<BacktestResultRecord>> {
        run_backtest_batch(self.config.clone(), self.db_client.clone()).await
    }
}

async fn run_backtest_batch(
    config: BacktestSchedulerConfig,
    db_client: Arc<DatabaseClient>,
) -> Result<Vec<BacktestResultRecord>> {
    // Fetch token universe (either from exchange or config)
    let token_universe = fetch_token_universe(&config).await?;

    info!(
        "Running backtest batch for {} tokens from {}",
        token_universe.len(),
        if config.fetch_universe_from_exchange { "Hyperliquid API" } else { "config" }
    );

    let end_time = Utc::now();
    let start_time = end_time - Duration::days(config.backtest_window_days);
    let mut results = Vec::new();

    for symbol in &token_universe {
        match run_single_backtest(
            symbol,
            &config.exchange,
            &config.strategy_name,
            start_time,
            end_time,
            db_client.clone(),
        )
        .await
        {
            Ok(record) => {
                info!(
                    "Backtest completed for {}: Sharpe={:.2}, PnL={}, Trades={}",
                    symbol, record.sharpe_ratio, record.total_pnl, record.num_trades
                );
                results.push(record);
            }
            Err(e) => {
                error!("Backtest failed for {}: {}", symbol, e);
            }
        }
    }

    // Store results in batch
    if !results.is_empty() {
        db_client
            .insert_backtest_results_batch(results.clone())
            .await
            .context("Failed to store backtest results")?;
        info!("Stored {} backtest results in database", results.len());
    }

    Ok(results)
}

async fn fetch_token_universe(config: &BacktestSchedulerConfig) -> Result<Vec<String>> {
    if config.fetch_universe_from_exchange {
        info!("Fetching token universe from Hyperliquid exchange");
        let client = HyperliquidClient::new(config.hyperliquid_api_url.clone());

        match client.fetch_available_symbols().await {
            Ok(symbols) => {
                info!("Fetched {} symbols from Hyperliquid", symbols.len());
                Ok(symbols)
            }
            Err(e) => {
                warn!("Failed to fetch symbols from exchange: {}. Falling back to config.", e);

                // Fallback to config if available
                if let Some(ref token_universe) = config.token_universe {
                    Ok(token_universe.clone())
                } else {
                    anyhow::bail!("Failed to fetch from exchange and no fallback token_universe in config: {}", e)
                }
            }
        }
    } else {
        // Use token list from config
        config.token_universe.clone()
            .ok_or_else(|| anyhow::anyhow!("token_universe is required when fetch_universe_from_exchange = false"))
    }
}

async fn run_single_backtest(
    symbol: &str,
    exchange: &str,
    strategy_name: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    db_client: Arc<DatabaseClient>,
) -> Result<BacktestResultRecord> {
    // Fetch historical data from database
    let ohlcv_data = db_client
        .query_ohlcv(symbol, start_time, end_time)
        .await
        .context("Failed to fetch OHLCV data")?;

    if ohlcv_data.is_empty() {
        anyhow::bail!("No OHLCV data found for {}", symbol);
    }

    // For MVP: Calculate simple metrics directly from OHLCV data
    // In production, you would:
    // 1. Convert OHLCV to MarketEvent::Bar variants
    // 2. Create a BacktestDataProvider with those events
    // 3. Instantiate the actual strategy (quad_ma, etc.)
    // 4. Run the full backtest engine
    // 5. Get metrics from the engine

    // Calculate simple metrics from price data
    let metrics = calculate_simple_metrics(&ohlcv_data)?;

    // Calculate total PnL (for MVP, use simplified calculation)
    let initial_price = ohlcv_data.first().unwrap().close;
    let final_price = ohlcv_data.last().unwrap().close;
    let price_change = final_price - initial_price;
    let total_pnl = price_change; // Simplified: assumes 1 unit position

    // Convert metrics to database record
    let record = BacktestResultRecord {
        timestamp: Utc::now(),
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        strategy_name: strategy_name.to_string(),
        sharpe_ratio: metrics.sharpe_ratio,
        sortino_ratio: None,
        total_pnl,
        total_return: metrics.total_return,
        win_rate: metrics.win_rate,
        max_drawdown: metrics.max_drawdown,
        num_trades: i32::try_from(metrics.num_trades)?,
        parameters: None,
    };

    Ok(record)
}

fn calculate_simple_metrics(
    ohlcv_data: &[algo_trade_data::OhlcvRecord],
) -> Result<SimplifiedMetrics> {
    if ohlcv_data.is_empty() {
        anyhow::bail!("Cannot calculate metrics from empty data");
    }

    let prices: Vec<Decimal> = ohlcv_data.iter().map(|r| r.close).collect();
    let returns: Vec<f64> = prices
        .windows(2)
        .map(|w| {
            let ret = (w[1] - w[0]) / w[0];
            ret.to_string().parse::<f64>().unwrap_or(0.0)
        })
        .collect();

    // Calculate mean and std dev
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance: f64 = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
    let std_dev = variance.sqrt();

    // Calculate Sharpe ratio (annualized, assuming daily data)
    let sharpe_ratio = if std_dev > 0.0 {
        mean / std_dev * (252.0_f64).sqrt()
    } else {
        0.0
    };

    // Calculate total return
    let total_return = (prices.last().unwrap() - prices.first().unwrap()) / prices.first().unwrap();

    // Calculate max drawdown
    let mut max_drawdown = Decimal::ZERO;
    let mut peak = prices[0];
    for &price in &prices {
        if price > peak {
            peak = price;
        }
        let drawdown = (peak - price) / peak;
        if drawdown > max_drawdown {
            max_drawdown = drawdown;
        }
    }

    // Simple win rate calculation (count positive vs negative daily returns)
    let wins = returns.iter().filter(|&&r| r > 0.0).count();
    let losses = returns.iter().filter(|&&r| r < 0.0).count();
    let total_trades = wins + losses;
    let win_rate = if total_trades > 0 {
        wins as f64 / total_trades as f64
    } else {
        0.0
    };

    Ok(SimplifiedMetrics {
        sharpe_ratio,
        total_return,
        max_drawdown,
        win_rate,
        num_trades: total_trades,
    })
}

struct SimplifiedMetrics {
    sharpe_ratio: f64,
    total_return: Decimal,
    max_drawdown: Decimal,
    win_rate: f64,
    num_trades: usize,
}
