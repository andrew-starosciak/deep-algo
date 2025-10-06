pub mod ma_crossover;
pub mod quad_ma;
pub mod risk_manager;

pub use ma_crossover::MaCrossoverStrategy;
pub use quad_ma::QuadMaStrategy;
pub use risk_manager::SimpleRiskManager;

use algo_trade_core::Strategy;
use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Creates a strategy from name and configuration
///
/// # Arguments
/// * `strategy_name` - Name of strategy ("ma_crossover", "quad_ma")
/// * `symbol` - Trading symbol
/// * `config` - Optional JSON configuration string
///
/// # Errors
/// Returns error if strategy name is unknown or config is invalid
pub fn create_strategy(
    strategy_name: &str,
    symbol: String,
    config: Option<String>,
) -> Result<Arc<Mutex<dyn Strategy>>> {
    let strategy: Arc<Mutex<dyn Strategy>> = match strategy_name {
        "ma_crossover" => {
            let (fast, slow) = if let Some(cfg) = config {
                let json: Value = serde_json::from_str(&cfg)
                    .context("Failed to parse ma_crossover config JSON")?;

                let fast = json.get("fast")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(10);

                let slow = json.get("slow")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(30);

                (fast, slow)
            } else {
                (10, 30)
            };

            let strategy = MaCrossoverStrategy::new(symbol, fast, slow);
            Arc::new(Mutex::new(strategy))
        }
        "quad_ma" => {
            let strategy = if let Some(cfg) = config {
                let json: Value = serde_json::from_str(&cfg)
                    .context("Failed to parse quad_ma config JSON")?;

                let ma1 = json.get("ma1").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(5);
                let ma2 = json.get("ma2").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(10);
                let ma3 = json.get("ma3").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(20);
                let ma4 = json.get("ma4").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(50);
                let trend_period = json.get("trend_period").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(100);
                let volume_factor = json.get("volume_factor").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(150);
                let take_profit = json.get("take_profit").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(200);
                let stop_loss = json.get("stop_loss").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(100);
                let reversal_confirmation_bars = json.get("reversal_confirmation_bars").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(3);

                // Convert percentage values to f64
                #[allow(clippy::cast_precision_loss)]
                let volume_factor_f64 = volume_factor as f64 / 100.0;
                #[allow(clippy::cast_precision_loss)]
                let take_profit_pct = take_profit as f64 / 10000.0;
                #[allow(clippy::cast_precision_loss)]
                let stop_loss_pct = stop_loss as f64 / 10000.0;

                QuadMaStrategy::with_full_config(
                    symbol,
                    ma1,
                    ma2,
                    ma3,
                    ma4,
                    trend_period,
                    true, // volume_filter_enabled
                    volume_factor_f64,
                    take_profit_pct,
                    stop_loss_pct,
                    reversal_confirmation_bars,
                )
            } else {
                QuadMaStrategy::new(symbol)
            };

            Arc::new(Mutex::new(strategy))
        }
        _ => anyhow::bail!("Unknown strategy: '{strategy_name}'. Available: ma_crossover, quad_ma"),
    };

    Ok(strategy)
}
