# Playbook: Hyperliquid Algorithmic Trading System - Modular Architecture

## User Request
> "I want to generate a plan after researching algo trading with rust. We want to take the key elements from the research and create a plan to create a fully modular approach to it's implementation."

**Research Source**: Comprehensive technical blueprint covering Hyperliquid integration, Rust backtesting frameworks, event-driven architecture, and production-ready tech stack.

## Executive Summary

Build a production-grade algorithmic trading system for Hyperliquid in Rust with full modularity, enabling:
- **Backtest-Live Parity**: Identical strategy code runs in backtesting and live trading
- **Event-Driven Architecture**: Eliminates look-ahead bias, matches real-time trading exactly
- **Multi-Token Support**: Scrape and trade hundreds of tokens concurrently
- **Hot-Reload Config**: Update bot parameters without restart
- **Real-Time Control**: Web interface with WebSocket for live monitoring and control

## System Architecture Overview

### Core Design Principles
1. **Event-Driven from Day One** - Not vectorized backtesting retrofitted later
2. **Trait-Based Abstraction** - Only DataProvider and ExecutionHandler differ between backtest/live
3. **Actor-Pattern Concurrency** - Tokio channels for bot orchestration, no heavyweight frameworks
4. **Multi-Tier Storage** - Arrow (RAM), TimescaleDB (warm), Parquet (cold)
5. **Workspace Modularity** - Separate crates for each major subsystem

### Technology Stack (Research-Validated)

**Core Trading**
- `hyperliquid-rust-sdk` v0.6.0 - Official SDK (maintain fork for production)
- `barter` - Event-driven engine architecture patterns
- `tokio` - Async runtime
- `serde` - Serialization

**Data & Analytics**
- `polars` - DataFrame operations (10-100x faster than pandas)
- `yata` or `rust_ti` - Technical indicators
- `arrow` - Columnar in-memory format
- `parquet` - Archival storage

**Persistence**
- `sqlx` - Async PostgreSQL/TimescaleDB client
- `tokio-postgres` - Low-level PostgreSQL access
- `bb8` or `deadpool` - Connection pooling

**Web Interface**
- `axum` - Web framework (147K req/s, 12.4MB idle)
- `axum::extract::ws` - WebSocket support
- `tower` - Middleware

**Configuration & Monitoring**
- `figment` - Multi-source config with hot-reload
- `notify` - File system watching
- `prometheus` - Metrics

**Utilities**
- `governor` - Rate limiting (per-exchange quotas)
- `anyhow` / `thiserror` - Error handling

## Workspace Structure

```
algo-trade/
├── Cargo.toml                 # Workspace root
├── CLAUDE.md                  # Project documentation
├── .claude/
│   ├── agents/
│   │   └── taskmaster.md
│   └── playbooks/
│       └── 2025-10-01_hyperliquid-trading-system.md
├── crates/
│   ├── core/                  # Event types, traits, engine
│   ├── exchange-hyperliquid/  # Hyperliquid integration
│   ├── data/                  # Market data streaming & storage
│   ├── strategy/              # Strategy trait & implementations
│   ├── execution/             # Order management & execution
│   ├── backtest/              # Backtesting engine
│   ├── bot-orchestrator/      # Multi-bot coordination
│   ├── web-api/               # Axum REST + WebSocket
│   └── cli/                   # Command-line interface
├── config/
│   ├── Config.toml            # Base configuration
│   └── Config.example.toml
└── scripts/
    └── setup_timescale.sql
```

## Phase 1: Foundation (Core Architecture)

### Objective
Establish workspace, core event types, and trait abstractions that enable backtest-live parity.

### MUST DO
- [ ] Create Cargo workspace with 9 crates
- [ ] Define core event types: `MarketEvent`, `SignalEvent`, `OrderEvent`, `FillEvent`
- [ ] Define core traits: `DataProvider`, `Strategy`, `ExecutionHandler`, `RiskManager`
- [ ] Implement generic `TradingSystem<D, E>` that works with any provider/handler
- [ ] Add workspace dependencies in root Cargo.toml
- [ ] Create basic config structures with Figment

### MUST NOT DO
- Implement any exchange integration yet
- Add any strategies or indicators
- Build web interface components
- Create database schemas
- Add logging or metrics beyond basic setup

### Atomic Tasks

#### Task 1.1: Initialize Cargo Workspace
**Files**:
- `/home/a/Work/algo-trade/Cargo.toml` (create)
- `/home/a/Work/algo-trade/.gitignore` (create)

**Action**: Create workspace root configuration
```toml
[workspace]
members = [
    "crates/core",
    "crates/exchange-hyperliquid",
    "crates/data",
    "crates/strategy",
    "crates/execution",
    "crates/backtest",
    "crates/bot-orchestrator",
    "crates/web-api",
    "crates/cli",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
thiserror = "1.0"
tracing = "0.1"
```

**Verification**: `cargo check` (should warn about missing crates)

**Acceptance**:
- Workspace file exists with all 9 members listed
- resolver = "2" set
- Common dependencies in workspace.dependencies

**Estimated Lines**: 25

#### Task 1.2: Create Core Crate Structure
**Files**:
- `/home/a/Work/algo-trade/crates/core/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/core/src/lib.rs` (create)

**Action**: Initialize core library crate
```toml
[package]
name = "algo-trade-core"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
rust_decimal = "1.33"
chrono = "0.4"
```

**Verification**: `cargo check -p algo-trade-core`

**Acceptance**:
- Crate compiles successfully
- Dependencies use workspace versions

**Estimated Lines**: 15

#### Task 1.3: Define Core Event Types
**File**: `/home/a/Work/algo-trade/crates/core/src/events.rs` (create)

**Action**: Create event type definitions
```rust
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    Quote { symbol: String, bid: Decimal, ask: Decimal, timestamp: DateTime<Utc> },
    Trade { symbol: String, price: Decimal, size: Decimal, timestamp: DateTime<Utc> },
    Bar { symbol: String, open: Decimal, high: Decimal, low: Decimal, close: Decimal, volume: Decimal, timestamp: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    pub symbol: String,
    pub direction: SignalDirection,
    pub strength: f64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalDirection {
    Long,
    Short,
    Exit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderEvent {
    pub symbol: String,
    pub order_type: OrderType,
    pub direction: OrderDirection,
    pub quantity: Decimal,
    pub price: Option<Decimal>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderDirection {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub order_id: String,
    pub symbol: String,
    pub direction: OrderDirection,
    pub quantity: Decimal,
    pub price: Decimal,
    pub commission: Decimal,
    pub timestamp: DateTime<Utc>,
}
```

**Verification**: `cargo check -p algo-trade-core`

**Acceptance**:
- All event types compile
- All types derive Clone, Debug, Serialize, Deserialize
- Use Decimal for prices/quantities, f64 only for dimensionless values

**Estimated Lines**: 65

#### Task 1.4: Define Core Traits
**File**: `/home/a/Work/algo-trade/crates/core/src/traits.rs` (create)

**Action**: Create trait abstractions for modularity
```rust
use crate::events::{FillEvent, MarketEvent, OrderEvent, SignalEvent};
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DataProvider: Send + Sync {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>>;
}

#[async_trait]
pub trait Strategy: Send + Sync {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>>;
    fn name(&self) -> &str;
}

#[async_trait]
pub trait ExecutionHandler: Send + Sync {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent>;
}

#[async_trait]
pub trait RiskManager: Send + Sync {
    async fn evaluate_signal(&self, signal: &SignalEvent) -> Result<Option<OrderEvent>>;
}
```

**Verification**:
```bash
cargo add async-trait -p algo-trade-core
cargo check -p algo-trade-core
```

**Acceptance**:
- All traits use async_trait for async methods
- All traits are Send + Sync for thread safety
- Methods return Result for error handling

**Estimated Lines**: 25

#### Task 1.5: Implement Generic TradingSystem
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs` (create)

**Action**: Create generic trading system that works with any implementations
```rust
use crate::events::{FillEvent, OrderEvent};
use crate::traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    data_provider: D,
    execution_handler: E,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
}

impl<D, E> TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    pub fn new(
        data_provider: D,
        execution_handler: E,
        strategies: Vec<Arc<Mutex<dyn Strategy>>>,
        risk_manager: Arc<dyn RiskManager>,
    ) -> Self {
        Self {
            data_provider,
            execution_handler,
            strategies,
            risk_manager,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while let Some(market_event) = self.data_provider.next_event().await? {
            // Generate signals from all strategies
            for strategy in &self.strategies {
                let mut strategy = strategy.lock().await;
                if let Some(signal) = strategy.on_market_event(&market_event).await? {
                    // Risk management evaluation
                    if let Some(order) = self.risk_manager.evaluate_signal(&signal).await? {
                        // Execute order
                        let fill = self.execution_handler.execute_order(order).await?;
                        tracing::info!("Order filled: {:?}", fill);
                    }
                }
            }
        }
        Ok(())
    }
}
```

**Verification**:
```bash
cargo add tracing -p algo-trade-core
cargo check -p algo-trade-core
```

**Acceptance**:
- TradingSystem is generic over DataProvider and ExecutionHandler
- Event loop processes: MarketEvent → Signal → Order → Fill
- All strategy logic is provider-agnostic
- Compiles without errors

**Estimated Lines**: 50

#### Task 1.6: Wire Up Core Module Exports
**File**: `/home/a/Work/algo-trade/crates/core/src/lib.rs`

**Action**: Export all core modules
```rust
pub mod engine;
pub mod events;
pub mod traits;

pub use engine::TradingSystem;
pub use events::{FillEvent, MarketEvent, OrderEvent, SignalEvent, SignalDirection};
pub use traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
```

**Verification**: `cargo check -p algo-trade-core`

**Acceptance**:
- All modules exported
- Public API accessible via re-exports
- No compiler warnings

**Estimated Lines**: 8

### Phase 1 Verification Checklist
- [ ] `cargo check` passes for entire workspace
- [ ] `cargo build -p algo-trade-core` succeeds
- [ ] `cargo clippy -p algo-trade-core -- -D warnings` passes
- [ ] 4 files created in core crate
- [ ] Event types use Decimal for financial values
- [ ] Traits are async and Send + Sync
- [ ] TradingSystem is generic over providers

**Estimated Total Lines**: ~190

---

## Phase 2: Hyperliquid Exchange Integration

### Objective
Integrate Hyperliquid REST API, WebSocket, authentication, and rate limiting.

### MUST DO
- [ ] Create exchange-hyperliquid crate with official SDK
- [ ] Implement WebSocket market data streaming
- [ ] Implement REST API for order execution
- [ ] Add EIP-712 authentication with key management
- [ ] Implement rate limiting (1200 weight/min, per-user quotas)
- [ ] Create LiveDataProvider implementing DataProvider trait
- [ ] Create LiveExecutionHandler implementing ExecutionHandler trait

### MUST NOT DO
- Implement backtesting components yet
- Add strategies or signals
- Create web interface
- Add database persistence
- Implement hardware wallet support (future enhancement)

### Atomic Tasks

#### Task 2.1: Initialize Hyperliquid Crate
**Files**:
- `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs` (create)

**Action**: Setup crate with dependencies
```toml
[package]
name = "algo-trade-hyperliquid"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-core = { path = "../core" }
tokio = { workspace = true, features = ["full"] }
serde = { workspace = true }
serde_json = "1.0"
reqwest = { version = "0.12", features = ["json"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
governor = "0.6"
ethers = "2.0"
anyhow = { workspace = true }
tracing = { workspace = true }
url = "2.5"
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- Crate compiles successfully
- All dependencies resolve
- References core crate via path

**Estimated Lines**: 20

#### Task 2.2: Define Hyperliquid API Client Structure
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs` (create)

**Action**: Create REST API client with rate limiting
```rust
use anyhow::Result;
use governor::{Quota, RateLimiter};
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
}

impl HyperliquidClient {
    pub fn new(base_url: String) -> Self {
        // 1200 requests per minute = 20 per second
        let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            http_client: Client::new(),
            base_url,
            rate_limiter,
        }
    }

    pub async fn get(&self, endpoint: &str) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.get(&url).send().await?;
        let json = response.json().await?;
        Ok(json)
    }

    pub async fn post(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.post(&url).json(&body).send().await?;
        let json = response.json().await?;
        Ok(json)
    }
}
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- Rate limiter enforces 20 req/s (1200/min)
- Methods are async
- Returns generic JSON for flexibility

**Estimated Lines**: 45

#### Task 2.3: Implement WebSocket Market Data Stream
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/websocket.rs` (create)

**Action**: Create WebSocket connection with auto-reconnect
```rust
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct HyperliquidWebSocket {
    ws_url: String,
    stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl HyperliquidWebSocket {
    pub fn new(ws_url: String) -> Self {
        Self {
            ws_url,
            stream: None,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;
        self.stream = Some(ws_stream);
        tracing::info!("WebSocket connected to {}", self.ws_url);
        Ok(())
    }

    pub async fn subscribe(&mut self, subscription: serde_json::Value) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            let msg = Message::Text(subscription.to_string());
            stream.send(msg).await?;
            Ok(())
        } else {
            anyhow::bail!("WebSocket not connected")
        }
    }

    pub async fn next_message(&mut self) -> Result<Option<serde_json::Value>> {
        if let Some(stream) = &mut self.stream {
            if let Some(msg) = stream.next().await {
                match msg? {
                    Message::Text(text) => {
                        let json: serde_json::Value = serde_json::from_str(&text)?;
                        Ok(Some(json))
                    }
                    Message::Close(_) => {
                        tracing::warn!("WebSocket closed, reconnecting...");
                        self.reconnect().await?;
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            } else {
                Ok(None)
            }
        } else {
            anyhow::bail!("WebSocket not connected")
        }
    }

    async fn reconnect(&mut self) -> Result<()> {
        self.stream = None;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        self.connect().await
    }
}
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- Auto-reconnect on connection close
- Subscribe method for channel subscriptions
- Handles text messages and parses JSON

**Estimated Lines**: 65

#### Task 2.4: Implement LiveDataProvider
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/data_provider.rs` (create)

**Action**: Create DataProvider implementation for live market data
```rust
use algo_trade_core::events::MarketEvent;
use algo_trade_core::traits::DataProvider;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::websocket::HyperliquidWebSocket;

pub struct LiveDataProvider {
    ws: HyperliquidWebSocket,
}

impl LiveDataProvider {
    pub async fn new(ws_url: String, symbols: Vec<String>) -> Result<Self> {
        let mut ws = HyperliquidWebSocket::new(ws_url);
        ws.connect().await?;

        // Subscribe to trades for all symbols
        for symbol in symbols {
            let subscription = serde_json::json!({
                "method": "subscribe",
                "subscription": {
                    "type": "trades",
                    "coin": symbol
                }
            });
            ws.subscribe(subscription).await?;
        }

        Ok(Self { ws })
    }
}

#[async_trait]
impl DataProvider for LiveDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        while let Some(msg) = self.ws.next_message().await? {
            if let Some(data) = msg.get("data") {
                if let Some(trades) = data.as_array() {
                    for trade in trades {
                        let symbol = trade["coin"].as_str().unwrap_or("").to_string();
                        let price = Decimal::from_str(trade["px"].as_str().unwrap_or("0"))?;
                        let size = Decimal::from_str(trade["sz"].as_str().unwrap_or("0"))?;
                        let timestamp = Utc::now();

                        return Ok(Some(MarketEvent::Trade {
                            symbol,
                            price,
                            size,
                            timestamp,
                        }));
                    }
                }
            }
        }
        Ok(None)
    }
}
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- Implements DataProvider trait from core
- Converts Hyperliquid messages to MarketEvent
- Handles multiple symbol subscriptions
- Uses async_trait

**Estimated Lines**: 60

#### Task 2.5: Implement LiveExecutionHandler
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs` (create)

**Action**: Create ExecutionHandler for live order execution
```rust
use algo_trade_core::events::{FillEvent, OrderDirection, OrderEvent, OrderType};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use crate::client::HyperliquidClient;

pub struct LiveExecutionHandler {
    client: HyperliquidClient,
}

impl LiveExecutionHandler {
    pub fn new(client: HyperliquidClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ExecutionHandler for LiveExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        let order_payload = json!({
            "type": match order.order_type {
                OrderType::Market => "market",
                OrderType::Limit => "limit",
            },
            "coin": order.symbol,
            "is_buy": matches!(order.direction, OrderDirection::Buy),
            "sz": order.quantity.to_string(),
            "limit_px": order.price.map(|p| p.to_string()),
        });

        let response = self.client.post("/exchange", order_payload).await?;

        // Parse response and create FillEvent
        // Note: Actual implementation needs to parse Hyperliquid response format
        let fill = FillEvent {
            order_id: response["orderId"].as_str().unwrap_or("").to_string(),
            symbol: order.symbol,
            direction: order.direction,
            quantity: order.quantity,
            price: order.price.unwrap_or(Decimal::ZERO),
            commission: Decimal::ZERO, // Extract from response
            timestamp: Utc::now(),
        };

        Ok(fill)
    }
}
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- Implements ExecutionHandler trait from core
- Converts OrderEvent to Hyperliquid API format
- Returns FillEvent
- Uses HyperliquidClient for rate-limited requests

**Estimated Lines**: 55

#### Task 2.6: Export Exchange Module
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`

**Action**: Wire up module exports
```rust
pub mod client;
pub mod data_provider;
pub mod execution;
pub mod websocket;

pub use client::HyperliquidClient;
pub use data_provider::LiveDataProvider;
pub use execution::LiveExecutionHandler;
pub use websocket::HyperliquidWebSocket;
```

**Verification**: `cargo check -p algo-trade-hyperliquid`

**Acceptance**:
- All modules exported
- Public API accessible
- No warnings

**Estimated Lines**: 10

### Phase 2 Verification Checklist
- [ ] `cargo check -p algo-trade-hyperliquid` passes
- [ ] `cargo build -p algo-trade-hyperliquid` succeeds
- [ ] `cargo clippy -p algo-trade-hyperliquid -- -D warnings` passes
- [ ] 5 source files created
- [ ] LiveDataProvider implements DataProvider trait
- [ ] LiveExecutionHandler implements ExecutionHandler trait
- [ ] Rate limiting enforced at 1200 req/min
- [ ] WebSocket auto-reconnects on disconnect

**Estimated Total Lines**: ~255

---

## Phase 3: Backtesting Engine

### Objective
Create backtesting infrastructure with simulated execution and historical data replay.

### MUST DO
- [ ] Create backtest crate
- [ ] Implement HistoricalDataProvider for CSV/Parquet files
- [ ] Implement SimulatedExecutionHandler with realistic fill simulation
- [ ] Create market replay with time-based ordering
- [ ] Add performance metrics (PnL, Sharpe, max drawdown)
- [ ] Ensure identical TradingSystem usage as live trading

### MUST NOT DO
- Implement strategies yet (Phase 4)
- Add database integration yet (Phase 5)
- Create web interface
- Add advanced fill simulation (queue position, latency) initially

### Atomic Tasks

#### Task 3.1: Initialize Backtest Crate
**Files**:
- `/home/a/Work/algo-trade/crates/backtest/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/backtest/src/lib.rs` (create)

**Action**: Setup backtest crate with dependencies
```toml
[package]
name = "algo-trade-backtest"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-core = { path = "../core" }
tokio = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
chrono = "0.4"
rust_decimal = "1.33"
csv = "1.3"
polars = { version = "0.43", features = ["lazy", "parquet"] }
```

**Verification**: `cargo check -p algo-trade-backtest`

**Acceptance**:
- Crate compiles
- Polars dependency for data processing
- CSV for simple file reading

**Estimated Lines**: 15

#### Task 3.2: Implement HistoricalDataProvider
**File**: `/home/a/Work/algo-trade/crates/backtest/src/data_provider.rs` (create)

**Action**: Create DataProvider for historical CSV data
```rust
use algo_trade_core::events::MarketEvent;
use algo_trade_core::traits::DataProvider;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct HistoricalDataProvider {
    events: Vec<MarketEvent>,
    current_index: usize,
}

impl HistoricalDataProvider {
    pub fn from_csv(path: &str) -> Result<Self> {
        let mut reader = csv::Reader::from_path(path)?;
        let mut events = Vec::new();

        for result in reader.records() {
            let record = result?;
            // Assuming CSV format: timestamp,symbol,open,high,low,close,volume
            let timestamp: DateTime<Utc> = record[0].parse()?;
            let symbol = record[1].to_string();
            let open = Decimal::from_str(&record[2])?;
            let high = Decimal::from_str(&record[3])?;
            let low = Decimal::from_str(&record[4])?;
            let close = Decimal::from_str(&record[5])?;
            let volume = Decimal::from_str(&record[6])?;

            events.push(MarketEvent::Bar {
                symbol,
                open,
                high,
                low,
                close,
                volume,
                timestamp,
            });
        }

        // Sort by timestamp to ensure chronological order
        events.sort_by_key(|e| match e {
            MarketEvent::Bar { timestamp, .. } => *timestamp,
            MarketEvent::Trade { timestamp, .. } => *timestamp,
            MarketEvent::Quote { timestamp, .. } => *timestamp,
        });

        Ok(Self {
            events,
            current_index: 0,
        })
    }
}

#[async_trait]
impl DataProvider for HistoricalDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        if self.current_index < self.events.len() {
            let event = self.events[self.current_index].clone();
            self.current_index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }
}
```

**Verification**: `cargo check -p algo-trade-backtest`

**Acceptance**:
- Implements DataProvider trait
- Loads CSV files with OHLCV data
- Events sorted chronologically
- Returns None when data exhausted

**Estimated Lines**: 70

#### Task 3.3: Implement SimulatedExecutionHandler
**File**: `/home/a/Work/algo-trade/crates/backtest/src/execution.rs` (create)

**Action**: Create ExecutionHandler for simulated fills
```rust
use algo_trade_core::events::{FillEvent, OrderDirection, OrderEvent, OrderType};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct SimulatedExecutionHandler {
    commission_rate: Decimal,
    slippage_bps: Decimal,
}

impl SimulatedExecutionHandler {
    pub fn new(commission_rate: f64, slippage_bps: f64) -> Self {
        Self {
            commission_rate: Decimal::from_str(&commission_rate.to_string()).unwrap(),
            slippage_bps: Decimal::from_str(&slippage_bps.to_string()).unwrap(),
        }
    }

    fn apply_slippage(&self, price: Decimal, direction: &OrderDirection) -> Decimal {
        let slippage = price * self.slippage_bps / Decimal::from(10000);
        match direction {
            OrderDirection::Buy => price + slippage,
            OrderDirection::Sell => price - slippage,
        }
    }
}

#[async_trait]
impl ExecutionHandler for SimulatedExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // For market orders, use provided price (assumed to be current market price)
        // For limit orders, assume immediate fill at limit price (simplified)
        let fill_price = match order.order_type {
            OrderType::Market => {
                let base_price = order.price.unwrap_or(Decimal::ZERO);
                self.apply_slippage(base_price, &order.direction)
            }
            OrderType::Limit => order.price.unwrap_or(Decimal::ZERO),
        };

        let commission = fill_price * order.quantity * self.commission_rate;

        let fill = FillEvent {
            order_id: uuid::Uuid::new_v4().to_string(),
            symbol: order.symbol,
            direction: order.direction,
            quantity: order.quantity,
            price: fill_price,
            commission,
            timestamp: Utc::now(),
        };

        Ok(fill)
    }
}
```

**Verification**:
```bash
cargo add uuid -p algo-trade-backtest --features v4
cargo check -p algo-trade-backtest
```

**Acceptance**:
- Implements ExecutionHandler trait
- Applies commission and slippage
- Market orders get slippage applied
- Generates unique order IDs

**Estimated Lines**: 60

#### Task 3.4: Implement Performance Metrics
**File**: `/home/a/Work/algo-trade/crates/backtest/src/metrics.rs` (create)

**Action**: Create performance calculation utilities
```rust
use rust_decimal::Decimal;

pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
}

pub struct MetricsCalculator {
    returns: Vec<Decimal>,
    equity_curve: Vec<Decimal>,
    wins: usize,
    losses: usize,
}

impl MetricsCalculator {
    pub fn new(initial_capital: Decimal) -> Self {
        Self {
            returns: Vec::new(),
            equity_curve: vec![initial_capital],
            wins: 0,
            losses: 0,
        }
    }

    pub fn add_trade(&mut self, pnl: Decimal) {
        let current_equity = self.equity_curve.last().unwrap();
        let new_equity = current_equity + pnl;

        self.equity_curve.push(new_equity);

        if pnl > Decimal::ZERO {
            self.wins += 1;
        } else if pnl < Decimal::ZERO {
            self.losses += 1;
        }

        let return_pct = pnl / current_equity;
        self.returns.push(return_pct);
    }

    pub fn calculate(&self) -> PerformanceMetrics {
        let total_return = (self.equity_curve.last().unwrap() - self.equity_curve.first().unwrap())
            / self.equity_curve.first().unwrap();

        let mean_return: f64 = self.returns.iter()
            .map(|r| r.to_string().parse::<f64>().unwrap_or(0.0))
            .sum::<f64>() / self.returns.len() as f64;

        let variance: f64 = self.returns.iter()
            .map(|r| {
                let val = r.to_string().parse::<f64>().unwrap_or(0.0);
                (val - mean_return).powi(2)
            })
            .sum::<f64>() / self.returns.len() as f64;

        let std_dev = variance.sqrt();
        let sharpe_ratio = if std_dev > 0.0 {
            mean_return / std_dev * (252.0_f64).sqrt() // Annualized
        } else {
            0.0
        };

        let max_drawdown = self.calculate_max_drawdown();

        let total_trades = self.wins + self.losses;
        let win_rate = if total_trades > 0 {
            self.wins as f64 / total_trades as f64
        } else {
            0.0
        };

        PerformanceMetrics {
            total_return,
            sharpe_ratio,
            max_drawdown,
            num_trades: total_trades,
            win_rate,
        }
    }

    fn calculate_max_drawdown(&self) -> Decimal {
        let mut max_drawdown = Decimal::ZERO;
        let mut peak = self.equity_curve[0];

        for &equity in &self.equity_curve {
            if equity > peak {
                peak = equity;
            }
            let drawdown = (peak - equity) / peak;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        max_drawdown
    }
}
```

**Verification**: `cargo check -p algo-trade-backtest`

**Acceptance**:
- Calculates total return, Sharpe ratio, max drawdown
- Tracks win rate and number of trades
- Uses Decimal for financial calculations

**Estimated Lines**: 100

#### Task 3.5: Export Backtest Module
**File**: `/home/a/Work/algo-trade/crates/backtest/src/lib.rs`

**Action**: Wire up exports
```rust
pub mod data_provider;
pub mod execution;
pub mod metrics;

pub use data_provider::HistoricalDataProvider;
pub use execution::SimulatedExecutionHandler;
pub use metrics::{MetricsCalculator, PerformanceMetrics};
```

**Verification**: `cargo check -p algo-trade-backtest`

**Acceptance**:
- All modules exported
- Public API clean
- No warnings

**Estimated Lines**: 8

### Phase 3 Verification Checklist
- [ ] `cargo check -p algo-trade-backtest` passes
- [ ] `cargo build -p algo-trade-backtest` succeeds
- [ ] `cargo clippy -p algo-trade-backtest -- -D warnings` passes
- [ ] HistoricalDataProvider implements DataProvider trait
- [ ] SimulatedExecutionHandler implements ExecutionHandler trait
- [ ] Performance metrics calculate correctly
- [ ] Can use same TradingSystem with backtest providers

**Estimated Total Lines**: ~253

---

## Phase 4: Strategy Framework

### Objective
Create pluggable strategy system with sample implementations.

### MUST DO
- [ ] Create strategy crate
- [ ] Implement example strategies: MA crossover, RSI, Grid trading
- [ ] Add technical indicator integration (yata or rust_ti)
- [ ] Ensure strategies work identically in backtest and live
- [ ] Add strategy configuration structures

### MUST NOT DO
- Implement database persistence yet
- Add ML/AI strategies yet (future)
- Create complex multi-factor strategies initially
- Add strategy optimization (Phase 6)

### Atomic Tasks

#### Task 4.1: Initialize Strategy Crate
**Files**:
- `/home/a/Work/algo-trade/crates/strategy/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/strategy/src/lib.rs` (create)

**Action**: Setup strategy crate
```toml
[package]
name = "algo-trade-strategy"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-core = { path = "../core" }
tokio = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
yata = "0.7"
rust_decimal = "1.33"
```

**Verification**: `cargo check -p algo-trade-strategy`

**Acceptance**:
- Crate compiles
- yata for technical indicators
- References core crate

**Estimated Lines**: 15

#### Task 4.2: Implement Moving Average Crossover Strategy
**File**: `/home/a/Work/algo-trade/crates/strategy/src/ma_crossover.rs` (create)

**Action**: Create simple MA crossover strategy
```rust
use algo_trade_core::events::{MarketEvent, SignalDirection, SignalEvent};
use algo_trade_core::traits::Strategy;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::VecDeque;

pub struct MaCrossoverStrategy {
    symbol: String,
    fast_period: usize,
    slow_period: usize,
    fast_prices: VecDeque<Decimal>,
    slow_prices: VecDeque<Decimal>,
    last_signal: Option<SignalDirection>,
}

impl MaCrossoverStrategy {
    pub fn new(symbol: String, fast_period: usize, slow_period: usize) -> Self {
        Self {
            symbol,
            fast_period,
            slow_period,
            fast_prices: VecDeque::new(),
            slow_prices: VecDeque::new(),
            last_signal: None,
        }
    }

    fn calculate_ma(prices: &VecDeque<Decimal>) -> Decimal {
        let sum: Decimal = prices.iter().sum();
        sum / Decimal::from(prices.len())
    }
}

#[async_trait]
impl Strategy for MaCrossoverStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        let (symbol, price) = match event {
            MarketEvent::Bar { symbol, close, .. } => (symbol, close),
            MarketEvent::Trade { symbol, price, .. } => (symbol, price),
            _ => return Ok(None),
        };

        if symbol != &self.symbol {
            return Ok(None);
        }

        self.fast_prices.push_back(*price);
        self.slow_prices.push_back(*price);

        if self.fast_prices.len() > self.fast_period {
            self.fast_prices.pop_front();
        }
        if self.slow_prices.len() > self.slow_period {
            self.slow_prices.pop_front();
        }

        if self.fast_prices.len() < self.fast_period || self.slow_prices.len() < self.slow_period {
            return Ok(None);
        }

        let fast_ma = Self::calculate_ma(&self.fast_prices);
        let slow_ma = Self::calculate_ma(&self.slow_prices);

        let new_signal = if fast_ma > slow_ma {
            Some(SignalDirection::Long)
        } else if fast_ma < slow_ma {
            Some(SignalDirection::Short)
        } else {
            None
        };

        // Only emit signal on crossover (direction change)
        if new_signal != self.last_signal && new_signal.is_some() {
            let signal = SignalEvent {
                symbol: self.symbol.clone(),
                direction: new_signal.clone().unwrap(),
                strength: 1.0,
                timestamp: Utc::now(),
            };
            self.last_signal = new_signal;
            Ok(Some(signal))
        } else {
            Ok(None)
        }
    }

    fn name(&self) -> &str {
        "MA Crossover"
    }
}
```

**Verification**: `cargo check -p algo-trade-strategy`

**Acceptance**:
- Implements Strategy trait
- Maintains state (price buffers)
- Only signals on crossover, not every tick
- Works with both Bar and Trade events

**Estimated Lines**: 95

#### Task 4.3: Implement Basic RiskManager
**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs` (create)

**Action**: Create simple position sizing risk manager
```rust
use algo_trade_core::events::{OrderDirection, OrderEvent, OrderType, SignalDirection, SignalEvent};
use algo_trade_core::traits::RiskManager;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct SimpleRiskManager {
    max_position_size: Decimal,
    fixed_quantity: Decimal,
}

impl SimpleRiskManager {
    pub fn new(max_position_size: f64, fixed_quantity: f64) -> Self {
        Self {
            max_position_size: Decimal::from_str(&max_position_size.to_string()).unwrap(),
            fixed_quantity: Decimal::from_str(&fixed_quantity.to_string()).unwrap(),
        }
    }
}

#[async_trait]
impl RiskManager for SimpleRiskManager {
    async fn evaluate_signal(&self, signal: &SignalEvent) -> Result<Option<OrderEvent>> {
        // Simple implementation: convert signal to fixed-size market order
        let direction = match signal.direction {
            SignalDirection::Long => OrderDirection::Buy,
            SignalDirection::Short => OrderDirection::Sell,
            SignalDirection::Exit => return Ok(None), // Handle exits separately
        };

        let order = OrderEvent {
            symbol: signal.symbol.clone(),
            order_type: OrderType::Market,
            direction,
            quantity: self.fixed_quantity,
            price: None,
            timestamp: Utc::now(),
        };

        Ok(Some(order))
    }
}
```

**Verification**: `cargo check -p algo-trade-strategy`

**Acceptance**:
- Implements RiskManager trait
- Converts signals to orders with position sizing
- Fixed quantity for simplicity (advanced sizing in future)

**Estimated Lines**: 50

#### Task 4.4: Export Strategy Module
**File**: `/home/a/Work/algo-trade/crates/strategy/src/lib.rs`

**Action**: Wire up exports
```rust
pub mod ma_crossover;
pub mod risk_manager;

pub use ma_crossover::MaCrossoverStrategy;
pub use risk_manager::SimpleRiskManager;
```

**Verification**: `cargo check -p algo-trade-strategy`

**Acceptance**:
- Modules exported
- Public API accessible
- No warnings

**Estimated Lines**: 6

### Phase 4 Verification Checklist
- [ ] `cargo check -p algo-trade-strategy` passes
- [ ] `cargo build -p algo-trade-strategy` succeeds
- [ ] `cargo clippy -p algo-trade-strategy -- -D warnings` passes
- [ ] MaCrossoverStrategy implements Strategy trait
- [ ] SimpleRiskManager implements RiskManager trait
- [ ] Strategies are stateful and work in event-driven context

**Estimated Total Lines**: ~166

---

## Phase 5: Data Storage & Multi-Tier Architecture

### Objective
Implement TimescaleDB integration, Arrow in-memory processing, and Parquet archival.

### MUST DO
- [ ] Create data crate
- [ ] Setup TimescaleDB schema with hypertables
- [ ] Implement Arrow RecordBatch processing
- [ ] Add Parquet file I/O
- [ ] Create multi-tier storage manager (hot/warm/cold)
- [ ] Add batch write optimization

### MUST NOT DO
- Implement full data scraping yet (Phase 7)
- Add complex data transformations
- Create data visualization
- Add ML feature engineering

### Atomic Tasks

#### Task 5.1: Initialize Data Crate
**Files**:
- `/home/a/Work/algo-trade/crates/data/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/data/src/lib.rs` (create)

**Action**: Setup data crate with dependencies
```toml
[package]
name = "algo-trade-data"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono", "rust_decimal"] }
arrow = "53.0"
parquet = "53.0"
polars = { version = "0.43", features = ["lazy", "parquet", "temporal"] }
rust_decimal = "1.33"
chrono = "0.4"
```

**Verification**: `cargo check -p algo-trade-data`

**Acceptance**:
- Crate compiles
- SQLx for TimescaleDB
- Arrow and Parquet for storage
- Polars for processing

**Estimated Lines**: 17

#### Task 5.2: Create TimescaleDB Schema Script
**File**: `/home/a/Work/algo-trade/scripts/setup_timescale.sql` (create)

**Action**: Define database schema
```sql
-- Create OHLCV hypertable
CREATE TABLE IF NOT EXISTS ohlcv (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    open DECIMAL(20, 8) NOT NULL,
    high DECIMAL(20, 8) NOT NULL,
    low DECIMAL(20, 8) NOT NULL,
    close DECIMAL(20, 8) NOT NULL,
    volume DECIMAL(20, 8) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);

-- Convert to hypertable (partitioned by time)
SELECT create_hypertable('ohlcv', 'timestamp', if_not_exists => TRUE);

-- Create indexes for common queries
CREATE INDEX IF NOT EXISTS idx_ohlcv_symbol_time ON ohlcv (symbol, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_ohlcv_exchange_time ON ohlcv (exchange, timestamp DESC);

-- Enable compression for old data
ALTER TABLE ohlcv SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol, exchange'
);

-- Compress data older than 7 days
SELECT add_compression_policy('ohlcv', INTERVAL '7 days');

-- Create trades table
CREATE TABLE IF NOT EXISTS trades (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    size DECIMAL(20, 8) NOT NULL,
    side TEXT NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);

SELECT create_hypertable('trades', 'timestamp', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_trades_symbol_time ON trades (symbol, timestamp DESC);

-- Create fills table for tracking executed orders
CREATE TABLE IF NOT EXISTS fills (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    order_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    direction TEXT NOT NULL,
    quantity DECIMAL(20, 8) NOT NULL,
    price DECIMAL(20, 8) NOT NULL,
    commission DECIMAL(20, 8) NOT NULL,
    strategy TEXT
);

CREATE INDEX IF NOT EXISTS idx_fills_timestamp ON fills (timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_fills_order_id ON fills (order_id);
CREATE INDEX IF NOT EXISTS idx_fills_symbol ON fills (symbol);
```

**Verification**:
```bash
# Manual verification: psql -f scripts/setup_timescale.sql
```

**Acceptance**:
- OHLCV and trades tables with hypertable partitioning
- Compression policy for old data
- Indexes for common query patterns
- Fills table for trade tracking

**Estimated Lines**: 60

#### Task 5.3: Implement Database Client
**File**: `/home/a/Work/algo-trade/crates/data/src/database.rs` (create)

**Action**: Create TimescaleDB client with batch writes
```rust
use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgPool, postgres::PgPoolOptions};

pub struct DatabaseClient {
    pool: PgPool,
}

impl DatabaseClient {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub async fn insert_ohlcv_batch(
        &self,
        records: Vec<OhlcvRecord>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for record in records {
            sqlx::query!(
                r#"
                INSERT INTO ohlcv (timestamp, symbol, exchange, open, high, low, close, volume)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (timestamp, symbol, exchange) DO NOTHING
                "#,
                record.timestamp,
                record.symbol,
                record.exchange,
                record.open,
                record.high,
                record.low,
                record.close,
                record.volume,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn query_ohlcv(
        &self,
        symbol: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let records = sqlx::query_as!(
            OhlcvRecord,
            r#"
            SELECT timestamp, symbol, exchange, open, high, low, close, volume
            FROM ohlcv
            WHERE symbol = $1 AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp ASC
            "#,
            symbol,
            start,
            end,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

#[derive(Debug, Clone)]
pub struct OhlcvRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub exchange: String,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}
```

**Verification**: `cargo check -p algo-trade-data`

**Acceptance**:
- Connection pooling with 10 max connections
- Batch insert with transaction
- ON CONFLICT DO NOTHING for idempotency
- Query by symbol and time range

**Estimated Lines**: 85

#### Task 5.4: Implement Parquet Storage
**File**: `/home/a/Work/algo-trade/crates/data/src/parquet_storage.rs` (create)

**Action**: Create Parquet file writer/reader
```rust
use anyhow::Result;
use arrow::array::{
    Decimal128Array, StringArray, TimestampMillisecondArray, ArrayRef,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;

pub struct ParquetStorage;

impl ParquetStorage {
    pub fn write_ohlcv_batch(path: &str, records: Vec<OhlcvRecord>) -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
            Field::new("symbol", DataType::Utf8, false),
            Field::new("exchange", DataType::Utf8, false),
            Field::new("open", DataType::Decimal128(20, 8), false),
            Field::new("high", DataType::Decimal128(20, 8), false),
            Field::new("low", DataType::Decimal128(20, 8), false),
            Field::new("close", DataType::Decimal128(20, 8), false),
            Field::new("volume", DataType::Decimal128(20, 8), false),
        ]));

        let timestamps: Vec<i64> = records.iter()
            .map(|r| r.timestamp.timestamp_millis())
            .collect();
        let symbols: Vec<String> = records.iter()
            .map(|r| r.symbol.clone())
            .collect();
        let exchanges: Vec<String> = records.iter()
            .map(|r| r.exchange.clone())
            .collect();

        let timestamp_array = TimestampMillisecondArray::from(timestamps);
        let symbol_array = StringArray::from(symbols);
        let exchange_array = StringArray::from(exchanges);

        // Convert Decimal to i128 for Decimal128Array
        let open_array = Decimal128Array::from(
            records.iter().map(|r| Some(r.open.mantissa())).collect::<Vec<_>>()
        ).with_precision_and_scale(20, 8)?;

        let high_array = Decimal128Array::from(
            records.iter().map(|r| Some(r.high.mantissa())).collect::<Vec<_>>()
        ).with_precision_and_scale(20, 8)?;

        let low_array = Decimal128Array::from(
            records.iter().map(|r| Some(r.low.mantissa())).collect::<Vec<_>>()
        ).with_precision_and_scale(20, 8)?;

        let close_array = Decimal128Array::from(
            records.iter().map(|r| Some(r.close.mantissa())).collect::<Vec<_>>()
        ).with_precision_and_scale(20, 8)?;

        let volume_array = Decimal128Array::from(
            records.iter().map(|r| Some(r.volume.mantissa())).collect::<Vec<_>>()
        ).with_precision_and_scale(20, 8)?;

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(timestamp_array) as ArrayRef,
                Arc::new(symbol_array) as ArrayRef,
                Arc::new(exchange_array) as ArrayRef,
                Arc::new(open_array) as ArrayRef,
                Arc::new(high_array) as ArrayRef,
                Arc::new(low_array) as ArrayRef,
                Arc::new(close_array) as ArrayRef,
                Arc::new(volume_array) as ArrayRef,
            ],
        )?;

        let file = File::create(path)?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;

        writer.write(&batch)?;
        writer.close()?;

        Ok(())
    }
}

use crate::database::OhlcvRecord;
```

**Verification**: `cargo check -p algo-trade-data`

**Acceptance**:
- Uses Arrow RecordBatch format
- Decimal128 for precise financial data
- Snappy compression
- Can write batch to Parquet file

**Estimated Lines**: 90

#### Task 5.5: Export Data Module
**File**: `/home/a/Work/algo-trade/crates/data/src/lib.rs`

**Action**: Wire up exports
```rust
pub mod database;
pub mod parquet_storage;

pub use database::{DatabaseClient, OhlcvRecord};
pub use parquet_storage::ParquetStorage;
```

**Verification**: `cargo check -p algo-trade-data`

**Acceptance**:
- All modules exported
- Public API accessible
- No warnings

**Estimated Lines**: 6

### Phase 5 Verification Checklist
- [ ] `cargo check -p algo-trade-data` passes
- [ ] `cargo build -p algo-trade-data` succeeds
- [ ] `cargo clippy -p algo-trade-data -- -D warnings` passes
- [ ] TimescaleDB schema script complete
- [ ] Database client supports batch writes
- [ ] Parquet storage uses Arrow format
- [ ] DECIMAL types used for financial precision

**Estimated Total Lines**: ~258

---

## Phase 6: Bot Orchestration with Actor Pattern

### Objective
Implement multi-bot coordination using Tokio channels following DIY actor pattern.

### MUST DO
- [ ] Create bot-orchestrator crate
- [ ] Implement actor-based bot structure (handle + task)
- [ ] Add command system (Start, Stop, UpdateConfig, GetStatus)
- [ ] Create bot registry for managing multiple bots
- [ ] Add health monitoring with heartbeats
- [ ] Implement graceful shutdown

### MUST NOT DO
- Add complex scheduling yet
- Implement bot deployment automation
- Create distributed bot coordination
- Add Kubernetes/Docker orchestration yet

### Atomic Tasks

#### Task 6.1: Initialize Bot Orchestrator Crate
**Files**:
- `/home/a/Work/algo-trade/crates/bot-orchestrator/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs` (create)

**Action**: Setup orchestrator crate
```toml
[package]
name = "algo-trade-bot-orchestrator"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-core = { path = "../core" }
algo-trade-strategy = { path = "../strategy" }
tokio = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- Crate compiles
- References core and strategy crates
- Tokio for async

**Estimated Lines**: 15

#### Task 6.2: Define Bot Commands
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs` (create)

**Action**: Create command enum for bot control
```rust
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum BotCommand {
    Start,
    Stop,
    Pause,
    Resume,
    UpdateConfig(BotConfig),
    GetStatus(oneshot::Sender<BotStatus>),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStatus {
    pub bot_id: String,
    pub state: BotState,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BotState {
    Stopped,
    Running,
    Paused,
    Error,
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- Commands for control flow
- GetStatus uses oneshot for sync response
- Serializable config and status

**Estimated Lines**: 40

#### Task 6.3: Implement Bot Actor
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs` (create)

**Action**: Create actor task with message handling
```rust
use crate::commands::{BotCommand, BotConfig, BotState, BotStatus};
use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc;

pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
}

impl BotActor {
    pub fn new(config: BotConfig, rx: mpsc::Receiver<BotCommand>) -> Self {
        Self {
            config,
            state: BotState::Stopped,
            rx,
        }
    }

    pub async fn run(mut self) -> Result<()> {
        tracing::info!("Bot {} starting", self.config.bot_id);

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                BotCommand::Start => {
                    tracing::info!("Bot {} started", self.config.bot_id);
                    self.state = BotState::Running;
                }
                BotCommand::Stop => {
                    tracing::info!("Bot {} stopped", self.config.bot_id);
                    self.state = BotState::Stopped;
                }
                BotCommand::Pause => {
                    tracing::info!("Bot {} paused", self.config.bot_id);
                    self.state = BotState::Paused;
                }
                BotCommand::Resume => {
                    tracing::info!("Bot {} resumed", self.config.bot_id);
                    self.state = BotState::Running;
                }
                BotCommand::UpdateConfig(new_config) => {
                    tracing::info!("Bot {} config updated", self.config.bot_id);
                    self.config = new_config;
                }
                BotCommand::GetStatus(tx) => {
                    let status = BotStatus {
                        bot_id: self.config.bot_id.clone(),
                        state: self.state.clone(),
                        last_heartbeat: Utc::now(),
                        error: None,
                    };
                    let _ = tx.send(status);
                }
                BotCommand::Shutdown => {
                    tracing::info!("Bot {} shutting down", self.config.bot_id);
                    break;
                }
            }
        }

        tracing::info!("Bot {} stopped", self.config.bot_id);
        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- Actor owns receiver and state
- Processes commands in loop
- GetStatus responds via oneshot
- Shutdown breaks loop for graceful stop

**Estimated Lines**: 65

#### Task 6.4: Implement Bot Handle
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs` (create)

**Action**: Create cloneable handle for bot control
```rust
use crate::commands::{BotCommand, BotConfig, BotStatus};
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
}

impl BotHandle {
    pub fn new(tx: mpsc::Sender<BotCommand>) -> Self {
        Self { tx }
    }

    pub async fn start(&self) -> Result<()> {
        self.tx.send(BotCommand::Start).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.tx.send(BotCommand::Stop).await?;
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        self.tx.send(BotCommand::Pause).await?;
        Ok(())
    }

    pub async fn resume(&self) -> Result<()> {
        self.tx.send(BotCommand::Resume).await?;
        Ok(())
    }

    pub async fn update_config(&self, config: BotConfig) -> Result<()> {
        self.tx.send(BotCommand::UpdateConfig(config)).await?;
        Ok(())
    }

    pub async fn get_status(&self) -> Result<BotStatus> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(BotCommand::GetStatus(tx)).await?;
        let status = rx.await?;
        Ok(status)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.tx.send(BotCommand::Shutdown).await?;
        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- Handle is Clone (multiple controllers)
- All commands have async methods
- GetStatus uses oneshot for response
- Ergonomic API

**Estimated Lines**: 55

#### Task 6.5: Implement Bot Registry
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs` (create)

**Action**: Create registry for managing multiple bots
```rust
use crate::bot_actor::BotActor;
use crate::bot_handle::BotHandle;
use crate::commands::BotConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub struct BotRegistry {
    bots: Arc<RwLock<HashMap<String, BotHandle>>>,
}

impl BotRegistry {
    pub fn new() -> Self {
        Self {
            bots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn spawn_bot(&self, config: BotConfig) -> Result<BotHandle> {
        let (tx, rx) = mpsc::channel(32);
        let handle = BotHandle::new(tx);

        let actor = BotActor::new(config.clone(), rx);
        tokio::spawn(async move {
            if let Err(e) = actor.run().await {
                tracing::error!("Bot {} error: {}", config.bot_id, e);
            }
        });

        self.bots.write().await.insert(config.bot_id.clone(), handle.clone());

        Ok(handle)
    }

    pub async fn get_bot(&self, bot_id: &str) -> Option<BotHandle> {
        self.bots.read().await.get(bot_id).cloned()
    }

    pub async fn remove_bot(&self, bot_id: &str) -> Result<()> {
        if let Some(handle) = self.bots.write().await.remove(bot_id) {
            handle.shutdown().await?;
        }
        Ok(())
    }

    pub async fn list_bots(&self) -> Vec<String> {
        self.bots.read().await.keys().cloned().collect()
    }

    pub async fn shutdown_all(&self) -> Result<()> {
        let bots = self.bots.read().await;
        for handle in bots.values() {
            handle.shutdown().await?;
        }
        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- Manages multiple bots by ID
- spawn_bot creates actor task
- Uses RwLock for concurrent access
- shutdown_all for graceful cleanup

**Estimated Lines**: 60

#### Task 6.6: Export Bot Orchestrator Module
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`

**Action**: Wire up exports
```rust
pub mod bot_actor;
pub mod bot_handle;
pub mod commands;
pub mod registry;

pub use bot_actor::BotActor;
pub use bot_handle::BotHandle;
pub use commands::{BotCommand, BotConfig, BotState, BotStatus};
pub use registry::BotRegistry;
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`

**Acceptance**:
- All modules exported
- Public API clean
- No warnings

**Estimated Lines**: 10

### Phase 6 Verification Checklist
- [ ] `cargo check -p algo-trade-bot-orchestrator` passes
- [ ] `cargo build -p algo-trade-bot-orchestrator` succeeds
- [ ] `cargo clippy -p algo-trade-bot-orchestrator -- -D warnings` passes
- [ ] Actor pattern with handle + task implemented
- [ ] Commands flow through bounded channels
- [ ] Registry manages multiple bots concurrently
- [ ] Graceful shutdown supported

**Estimated Total Lines**: ~245

---

## Phase 7: Web API with Axum

### Objective
Build REST API and WebSocket interface for bot control and monitoring.

### MUST DO
- [ ] Create web-api crate with Axum
- [ ] Implement REST endpoints (GET bots, POST create, PUT update, DELETE remove)
- [ ] Add WebSocket for real-time status updates
- [ ] Integrate with BotRegistry
- [ ] Add CORS for frontend integration
- [ ] Implement error handling and logging

### MUST NOT DO
- Build frontend SPA yet (future)
- Add authentication/authorization yet
- Implement HTTPS/TLS yet
- Add GraphQL endpoint

### Atomic Tasks

#### Task 7.1: Initialize Web API Crate
**Files**:
- `/home/a/Work/algo-trade/crates/web-api/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/web-api/src/lib.rs` (create)

**Action**: Setup web-api crate
```toml
[package]
name = "algo-trade-web-api"
version = "0.1.0"
edition = "2021"

[dependencies]
algo-trade-bot-orchestrator = { path = "../bot-orchestrator" }
tokio = { workspace = true }
axum = "0.7"
tower = "0.5"
tower-http = { version = "0.5", features = ["cors", "trace"] }
serde = { workspace = true }
serde_json = "1.0"
tracing = { workspace = true }
anyhow = { workspace = true }
```

**Verification**: `cargo check -p algo-trade-web-api`

**Acceptance**:
- Crate compiles
- Axum for web framework
- Tower for middleware (CORS, tracing)
- References bot-orchestrator

**Estimated Lines**: 17

#### Task 7.2: Implement REST Handlers
**File**: `/home/a/Work/algo-trade/crates/web-api/src/handlers.rs` (create)

**Action**: Create CRUD handlers for bots
```rust
use algo_trade_bot_orchestrator::{BotConfig, BotRegistry};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct BotListResponse {
    pub bots: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateBotRequest {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
}

pub async fn list_bots(
    State(registry): State<Arc<BotRegistry>>,
) -> Result<Json<BotListResponse>, StatusCode> {
    let bots = registry.list_bots().await;
    Ok(Json(BotListResponse { bots }))
}

pub async fn create_bot(
    State(registry): State<Arc<BotRegistry>>,
    Json(req): Json<CreateBotRequest>,
) -> Result<StatusCode, StatusCode> {
    let config = BotConfig {
        bot_id: req.bot_id,
        symbol: req.symbol,
        strategy: req.strategy,
        enabled: true,
    };

    registry
        .spawn_bot(config)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}

pub async fn get_bot_status(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<Json<algo_trade_bot_orchestrator::BotStatus>, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let status = handle
        .get_status()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(status))
}

pub async fn start_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    handle
        .start()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

pub async fn stop_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    handle
        .stop()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

pub async fn delete_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    registry
        .remove_bot(&bot_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}
```

**Verification**: `cargo check -p algo-trade-web-api`

**Acceptance**:
- CRUD operations for bots
- Uses Axum extractors (State, Path, Json)
- Proper HTTP status codes
- Error handling with Result

**Estimated Lines**: 105

#### Task 7.3: Implement WebSocket Handler
**File**: `/home/a/Work/algo-trade/crates/web-api/src/websocket.rs` (create)

**Action**: Create WebSocket for real-time updates
```rust
use algo_trade_bot_orchestrator::BotRegistry;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(registry): State<Arc<BotRegistry>>,
) -> Response {
    ws.on_upgrade(|socket| websocket_connection(socket, registry))
}

async fn websocket_connection(mut socket: WebSocket, registry: Arc<BotRegistry>) {
    let mut tick = interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Send bot statuses every second
                let bot_ids = registry.list_bots().await;
                let mut statuses = Vec::new();

                for bot_id in bot_ids {
                    if let Some(handle) = registry.get_bot(&bot_id).await {
                        if let Ok(status) = handle.get_status().await {
                            statuses.push(status);
                        }
                    }
                }

                let json = serde_json::to_string(&statuses).unwrap_or_default();
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                    _ => {}
                }
            }
        }
    }

    tracing::info!("WebSocket connection closed");
}
```

**Verification**: `cargo check -p algo-trade-web-api`

**Acceptance**:
- WebSocket upgrade handler
- Sends bot statuses every 1 second
- Handles client disconnect gracefully
- Uses tokio::select for concurrent ops

**Estimated Lines**: 55

#### Task 7.4: Create Router and Server
**File**: `/home/a/Work/algo-trade/crates/web-api/src/server.rs` (create)

**Action**: Setup Axum router with all endpoints
```rust
use crate::{handlers, websocket};
use algo_trade_bot_orchestrator::BotRegistry;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub struct ApiServer {
    registry: Arc<BotRegistry>,
}

impl ApiServer {
    pub fn new(registry: Arc<BotRegistry>) -> Self {
        Self { registry }
    }

    pub fn router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/api/bots", get(handlers::list_bots))
            .route("/api/bots", post(handlers::create_bot))
            .route("/api/bots/:bot_id", get(handlers::get_bot_status))
            .route("/api/bots/:bot_id/start", put(handlers::start_bot))
            .route("/api/bots/:bot_id/stop", put(handlers::stop_bot))
            .route("/api/bots/:bot_id", delete(handlers::delete_bot))
            .route("/ws", get(websocket::websocket_handler))
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(self.registry.clone())
    }

    pub async fn serve(self, addr: &str) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Web API listening on {}", addr);

        axum::serve(listener, self.router()).await?;

        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-web-api`

**Acceptance**:
- All REST routes defined
- WebSocket route at /ws
- CORS enabled for frontend
- TraceLayer for request logging
- Shared state (registry) via Arc

**Estimated Lines**: 50

#### Task 7.5: Export Web API Module
**File**: `/home/a/Work/algo-trade/crates/web-api/src/lib.rs`

**Action**: Wire up exports
```rust
pub mod handlers;
pub mod server;
pub mod websocket;

pub use server::ApiServer;
```

**Verification**: `cargo check -p algo-trade-web-api`

**Acceptance**:
- Modules exported
- ApiServer public
- No warnings

**Estimated Lines**: 6

### Phase 7 Verification Checklist
- [ ] `cargo check -p algo-trade-web-api` passes
- [ ] `cargo build -p algo-trade-web-api` succeeds
- [ ] `cargo clippy -p algo-trade-web-api -- -D warnings` passes
- [ ] REST API has CRUD endpoints for bots
- [ ] WebSocket sends real-time status updates
- [ ] CORS enabled for cross-origin requests
- [ ] Shared BotRegistry state via Arc

**Estimated Total Lines**: ~233

---

## Phase 8: Configuration Management with Hot Reload

### Objective
Implement Figment-based config with hot-reload capability.

### MUST DO
- [ ] Add Figment configuration merging (TOML + env vars + JSON)
- [ ] Implement file watching with notify crate
- [ ] Create watch channel for config updates
- [ ] Add config validation
- [ ] Create example config files

### MUST NOT DO
- Implement dynamic code reloading
- Add config encryption yet
- Create complex config DSL
- Add remote config fetching

### Atomic Tasks

#### Task 8.1: Create Configuration Module in Core
**File**: `/home/a/Work/algo-trade/crates/core/src/config.rs` (create)

**Action**: Define config structures
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub hyperliquid: HyperliquidConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperliquidConfig {
    pub api_url: String,
    pub ws_url: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "postgresql://localhost/algo_trade".to_string(),
                max_connections: 10,
            },
            hyperliquid: HyperliquidConfig {
                api_url: "https://api.hyperliquid.xyz".to_string(),
                ws_url: "wss://api.hyperliquid.xyz/ws".to_string(),
            },
        }
    }
}
```

**Verification**:
```bash
cargo check -p algo-trade-core
```

**Acceptance**:
- Config structures with serde
- Default implementation
- Hierarchical config

**Estimated Lines**: 50

#### Task 8.2: Implement Figment Config Loader
**File**: `/home/a/Work/algo-trade/crates/core/src/config_loader.rs` (create)

**Action**: Create Figment-based config merging
```rust
use crate::config::AppConfig;
use anyhow::Result;
use figment::{
    providers::{Env, Format, Json, Toml},
    Figment,
};

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load() -> Result<AppConfig> {
        let config: AppConfig = Figment::new()
            .merge(Toml::file("config/Config.toml"))
            .merge(Env::prefixed("APP_"))
            .join(Json::file("config/Config.json"))
            .extract()?;

        Ok(config)
    }

    pub fn load_with_profile(profile: &str) -> Result<AppConfig> {
        let config: AppConfig = Figment::new()
            .merge(Toml::file("config/Config.toml"))
            .merge(Toml::file(format!("config/Config.{}.toml", profile)))
            .merge(Env::prefixed("APP_"))
            .join(Json::file("config/Config.json"))
            .extract()?;

        Ok(config)
    }
}
```

**Verification**:
```bash
cargo add figment -p algo-trade-core --features toml,json,env
cargo check -p algo-trade-core
```

**Acceptance**:
- Merges TOML base + env vars + JSON
- Profile support (dev/prod)
- Returns structured AppConfig

**Estimated Lines**: 35

#### Task 8.3: Implement Hot Reload Watcher
**File**: `/home/a/Work/algo-trade/crates/core/src/config_watcher.rs` (create)

**Action**: Create file watcher for config hot-reload
```rust
use crate::config::AppConfig;
use crate::config_loader::ConfigLoader;
use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::watch;

pub struct ConfigWatcher {
    tx: watch::Sender<AppConfig>,
}

impl ConfigWatcher {
    pub fn new(initial_config: AppConfig) -> (Self, watch::Receiver<AppConfig>) {
        let (tx, rx) = watch::channel(initial_config);
        (Self { tx }, rx)
    }

    pub async fn watch(&self, config_path: &str) -> Result<()> {
        let tx = self.tx.clone();
        let config_path = config_path.to_string();

        tokio::task::spawn_blocking(move || {
            let (notify_tx, notify_rx) = std::sync::mpsc::channel();

            let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
                if let Ok(event) = res {
                    let _ = notify_tx.send(event);
                }
            })?;

            watcher.watch(Path::new(&config_path), RecursiveMode::NonRecursive)?;

            for event in notify_rx {
                if event.kind.is_modify() {
                    tracing::info!("Config file changed, reloading...");
                    match ConfigLoader::load() {
                        Ok(new_config) => {
                            let _ = tx.send(new_config);
                            tracing::info!("Config reloaded successfully");
                        }
                        Err(e) => {
                            tracing::error!("Failed to reload config: {}", e);
                        }
                    }
                }
            }

            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }
}
```

**Verification**:
```bash
cargo add notify -p algo-trade-core
cargo check -p algo-trade-core
```

**Acceptance**:
- Watches config file for changes
- Reloads on modification
- Broadcasts via watch channel
- Error handling for invalid config

**Estimated Lines**: 55

#### Task 8.4: Update Core Exports for Config
**File**: `/home/a/Work/algo-trade/crates/core/src/lib.rs`

**Action**: Add config exports
```rust
pub mod config;
pub mod config_loader;
pub mod config_watcher;
pub mod engine;
pub mod events;
pub mod traits;

pub use config::{AppConfig, DatabaseConfig, HyperliquidConfig, ServerConfig};
pub use config_loader::ConfigLoader;
pub use config_watcher::ConfigWatcher;
pub use engine::TradingSystem;
pub use events::{FillEvent, MarketEvent, OrderEvent, SignalEvent, SignalDirection};
pub use traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
```

**Verification**: `cargo check -p algo-trade-core`

**Acceptance**:
- Config modules exported
- Public API updated
- No warnings

**Estimated Lines**: 15

#### Task 8.5: Create Example Config Files
**Files**:
- `/home/a/Work/algo-trade/config/Config.toml` (create)
- `/home/a/Work/algo-trade/config/Config.example.toml` (create)

**Action**: Create base TOML config
```toml
[server]
host = "0.0.0.0"
port = 8080

[database]
url = "postgresql://localhost/algo_trade"
max_connections = 10

[hyperliquid]
api_url = "https://api.hyperliquid.xyz"
ws_url = "wss://api.hyperliquid.xyz/ws"
```

**Verification**: Manual inspection

**Acceptance**:
- Valid TOML syntax
- Matches AppConfig structure
- Example file for reference

**Estimated Lines**: 20 (10 per file)

### Phase 8 Verification Checklist
- [ ] `cargo check -p algo-trade-core` passes for updated core
- [ ] Config structures defined with serde
- [ ] Figment merges TOML + env + JSON
- [ ] File watcher reloads config on change
- [ ] Watch channel broadcasts config updates
- [ ] Example config files created

**Estimated Total Lines**: ~175

---

## Phase 9: CLI Application

### Objective
Create command-line interface for running bots and managing system.

### MUST DO
- [ ] Create cli crate
- [ ] Implement main binary
- [ ] Add subcommands: run, backtest, list-bots, start-bot
- [ ] Wire up all components
- [ ] Add logging setup
- [ ] Create end-to-end examples

### MUST NOT DO
- Implement TUI (terminal UI) yet
- Add interactive shell
- Create deployment scripts yet
- Add monitoring dashboards

### Atomic Tasks

#### Task 9.1: Initialize CLI Crate
**Files**:
- `/home/a/Work/algo-trade/crates/cli/Cargo.toml` (create)
- `/home/a/Work/algo-trade/crates/cli/src/main.rs` (create)

**Action**: Setup CLI binary crate
```toml
[package]
name = "algo-trade-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "algo-trade"
path = "src/main.rs"

[dependencies]
algo-trade-core = { path = "../core" }
algo-trade-hyperliquid = { path = "../exchange-hyperliquid" }
algo-trade-backtest = { path = "../backtest" }
algo-trade-strategy = { path = "../strategy" }
algo-trade-bot-orchestrator = { path = "../bot-orchestrator" }
algo-trade-web-api = { path = "../web-api" }
tokio = { workspace = true }
clap = { version = "4.5", features = ["derive"] }
tracing = { workspace = true }
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = { workspace = true }
```

**Verification**: `cargo check -p algo-trade-cli`

**Acceptance**:
- Binary crate with main.rs
- clap for CLI parsing
- All workspace crates as dependencies
- tracing-subscriber for logging

**Estimated Lines**: 22

#### Task 9.2: Define CLI Commands
**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`

**Action**: Create CLI structure with subcommands
```rust
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
    use algo_trade_backtest::{HistoricalDataProvider, MetricsCalculator, SimulatedExecutionHandler};
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
```

**Verification**: `cargo check -p algo-trade-cli`

**Acceptance**:
- Three subcommands: run, backtest, server
- Logging initialized with tracing-subscriber
- End-to-end integration of all components
- Compiles without errors

**Estimated Lines**: 125

### Phase 9 Verification Checklist
- [ ] `cargo check -p algo-trade-cli` passes
- [ ] `cargo build -p algo-trade-cli` succeeds
- [ ] `cargo build --release` creates binary
- [ ] CLI help shows all commands: `cargo run -p algo-trade-cli -- --help`
- [ ] All workspace crates integrate successfully
- [ ] Logging outputs to console

**Estimated Total Lines**: ~147

---

## Phase 10: Integration & Testing

### Objective
Create integration tests and example workflows.

### MUST DO
- [ ] Add workspace-level tests
- [ ] Create end-to-end backtest example
- [ ] Create live trading simulation example
- [ ] Add README with usage instructions
- [ ] Create CLAUDE.md with development guide

### MUST NOT DO
- Add unit tests for every function (future)
- Implement property-based testing yet
- Create benchmarks yet
- Add CI/CD pipelines yet

### Atomic Tasks

#### Task 10.1: Create Integration Test
**File**: `/home/a/Work/algo-trade/tests/integration_test.rs` (create)

**Action**: Write end-to-end integration test
```rust
use algo_trade_backtest::{HistoricalDataProvider, SimulatedExecutionHandler};
use algo_trade_core::TradingSystem;
use algo_trade_strategy::{MaCrossoverStrategy, SimpleRiskManager};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn test_backtest_ma_crossover() {
    // This test requires a sample CSV file
    // Skip if file doesn't exist
    if !std::path::Path::new("tests/data/sample.csv").exists() {
        return;
    }

    let data_provider = HistoricalDataProvider::from_csv("tests/data/sample.csv")
        .expect("Failed to load test data");

    let execution_handler = SimulatedExecutionHandler::new(0.001, 5.0);

    let strategy = MaCrossoverStrategy::new("BTC".to_string(), 5, 15);
    let strategies: Vec<Arc<Mutex<dyn algo_trade_core::Strategy>>> = vec![
        Arc::new(Mutex::new(strategy))
    ];

    let risk_manager: Arc<dyn algo_trade_core::RiskManager> =
        Arc::new(SimpleRiskManager::new(10000.0, 0.1));

    let mut system = TradingSystem::new(
        data_provider,
        execution_handler,
        strategies,
        risk_manager,
    );

    // Should run without errors
    system.run().await.expect("Backtest failed");
}
```

**Verification**:
```bash
mkdir -p tests/data
cargo test --test integration_test
```

**Acceptance**:
- Integration test compiles
- Tests end-to-end workflow
- Can be run with `cargo test`

**Estimated Lines**: 45

#### Task 10.2: Create README
**File**: `/home/a/Work/algo-trade/README.md` (create)

**Action**: Write project README
```markdown
# Hyperliquid Algorithmic Trading System

A production-grade algorithmic trading system for Hyperliquid exchange, built in Rust with full modularity and backtest-live parity.

## Features

- **Event-Driven Architecture**: Identical code runs in backtesting and live trading
- **Pluggable Strategies**: Implement `Strategy` trait for custom algorithms
- **Multi-Tier Storage**: Arrow (hot), TimescaleDB (warm), Parquet (cold)
- **Web API**: Axum-based REST + WebSocket for real-time control
- **Bot Orchestration**: Actor-pattern multi-bot coordination with Tokio
- **Hot-Reload Config**: Update parameters without restart

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- PostgreSQL with TimescaleDB extension
- Hyperliquid API access

### Installation

```bash
# Clone repository
git clone https://github.com/yourusername/algo-trade
cd algo-trade

# Build
cargo build --release

# Setup database
psql -f scripts/setup_timescale.sql
```

### Configuration

Copy example config:
```bash
cp config/Config.example.toml config/Config.toml
# Edit config/Config.toml with your settings
```

### Run Backtest

```bash
cargo run -p algo-trade-cli -- backtest \
  --data tests/data/sample.csv \
  --strategy ma_crossover
```

### Run Live Trading

```bash
cargo run -p algo-trade-cli -- run --config config/Config.toml
```

### Start Web API

```bash
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080
```

Then access:
- REST API: `http://localhost:8080/api/bots`
- WebSocket: `ws://localhost:8080/ws`

## Architecture

### Workspace Crates

- **core**: Event types, traits, trading engine
- **exchange-hyperliquid**: Hyperliquid REST/WebSocket integration
- **data**: TimescaleDB, Arrow, Parquet storage
- **strategy**: Strategy implementations (MA crossover, RSI, etc.)
- **execution**: Order management and execution
- **backtest**: Historical simulation with performance metrics
- **bot-orchestrator**: Multi-bot actor-pattern coordination
- **web-api**: Axum REST + WebSocket API
- **cli**: Command-line interface

### Event Flow

```
MarketEvent → Strategy → SignalEvent → RiskManager → OrderEvent → ExecutionHandler → FillEvent
```

### Backtest-Live Parity

Same `TradingSystem` works with different providers:

**Backtest**:
```rust
TradingSystem::new(
    HistoricalDataProvider::from_csv("data.csv")?,
    SimulatedExecutionHandler::new(0.001, 5.0),
    strategies,
    risk_manager,
)
```

**Live**:
```rust
TradingSystem::new(
    LiveDataProvider::new(ws_url, symbols).await?,
    LiveExecutionHandler::new(client),
    strategies, // SAME strategies!
    risk_manager, // SAME risk manager!
)
```

## Development

See [CLAUDE.md](CLAUDE.md) for detailed development guide.

### Common Commands

```bash
# Check all crates
cargo check

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run -p algo-trade-cli -- run

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

## License

MIT
```

**Verification**: Manual inspection

**Acceptance**:
- Complete README with features, setup, usage
- Architecture overview
- Development commands
- Links to CLAUDE.md

**Estimated Lines**: 145

#### Task 10.3: Create CLAUDE.md
**File**: `/home/a/Work/algo-trade/CLAUDE.md` (create)

**Action**: Write development guide for future Claude instances
```markdown
# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hyperliquid algorithmic trading system in Rust with modular architecture enabling backtest-live parity. Event-driven design ensures identical strategy code runs in backtesting and production.

## Architecture

### Core Design Pattern

**Event-Driven Architecture**: All components process discrete events sequentially, eliminating look-ahead bias and matching real-time trading exactly.

**Trait Abstraction**: `DataProvider` and `ExecutionHandler` traits enable swapping between backtest (historical data, simulated fills) and live (WebSocket data, real orders) without changing strategy code.

**Actor Pattern**: Bots use Tokio channels (mpsc for commands, watch for config updates, broadcast for status) following Alice Ryhl's DIY actor guide—no heavyweight frameworks.

### Workspace Structure

```
crates/
├── core/               # Event types, traits, TradingSystem engine
├── exchange-hyperliquid/ # REST/WebSocket, rate limiting, auth
├── data/               # TimescaleDB, Arrow, Parquet
├── strategy/           # Strategy trait impls (MA, RSI, etc.)
├── execution/          # Order management
├── backtest/           # Historical simulation, metrics
├── bot-orchestrator/   # Multi-bot coordination
├── web-api/            # Axum REST + WebSocket
└── cli/                # Command-line interface
```

### Event Flow

```
MarketEvent → Strategy::on_market_event() → SignalEvent
SignalEvent → RiskManager::evaluate_signal() → OrderEvent
OrderEvent → ExecutionHandler::execute_order() → FillEvent
```

### Key Dependencies

- **tokio**: Async runtime (all async code uses Tokio)
- **axum**: Web framework for API (preferred over actix-web for memory efficiency)
- **sqlx**: PostgreSQL/TimescaleDB client (async, compile-time checked queries)
- **polars**: DataFrame processing (10-100x faster than pandas)
- **arrow/parquet**: Columnar storage
- **figment**: Multi-source config (TOML + env + JSON)
- **hyperliquid-rust-sdk**: Official exchange SDK (maintain fork for production)

## Development Commands

### Building

```bash
# Check all crates
cargo check

# Build release
cargo build --release

# Build specific crate
cargo build -p algo-trade-core
```

### Testing

```bash
# All tests
cargo test

# Integration tests only
cargo test --test integration_test

# Specific crate
cargo test -p algo-trade-backtest
```

### Running

```bash
# Backtest
cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv --strategy ma_crossover

# Live trading
cargo run -p algo-trade-cli -- run --config config/Config.toml

# Web API only
cargo run -p algo-trade-cli -- server --addr 0.0.0.0:8080

# With debug logging
RUST_LOG=debug cargo run -p algo-trade-cli -- run
```

### Linting

```bash
# Clippy (all warnings as errors)
cargo clippy -- -D warnings

# Clippy for specific crate
cargo clippy -p algo-trade-core -- -D warnings

# Format
cargo fmt
```

## Critical Patterns

### 1. Financial Precision

**ALWAYS use `rust_decimal::Decimal` for prices, quantities, PnL**. Never use `f64` for financial calculations—rounding errors compound over thousands of operations.

```rust
// CORRECT
use rust_decimal::Decimal;
let price: Decimal = "42750.50".parse()?;

// WRONG - will accumulate errors
let price: f64 = 42750.50;
```

### 2. Backtest-Live Parity

Strategy and RiskManager implementations must be provider-agnostic. Only `DataProvider` and `ExecutionHandler` differ between backtest and live.

```rust
// Strategy sees MarketEvent - doesn't know if backtest or live
async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
    // Same logic runs everywhere
}
```

### 3. Actor Pattern for Bots

Each bot is a spawned task owning `mpsc::Receiver<BotCommand>`. Handle is `Clone` with `mpsc::Sender` for multiple controllers.

```rust
// Spawn bot
let (tx, rx) = mpsc::channel(32);
let handle = BotHandle::new(tx);
tokio::spawn(async move { BotActor::new(config, rx).run().await });
```

### 4. Rate Limiting

Use `governor` crate with per-exchange quotas:
- Hyperliquid: 1200 weight/min (20 req/s)
- Binance: 1200 req/min
- Apply backoff on rate limit errors

### 5. Database Operations

**Batch writes for performance**: Single inserts ~390µs, batching 100 inserts ~13ms (3x speedup per record).

```rust
// Collect records, then batch insert
db.insert_ohlcv_batch(records).await?;
```

**Use hypertables**: TimescaleDB's `create_hypertable()` for time-series data, automatic partitioning.

### 6. Configuration Hot-Reload

Config updates flow via `tokio::sync::watch` channels. Bots subscribe and receive latest config without restart.

```rust
let (watcher, mut config_rx) = ConfigWatcher::new(config);
tokio::select! {
    _ = config_rx.changed() => {
        let new_config = config_rx.borrow().clone();
        // Apply new config
    }
}
```

## Adding New Features

### New Strategy

1. Implement `Strategy` trait in `crates/strategy/src/`
2. Add state (price buffers, indicators) as struct fields
3. Process `MarketEvent` in `on_market_event()`
4. Return `SignalEvent` on signal generation

```rust
pub struct MyStrategy { /* state */ }

#[async_trait]
impl Strategy for MyStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        // Update state, generate signal
    }
    fn name(&self) -> &str { "My Strategy" }
}
```

### New Exchange Integration

1. Create crate `crates/exchange-{name}/`
2. Implement `DataProvider` for WebSocket market data
3. Implement `ExecutionHandler` for order execution
4. Add rate limiting with `governor`
5. Handle authentication and reconnection

### New REST Endpoint

Add to `crates/web-api/src/handlers.rs`:

```rust
pub async fn my_handler(
    State(registry): State<Arc<BotRegistry>>,
    Json(req): Json<MyRequest>,
) -> Result<Json<MyResponse>, StatusCode> {
    // Implementation
}
```

Add route in `crates/web-api/src/server.rs`:

```rust
.route("/api/my-endpoint", post(handlers::my_handler))
```

## Database Schema

### OHLCV Table (Hypertable)

```sql
CREATE TABLE ohlcv (
    timestamp TIMESTAMPTZ NOT NULL,
    symbol TEXT NOT NULL,
    exchange TEXT NOT NULL,
    open DECIMAL(20, 8) NOT NULL,
    high DECIMAL(20, 8) NOT NULL,
    low DECIMAL(20, 8) NOT NULL,
    close DECIMAL(20, 8) NOT NULL,
    volume DECIMAL(20, 8) NOT NULL,
    PRIMARY KEY (timestamp, symbol, exchange)
);
```

- **DECIMAL(20, 8)**: Precise financial data (never FLOAT/DOUBLE)
- **Hypertable**: Automatic time-based partitioning
- **Compression**: Enabled for data >7 days old

## Troubleshooting

### "Task panicked" errors

Check Tokio runtime: all async code must run inside `#[tokio::main]` or spawned tasks.

### Rate limit errors from Hyperliquid

Check `governor` quota configuration. Hyperliquid allows 1200 weight/min, most requests cost 1 weight.

### Database connection errors

Verify TimescaleDB extension: `CREATE EXTENSION IF NOT EXISTS timescaledb;`

### WebSocket disconnects

Check auto-reconnect logic in `HyperliquidWebSocket::reconnect()`. Should have exponential backoff.

### Backtest vs Live divergence

Strategy implementation likely has look-ahead bias. Ensure all logic works event-by-event, not on future data.

## References

- **Barter-rs**: Event-driven architecture patterns (https://github.com/barter-rs/barter-rs)
- **Hyperliquid Docs**: API reference (https://hyperliquid.gitbook.io)
- **Alice Ryhl's Actor Guide**: Tokio channel patterns (https://ryhl.io/blog/actors-with-tokio/)
- **TimescaleDB**: Time-series best practices (https://docs.timescale.com)
```

**Verification**: Manual inspection

**Acceptance**:
- Complete development guide
- Architecture patterns documented
- Critical implementation rules
- Troubleshooting section
- References to source materials

**Estimated Lines**: 275

### Phase 10 Verification Checklist
- [ ] Integration test compiles and runs
- [ ] README complete with setup and usage
- [ ] CLAUDE.md provides comprehensive development guide
- [ ] All documentation references correct file paths
- [ ] Project is ready for handoff to TaskMaster agent

**Estimated Total Lines**: ~465

---

## Final Verification

### Entire Workspace
- [ ] `cargo check` passes for all crates
- [ ] `cargo build --release` succeeds
- [ ] `cargo test` runs all tests
- [ ] `cargo clippy --all-targets -- -D warnings` passes with no warnings
- [ ] All 9 crates compile independently
- [ ] CLI binary runs: `./target/release/algo-trade --help`

### Architecture Validation
- [ ] Event-driven architecture implemented with trait abstraction
- [ ] Backtest and live trading use identical `TradingSystem`
- [ ] Actor pattern used for bot orchestration (no heavyweight frameworks)
- [ ] Multi-tier storage: Arrow + TimescaleDB + Parquet
- [ ] Web API with REST + WebSocket functional
- [ ] Config hot-reload via watch channels

### Code Quality
- [ ] DECIMAL used for all financial calculations (no floats)
- [ ] All traits are async + Send + Sync
- [ ] Error handling with Result and anyhow
- [ ] Logging with tracing throughout
- [ ] Rate limiting enforced (governor)
- [ ] Graceful shutdown implemented

## Rollback Plan

If any verification fails:

1. **Identify failing phase**: Check which phase's verification failed
2. **Rollback crate**: `git checkout -- crates/{crate_name}`
3. **Review playbook**: Re-read atomic tasks for that phase
4. **Fix issue**: Address specific compilation or logic error
5. **Re-verify**: Run phase verification checklist again

For complete reset:
```bash
git clean -fd crates/
git checkout -- crates/
```

## Success Criteria

Playbook is complete when:
- ✅ All 9 crates compile and pass clippy
- ✅ CLI binary successfully runs all subcommands
- ✅ Integration test passes
- ✅ README and CLAUDE.md complete
- ✅ Can run backtest with sample data
- ✅ Can start web API and access endpoints
- ✅ Architecture follows research-validated patterns
- ✅ No dead code or unimplemented stubs

**Total Estimated Lines**: ~2,407

## Next Steps After Playbook Completion

1. **Run TaskMaster Agent**: Pass this playbook to TaskMaster for atomic execution
2. **Validate Each Phase**: TaskMaster will execute phases sequentially with verification
3. **Integration Testing**: Run end-to-end tests after all phases complete
4. **Production Hardening**: Add authentication, TLS, monitoring, alerting
5. **Strategy Development**: Implement advanced strategies (RSI, Grid, ML-based)
6. **Optimization**: Profile hot paths, optimize data structures, tune batch sizes
7. **Deployment**: Docker, Kubernetes, CI/CD pipelines

---

## Key Takeaways from Research

This implementation follows battle-tested patterns from:

1. **Barter-rs**: Event-driven architecture, trait-based abstraction, O(1) indexed state
2. **passivbot**: Production Hyperliquid integration, grid trading, config optimization
3. **rust-trade**: Full-stack architecture, multi-tier caching, TimescaleDB schemas
4. **hftbacktest**: Queue position simulation, latency modeling, L2/L3 orderbook
5. **Nautilus Trader**: Redis state persistence, nanosecond backtesting, modular adapters

**Critical Success Factor**: Event-driven from day one, not vectorized backtesting retrofitted later. This ensures backtest-live parity and eliminates look-ahead bias from the start.
