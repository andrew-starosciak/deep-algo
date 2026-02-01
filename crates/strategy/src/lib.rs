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

// Re-export bridge types when feature is enabled
#[cfg(feature = "microstructure-bridge")]
pub use algo_trade_signals::bridge::{
    CachedMicroSignals, EnhancedStrategy, MicrostructureFilter, MicrostructureFilterConfig,
    MicrostructureOrchestrator, OrchestratorCommand, SharedMicroSignals,
};

/// Configuration for microstructure bridge integration
#[cfg(feature = "microstructure-bridge")]
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Shared signal cache (created externally by orchestrator)
    pub signals: SharedMicroSignals,
    /// Filter configuration
    pub filter_config: MicrostructureFilterConfig,
}

/// Creates a strategy from name and configuration
///
/// # Arguments
/// * `strategy_name` - Name of strategy (`ma_crossover`, `quad_ma`)
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

                let fast = json
                    .get("fast")
                    .and_then(Value::as_u64)
                    .map_or(10, |v| usize::try_from(v).unwrap_or(10));

                let slow = json
                    .get("slow")
                    .and_then(Value::as_u64)
                    .map_or(30, |v| usize::try_from(v).unwrap_or(30));

                (fast, slow)
            } else {
                (10, 30)
            };

            let strategy = MaCrossoverStrategy::new(symbol, fast, slow);
            Arc::new(Mutex::new(strategy))
        }
        "quad_ma" => {
            let strategy = if let Some(cfg) = config {
                let json: Value =
                    serde_json::from_str(&cfg).context("Failed to parse quad_ma config JSON")?;

                let ma1 = json
                    .get("ma1")
                    .and_then(Value::as_u64)
                    .map_or(5, |v| usize::try_from(v).unwrap_or(5));
                let ma2 = json
                    .get("ma2")
                    .and_then(Value::as_u64)
                    .map_or(10, |v| usize::try_from(v).unwrap_or(10));
                let ma3 = json
                    .get("ma3")
                    .and_then(Value::as_u64)
                    .map_or(20, |v| usize::try_from(v).unwrap_or(20));
                let ma4 = json
                    .get("ma4")
                    .and_then(Value::as_u64)
                    .map_or(50, |v| usize::try_from(v).unwrap_or(50));
                let trend_period = json
                    .get("trend_period")
                    .and_then(Value::as_u64)
                    .map_or(100, |v| usize::try_from(v).unwrap_or(100));
                let volume_factor = json
                    .get("volume_factor")
                    .and_then(Value::as_u64)
                    .map_or(150, |v| usize::try_from(v).unwrap_or(150));
                let take_profit = json
                    .get("take_profit")
                    .and_then(Value::as_u64)
                    .map_or(200, |v| usize::try_from(v).unwrap_or(200));
                let stop_loss = json
                    .get("stop_loss")
                    .and_then(Value::as_u64)
                    .map_or(100, |v| usize::try_from(v).unwrap_or(100));
                let reversal_confirmation_bars = json
                    .get("reversal_confirmation_bars")
                    .and_then(Value::as_u64)
                    .map_or(3, |v| usize::try_from(v).unwrap_or(3));

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

/// Creates a strategy with optional microstructure bridge wrapping
///
/// When `bridge_config` is provided, wraps the base strategy with `EnhancedStrategy`
/// which applies microstructure filtering (entry filter, exit triggers, sizing adjustment).
///
/// # Arguments
/// * `strategy_name` - Name of strategy (`ma_crossover`, `quad_ma`)
/// * `symbol` - Trading symbol
/// * `config` - Optional JSON configuration string for base strategy
/// * `bridge_config` - Optional bridge configuration for microstructure filtering
///
/// # Errors
/// Returns error if strategy name is unknown or config is invalid
#[cfg(feature = "microstructure-bridge")]
pub fn create_strategy_with_bridge(
    strategy_name: &str,
    symbol: String,
    config: Option<String>,
    bridge_config: Option<BridgeConfig>,
) -> Result<Arc<Mutex<dyn Strategy>>> {
    // Create base strategy based on name
    match strategy_name {
        "ma_crossover" => {
            let (fast, slow) = parse_ma_crossover_config(config)?;
            let base_strategy = MaCrossoverStrategy::new(symbol, fast, slow);

            if let Some(bridge) = bridge_config {
                let enhanced = EnhancedStrategy::new(base_strategy, bridge.signals, bridge.filter_config);
                Ok(Arc::new(Mutex::new(enhanced)))
            } else {
                Ok(Arc::new(Mutex::new(base_strategy)))
            }
        }
        "quad_ma" => {
            let base_strategy = parse_and_create_quad_ma(symbol, config)?;

            if let Some(bridge) = bridge_config {
                let enhanced = EnhancedStrategy::new(base_strategy, bridge.signals, bridge.filter_config);
                Ok(Arc::new(Mutex::new(enhanced)))
            } else {
                Ok(Arc::new(Mutex::new(base_strategy)))
            }
        }
        _ => anyhow::bail!("Unknown strategy: '{strategy_name}'. Available: ma_crossover, quad_ma"),
    }
}

/// Parse MA crossover config from JSON
fn parse_ma_crossover_config(config: Option<String>) -> Result<(usize, usize)> {
    if let Some(cfg) = config {
        let json: Value =
            serde_json::from_str(&cfg).context("Failed to parse ma_crossover config JSON")?;

        let fast = json
            .get("fast")
            .and_then(Value::as_u64)
            .map_or(10, |v| usize::try_from(v).unwrap_or(10));

        let slow = json
            .get("slow")
            .and_then(Value::as_u64)
            .map_or(30, |v| usize::try_from(v).unwrap_or(30));

        Ok((fast, slow))
    } else {
        Ok((10, 30))
    }
}

/// Parse and create QuadMA strategy from config
fn parse_and_create_quad_ma(symbol: String, config: Option<String>) -> Result<QuadMaStrategy> {
    if let Some(cfg) = config {
        let json: Value =
            serde_json::from_str(&cfg).context("Failed to parse quad_ma config JSON")?;

        let ma1 = json
            .get("ma1")
            .and_then(Value::as_u64)
            .map_or(5, |v| usize::try_from(v).unwrap_or(5));
        let ma2 = json
            .get("ma2")
            .and_then(Value::as_u64)
            .map_or(10, |v| usize::try_from(v).unwrap_or(10));
        let ma3 = json
            .get("ma3")
            .and_then(Value::as_u64)
            .map_or(20, |v| usize::try_from(v).unwrap_or(20));
        let ma4 = json
            .get("ma4")
            .and_then(Value::as_u64)
            .map_or(50, |v| usize::try_from(v).unwrap_or(50));
        let trend_period = json
            .get("trend_period")
            .and_then(Value::as_u64)
            .map_or(100, |v| usize::try_from(v).unwrap_or(100));
        let volume_factor = json
            .get("volume_factor")
            .and_then(Value::as_u64)
            .map_or(150, |v| usize::try_from(v).unwrap_or(150));
        let take_profit = json
            .get("take_profit")
            .and_then(Value::as_u64)
            .map_or(200, |v| usize::try_from(v).unwrap_or(200));
        let stop_loss = json
            .get("stop_loss")
            .and_then(Value::as_u64)
            .map_or(100, |v| usize::try_from(v).unwrap_or(100));
        let reversal_confirmation_bars = json
            .get("reversal_confirmation_bars")
            .and_then(Value::as_u64)
            .map_or(3, |v| usize::try_from(v).unwrap_or(3));

        // Convert percentage values to f64
        #[allow(clippy::cast_precision_loss)]
        let volume_factor_f64 = volume_factor as f64 / 100.0;
        #[allow(clippy::cast_precision_loss)]
        let take_profit_pct = take_profit as f64 / 10000.0;
        #[allow(clippy::cast_precision_loss)]
        let stop_loss_pct = stop_loss as f64 / 10000.0;

        Ok(QuadMaStrategy::with_full_config(
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
        ))
    } else {
        Ok(QuadMaStrategy::new(symbol))
    }
}
