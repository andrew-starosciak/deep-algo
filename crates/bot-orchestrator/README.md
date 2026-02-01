# Bot Orchestrator

Multi-bot orchestration using actor pattern with Tokio channels.

## Features

- Actor-based bot lifecycle management
- Command system for bot control (Start, Stop, Pause, Resume)
- Bot registry for managing multiple bots
- Graceful shutdown support
- Thread-safe concurrent access using RwLock

## Usage

```rust
use algo_trade_bot_orchestrator::{BotRegistry, BotConfig, ExecutionMode, MarginMode};
use rust_decimal::Decimal;

let registry = BotRegistry::new();
let config = BotConfig {
    bot_id: "bot1".to_string(),
    symbol: "BTC-USD".to_string(),
    strategy: "ma_crossover".to_string(),
    enabled: true,
    interval: "1m".to_string(),
    ws_url: "wss://api.hyperliquid.xyz/ws".to_string(),
    api_url: "https://api.hyperliquid.xyz".to_string(),
    warmup_periods: 100,
    strategy_config: None,
    initial_capital: Decimal::from(10000),
    risk_per_trade_pct: 0.05,
    max_position_pct: 0.20,
    leverage: 1,
    margin_mode: MarginMode::Cross,
    execution_mode: ExecutionMode::Paper,
    paper_slippage_bps: 10.0,
    paper_commission_rate: 0.00025,
    wallet: None,
    // Microstructure bridge (optional)
    microstructure_enabled: false,
    microstructure_entry_filter_threshold: 0.6,
    microstructure_exit_liquidation_threshold: 0.8,
    microstructure_exit_funding_threshold: 0.9,
    microstructure_stress_size_multiplier: 0.5,
    microstructure_entry_timing_enabled: false,
    microstructure_timing_support_threshold: 0.3,
};

let handle = registry.spawn_bot(config).await?;
handle.start().await?;
```
