pub mod ma_crossover;
pub mod quad_ma;
pub mod risk_manager;

pub use ma_crossover::MaCrossoverStrategy;
pub use quad_ma::QuadMaStrategy;
pub use risk_manager::SimpleRiskManager;

use algo_trade_core::Strategy;
use anyhow::{Context, Result};
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
    _config: Option<String>,
) -> Result<Arc<Mutex<dyn Strategy>>> {
    match strategy_name {
        "ma_crossover" => {
            let strategy = MaCrossoverStrategy::new(symbol, 10, 30);
            Ok(Arc::new(Mutex::new(strategy)))
        }
        "quad_ma" => {
            let strategy = QuadMaStrategy::new(symbol);
            Ok(Arc::new(Mutex::new(strategy)))
        }
        _ => anyhow::bail!("Unknown strategy: '{strategy_name}'. Available: ma_crossover, quad_ma"),
    }
    .context("Failed to create strategy")
}
