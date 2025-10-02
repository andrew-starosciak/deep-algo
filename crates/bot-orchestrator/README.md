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
use algo_trade_bot_orchestrator::{BotRegistry, BotConfig};

let registry = BotRegistry::new();
let config = BotConfig {
    bot_id: "bot1".to_string(),
    symbol: "BTC-USD".to_string(),
    strategy: "momentum".to_string(),
    enabled: true,
};

let handle = registry.spawn_bot(config).await?;
handle.start().await?;
```
