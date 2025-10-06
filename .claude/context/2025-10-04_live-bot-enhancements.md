# Context Report: Live Bot Operations Visibility & Hyperliquid Trading Enhancements

**Date**: 2025-10-04
**Agent**: Context Gatherer
**Request**: Enable bot operations visibility, Hyperliquid wallet integration, and position sizing/leverage configuration

---

## Section 1: Request Analysis

### User Requirements (Verbatim)
> "I am able to make a bot and press S to start and x to stop. But they are not presenting anything happening. Let's use the context manager agent and identify exactly how we can get more details of our bots operations. Since we are working with the hyperliquid dex I expect we can associate a crypto wallet that can be used to exchange with hyperliquid. We also expect to be able to configure our position sizes how much we can put into a position and how much leverage hyperliquid allows us to use."

### Decomposed Requirements

**Requirement 1: Bot Operations Visibility**
- **Explicit**: "not presenting anything happening" → Need real-time visibility of bot operations
- **Implicit**:
  - TUI should show live trading events (signals, orders, fills, PnL)
  - Bot status should include performance metrics (win rate, equity, position info)
  - Need event streaming from BotActor to TUI

**Requirement 2: Hyperliquid Wallet Integration**
- **Explicit**: "associate a crypto wallet that can be used to exchange with hyperliquid"
- **Implicit**:
  - Private key management for signing orders
  - Support for API wallets (agent wallets) to avoid nonce conflicts
  - Secure configuration via environment variables
  - Per-bot wallet configuration (multiple bots can use same wallet or separate wallets)

**Requirement 3: Position Sizing & Leverage Configuration**
- **Explicit**: "configure our position sizes", "how much leverage hyperliquid allows us to use"
- **Implicit**:
  - Max position size as percentage of capital (risk management)
  - Leverage setting per asset (1x to 50x depending on asset)
  - Cross margin vs isolated margin selection
  - Integration with RiskManager for position sizing calculations

---

## Section 2: Codebase Reconnaissance

### 2.1 Bot Actor & Status Reporting

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`

**Lines 10-15**: BotActor structure
```rust
pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
    system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,
}
```
- **Gap**: `system` is private, no way to extract live metrics (equity, PnL, fills)
- **Gap**: No event broadcasting mechanism (only request/response via GetStatus)

**Lines 33-92**: `initialize_system()` method
- Creates `LiveDataProvider` (WebSocket), `LiveExecutionHandler`, strategy, risk manager
- **Line 79-80**: RiskManager hardcoded with `SimpleRiskManager::new(0.05, 0.20)`
  - 5% risk per trade, 20% max position size
  - **Gap**: No configuration for these values
- **Line 56**: `HyperliquidClient::new(api_url)` has no wallet/private key parameter
  - **Gap**: No authentication mechanism for order execution

**Lines 95-124**: `trading_loop()` processes events
- **Line 113**: `system.process_next_event()` returns `Result<bool>` but no metrics exposed
- **Gap**: No logging of signals, orders, fills (only errors at line 115)

**Lines 169-177**: `GetStatus` command handler
```rust
BotCommand::GetStatus(tx) => {
    let status = BotStatus {
        bot_id: self.config.bot_id.clone(),
        state: self.state.clone(),
        last_heartbeat: Utc::now(),
        error: None,
    };
    let _ = tx.send(status);
}
```
- **Gap**: BotStatus only contains state (Stopped/Running/Paused/Error), no trading metrics

### 2.2 BotStatus Structure

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`

**Lines 28-34**: Current BotStatus definition
```rust
pub struct BotStatus {
    pub bot_id: String,
    pub state: BotState,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
}
```
- **Missing Fields**:
  - Current equity / capital
  - Open positions (symbol, quantity, unrealized PnL)
  - Recent fills (last N trades)
  - Performance metrics (win rate, Sharpe ratio, drawdown)
  - Signal history (recent Long/Short/Exit signals)

### 2.3 BotConfig Structure

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`

**Lines 15-26**: Current BotConfig definition
```rust
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
    pub interval: String,
    pub ws_url: String,
    pub api_url: String,
    pub warmup_periods: usize,
    pub strategy_config: Option<String>,
}
```
- **Missing Fields**:
  - `wallet_private_key: Option<String>` or `wallet_config: WalletConfig`
  - `initial_capital: Decimal`
  - `risk_per_trade_pct: f64`
  - `max_position_pct: f64`
  - `leverage: u8` (1-50x)
  - `margin_mode: MarginMode` (Cross or Isolated)

### 2.4 TradingSystem Event Processing

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`

**Lines 27-50**: TradingSystem structure
```rust
pub struct TradingSystem<D, E> {
    data_provider: D,
    execution_handler: E,
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,
    risk_manager: Arc<dyn RiskManager>,
    position_tracker: PositionTracker,
    initial_capital: Decimal,
    returns: Vec<Decimal>,
    equity_curve: Vec<Decimal>,
    wins: usize,
    losses: usize,
    // ... other metrics
}
```
- **Lines 307-388**: `process_next_event()` method
  - Tracks equity, PnL, wins/losses internally
  - **Gap**: No way to extract real-time metrics (all fields private)
  - **Line 349**: Logs fills with `tracing::info!` but no structured event emission

**Lines 63-84**: Constructor with hardcoded initial capital of $10,000
```rust
pub fn new(...) -> Self {
    let initial_capital = Decimal::from(10000); // Default $10k
    // ...
}
```
- **Gap**: No way to set custom initial capital per bot

**Lines 86-113**: `with_capital()` constructor allows custom capital
- **Integration Point**: BotActor should use this instead of `new()`

### 2.5 Position Tracker

**File**: `/home/a/Work/algo-trade/crates/core/src/position.rs`

**Lines 5-20**: Position struct
```rust
pub struct Position {
    pub symbol: String,
    pub quantity: Decimal,  // Positive = long, Negative = short
    pub avg_price: Decimal,
}
```

**Lines 23-33**: PositionTracker
```rust
pub struct PositionTracker {
    positions: HashMap<String, Position>,
}
```
- **Line 108-111**: `get_position(&str) -> Option<&Position>` public method
- **Line 113-116**: `all_positions() -> &HashMap<String, Position>` public method
- **Integration Point**: Can expose open positions via TradingSystem accessor

### 2.6 RiskManager & Position Sizing

**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`

**Lines 8-11**: SimpleRiskManager
```rust
pub struct SimpleRiskManager {
    risk_per_trade_pct: Decimal,  // e.g., 0.05 = 5%
    max_position_pct: Decimal,     // e.g., 0.20 = 20%
}
```

**Lines 13-36**: Constructor
```rust
pub fn new(risk_per_trade_pct: f64, max_position_pct: f64) -> Self {
    Self {
        risk_per_trade_pct: Decimal::from_str(&risk_per_trade_pct.to_string()).unwrap(),
        max_position_pct: Decimal::from_str(&max_position_pct.to_string()).unwrap(),
    }
}
```
- **Integration Point**: BotConfig should specify these parameters

**Lines 77-90**: Position sizing calculation
```rust
// Step 1: Calculate target position value in USDC
let target_position_value = account_equity * self.risk_per_trade_pct;

// Step 2: Apply maximum position limit
let max_position_value = account_equity * self.max_position_pct;
let position_value = target_position_value.min(max_position_value);

// Step 3: Convert USDC value to token quantity
let target_quantity = position_value / signal.price;

// Step 4: Round to 8 decimal places
let rounded_qty = (target_quantity * Decimal::from(100_000_000))
    .round() / Decimal::from(100_000_000);
```
- **Current Behavior**: Uses equity-based sizing (good for backtesting)
- **Gap**: No leverage multiplier applied (Hyperliquid supports 1x-50x leverage)
- **Enhancement Needed**: `position_value = (account_equity * risk_pct) * leverage`

### 2.7 Hyperliquid Execution Handler

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`

**Lines 11-20**: LiveExecutionHandler structure
```rust
pub struct LiveExecutionHandler {
    client: HyperliquidClient,
}

impl LiveExecutionHandler {
    #[must_use]
    pub const fn new(client: HyperliquidClient) -> Self {
        Self { client }
    }
}
```
- **Gap**: No wallet/private key parameter for signing orders

**Lines 25-52**: `execute_order()` implementation
```rust
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
    // ...
}
```
- **Gap**: No signature/authentication (order will be rejected by Hyperliquid)
- **Required**: EIP-712 signing with private key (see Section 3)

### 2.8 Hyperliquid Client

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`

**Lines 15-37**: HyperliquidClient structure
```rust
pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<...>>,
}

impl HyperliquidClient {
    pub fn new(base_url: String) -> Self {
        let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
        // ...
    }
}
```
- **Gap**: No wallet/private key storage
- **Gap**: No signing methods for authenticated requests

**Lines 51-61**: POST method (unsigned)
```rust
pub async fn post(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    self.rate_limiter.until_ready().await;
    let url = format!("{}{}", self.base_url, endpoint);
    let response = self.http_client.post(&url).json(&body).send().await?;
    let json = response.json().await?;
    Ok(json)
}
```
- **Required**: Add `post_signed()` method with EIP-712 signature

### 2.9 TUI Bot List Rendering

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`

**Lines 518-568**: `render_bot_list()` function
```rust
fn render_bot_list(f: &mut Frame, app: &App) {
    // ...
    let items: Vec<ListItem> = app
        .cached_bots
        .iter()
        .enumerate()
        .map(|(i, bot_id)| {
            let style = if i == app.selected_bot {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(bot_id.as_str()).style(style)
        })
        .collect();
    // ...
}
```
- **Gap**: Only displays bot_id (e.g., "bot_20251004_120530_BTC")
- **Enhancement Needed**: Show state, equity, position, recent PnL

**Lines 241-291**: `handle_bot_list_keys()` - Key event handlers
- **Line 250-257**: Start bot with 's' key
- **Line 259-267**: Stop bot with 'x' key
- **Gap**: No status polling loop to refresh metrics
- **Gap**: No detail view for individual bot operations

### 2.10 Event Types

**File**: `/home/a/Work/algo-trade/crates/core/src/events.rs`

**Lines 6-28**: MarketEvent, SignalEvent, OrderEvent, FillEvent defined
- **All events are processed internally** (not exposed to TUI)
- **Integration Point**: Broadcast these events to TUI via `tokio::sync::broadcast` channel

---

## Section 3: External Research

### 3.1 Hyperliquid Wallet & Authentication

#### 3.1.1 API Wallet (Agent Wallet) System

**Source**: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/nonces-and-api-wallets

**Key Findings**:
1. **Master Account vs API Wallet**:
   - Master account (your main wallet) has a private key
   - API wallets are "agent wallets" approved to sign on behalf of master account
   - **Critical**: When querying account data, use master address, NOT agent address
   - **Best Practice**: Generate separate API wallet per trading bot to avoid nonce conflicts

2. **Nonce Management**:
   - Nonces tracked per signer (API wallet or master private key)
   - Must be within `(current_time - 2 days, current_time + 1 day)`
   - Hyperliquid stores 100 highest nonces per address
   - New transaction nonce must be > smallest nonce in this set
   - **Recommendation**: Use atomic counter to ensure unique nonces

3. **API Wallet Limits**:
   - 1 unnamed approved wallet + 3 named wallets per master account
   - Additional 2 named agents per subaccount
   - **Important**: Do NOT reuse API wallet addresses after deregistration

4. **Wallet Expiration**:
   - API wallets may be pruned if:
     - Deregistered
     - Wallet expires
     - Account loses funds

#### 3.1.2 Order Signing (EIP-712)

**Source**: https://docs.chainstack.com/docs/hyperliquid-signing-overview, Web search results

**Signing Mechanism**:
1. **L1 Actions** (Chain ID 1337):
   - Trading operations: place order, cancel order
   - Uses "phantom agent" mechanism
   - Requires msgpack serialization + EIP-712 signing

2. **User-Signed Actions** (Chain ID 0x66eee):
   - Account management: withdraw, transfer
   - Standard EIP-712 typed data signing

**Rust Implementation Requirements**:
- EIP-712 signing library (e.g., `ethers-rs` or `alloy`)
- msgpack serialization (e.g., `rmp-serde`)
- Wallet creation from private key (secp256k1 curve)

#### 3.1.3 Hyperliquid Rust SDK

**Source**: https://github.com/hyperliquid-dex/hyperliquid-rust-sdk, crates.io

**Available Crates**:
1. **hyperliquid_rust_sdk** (Official):
   - Version: 0.6.0
   - Provides signing, order placement, account queries
   - Examples in `src/bin/` directory
   - **Recommendation**: Use this for wallet integration

2. **hl_ranger** (Fork with unsigned transactions):
   - Adds `UnsignedTransactionBuilder`
   - Useful for hardware wallets, multi-sig setups
   - **Use Case**: If users want to sign externally (e.g., Ledger)

**Integration Pattern**:
```rust
use hyperliquid_rust_sdk::{Hyperliquid, Wallet};

// Create wallet from private key
let wallet = Wallet::from_private_key("0x...")?;

// Create client
let client = Hyperliquid::new(wallet, api_url)?;

// Place order
let order = client.place_order(symbol, is_buy, size, price, order_type).await?;
```

### 3.2 Hyperliquid Leverage & Position Sizing

#### 3.2.1 Leverage Configuration

**Source**: https://hyperliquid.gitbook.io/hyperliquid-docs/trading/margining

**Leverage Ranges by Asset**:
- **BTC, ETH**: Up to 50x
- **Major altcoins**: 20x - 40x
- **Long-tail assets**: 3x - 10x

**Margin Modes**:
1. **Cross Margin (Default)**:
   - Shares collateral across all positions
   - Maximal capital efficiency
   - Account liquidated if total account value < maintenance margin
   - Formula: `margin_required = position_size * mark_price / leverage`

2. **Isolated Margin**:
   - Collateral constrained to single asset
   - Can add/remove margin after opening
   - Only that position liquidated if margin insufficient
   - Requires 10% of total position value to remain when transferring out

**Leverage Setting**:
- Leverage only checked upon opening position
- Can increase leverage without closing position
- Cannot decrease leverage below current usage

**Maintenance Margin**:
- Currently set to **half of initial margin at max leverage**
- Example: 20x max leverage → maintenance margin = 2.5%
- Range: 1.25% (40x assets) to 16.7% (3x assets)

#### 3.2.2 Margin Tiers

**Source**: Web search results (2025 data)

**Tiered Margin Examples** (varies by asset):
- **$0 - $500K**: 2% initial, 1% maintenance
- **$500K - $1M**: 3% initial, 1.5% maintenance
- **$1M+**: 5% initial, 2.5% maintenance

**Alternative Data** (some assets have flat structure):
- 1% initial margin uniformly
- 0.5% maintenance margin uniformly

**API Endpoint for Leverage Setting**:
- **No documented REST endpoint** for setting leverage programmatically
- Leverage implied by position size relative to margin
- **Workaround**: Calculate position size based on desired leverage
  - `position_size = (capital * leverage) / price`

#### 3.2.3 Position Sizing Best Practices

**Minimum Order Value**: $10 (Hyperliquid requirement)

**Risk-Based Position Sizing with Leverage**:
```
// Without leverage (current implementation)
position_value = account_equity * risk_per_trade_pct

// With leverage
leveraged_capital = account_equity * leverage
position_value = leveraged_capital * risk_per_trade_pct

// Effective position size
position_size = position_value / price
```

**Example**:
- Account equity: $10,000
- Risk per trade: 5% ($500)
- Leverage: 10x
- Price: $50,000

```
Leveraged capital = $10,000 * 10 = $100,000
Position value = $100,000 * 5% = $5,000
Position size = $5,000 / $50,000 = 0.1 BTC
```

**Without leverage**: 0.01 BTC
**With 10x leverage**: 0.1 BTC (10x larger position)

### 3.3 Event Streaming Patterns (Tokio)

**Source**: Alice Ryhl's Actor Guide (referenced in CLAUDE.md)

**Broadcasting Events from Actor**:

**Pattern 1: `tokio::sync::broadcast`** (Multiple subscribers)
```rust
pub struct BotActor {
    // Existing fields...
    event_tx: broadcast::Sender<BotEvent>,
}

// In TUI
let mut event_rx = bot_handle.subscribe_events();
while let Ok(event) = event_rx.recv().await {
    // Update UI with event
}
```
- **Use Case**: TUI, web API, logging all need events
- **Limitation**: Slow subscribers can cause lag (channel overflow)

**Pattern 2: `tokio::sync::watch`** (Latest value)
```rust
pub struct BotActor {
    status_tx: watch::Sender<BotStatus>,
}

// In TUI polling loop
let latest_status = status_rx.borrow().clone();
```
- **Use Case**: Status updates (state, equity, position)
- **Benefit**: Always get latest value, never blocks

**Pattern 3: Hybrid Approach** (Recommended)
- `broadcast` for discrete events (signals, orders, fills)
- `watch` for continuous state (status, metrics)

---

## Section 4: Architectural Recommendations

### 4.1 Real-Time Bot Event Streaming

**Proposed Design**: Hybrid event streaming with broadcast + watch channels

#### 4.1.1 New Event Types

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs` (NEW)
```rust
use algo_trade_core::events::{FillEvent, OrderEvent, SignalEvent};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BotEvent {
    /// Market event received (candle close)
    MarketUpdate {
        symbol: String,
        price: Decimal,
        volume: Decimal,
        timestamp: DateTime<Utc>,
    },

    /// Strategy generated signal
    SignalGenerated(SignalEvent),

    /// Order submitted to exchange
    OrderPlaced(OrderEvent),

    /// Order filled by exchange
    OrderFilled(FillEvent),

    /// Position opened/modified/closed
    PositionUpdate {
        symbol: String,
        quantity: Decimal,
        avg_price: Decimal,
        unrealized_pnl: Decimal,
    },

    /// Trade closed (realized PnL)
    TradeClosed {
        symbol: String,
        pnl: Decimal,
        win: bool,
    },

    /// Error occurred
    Error {
        message: String,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedBotStatus {
    pub bot_id: String,
    pub state: BotState,
    pub last_heartbeat: DateTime<Utc>,

    // Performance metrics
    pub current_equity: Decimal,
    pub initial_capital: Decimal,
    pub total_return_pct: f64,
    pub sharpe_ratio: f64,
    pub max_drawdown: f64,
    pub win_rate: f64,
    pub num_trades: usize,

    // Open positions
    pub open_positions: Vec<PositionInfo>,

    // Recent events (last 10)
    pub recent_events: Vec<BotEvent>,

    // Error if any
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionInfo {
    pub symbol: String,
    pub quantity: Decimal,
    pub avg_price: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub unrealized_pnl_pct: f64,
}
```

#### 4.1.2 Modified BotActor with Event Streaming

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`

**Changes at lines 10-18**:
```rust
pub struct BotActor {
    config: BotConfig,
    state: BotState,
    rx: mpsc::Receiver<BotCommand>,
    system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,

    // Event streaming
    event_tx: broadcast::Sender<BotEvent>,  // For discrete events
    status_tx: watch::Sender<EnhancedBotStatus>,  // For continuous status
    recent_events: VecDeque<BotEvent>,  // Ring buffer (last 10 events)
}
```

**Changes in `trading_loop()` (lines 95-124)**:
```rust
async fn trading_loop(&mut self) -> Result<()> {
    if let Some(ref mut system) = self.system {
        loop {
            tokio::select! {
                cmd = self.rx.recv() => { /* ... */ }

                result = system.process_next_event() => {
                    if let Ok(true) = result {
                        // Extract events from TradingSystem (new accessor methods needed)
                        let events = system.drain_events();

                        for event in events {
                            // Broadcast to subscribers
                            let _ = self.event_tx.send(event.clone());

                            // Add to recent events ring buffer
                            self.recent_events.push_back(event);
                            if self.recent_events.len() > 10 {
                                self.recent_events.pop_front();
                            }
                        }

                        // Update status with latest metrics
                        let status = self.build_enhanced_status();
                        let _ = self.status_tx.send(status);
                    } else if let Err(e) = result {
                        // Emit error event
                        let error_event = BotEvent::Error {
                            message: e.to_string(),
                            timestamp: Utc::now(),
                        };
                        let _ = self.event_tx.send(error_event);
                        self.state = BotState::Error;
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

fn build_enhanced_status(&self) -> EnhancedBotStatus {
    let system = self.system.as_ref().unwrap();

    EnhancedBotStatus {
        bot_id: self.config.bot_id.clone(),
        state: self.state.clone(),
        last_heartbeat: Utc::now(),
        current_equity: system.current_equity(),  // NEW accessor
        initial_capital: system.initial_capital(),
        total_return_pct: system.total_return_pct(),
        sharpe_ratio: system.sharpe_ratio(),
        max_drawdown: system.max_drawdown(),
        win_rate: system.win_rate(),
        num_trades: system.num_trades(),
        open_positions: system.open_positions().iter().map(|(symbol, pos)| {
            PositionInfo {
                symbol: symbol.clone(),
                quantity: pos.quantity,
                avg_price: pos.avg_price,
                current_price: system.current_price(symbol).unwrap_or(Decimal::ZERO),
                unrealized_pnl: system.unrealized_pnl(symbol).unwrap_or(Decimal::ZERO),
                unrealized_pnl_pct: /* calculate */,
            }
        }).collect(),
        recent_events: self.recent_events.iter().cloned().collect(),
        error: if self.state == BotState::Error { Some("Trading error".to_string()) } else { None },
    }
}
```

#### 4.1.3 Modified BotHandle with Event Subscription

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs`

**Changes at lines 5-9**:
```rust
#[derive(Clone)]
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
    event_rx: broadcast::Receiver<BotEvent>,  // Clone creates new receiver
    status_rx: watch::Receiver<EnhancedBotStatus>,
}

impl BotHandle {
    pub fn new(
        tx: mpsc::Sender<BotCommand>,
        event_rx: broadcast::Receiver<BotEvent>,
        status_rx: watch::Receiver<EnhancedBotStatus>,
    ) -> Self {
        Self { tx, event_rx, status_rx }
    }

    /// Subscribe to bot events (for TUI, web API, logging)
    pub fn subscribe_events(&self) -> broadcast::Receiver<BotEvent> {
        self.event_rx.resubscribe()
    }

    /// Get latest bot status (non-blocking)
    pub fn latest_status(&self) -> EnhancedBotStatus {
        self.status_rx.borrow().clone()
    }

    /// Watch for status changes
    pub async fn wait_for_status_change(&mut self) -> Result<EnhancedBotStatus> {
        self.status_rx.changed().await?;
        Ok(self.status_rx.borrow().clone())
    }
}
```

#### 4.1.4 TradingSystem Accessor Methods (NEW)

**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`

**Add at end of TradingSystem impl block (after line 405)**:
```rust
// Public accessor methods for live monitoring
impl<D, E> TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
    pub fn current_equity(&self) -> Decimal {
        *self.equity_curve.last().unwrap_or(&self.initial_capital)
    }

    pub const fn initial_capital(&self) -> Decimal {
        self.initial_capital
    }

    pub fn total_return_pct(&self) -> f64 {
        let current = self.current_equity();
        ((current - self.initial_capital) / self.initial_capital)
            .to_string().parse().unwrap_or(0.0)
    }

    pub fn sharpe_ratio(&self) -> f64 {
        // Extract from calculate_metrics() logic
        if self.returns.is_empty() { return 0.0; }

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
        if std_dev > 0.0 {
            mean_return / std_dev * (252.0_f64).sqrt()
        } else {
            0.0
        }
    }

    pub fn max_drawdown(&self) -> f64 {
        self.calculate_max_drawdown().to_string().parse().unwrap_or(0.0)
    }

    pub fn win_rate(&self) -> f64 {
        let total = self.wins + self.losses;
        if total > 0 {
            self.wins as f64 / total as f64
        } else {
            0.0
        }
    }

    pub const fn num_trades(&self) -> usize {
        self.wins + self.losses
    }

    pub fn open_positions(&self) -> &HashMap<String, Position> {
        self.position_tracker.all_positions()
    }

    pub fn unrealized_pnl(&self, symbol: &str, current_price: Decimal) -> Option<Decimal> {
        self.position_tracker.get_position(symbol).map(|pos| {
            (current_price - pos.avg_price) * pos.quantity
        })
    }
}
```

### 4.2 Hyperliquid Wallet Integration

**Proposed Design**: Wallet configuration per bot with API wallet support

#### 4.2.1 Wallet Configuration Structure

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`

**Add after BotConfig (line 26)**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    /// Master account address (42-character hex)
    pub account_address: String,

    /// API wallet private key (0x-prefixed hex, 64 chars)
    /// SECURITY: Should be loaded from env var, not stored in config file
    #[serde(skip_serializing)]
    pub api_wallet_private_key: Option<String>,

    /// Nonce counter (atomic, per-wallet)
    /// Not serialized - managed at runtime
    #[serde(skip)]
    pub nonce_counter: Arc<AtomicU64>,
}

impl WalletConfig {
    /// Load wallet from environment variables
    ///
    /// Expected env vars:
    /// - HYPERLIQUID_ACCOUNT_ADDRESS: Master account address
    /// - HYPERLIQUID_API_WALLET_KEY: API wallet private key
    pub fn from_env() -> Result<Self> {
        let account_address = std::env::var("HYPERLIQUID_ACCOUNT_ADDRESS")
            .context("Missing HYPERLIQUID_ACCOUNT_ADDRESS env var")?;

        let api_wallet_private_key = std::env::var("HYPERLIQUID_API_WALLET_KEY")
            .context("Missing HYPERLIQUID_API_WALLET_KEY env var")?;

        // Validate address format (42-char hex)
        if !account_address.starts_with("0x") || account_address.len() != 42 {
            anyhow::bail!("Invalid account address format: must be 0x-prefixed 42-char hex");
        }

        // Validate private key format (64-char hex + 0x prefix)
        if !api_wallet_private_key.starts_with("0x") || api_wallet_private_key.len() != 66 {
            anyhow::bail!("Invalid private key format: must be 0x-prefixed 66-char hex");
        }

        Ok(Self {
            account_address,
            api_wallet_private_key: Some(api_wallet_private_key),
            nonce_counter: Arc::new(AtomicU64::new(
                Utc::now().timestamp_millis() as u64
            )),
        })
    }

    /// Get next nonce (atomic increment)
    pub fn next_nonce(&self) -> u64 {
        self.nonce_counter.fetch_add(1, Ordering::SeqCst)
    }
}
```

#### 4.2.2 Updated BotConfig with Wallet Support

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`

**Modify BotConfig (lines 15-26)**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,
    pub interval: String,
    pub ws_url: String,
    pub api_url: String,
    pub warmup_periods: usize,
    pub strategy_config: Option<String>,

    // NEW: Trading parameters
    pub initial_capital: Decimal,
    pub risk_per_trade_pct: f64,  // 0.0 - 1.0 (e.g., 0.05 = 5%)
    pub max_position_pct: f64,     // 0.0 - 1.0 (e.g., 0.20 = 20%)

    // NEW: Hyperliquid-specific
    pub leverage: u8,              // 1 - 50x (asset-dependent)
    pub margin_mode: MarginMode,   // Cross or Isolated

    // NEW: Wallet configuration
    /// Wallet config (loaded from env vars, not config file)
    #[serde(skip)]
    pub wallet: Option<WalletConfig>,
}

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

#### 4.2.3 Hyperliquid Client with Signing

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`

**Add dependency to Cargo.toml**:
```toml
hyperliquid_rust_sdk = "0.6.0"
ethers = "2.0"  # For wallet/signing if not using SDK
```

**Modify HyperliquidClient (lines 15-37)**:
```rust
use hyperliquid_rust_sdk::{Hyperliquid, Wallet};

pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<...>>,

    // NEW: Wallet for signing
    wallet: Option<Wallet>,
    account_address: Option<String>,
}

impl HyperliquidClient {
    pub fn new(base_url: String) -> Self {
        // Unsigned client (for public endpoints)
        // ...
    }

    /// Create authenticated client with wallet
    pub fn with_wallet(
        base_url: String,
        api_wallet_private_key: String,
        account_address: String,
    ) -> Result<Self> {
        let wallet = Wallet::from_private_key(&api_wallet_private_key)?;

        Ok(Self {
            http_client: Client::new(),
            base_url,
            rate_limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(20).unwrap())
            )),
            wallet: Some(wallet),
            account_address: Some(account_address),
        })
    }

    /// Sign and POST authenticated request
    pub async fn post_signed(
        &self,
        endpoint: &str,
        body: serde_json::Value,
        nonce: u64,
    ) -> Result<serde_json::Value> {
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client not authenticated"))?;

        // Use hyperliquid_rust_sdk to sign
        let hyperliquid = Hyperliquid::new(
            wallet.clone(),
            &self.base_url,
        )?;

        // Sign and send request
        // (Exact implementation depends on hyperliquid_rust_sdk API)
        // For now, assume SDK handles signing internally

        self.rate_limiter.until_ready().await;
        let response = hyperliquid.raw_request(endpoint, body, nonce).await?;
        Ok(response)
    }
}
```

**Alternative (Manual Signing)**:
If not using SDK, implement EIP-712 signing:
```rust
use ethers::types::{transaction::eip712::Eip712, H256, U256};
use ethers::signers::{LocalWallet, Signer};

async fn sign_order(
    wallet: &LocalWallet,
    order_data: OrderData,
    nonce: u64,
) -> Result<Signature> {
    // Construct EIP-712 typed data
    let domain = eip712::EIP712Domain {
        name: Some("Hyperliquid".to_string()),
        version: Some("1".to_string()),
        chain_id: Some(U256::from(1337)),  // L1 actions
        // ...
    };

    let typed_data = TypedData {
        domain,
        types: /* order types */,
        primary_type: "Order".to_string(),
        message: /* order data */,
    };

    // Sign
    let signature = wallet.sign_typed_data(&typed_data).await?;
    Ok(signature)
}
```

#### 4.2.4 LiveExecutionHandler with Wallet

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`

**Modify LiveExecutionHandler (lines 11-21)**:
```rust
pub struct LiveExecutionHandler {
    client: HyperliquidClient,
    nonce_counter: Arc<AtomicU64>,  // Shared with WalletConfig
}

impl LiveExecutionHandler {
    pub fn new(client: HyperliquidClient, nonce_counter: Arc<AtomicU64>) -> Self {
        Self { client, nonce_counter }
    }
}
```

**Modify execute_order (lines 25-52)**:
```rust
async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
    let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);

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

    // Use signed POST
    let response = self.client.post_signed("/exchange", order_payload, nonce).await?;

    // Parse response
    let fill = FillEvent {
        order_id: response["status"]["oid"]
            .as_u64()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        symbol: order.symbol,
        direction: order.direction,
        quantity: order.quantity,
        price: response["status"]["px"]
            .as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(order.price.unwrap_or(Decimal::ZERO)),
        commission: Decimal::ZERO, // Parse from response if available
        timestamp: Utc::now(),
    };

    Ok(fill)
}
```

#### 4.2.5 BotActor Wallet Integration

**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`

**Modify initialize_system (lines 33-92)**:
```rust
async fn initialize_system(&mut self) -> Result<()> {
    tracing::info!("Initializing trading system for bot {}", self.config.bot_id);

    // Load wallet if not present
    if self.config.wallet.is_none() {
        self.config.wallet = Some(WalletConfig::from_env()?);
    }

    let wallet = self.config.wallet.as_ref().unwrap();

    // Create live data provider
    let mut data_provider = LiveDataProvider::new(
        self.config.ws_url.clone(),
        self.config.symbol.clone(),
        self.config.interval.clone(),
    ).await?;

    // Warmup
    let warmup_events = data_provider.warmup(
        self.config.api_url.clone(),
        self.config.warmup_periods,
    ).await?;

    // Create authenticated HTTP client
    let client = HyperliquidClient::with_wallet(
        self.config.api_url.clone(),
        wallet.api_wallet_private_key.clone().unwrap(),
        wallet.account_address.clone(),
    )?;

    // Create execution handler with shared nonce counter
    let execution_handler = LiveExecutionHandler::new(
        client,
        wallet.nonce_counter.clone(),
    );

    // Create strategy
    let strategy = create_strategy(/* ... */)?;

    // Feed warmup events
    {
        let mut strat = strategy.lock().await;
        for event in warmup_events {
            let _ = strat.on_market_event(&event).await?;
        }
    }

    // Create risk manager with configured parameters
    let risk_manager: Arc<dyn RiskManager> = Arc::new(
        SimpleRiskManager::new(
            self.config.risk_per_trade_pct,
            self.config.max_position_pct,
        )
    );

    // Create trading system with custom capital
    let system = TradingSystem::with_capital(
        data_provider,
        execution_handler,
        vec![strategy],
        risk_manager,
        self.config.initial_capital,
    );

    self.system = Some(system);
    Ok(())
}
```

### 4.3 Position Sizing & Leverage Configuration

**Proposed Design**: Extend RiskManager to support leverage

#### 4.3.1 Enhanced RiskManager with Leverage

**File**: `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`

**Modify SimpleRiskManager (lines 8-11)**:
```rust
pub struct SimpleRiskManager {
    risk_per_trade_pct: Decimal,
    max_position_pct: Decimal,
    leverage: Decimal,  // NEW: 1x - 50x
}

impl SimpleRiskManager {
    pub fn new(risk_per_trade_pct: f64, max_position_pct: f64) -> Self {
        Self::with_leverage(risk_per_trade_pct, max_position_pct, 1.0)
    }

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
}
```

**Modify evaluate_signal position sizing (lines 77-90)**:
```rust
// Step 1: Calculate leveraged capital
let leveraged_capital = account_equity * self.leverage;

// Step 2: Calculate target position value
let target_position_value = leveraged_capital * self.risk_per_trade_pct;

// Step 3: Apply maximum position limit (also leveraged)
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

**Example Calculation**:
- Account equity: $10,000
- Risk per trade: 5%
- Max position: 20%
- Leverage: 10x
- BTC price: $50,000

```
Leveraged capital = $10,000 * 10 = $100,000
Target position value = $100,000 * 5% = $5,000
Max position value = $100,000 * 20% = $20,000
Position value = min($5,000, $20,000) = $5,000
Position size = $5,000 / $50,000 = 0.1 BTC

Margin required = $5,000 / 10 = $500 (5% of equity)
```

#### 4.3.2 Margin Mode Handling

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`

**Add margin mode to order payload**:
```rust
async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
    // ... existing code ...

    let order_payload = json!({
        "type": match order.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
        },
        "coin": order.symbol,
        "is_buy": matches!(order.direction, OrderDirection::Buy),
        "sz": order.quantity.to_string(),
        "limit_px": order.price.map(|p| p.to_string()),

        // NEW: Margin mode (if supported by API)
        // Note: Hyperliquid API docs don't show explicit margin_mode field
        // Margin mode may be account-level setting, not per-order
        // Research needed: Check if order submission supports margin mode
    });

    // ... rest of implementation
}
```

**Note**: Hyperliquid documentation doesn't clearly specify per-order margin mode. It may be:
1. **Account-level setting** (set once via web UI or API)
2. **Inferred from position** (cross = shared collateral, isolated = separate)

**Recommendation**: Start with cross margin (default), add isolated support after testing.

### 4.4 TUI Enhancements

**Proposed Design**: Multi-pane TUI with bot detail view

#### 4.4.1 Enhanced Bot List View

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`

**Modify render_bot_list (lines 518-568)**:
```rust
fn render_bot_list(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Min(10),     // Bot list
            Constraint::Length(8),   // Bot detail panel (NEW)
            Constraint::Length(10),  // Messages
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    // ... existing code ...

    // Bot list with enhanced display
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
                    status.state,  // Running/Stopped/Error
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

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Bots"))
        .highlight_style(Style::default().bg(Color::DarkGray));

    f.render_widget(list, chunks[1]);

    // Bot detail panel (NEW)
    if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
        if let Some(handle) = app.registry.get_bot(bot_id).await {
            let status = handle.latest_status();

            let detail_lines = vec![
                Line::from(format!("Bot: {}", status.bot_id)),
                Line::from(format!("Capital: ${:.2} → ${:.2} ({:.2}%)",
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
                // Display each open position
                ..status.open_positions.iter().map(|pos| {
                    Line::from(format!(
                        "  {} | Qty: {} | Avg: ${:.2} | Current: ${:.2} | PnL: ${:.2} ({:.2}%)",
                        pos.symbol,
                        pos.quantity,
                        pos.avg_price,
                        pos.current_price,
                        pos.unrealized_pnl,
                        pos.unrealized_pnl_pct * 100.0,
                    ))
                }).collect::<Vec<_>>(),
            ];

            let detail = Paragraph::new(detail_lines)
                .block(Block::default().borders(Borders::ALL).title("Bot Details"));

            f.render_widget(detail, chunks[2]);
        }
    }

    // Messages
    // ... existing code ...
}
```

#### 4.4.2 Real-Time Event Streaming in TUI

**Modify App struct (lines 105-128)**:
```rust
struct App {
    registry: Arc<BotRegistry>,
    current_screen: BotScreen,

    // Bot list
    cached_bots: Vec<String>,
    selected_bot: usize,
    messages: Vec<String>,

    // NEW: Event subscription
    event_subscriptions: HashMap<String, broadcast::Receiver<BotEvent>>,

    // ... rest of fields
}
```

**Add event polling in run_app (lines 188-230)**:
```rust
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    // Subscribe to all bot events
    for bot_id in app.registry.list_bots().await {
        if let Some(handle) = app.registry.get_bot(&bot_id).await {
            let event_rx = handle.subscribe_events();
            app.event_subscriptions.insert(bot_id.clone(), event_rx);
        }
    }

    loop {
        // Refresh bot list
        if app.current_screen == BotScreen::BotList {
            app.cached_bots = app.registry.list_bots().await;
        }

        terminal.draw(|f| ui(f, app))?;

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
                        let emoji = if win { "✅" } else { "❌" };
                        app.add_message(format!(
                            "[{}] Trade closed: ${:.2} {}",
                            bot_id, pnl, emoji
                        ));
                    }
                    BotEvent::Error { message, .. } => {
                        app.add_message(format!("[{}] ERROR: {}", bot_id, message));
                    }
                    _ => {}
                }
            }
        }

        // Keyboard input handling
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(key, app).await? {
                    break;
                }
            }
        }
    }

    Ok(())
}
```

---

## Section 5: Edge Cases & Constraints

### 5.1 Wallet & Authentication Edge Cases

**Edge Case 1: Missing Environment Variables**
- **Scenario**: User starts bot without setting `HYPERLIQUID_ACCOUNT_ADDRESS` or `HYPERLIQUID_API_WALLET_KEY`
- **Impact**: Bot crashes on initialization
- **Solution**:
  - Early validation in `WalletConfig::from_env()`
  - Return descriptive error: "Missing HYPERLIQUID_ACCOUNT_ADDRESS. Set it with: export HYPERLIQUID_ACCOUNT_ADDRESS=0x..."
  - TUI should catch this error and display in messages panel

**Edge Case 2: Invalid Private Key Format**
- **Scenario**: User provides malformed private key (wrong length, invalid hex)
- **Impact**: Wallet creation fails, all orders rejected
- **Solution**:
  - Validate format in `WalletConfig::from_env()` (must be 0x + 64 hex chars)
  - Test wallet signing with test message before starting trading
  - Display clear error: "Invalid private key format"

**Edge Case 3: API Wallet Not Approved**
- **Scenario**: User generates API wallet but doesn't approve it on Hyperliquid web UI
- **Impact**: All order submissions return "User or API Wallet does not exist"
- **Solution**:
  - Document setup process: "Go to https://app.hyperliquid.xyz/API → Generate API wallet → Approve"
  - First order failure should emit BotEvent::Error with actionable message
  - Bot should pause (not crash) to allow user to fix

**Edge Case 4: Nonce Conflicts (Multiple Bots, Same Wallet)**
- **Scenario**: Two bots share same API wallet, nonces collide
- **Impact**: Some orders rejected with "Invalid nonce"
- **Solution**:
  - Use shared `Arc<AtomicU64>` for nonce counter if wallets shared
  - **Recommended**: One API wallet per bot (Hyperliquid allows 4 per account)
  - Document best practice: "Generate separate API wallet for each bot"

**Edge Case 5: Wallet Expiration/Pruning**
- **Scenario**: API wallet pruned due to account losing funds or manual deregistration
- **Impact**: All subsequent orders fail
- **Solution**:
  - Detect authentication errors (e.g., 403 Forbidden)
  - Emit BotEvent::Error("API wallet expired or deregistered")
  - Pause bot with actionable error message

### 5.2 Position Sizing Edge Cases

**Edge Case 6: Insufficient Capital for Leveraged Position**
- **Scenario**:
  - Account equity: $100
  - Leverage: 10x
  - Position value: $1,000 (10% of leveraged capital)
  - Margin required: $100 (entire balance)
- **Impact**: Position opens but no room for drawdown → immediate liquidation risk
- **Solution**:
  - Add margin safety buffer (e.g., use 80% of available margin max)
  - RiskManager validation: `required_margin <= account_equity * 0.8`
  - Reject signal if margin insufficient, log warning

**Edge Case 7: Position Below Minimum Order Value ($10)**
- **Scenario**:
  - Account equity: $50
  - Risk per trade: 5% = $2.50
  - Leverage: 1x
  - Position value: $2.50 < $10 minimum
- **Impact**: Order rejected by Hyperliquid
- **Solution**:
  - Already handled in Section 4.3.1 (validate $10 minimum)
  - Skip order, log: "Position value $2.50 below Hyperliquid minimum, skipping signal"
  - Emit BotEvent::Error for visibility

**Edge Case 8: Leverage Exceeds Asset Maximum**
- **Scenario**: User sets leverage=50x for asset with max 20x
- **Impact**: Order rejected or auto-adjusted by Hyperliquid
- **Solution**:
  - Fetch asset metadata (max leverage) from Hyperliquid API on startup
  - Validate `bot_config.leverage <= asset_max_leverage`
  - Clamp to max if exceeded, log warning

**Edge Case 9: Cross Margin Shared Across Multiple Positions**
- **Scenario**: Bot opens positions in BTC and ETH (cross margin mode)
- **Impact**: Both positions share same collateral → one bad trade can liquidate both
- **Solution**:
  - Track total margin usage across all positions
  - RiskManager should consider aggregate exposure: `sum(all_position_margins) <= account_equity * 0.8`
  - Consider isolated margin mode for independent risk

### 5.3 Event Streaming Edge Cases

**Edge Case 10: Slow TUI Consumer (Broadcast Channel Lag)**
- **Scenario**: Bot generates 100 events/sec, TUI consumes 10/sec
- **Impact**: Broadcast channel overflows, events dropped (TUI receives "lagged" error)
- **Solution**:
  - Set broadcast channel capacity (e.g., 1000 events)
  - TUI handles `RecvError::Lagged(n)` gracefully: "Skipped {n} events (TUI too slow)"
  - Rate-limit event emission: Only emit every Nth event, or batch events

**Edge Case 11: Bot Crashes After Event Emission**
- **Scenario**: Bot emits BotEvent::OrderFilled, then crashes before updating status
- **Impact**: TUI shows outdated status (no position update)
- **Solution**:
  - Status updates via `watch` channel are atomic (always consistent)
  - TUI should prioritize `watch` channel (status) over `broadcast` (events)
  - Periodically poll status even without events

**Edge Case 12: Multiple TUI Instances Subscribing**
- **Scenario**: User runs TUI in two terminals, both subscribe to same bot
- **Impact**: Both receive events (expected), but potential confusion
- **Solution**:
  - Document: "Multiple TUI instances supported, each receives independent event stream"
  - No technical issue (broadcast supports multiple subscribers by design)

### 5.4 Hyperliquid API Constraints

**Edge Case 13: Rate Limit Exceeded**
- **Scenario**: Bot places 25 orders/second (exceeds 20 req/sec limit)
- **Impact**: Requests queued by `governor` rate limiter, orders delayed
- **Solution**:
  - Already handled by HyperliquidClient (line 44 in client.rs: `rate_limiter.until_ready().await`)
  - No action needed, requests auto-throttled
  - Consider logging: "Rate limited, order delayed by Xms"

**Edge Case 14: WebSocket Disconnection During Trading**
- **Scenario**: Network hiccup, WebSocket closes mid-trading
- **Impact**: No more market events → bot stops processing signals
- **Solution**:
  - Implement auto-reconnect in `HyperliquidWebSocket` (check if already implemented)
  - Exponential backoff: 1s, 2s, 4s, 8s, max 60s
  - Emit BotEvent::Error("WebSocket disconnected, reconnecting...")
  - Resume trading after reconnect + re-warmup (fetch missed candles)

**Edge Case 15: Order Rejected by Exchange**
- **Scenario**: Order exceeds position limits, insufficient margin, etc.
- **Impact**: `execute_order()` returns error, trading loop breaks
- **Solution**:
  - Catch order rejection, parse error message
  - Emit BotEvent::Error("Order rejected: {reason}")
  - **Do NOT crash bot** - continue processing next signal
  - Modify `trading_loop` to handle order errors gracefully

### 5.5 Configuration Edge Cases

**Edge Case 16: Invalid Leverage Value**
- **Scenario**: User sets `leverage = 0` or `leverage = 100` in config
- **Impact**: Invalid margin calculation or order rejection
- **Solution**:
  - Validate in BotConfig deserialization: `leverage` must be 1-50
  - Clamp to valid range if out of bounds
  - Log warning: "Leverage 100 exceeds max, clamped to 50"

**Edge Case 17: Risk Parameters Exceed 100%**
- **Scenario**: `risk_per_trade_pct = 1.5` (150% of equity)
- **Impact**: Negative equity after first loss
- **Solution**:
  - Validate: `0.0 < risk_per_trade_pct <= 1.0`
  - Reject config if invalid
  - Document: "Risk per trade must be between 0.0 and 1.0 (0-100%)"

**Edge Case 18: Warmup Periods Too Large**
- **Scenario**: `warmup_periods = 100000` (API limit 5000 candles/request)
- **Impact**: Warmup takes very long, multiple paginated requests
- **Solution**:
  - Already handled by `HyperliquidClient::fetch_candles` (pagination logic)
  - Add timeout to warmup (e.g., 60 seconds max)
  - Log warning if > 5000 periods: "Large warmup ({n} periods) may take time"

---

## Section 6: TaskMaster Handoff Package

### 6.1 MUST DO

**Feature 1: Event Streaming Infrastructure**
1. ✅ Create `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs`
   - Define `BotEvent` enum (MarketUpdate, SignalGenerated, OrderPlaced, OrderFilled, PositionUpdate, TradeClosed, Error)
   - Define `EnhancedBotStatus` struct (equity, returns, positions, recent events)
   - Define `PositionInfo` struct

2. ✅ Modify `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
   - Add fields to `BotActor` (lines 10-18): `event_tx: broadcast::Sender<BotEvent>`, `status_tx: watch::Sender<EnhancedBotStatus>`, `recent_events: VecDeque<BotEvent>`
   - Add `use std::collections::VecDeque;` at top
   - Add `use tokio::sync::{broadcast, watch};` at top
   - Modify `new()` constructor to initialize channels (capacity: 1000 for broadcast)
   - Modify `trading_loop()` (lines 95-124) to emit events after `process_next_event()`
   - Add `build_enhanced_status()` method to construct status from TradingSystem metrics

3. ✅ Modify `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs`
   - Add fields (lines 5-9): `event_rx: broadcast::Receiver<BotEvent>`, `status_rx: watch::Receiver<EnhancedBotStatus>`
   - Update `new()` constructor signature
   - Add `subscribe_events()` method
   - Add `latest_status()` method
   - Add `wait_for_status_change()` method

4. ✅ Modify `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
   - Update `spawn_bot()` (line 35) to create broadcast/watch channels before spawning actor
   - Pass channel receivers to BotHandle

5. ✅ Modify `/home/a/Work/algo-trade/crates/core/src/engine.rs`
   - Add public accessor methods after line 405:
     - `current_equity() -> Decimal`
     - `initial_capital() -> Decimal`
     - `total_return_pct() -> f64`
     - `sharpe_ratio() -> f64`
     - `max_drawdown() -> f64`
     - `win_rate() -> f64`
     - `num_trades() -> usize`
     - `open_positions() -> &HashMap<String, Position>`
     - `unrealized_pnl(&str, Decimal) -> Option<Decimal>`

**Feature 2: Wallet Integration**
6. ✅ Modify `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
   - Add `WalletConfig` struct after line 26
   - Add `MarginMode` enum (Cross, Isolated)
   - Modify `BotConfig` struct (lines 15-26) to add:
     - `initial_capital: Decimal`
     - `risk_per_trade_pct: f64`
     - `max_position_pct: f64`
     - `leverage: u8`
     - `margin_mode: MarginMode`
     - `wallet: Option<WalletConfig>` (with `#[serde(skip)]`)

7. ✅ Add to `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
   - Dependency: `hyperliquid_rust_sdk = "0.6.0"`

8. ✅ Modify `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
   - Add fields to `HyperliquidClient` (lines 15-19): `wallet: Option<Wallet>`, `account_address: Option<String>`
   - Add `with_wallet()` constructor
   - Add `post_signed()` method for authenticated requests

9. ✅ Modify `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
   - Add field to `LiveExecutionHandler` (line 12): `nonce_counter: Arc<AtomicU64>`
   - Update `new()` constructor signature
   - Modify `execute_order()` (lines 25-52) to use `post_signed()` with nonce
   - Parse actual fill response from Hyperliquid (order_id, price, commission)

10. ✅ Modify `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
    - Update `initialize_system()` (lines 33-92):
      - Load wallet from env if not present: `WalletConfig::from_env()`
      - Create authenticated client: `HyperliquidClient::with_wallet()`
      - Pass nonce counter to execution handler
      - Use `SimpleRiskManager::with_leverage()` with config values
      - Use `TradingSystem::with_capital()` with config value

**Feature 3: Position Sizing with Leverage**
11. ✅ Modify `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`
    - Add field to `SimpleRiskManager` (line 11): `leverage: Decimal`
    - Add `with_leverage()` constructor
    - Modify `evaluate_signal()` position sizing (lines 77-90):
      - Calculate leveraged capital: `account_equity * leverage`
      - Use leveraged capital for position sizing
      - Validate minimum order value ($10)
      - Add margin safety buffer (80% max utilization)

**Feature 4: TUI Enhancements**
12. ✅ Modify `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
    - Add field to `App` struct (lines 105-128): `event_subscriptions: HashMap<String, broadcast::Receiver<BotEvent>>`
    - Modify `render_bot_list()` (lines 518-568):
      - Display enhanced bot info (equity, return, trades, win rate)
      - Add bot detail panel (chunk layout change)
      - Show open positions with unrealized PnL
    - Modify `run_app()` (lines 188-230):
      - Subscribe to bot events on startup
      - Poll events in main loop (non-blocking `try_recv()`)
      - Display events in messages panel

**Feature 5: Configuration Defaults**
13. ✅ Modify `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
    - Update `create_bot()` function (lines 450-507) to set new BotConfig fields:
      - `initial_capital: Decimal::from(10000)`
      - `risk_per_trade_pct: 0.05`
      - `max_position_pct: 0.20`
      - `leverage: 1` (conservative default)
      - `margin_mode: MarginMode::Cross`
      - `wallet: None` (loaded from env at runtime)

**Feature 6: Documentation**
14. ✅ Create `/home/a/Work/algo-trade/docs/WALLET_SETUP.md`
    - Document Hyperliquid wallet setup process
    - Environment variable configuration
    - API wallet creation on Hyperliquid web UI
    - Security best practices (never commit private keys)

15. ✅ Update `/home/a/Work/algo-trade/config/Config.toml`
    - Add example bot configuration with new fields (commented out)

### 6.2 MUST NOT DO

**Scope Boundaries**:
1. ❌ **Do NOT implement isolated margin support** - Start with cross margin only (default behavior)
2. ❌ **Do NOT implement hardware wallet signing** - Use private key strings only (hl_ranger fork deferred)
3. ❌ **Do NOT implement per-asset leverage limits** - Use single leverage value for all assets (enhancement later)
4. ❌ **Do NOT implement margin tier API integration** - Position sizing based on leverage only (tiers not exposed by API)
5. ❌ **Do NOT implement WebSocket reconnection** - Assume existing implementation handles this (verify only)
6. ❌ **Do NOT implement multi-asset risk aggregation** - Each bot trades single symbol (multi-asset deferred)
7. ❌ **Do NOT implement position PnL streaming** - Calculate unrealized PnL on-demand only (real-time PnL requires market data subscription)
8. ❌ **Do NOT implement order modification/cancellation** - Market orders only (limit orders deferred)
9. ❌ **Do NOT implement subaccount support** - Master account + API wallet only
10. ❌ **Do NOT implement vault trading** - Regular account only

### 6.3 File Locations & Integration Points

**New Files**:
- `/home/a/Work/algo-trade/crates/bot-orchestrator/src/events.rs` (NEW)
- `/home/a/Work/algo-trade/docs/WALLET_SETUP.md` (NEW)

**Modified Files** (with exact line numbers):
1. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
   - Lines 1-9: Add imports
   - Lines 10-18: Add BotActor fields
   - Lines 23-30: Modify constructor
   - Lines 33-92: Modify initialize_system()
   - Lines 95-124: Modify trading_loop()
   - After line 187: Add build_enhanced_status()

2. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_handle.rs`
   - Lines 5-9: Add fields
   - Lines 16-24: Modify constructor
   - After line 84: Add subscribe_events(), latest_status(), wait_for_status_change()

3. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/commands.rs`
   - After line 26: Add WalletConfig, MarginMode
   - Lines 15-26: Modify BotConfig

4. `/home/a/Work/algo-trade/crates/bot-orchestrator/src/registry.rs`
   - Lines 35-50: Modify spawn_bot()

5. `/home/a/Work/algo-trade/crates/core/src/engine.rs`
   - After line 405: Add accessor methods

6. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
   - Add hyperliquid_rust_sdk dependency

7. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
   - Lines 15-19: Add fields
   - After line 37: Add with_wallet(), post_signed()

8. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
   - Lines 11-20: Add field, modify constructor
   - Lines 25-52: Modify execute_order()

9. `/home/a/Work/algo-trade/crates/strategy/src/risk_manager.rs`
   - Lines 8-11: Add leverage field
   - Lines 13-36: Add with_leverage() constructor
   - Lines 77-90: Modify position sizing logic

10. `/home/a/Work/algo-trade/crates/cli/src/tui_live_bot.rs`
    - Lines 105-128: Add App field
    - Lines 188-230: Modify run_app()
    - Lines 450-507: Modify create_bot()
    - Lines 518-568: Modify render_bot_list()

11. `/home/a/Work/algo-trade/config/Config.toml`
    - Add example bot config section

### 6.4 Dependencies & Prerequisites

**Crate Dependencies**:
- `hyperliquid_rust_sdk = "0.6.0"` (NEW)
- All existing dependencies remain

**Environment Variables** (Required):
```bash
export HYPERLIQUID_ACCOUNT_ADDRESS="0x..." # Master account address (42 chars)
export HYPERLIQUID_API_WALLET_KEY="0x..."  # API wallet private key (66 chars)
```

**External Setup**:
1. User must create Hyperliquid account: https://app.hyperliquid.xyz
2. Generate API wallet: https://app.hyperliquid.xyz/API
3. Approve API wallet for trading

### 6.5 Verification Checklist

**Phase 1: Event Streaming**
- [ ] `cargo build -p algo-trade-bot-orchestrator` succeeds
- [ ] BotActor spawns with broadcast/watch channels
- [ ] TUI subscribes to events without blocking
- [ ] Events appear in messages panel (signal, order, fill)
- [ ] Status updates reflect latest equity/positions

**Phase 2: Wallet Integration**
- [ ] `WalletConfig::from_env()` loads valid credentials
- [ ] `WalletConfig::from_env()` rejects invalid format
- [ ] `HyperliquidClient::with_wallet()` creates authenticated client
- [ ] Order submission includes signature (check with API response)
- [ ] First order succeeds (verify on Hyperliquid web UI)

**Phase 3: Position Sizing**
- [ ] Position size scales with leverage (10x leverage → 10x larger position)
- [ ] Minimum order value ($10) enforced
- [ ] Orders rejected if insufficient margin

**Phase 4: TUI Display**
- [ ] Bot list shows equity, return, trades, win rate
- [ ] Bot detail panel shows open positions
- [ ] Unrealized PnL updates on market events
- [ ] Messages panel shows signals/fills/errors

**Phase 5: Error Handling**
- [ ] Missing env vars → descriptive error (doesn't crash)
- [ ] Invalid private key → descriptive error
- [ ] Order rejection → logged, bot continues
- [ ] WebSocket disconnect → reconnect (verify existing behavior)

**Phase 6: Integration Test**
- [ ] Create test bot with real Hyperliquid testnet account
- [ ] Start bot, verify WebSocket connection
- [ ] Generate test signal (manual trigger or wait for strategy)
- [ ] Verify order submitted to Hyperliquid
- [ ] Verify fill event received and displayed
- [ ] Stop bot, verify graceful shutdown

---

## Section 7: Summary & Next Steps

### 7.1 Key Findings

**Current State**:
- Bot orchestration works (start/stop via TUI)
- No visibility into trading operations (events hidden inside TradingSystem)
- No Hyperliquid authentication (orders would be rejected)
- Position sizing doesn't use leverage (limited to 1x)

**Proposed Enhancements**:
1. **Event Streaming**: Broadcast/watch channels expose real-time trading events to TUI
2. **Wallet Integration**: Hyperliquid Rust SDK for EIP-712 signing, API wallet support
3. **Leverage**: RiskManager calculates position size with leverage multiplier (1x-50x)
4. **TUI**: Enhanced display with equity, positions, PnL, win rate, event feed

**Architectural Pattern**: Hybrid actor model
- `broadcast` for discrete events (signals, orders, fills) → TUI messages panel
- `watch` for continuous state (status, metrics) → TUI bot list display
- Follows Alice Ryhl's pattern (CLAUDE.md reference)

### 7.2 Risk Assessment

**High Risk**:
- Wallet private key management (must use env vars, never commit to git)
- Leverage misconfiguration (50x leverage = high liquidation risk)
- Nonce conflicts (mitigated by separate API wallets per bot)

**Medium Risk**:
- Event channel overflow (slow TUI consumer) - mitigated by try_recv() + capacity=1000
- WebSocket disconnection - existing reconnect logic (needs verification)

**Low Risk**:
- Position sizing calculation (validated with examples)
- Minimum order value enforcement (Hyperliquid will reject if missed)

### 7.3 Recommended Implementation Order

**Phase 1**: Event Streaming (2-3 hours)
- Implement BotEvent enum, EnhancedBotStatus struct
- Add broadcast/watch channels to BotActor
- Add TradingSystem accessor methods
- Test with print statements (no TUI yet)

**Phase 2**: Wallet Integration (3-4 hours)
- Add WalletConfig struct with env loading
- Integrate hyperliquid_rust_sdk
- Update HyperliquidClient with signing
- Test order submission on Hyperliquid testnet

**Phase 3**: Position Sizing (1-2 hours)
- Add leverage to RiskManager
- Update BotConfig with trading parameters
- Test position calculations with examples

**Phase 4**: TUI Display (2-3 hours)
- Update bot list rendering
- Add bot detail panel
- Add event subscription in run_app()
- Test with live bot

**Phase 5**: Documentation & Testing (1-2 hours)
- Write WALLET_SETUP.md
- Update Config.toml with examples
- End-to-end integration test

**Total Estimated Time**: 9-14 hours

### 7.4 Open Questions for User

1. **Leverage Defaults**: Should default leverage be 1x (conservative) or 10x (aggressive)?
2. **Multi-Bot Wallets**: Should all bots share one API wallet (simpler) or separate wallets (safer)?
3. **Margin Mode**: Start with cross margin only, or implement both cross/isolated?
4. **Event History**: Keep last 10 events (current plan) or make configurable?
5. **Testnet First**: Should we test on Hyperliquid testnet before mainnet, or go straight to mainnet?

### 7.5 External Dependencies

**Hyperliquid Rust SDK**:
- GitHub: https://github.com/hyperliquid-dex/hyperliquid-rust-sdk
- Crates.io: https://crates.io/crates/hyperliquid_rust_sdk (v0.6.0)
- Docs: https://docs.rs/hyperliquid_rust_sdk

**Hyperliquid API**:
- Base URL: https://api.hyperliquid.xyz
- WebSocket: wss://api.hyperliquid.xyz/ws
- Docs: https://hyperliquid.gitbook.io/hyperliquid-docs

**Account Setup**:
- Web UI: https://app.hyperliquid.xyz
- API Wallet Management: https://app.hyperliquid.xyz/API

---

## Appendix A: Code Examples

### A.1 Wallet Setup Script

**File**: `/home/a/Work/algo-trade/scripts/setup_wallet.sh` (NEW)
```bash
#!/bin/bash

echo "Hyperliquid Wallet Setup"
echo "========================"
echo ""
echo "1. Go to https://app.hyperliquid.xyz/API"
echo "2. Click 'Generate API Wallet'"
echo "3. Copy the private key (DO NOT SHARE)"
echo "4. Click 'Approve' to authorize the wallet"
echo ""
echo "Enter your master account address (0x...):"
read ACCOUNT_ADDRESS

echo "Enter your API wallet private key (0x...):"
read -s API_WALLET_KEY

# Validate format
if [[ ! $ACCOUNT_ADDRESS =~ ^0x[0-9a-fA-F]{40}$ ]]; then
    echo "Error: Invalid account address format"
    exit 1
fi

if [[ ! $API_WALLET_KEY =~ ^0x[0-9a-fA-F]{64}$ ]]; then
    echo "Error: Invalid private key format"
    exit 1
fi

# Write to .env (NOT tracked by git)
cat > .env <<EOF
HYPERLIQUID_ACCOUNT_ADDRESS=$ACCOUNT_ADDRESS
HYPERLIQUID_API_WALLET_KEY=$API_WALLET_KEY
EOF

echo ""
echo "✅ Wallet configured in .env"
echo "⚠️  NEVER commit .env to git!"
echo ""
echo "Load with: source .env"
```

### A.2 Example Bot Configuration

**File**: `/home/a/Work/algo-trade/config/bots/btc_scalper.toml` (NEW)
```toml
bot_id = "btc_scalper"
symbol = "BTC"
strategy = "quad_ma"
enabled = true
interval = "1m"

# Hyperliquid endpoints
ws_url = "wss://api.hyperliquid.xyz/ws"
api_url = "https://api.hyperliquid.xyz"

# Strategy warmup
warmup_periods = 100

# Trading parameters
initial_capital = 10000.0  # USDC
risk_per_trade_pct = 0.05   # 5% per trade
max_position_pct = 0.20     # 20% max position
leverage = 10               # 10x leverage
margin_mode = "Cross"       # or "Isolated"

# Strategy-specific config (JSON)
[strategy_config]
ma1 = 5
ma2 = 10
ma3 = 20
ma4 = 50
trend_period = 100
volume_factor = 150
take_profit = 200
stop_loss = 100
reversal_confirmation_bars = 2
```

### A.3 Monitoring Script

**File**: `/home/a/Work/algo-trade/scripts/monitor_bot.sh` (NEW)
```bash
#!/bin/bash

BOT_ID=${1:-"bot_*"}

echo "Monitoring bot: $BOT_ID"
echo ""

# Tail logs (assumes structured logging)
RUST_LOG=info cargo run -p algo-trade-cli -- live-bot 2>&1 | \
    grep -E "(SignalGenerated|OrderFilled|TradeClosed|Error)" | \
    jq -r '"\(.timestamp) | \(.bot_id) | \(.event_type) | \(.details)"'
```

---

**End of Context Report**

This report provides comprehensive analysis for implementing bot operations visibility, Hyperliquid wallet integration, and position sizing with leverage. TaskMaster agent can now use Section 6 (Handoff Package) to generate atomic implementation tasks.
