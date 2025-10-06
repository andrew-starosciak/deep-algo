# Playbook: Live Bot Operations Visibility & Hyperliquid Trading Enhancements

**Date**: 2025-10-04
**Agent**: TaskMaster
**Context Report**: `/home/a/Work/algo-trade/.claude/context/2025-10-04_live-bot-enhancements.md`

---

## User Request

> "I am able to make a bot and press S to start and x to stop. But they are not presenting anything happening. Let's use the context manager agent and identify exactly how we can get more details of our bots operations. Since we are working with the hyperliquid dex I expect we can associate a crypto wallet that can be used to exchange with hyperliquid. We also expect to be able to configure our position sizes how much we can put into a position and how much leverage hyperliquid allows us to use."

---

## Scope Boundaries

### MUST DO
1. ✅ Create event streaming infrastructure (BotEvent enum, broadcast/watch channels)
2. ✅ Expose TradingSystem metrics via public accessor methods
3. ✅ Add wallet configuration support (WalletConfig struct, env loading)
4. ✅ Integrate Hyperliquid Rust SDK for order signing
5. ✅ Add position sizing with leverage support (1x-50x)
6. ✅ Enhance TUI to display bot metrics (equity, positions, PnL, events)
7. ✅ Add BotConfig fields (initial_capital, risk_per_trade_pct, max_position_pct, leverage, margin_mode)
8. ✅ Implement authenticated order execution with nonce management
9. ✅ Display real-time events in TUI messages panel
10. ✅ Add bot detail panel showing open positions and performance
11. ✅ Validate wallet configuration format (address, private key)
12. ✅ Enforce minimum order value ($10 Hyperliquid requirement)
13. ✅ Set sensible defaults for new config fields
14. ✅ Document wallet setup process
15. ✅ Update example configuration files

### MUST NOT DO
1. ❌ Do NOT implement isolated margin support (cross margin only)
2. ❌ Do NOT implement hardware wallet signing (private key strings only)
3. ❌ Do NOT implement per-asset leverage limits (single leverage value)
4. ❌ Do NOT implement margin tier API integration (not exposed by Hyperliquid API)
5. ❌ Do NOT implement WebSocket reconnection logic (assume existing implementation works)
6. ❌ Do NOT implement multi-asset risk aggregation (single symbol per bot)
7. ❌ Do NOT implement position PnL streaming (calculate on-demand only)
8. ❌ Do NOT implement order modification/cancellation (market orders only)
9. ❌ Do NOT implement subaccount support (master account + API wallet only)
10. ❌ Do NOT implement vault trading (regular account only)

---

## Atomic Tasks

### Phase 1: Event Streaming Infrastructure

#### Task 1: Create BotEvent enum in new events.rs file
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs`
**Location**: New file
**Action**: Create `events.rs` and define `BotEvent` enum with 7 variants:
- `MarketUpdate { symbol: String, price: Decimal, volume: Decimal, timestamp: DateTime<Utc> }`
- `SignalGenerated(SignalEvent)`
- `OrderPlaced(OrderEvent)`
- `OrderFilled(FillEvent)`
- `PositionUpdate { symbol: String, quantity: Decimal, avg_price: Decimal, unrealized_pnl: Decimal }`
- `TradeClosed { symbol: String, pnl: Decimal, win: bool }`
- `Error { message: String, timestamp: DateTime<Utc> }`

Include all necessary imports: `use algo_trade_core::events::{FillEvent, OrderEvent, SignalEvent};`, `use chrono::{DateTime, Utc};`, `use rust_decimal::Decimal;`, `use serde::{Deserialize, Serialize};`.
Add `#[derive(Debug, Clone, Serialize, Deserialize)]` to the enum.

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 35
**Dependencies**: None

#### Task 2: Create EnhancedBotStatus struct in events.rs
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs`
**Location**: After BotEvent definition
**Action**: Define `EnhancedBotStatus` struct with fields:
- `bot_id: String`
- `state: BotState`
- `last_heartbeat: DateTime<Utc>`
- `current_equity: Decimal`
- `initial_capital: Decimal`
- `total_return_pct: f64`
- `sharpe_ratio: f64`
- `max_drawdown: f64`
- `win_rate: f64`
- `num_trades: usize`
- `open_positions: Vec<PositionInfo>`
- `recent_events: Vec<BotEvent>`
- `error: Option<String>`

Import `BotState` from `crate::commands::BotState`.
Add `#[derive(Debug, Clone, Serialize, Deserialize)]`.

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 20
**Dependencies**: Task 1

#### Task 3: Create PositionInfo struct in events.rs
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs`
**Location**: After EnhancedBotStatus definition
**Action**: Define `PositionInfo` struct with fields:
- `symbol: String`
- `quantity: Decimal`
- `avg_price: Decimal`
- `current_price: Decimal`
- `unrealized_pnl: Decimal`
- `unrealized_pnl_pct: f64`

Add `#[derive(Debug, Clone, Serialize, Deserialize)]`.

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 10
**Dependencies**: Task 2

#### Task 4: Expose events.rs module in bot-orchestrator lib.rs
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/lib.rs`
**Location**: After existing module declarations
**Action**: Add `pub mod events;` to expose the new events module.

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 1
**Dependencies**: Task 3

#### Task 5: Add event streaming fields to BotActor struct
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 1-9 (imports) and Lines 10-18 (BotActor struct)
**Action**:
1. Add imports at top of file:
   - `use std::collections::VecDeque;`
   - `use tokio::sync::{broadcast, watch};`
   - `use crate::events::{BotEvent, EnhancedBotStatus};`
2. Add fields to BotActor struct after line 14:
   - `event_tx: broadcast::Sender<BotEvent>,`
   - `status_tx: watch::Sender<EnhancedBotStatus>,`
   - `recent_events: VecDeque<BotEvent>,`

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 6
**Dependencies**: Task 4

#### Task 6: Modify BotActor::new() to initialize event channels
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 23-30 (new() constructor)
**Action**: Modify constructor to:
1. Create broadcast channel: `let (event_tx, _) = broadcast::channel(1000);`
2. Create initial status with defaults (use Decimal::from(10000) for capitals)
3. Create watch channel: `let (status_tx, _) = watch::channel(initial_status);`
4. Initialize `recent_events: VecDeque::with_capacity(10)`
5. Add these fields to Self constructor
6. Return tuple `(Self { ...fields }, event_rx, status_rx)` to expose receivers

Update signature to: `pub fn new(config: BotConfig, rx: mpsc::Receiver<BotCommand>) -> (Self, broadcast::Receiver<BotEvent>, watch::Receiver<EnhancedBotStatus>)`

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 20
**Dependencies**: Task 5

#### Task 7: Add TradingSystem accessor methods to engine.rs
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: After line 405 (end of existing impl block)
**Action**: Add new impl block with public accessor methods:
```rust
impl<D, E> TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    pub fn current_equity(&self) -> Decimal { /* implementation */ }
    pub const fn initial_capital(&self) -> Decimal { /* implementation */ }
    pub fn total_return_pct(&self) -> f64 { /* implementation */ }
    pub fn sharpe_ratio(&self) -> f64 { /* implementation */ }
    pub fn max_drawdown(&self) -> f64 { /* implementation */ }
    pub fn win_rate(&self) -> f64 { /* implementation */ }
    pub const fn num_trades(&self) -> usize { /* implementation */ }
    pub fn open_positions(&self) -> &HashMap<String, Position> { /* implementation */ }
    pub fn unrealized_pnl(&self, symbol: &str, current_price: Decimal) -> Option<Decimal> { /* implementation */ }
}
```

Use exact implementations from Context Report Section 4.1.4 (lines 788-859).

**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 70
**Dependencies**: None (core crate, no bot-orchestrator dependency)

#### Task 8: Add build_enhanced_status() method to BotActor
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: After line 187 (after existing methods)
**Action**: Add private method `fn build_enhanced_status(&self) -> EnhancedBotStatus` that:
1. Extracts metrics from `self.system.as_ref().unwrap()` using new accessor methods
2. Maps open positions to `Vec<PositionInfo>`
3. Clones `self.recent_events` to Vec
4. Returns populated EnhancedBotStatus

Use implementation from Context Report Section 4.1.2 (lines 713-741).

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 40
**Dependencies**: Task 7

#### Task 9: Modify trading_loop() to emit events
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 95-124 (trading_loop method)
**Action**: After `system.process_next_event()` call (around line 113):
1. On success, emit BotEvent based on event type (signal/order/fill)
2. Send event to broadcast channel: `let _ = self.event_tx.send(event.clone());`
3. Add event to ring buffer: `self.recent_events.push_back(event);`
4. Limit ring buffer to 10: `if self.recent_events.len() > 10 { self.recent_events.pop_front(); }`
5. Update status: `let _ = self.status_tx.send(self.build_enhanced_status());`
6. On error, emit BotEvent::Error and send to channel

Note: This requires TradingSystem to expose which events were processed. For now, create generic BotEvent::MarketUpdate after each successful process_next_event().

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 20
**Dependencies**: Task 8

#### Task 10: Add event subscription fields to BotHandle
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs`
**Location**: Lines 5-9 (BotHandle struct)
**Action**:
1. Add imports: `use crate::events::{BotEvent, EnhancedBotStatus};`, `use tokio::sync::{broadcast, watch};`
2. Add fields to struct:
   - `event_rx: broadcast::Receiver<BotEvent>,`
   - `status_rx: watch::Receiver<EnhancedBotStatus>,`

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 5
**Dependencies**: Task 4

#### Task 11: Update BotHandle::new() and add subscription methods
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs`
**Location**: Lines 16-24 (constructor) and after line 84 (new methods)
**Action**:
1. Update `new()` signature to accept event_rx and status_rx parameters
2. Add methods:
   - `pub fn subscribe_events(&self) -> broadcast::Receiver<BotEvent> { self.event_rx.resubscribe() }`
   - `pub fn latest_status(&self) -> EnhancedBotStatus { self.status_rx.borrow().clone() }`
   - `pub async fn wait_for_status_change(&mut self) -> anyhow::Result<EnhancedBotStatus> { self.status_rx.changed().await?; Ok(self.status_rx.borrow().clone()) }`

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 15
**Dependencies**: Task 10

#### Task 12: Update BotRegistry::spawn_bot() to wire event channels
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
**Location**: Lines 35-50 (spawn_bot method)
**Action**:
1. Modify `BotActor::new()` call to capture returned receivers
2. Pass receivers to `BotHandle::new()`

Change from:
```rust
let actor = BotActor::new(config.clone(), rx);
let handle = BotHandle::new(tx);
```

To:
```rust
let (actor, event_rx, status_rx) = BotActor::new(config.clone(), rx);
let handle = BotHandle::new(tx, event_rx, status_rx);
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 3
**Dependencies**: Task 11

---

### Phase 2: Wallet Integration

#### Task 13: Add hyperliquid_rust_sdk dependency
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
**Location**: [dependencies] section
**Action**: Add `hyperliquid_rust_sdk = "0.6.0"`

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 1
**Dependencies**: None

#### Task 14: Create MarginMode enum in commands.rs
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
**Location**: After line 26 (after BotConfig)
**Action**: Define enum:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarginMode {
    Cross,
    Isolated,
}

impl Default for MarginMode {
    fn default() -> Self {
        Self::Cross
    }
}
```

Add imports: `use serde::{Deserialize, Serialize};` (if not already present).

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 12
**Dependencies**: None

#### Task 15: Create WalletConfig struct in commands.rs
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
**Location**: After MarginMode definition
**Action**: Define struct with validation methods:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    pub account_address: String,
    #[serde(skip_serializing)]
    pub api_wallet_private_key: Option<String>,
    #[serde(skip)]
    pub nonce_counter: Arc<AtomicU64>,
}
```

Add `impl WalletConfig` with:
- `pub fn from_env() -> anyhow::Result<Self>` - loads from env vars, validates format
- `pub fn next_nonce(&self) -> u64` - atomic increment

Add imports: `use std::sync::{atomic::AtomicU64, Arc};`, `use chrono::Utc;`.
Use exact implementation from Context Report Section 4.2.1 (lines 871-924).

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 50
**Dependencies**: Task 14

#### Task 16: Add wallet and trading fields to BotConfig
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
**Location**: Lines 15-26 (BotConfig struct)
**Action**: Add fields after `strategy_config`:
```rust
// Trading parameters
pub initial_capital: Decimal,
pub risk_per_trade_pct: f64,
pub max_position_pct: f64,

// Hyperliquid-specific
pub leverage: u8,
pub margin_mode: MarginMode,

// Wallet configuration
#[serde(skip)]
pub wallet: Option<WalletConfig>,
```

Add import: `use rust_decimal::Decimal;`.

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 10
**Dependencies**: Task 15

#### Task 17: Add wallet fields to HyperliquidClient
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: Lines 15-19 (HyperliquidClient struct)
**Action**: Add fields:
```rust
wallet: Option<hyperliquid_rust_sdk::Wallet>,
account_address: Option<String>,
```

Add import: `use hyperliquid_rust_sdk::Wallet;` (conditional based on actual SDK API).

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 3
**Dependencies**: Task 13

#### Task 18: Add HyperliquidClient::with_wallet() constructor
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After line 37 (after existing new())
**Action**: Add method:
```rust
pub fn with_wallet(
    base_url: String,
    api_wallet_private_key: String,
    account_address: String,
) -> anyhow::Result<Self> {
    // Create wallet from private key
    // Initialize rate limiter (same as new())
    // Return authenticated client
}
```

Use implementation from Context Report Section 4.2.3 (lines 1003-1019).
Note: Implementation depends on hyperliquid_rust_sdk API - may need adjustment after reviewing SDK docs.

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 25
**Dependencies**: Task 17

#### Task 19: Add HyperliquidClient::post_signed() method
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After with_wallet() method
**Action**: Add method:
```rust
pub async fn post_signed(
    &self,
    endpoint: &str,
    body: serde_json::Value,
    nonce: u64,
) -> anyhow::Result<serde_json::Value> {
    // Verify wallet is present
    // Use hyperliquid_rust_sdk to sign request
    // Apply rate limiting
    // Send signed request
}
```

Use implementation outline from Context Report Section 4.2.3 (lines 1022-1045).
Note: Exact signing mechanism depends on SDK API.

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 30
**Dependencies**: Task 18

#### Task 20: Add nonce_counter to LiveExecutionHandler
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 11-20 (struct and constructor)
**Action**:
1. Add field: `nonce_counter: Arc<AtomicU64>,`
2. Update `new()` signature to accept nonce_counter
3. Store in struct

Add imports: `use std::sync::{atomic::{AtomicU64, Ordering}, Arc};`.

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 5
**Dependencies**: None

#### Task 21: Modify execute_order() to use signed requests
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 25-52 (execute_order method)
**Action**:
1. Get nonce: `let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);`
2. Replace `self.client.post()` with `self.client.post_signed(..., nonce)`
3. Parse actual fill response fields: order_id, fill price, commission
4. Handle error responses gracefully

Use implementation from Context Report Section 4.2.4 (lines 1100-1136).

**Verification**: `cargo check -p algo-trade-exchange-hyperliquid`
**Estimated LOC**: 25
**Dependencies**: Task 19, Task 20

#### Task 22: Update BotActor::initialize_system() for wallet integration
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 33-92 (initialize_system method)
**Action**: Modify to:
1. Load wallet if not present: `if self.config.wallet.is_none() { self.config.wallet = Some(WalletConfig::from_env()?); }`
2. Get wallet reference: `let wallet = self.config.wallet.as_ref().unwrap();`
3. Create authenticated client: `HyperliquidClient::with_wallet(self.config.api_url.clone(), wallet.api_wallet_private_key.clone().unwrap(), wallet.account_address.clone())?`
4. Pass nonce counter to execution handler: `LiveExecutionHandler::new(client, wallet.nonce_counter.clone())`
5. Use `SimpleRiskManager::with_leverage(self.config.risk_per_trade_pct, self.config.max_position_pct, self.config.leverage as f64)`
6. Use `TradingSystem::with_capital(..., self.config.initial_capital)`

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 15 (modifications to existing lines)
**Dependencies**: Task 21

---

### Phase 3: Position Sizing with Leverage

#### Task 23: Add leverage field to SimpleRiskManager
**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`
**Location**: Lines 8-11 (SimpleRiskManager struct)
**Action**: Add field:
```rust
leverage: Decimal,
```

**Verification**: `cargo check -p algo-trade-strategy`
**Estimated LOC**: 1
**Dependencies**: None

#### Task 24: Add SimpleRiskManager::with_leverage() constructor
**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`
**Location**: Lines 13-36 (after existing new())
**Action**: Add method:
```rust
pub fn with_leverage(
    risk_per_trade_pct: f64,
    max_position_pct: f64,
    leverage: f64,
) -> Self {
    Self {
        risk_per_trade_pct: Decimal::from_str(&risk_per_trade_pct.to_string()).unwrap(),
        max_position_pct: Decimal::from_str(&max_position_pct.to_string()).unwrap(),
        leverage: Decimal::from_str(&leverage.to_string()).unwrap(),
    }
}
```

Modify existing `new()` to delegate: `Self::with_leverage(risk_per_trade_pct, max_position_pct, 1.0)`.

**Verification**: `cargo check -p algo-trade-strategy`
**Estimated LOC**: 15
**Dependencies**: Task 23

#### Task 25: Modify position sizing logic in evaluate_signal()
**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`
**Location**: Lines 77-90 (position sizing calculation)
**Action**: Replace calculation with leveraged version:
```rust
// Step 1: Calculate leveraged capital
let leveraged_capital = account_equity * self.leverage;

// Step 2: Calculate target position value
let target_position_value = leveraged_capital * self.risk_per_trade_pct;

// Step 3: Apply maximum position limit
let max_position_value = leveraged_capital * self.max_position_pct;
let position_value = target_position_value.min(max_position_value);

// Step 4: Convert to token quantity
let target_quantity = position_value / signal.price;

// Step 5: Round to 8 decimals
let rounded_qty = (target_quantity * Decimal::from(100_000_000))
    .round() / Decimal::from(100_000_000);

// Step 6: Validate minimum order value ($10)
let order_value = rounded_qty * signal.price;
if order_value < Decimal::from(10) {
    tracing::warn!(
        "Order value ${} below Hyperliquid minimum of $10, skipping",
        order_value
    );
    return Ok(vec![]);
}
```

Use exact implementation from Context Report Section 4.3.1 (lines 1250-1276).

**Verification**: `cargo check -p algo-trade-strategy`
**Estimated LOC**: 20 (replacement)
**Dependencies**: Task 24

---

### Phase 4: TUI Enhancements

#### Task 26: Add event_subscriptions field to App struct
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 105-128 (App struct)
**Action**: Add field:
```rust
event_subscriptions: HashMap<String, broadcast::Receiver<BotEvent>>,
```

Add imports:
- `use std::collections::HashMap;`
- `use tokio::sync::broadcast;`
- `use algo_trade_bot_orchestrator::events::BotEvent;`

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 5
**Dependencies**: Task 4

#### Task 27: Initialize event_subscriptions in App constructor
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: App initialization (find struct construction)
**Action**: Initialize field with `event_subscriptions: HashMap::new()`

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 1
**Dependencies**: Task 26

#### Task 28: Add event subscription loop in run_app()
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 188-230 (run_app function, before main loop)
**Action**: Add before main loop:
```rust
// Subscribe to all bot events
for bot_id in app.registry.list_bots().await {
    if let Some(handle) = app.registry.get_bot(&bot_id).await {
        let event_rx = handle.subscribe_events();
        app.event_subscriptions.insert(bot_id.clone(), event_rx);
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 7
**Dependencies**: Task 27

#### Task 29: Add event polling in run_app() main loop
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 188-230 (run_app function, inside main loop)
**Action**: Add after `terminal.draw()`:
```rust
// Poll for events from all bots (non-blocking)
for (bot_id, event_rx) in &mut app.event_subscriptions {
    while let Ok(event) = event_rx.try_recv() {
        match event {
            BotEvent::SignalGenerated(signal) => {
                app.add_message(format!(
                    "[{}] Signal: {:?} at ${:.2}",
                    bot_id, signal.direction, signal.price
                ));
            }
            BotEvent::OrderFilled(fill) => {
                app.add_message(format!(
                    "[{}] Fill: {:?} {} @ ${:.2}",
                    bot_id, fill.direction, fill.quantity, fill.price
                ));
            }
            BotEvent::TradeClosed { pnl, win, .. } => {
                let status = if win { "WIN" } else { "LOSS" };
                app.add_message(format!(
                    "[{}] Trade closed: ${:.2} [{}]",
                    bot_id, pnl, status
                ));
            }
            BotEvent::Error { message, .. } => {
                app.add_message(format!("[{}] ERROR: {}", bot_id, message));
            }
            _ => {}
        }
    }
}
```

Use implementation from Context Report Section 4.4.2 (lines 1482-1510), but remove emoji per project guidelines.

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 30
**Dependencies**: Task 28

#### Task 30: Enhance render_bot_list() to display metrics
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 518-568 (render_bot_list function)
**Action**: Modify bot list item generation (around lines 356-362) to fetch and display status:
```rust
let items: Vec<ListItem> = app
    .cached_bots
    .iter()
    .enumerate()
    .map(|(i, bot_id)| {
        // Fetch latest status for this bot
        let status_text = if let Some(handle) = app.registry.get_bot(bot_id).await {
            let status = handle.latest_status();
            format!(
                "{} | {} | Equity: ${:.2} | Return: {:.2}% | Trades: {} | Win: {:.1}%",
                bot_id,
                status.state,
                status.current_equity,
                status.total_return_pct * 100.0,
                status.num_trades,
                status.win_rate * 100.0,
            )
        } else {
            bot_id.to_string()
        };

        let style = if i == app.selected_bot {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        ListItem::new(status_text).style(style)
    })
    .collect();
```

Note: This requires `app.registry` to be accessible. If inside async context is an issue, consider fetching statuses before rendering.

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 25 (replacement)
**Dependencies**: Task 29

#### Task 31: Add bot detail panel to render_bot_list()
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 518-568 (render_bot_list function, modify layout)
**Action**:
1. Modify layout constraints to add detail panel:
```rust
.constraints([
    Constraint::Length(3),   // Title
    Constraint::Min(10),     // Bot list
    Constraint::Length(8),   // Bot detail panel (NEW)
    Constraint::Length(10),  // Messages
    Constraint::Length(3),   // Help
])
```

2. After rendering bot list, add detail panel:
```rust
// Bot detail panel
if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
    if let Some(handle) = app.registry.get_bot(bot_id).await {
        let status = handle.latest_status();

        let mut detail_lines = vec![
            Line::from(format!("Bot: {}", status.bot_id)),
            Line::from(format!("Capital: ${:.2} -> ${:.2} ({:.2}%)",
                status.initial_capital,
                status.current_equity,
                status.total_return_pct * 100.0,
            )),
            Line::from(format!("Sharpe: {:.2} | Drawdown: {:.2}% | Win Rate: {:.1}%",
                status.sharpe_ratio,
                status.max_drawdown * 100.0,
                status.win_rate * 100.0,
            )),
            Line::from(""),
            Line::from(format!("Open Positions: {}", status.open_positions.len())),
        ];

        // Display each open position
        for pos in &status.open_positions {
            detail_lines.push(Line::from(format!(
                "  {} | Qty: {} | Avg: ${:.2} | Current: ${:.2} | PnL: ${:.2} ({:.2}%)",
                pos.symbol,
                pos.quantity,
                pos.avg_price,
                pos.current_price,
                pos.unrealized_pnl,
                pos.unrealized_pnl_pct * 100.0,
            )));
        }

        let detail = Paragraph::new(detail_lines)
            .block(Block::default().borders(Borders::ALL).title("Bot Details"));

        f.render_widget(detail, chunks[2]);
    }
}
```

Use implementation from Context Report Section 4.4.1 (lines 1395-1432).

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 40
**Dependencies**: Task 30

---

### Phase 5: Configuration & Defaults

#### Task 32: Update create_bot() to set new BotConfig fields
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
**Location**: Lines 450-507 (create_bot function)
**Action**: Add to BotConfig initialization (around line 487):
```rust
initial_capital: Decimal::from(10000),
risk_per_trade_pct: 0.05,
max_position_pct: 0.20,
leverage: 1,
margin_mode: MarginMode::Cross,
wallet: None,
```

Add imports: `use algo_trade_bot_orchestrator::commands::MarginMode;`, `use rust_decimal::Decimal;`.

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 8
**Dependencies**: Task 16

#### Task 33: Add example bot configuration to Config.toml
**File**: `/home/a/Work/algo-trade/config/Config.toml`
**Location**: End of file
**Action**: Add commented example:
```toml
# Example bot configuration
# [[bots]]
# bot_id = "btc_scalper"
# symbol = "BTC"
# strategy = "quad_ma"
# enabled = true
# interval = "1m"
# ws_url = "wss://api.hyperliquid.xyz/ws"
# api_url = "https://api.hyperliquid.xyz"
# warmup_periods = 100
# initial_capital = 10000.0
# risk_per_trade_pct = 0.05
# max_position_pct = 0.20
# leverage = 1
# margin_mode = "Cross"
```

**Verification**: Read file to verify syntax
**Estimated LOC**: 18
**Dependencies**: None

---

### Phase 6: Documentation

#### Task 34: Create WALLET_SETUP.md documentation
**File**: `/home/a/Work/algo-trade/docs/WALLET_SETUP.md`
**Location**: New file
**Action**: Create comprehensive guide covering:
1. Hyperliquid account creation
2. API wallet generation on https://app.hyperliquid.xyz/API
3. Approving API wallet for trading
4. Setting environment variables (HYPERLIQUID_ACCOUNT_ADDRESS, HYPERLIQUID_API_WALLET_KEY)
5. Format validation (42-char address, 66-char private key)
6. Security best practices:
   - Never commit private keys to git
   - Use .env files (add to .gitignore)
   - Separate API wallet per bot (avoid nonce conflicts)
   - Testnet vs mainnet considerations
7. Troubleshooting common errors

Use outline from Context Report Section 6.1 Feature 6 (line 1803-1807) and Appendix A.1 (lines 2039-2080).

**Verification**: Read file for completeness
**Estimated LOC**: 80
**Dependencies**: None

#### Task 35: Add .env to .gitignore
**File**: `/home/a/Work/algo-trade/.gitignore`
**Location**: End of file
**Action**: Add lines:
```
# Wallet credentials (NEVER commit)
.env
*.env
```

**Verification**: Read file to verify
**Estimated LOC**: 3
**Dependencies**: None

---

## Verification Checklist

### Phase 1: Event Streaming
- [ ] `cargo build -p algo-trade-bot-orchestrator` succeeds
- [ ] `cargo build -p algo-trade-core` succeeds
- [ ] BotActor spawns with broadcast/watch channels
- [ ] TUI subscribes to events without errors
- [ ] EnhancedBotStatus contains all required fields
- [ ] BotHandle provides subscribe_events() and latest_status() methods

### Phase 2: Wallet Integration
- [ ] `cargo build -p algo-trade-exchange-hyperliquid` succeeds
- [ ] `WalletConfig::from_env()` validates address and key format
- [ ] HyperliquidClient::with_wallet() creates authenticated client
- [ ] LiveExecutionHandler uses nonce counter
- [ ] BotActor initializes wallet from environment

### Phase 3: Position Sizing
- [ ] `cargo build -p algo-trade-strategy` succeeds
- [ ] SimpleRiskManager includes leverage field
- [ ] Position size calculation uses leveraged capital
- [ ] Minimum order value ($10) enforced
- [ ] Test: 10x leverage produces 10x larger position

### Phase 4: TUI Enhancements
- [ ] `cargo build -p algo-trade-cli` succeeds
- [ ] Bot list displays enhanced metrics (equity, return, trades)
- [ ] Bot detail panel shows open positions
- [ ] Events appear in messages panel
- [ ] No blocking in event polling (try_recv used)

### Phase 5: Configuration
- [ ] BotConfig includes all new fields
- [ ] Config.toml has example configuration
- [ ] Defaults are sensible (leverage=1, cross margin)

### Phase 6: Documentation
- [ ] WALLET_SETUP.md exists and is comprehensive
- [ ] .gitignore includes .env files
- [ ] Security warnings present in documentation

### Integration Testing
- [ ] Full build succeeds: `cargo build --workspace`
- [ ] Clippy passes: `cargo clippy --workspace -- -D warnings`
- [ ] Format check: `cargo fmt --check`
- [ ] Unit tests pass: `cargo test --workspace`

### Manual Testing (with Hyperliquid testnet account)
- [ ] Set environment variables (HYPERLIQUID_ACCOUNT_ADDRESS, HYPERLIQUID_API_WALLET_KEY)
- [ ] Start TUI: `cargo run -p algo-trade-cli -- live-bot`
- [ ] Create bot with 'c' key
- [ ] Start bot with 's' key
- [ ] Verify WebSocket connection in logs
- [ ] Observe events in messages panel (market updates, signals)
- [ ] Check bot list shows equity/return/trades
- [ ] Verify bot detail panel updates
- [ ] Stop bot with 'x' key
- [ ] Verify graceful shutdown

---

## Estimated Timeline

- **Phase 1**: Event Streaming Infrastructure - 2.5 hours (12 tasks)
- **Phase 2**: Wallet Integration - 3 hours (10 tasks)
- **Phase 3**: Position Sizing - 1 hour (3 tasks)
- **Phase 4**: TUI Enhancements - 2 hours (6 tasks)
- **Phase 5**: Configuration - 0.5 hours (2 tasks)
- **Phase 6**: Documentation - 0.5 hours (2 tasks)

**Total**: ~9.5 hours (35 tasks)

---

## Dependencies Graph

```
Phase 1: 1 → 2 → 3 → 4 → 5 → 6 → 8 → 9
         7 (parallel to Phase 1)
         10 → 11 → 12

Phase 2: 13 (parallel)
         14 → 15 → 16
         17 → 18 → 19 → 21
         20 (parallel to 17-19)
         22 (depends on 21)

Phase 3: 23 → 24 → 25

Phase 4: 26 → 27 → 28 → 29 → 30 → 31

Phase 5: 32, 33 (parallel)

Phase 6: 34, 35 (parallel)
```

---

## Notes for Implementation

1. **Hyperliquid SDK Integration**: Task 18-19 depend on actual hyperliquid_rust_sdk API. Review SDK documentation and examples before implementing.

2. **Async Context in TUI**: Task 30-31 require accessing async methods (app.registry.get_bot()) inside render function. May need to fetch statuses before entering render, or restructure App to cache latest statuses.

3. **Event Emission Timing**: Task 9 notes that TradingSystem doesn't expose which events were processed. Initial implementation emits generic MarketUpdate. Future enhancement: Add event drain method to TradingSystem.

4. **Minimum Order Value**: Task 25 enforces $10 minimum. Verify this is current Hyperliquid requirement.

5. **Margin Mode**: Task 14 defines MarginMode enum, but actual API usage (Task 21 note) indicates margin mode may be account-level, not per-order. Document this limitation.

6. **Security**: Task 15, 34, 35 handle wallet security. Emphasize in documentation: NEVER commit private keys, use environment variables, add .env to .gitignore.

7. **Testing Strategy**: Integration test requires Hyperliquid testnet account. Document testnet endpoint URLs if different from mainnet.

8. **Karen Review**: After each phase completion, invoke Karen agent for zero-tolerance quality review (per CLAUDE.md agent orchestration workflow).

---

**End of Playbook**

Ready for execution. After completing all tasks, invoke Karen agent for comprehensive quality review before marking feature complete.
