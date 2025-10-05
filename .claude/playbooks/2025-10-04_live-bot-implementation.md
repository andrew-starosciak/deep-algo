# Playbook: Live Bot Architecture Implementation

**Date**: 2025-10-04
**Status**: Ready for Execution
**Estimated Total LOC**: ~870

## User Request

"Full implementation of live bot architecture - all 8 tasks to enable bots to connect to Hyperliquid WebSocket, receive OHLCV data, execute strategies (QuadMa/MaCrossover), place real orders, and be managed through TUI."

## Scope Boundaries

### MUST DO

1. ✅ Implement candle WebSocket subscription in LiveDataProvider
2. ✅ Add historical data warmup (fetch last 200 candles on bot start)
3. ✅ Create strategy factory (instantiate QuadMa/MaCrossover from string)
4. ✅ Extend BotActor to own and run TradingSystem
5. ✅ Wire LiveDataProvider + LiveExecutionHandler + Strategy in BotActor
6. ✅ Add strategy parameters to BotConfig
7. ✅ Create live bot TUI (BotList, CreateBot, BotDetail screens)
8. ✅ Add TUI CLI command

### MUST NOT DO

- ❌ DO NOT modify Strategy trait (keep backtest-live parity)
- ❌ DO NOT use blocking I/O in BotActor or TUI
- ❌ DO NOT use f64 for prices/PnL (use rust_decimal::Decimal)
- ❌ DO NOT create new WebSocket client (use existing HyperliquidWebSocket)
- ❌ DO NOT store API keys in BotConfig struct
- ❌ DO NOT add external dependencies beyond what exists in workspace

## Atomic Tasks

### Task 1: Implement Candle WebSocket Subscription

**File**: `/home/andrew/Projects/deep-algo/crates/exchange-hyperliquid/src/data_provider.rs`
**Location**: Lines 24-34 (subscription in connect method) + Lines 42-62 (response parsing)
**Action**: Change subscription type from "trades" to "candle" and parse candle JSON format

**Exact Changes**:

1. **Subscription Message** (around line 29):
```rust
// OLD:
"type": "trades"

// NEW:
"type": "candle",
"coin": self.symbol,
"interval": "1m"
```

2. **Response Parsing** (lines 42-62):
- Parse candle JSON structure: `{"t": timestamp_ms, "T": close_time_ms, "s": symbol, "i": interval, "o": open, "h": high, "l": low, "c": close, "v": volume}`
- Emit `MarketEvent::Bar` instead of `MarketEvent::Trade`
- Convert timestamp from milliseconds to `DateTime<Utc>`
- Parse OHLCV values as `Decimal`

**Example Code**:
```rust
// In message processing loop
let candle_data = msg.as_object().ok_or_else(|| anyhow!("Invalid candle format"))?;

let timestamp_ms = candle_data.get("t")
    .and_then(|v| v.as_i64())
    .ok_or_else(|| anyhow!("Missing timestamp"))?;
let timestamp = DateTime::from_timestamp_millis(timestamp_ms)
    .ok_or_else(|| anyhow!("Invalid timestamp"))?;

let open = candle_data.get("o")
    .and_then(|v| v.as_str())
    .ok_or_else(|| anyhow!("Missing open"))?
    .parse::<Decimal>()?;

// Similar for high, low, close, volume

let event = MarketEvent::Bar {
    symbol: self.symbol.clone(),
    open,
    high,
    low,
    close,
    volume,
    timestamp,
};

tx.send(event).await?;
```

**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated LOC**: 35

---

### Task 2: Add Historical Data Warmup Method

**File**: `/home/andrew/Projects/deep-algo/crates/exchange-hyperliquid/src/data_provider.rs`
**Location**: After `new()` method (around line 38)
**Action**: Add async method to fetch and feed historical candles

**Code**:
```rust
/// Warms up the data provider with historical candles before starting live stream.
///
/// Fetches `lookback_candles` number of 1-minute candles ending at current time.
///
/// # Errors
/// Returns error if fetching historical data fails or timestamp conversion fails.
pub async fn warmup_with_historical(
    &self,
    client: &HyperliquidClient,
    lookback_candles: usize,
) -> Result<Vec<MarketEvent>> {
    use chrono::{Duration, Utc};

    let end = Utc::now();
    let start = end - Duration::minutes(i64::try_from(lookback_candles)?);

    tracing::info!(
        "Fetching {} historical candles for {} from {} to {}",
        lookback_candles,
        self.symbol,
        start,
        end
    );

    let records = client
        .fetch_candles(&self.symbol, "1m", start, end)
        .await?;

    let events: Vec<MarketEvent> = records
        .into_iter()
        .map(|r| MarketEvent::Bar {
            symbol: r.symbol,
            open: r.open,
            high: r.high,
            low: r.low,
            close: r.close,
            volume: r.volume,
            timestamp: r.timestamp,
        })
        .collect();

    tracing::info!(
        "Warmed up with {} historical candles for {}",
        events.len(),
        self.symbol
    );

    Ok(events)
}
```

**Dependencies**: Requires `HyperliquidClient::fetch_candles()` method exists
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated LOC**: 40

---

### Task 3: Create Strategy Factory

**File**: `/home/andrew/Projects/deep-algo/crates/strategy/src/lib.rs`
**Location**: After existing exports (around line 12)
**Action**: Add strategy factory function for runtime strategy instantiation

**Code**:
```rust
use algo_trade_core::Strategy;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Creates a strategy instance from a name string.
///
/// # Arguments
/// * `name` - Strategy name ("quad_ma" or "ma_crossover")
/// * `symbol` - Trading symbol (e.g., "BTC")
///
/// # Errors
/// Returns error if strategy name is unknown.
///
/// # Examples
/// ```ignore
/// let strategy = create_strategy("quad_ma", "BTC".to_string())?;
/// ```
pub fn create_strategy(
    name: &str,
    symbol: String,
) -> Result<Arc<Mutex<dyn Strategy>>> {
    match name {
        "quad_ma" => {
            let strategy = QuadMaStrategy::new(symbol);
            Ok(Arc::new(Mutex::new(strategy)))
        }
        "ma_crossover" => {
            // Default periods: fast=10, slow=30
            let strategy = MaCrossoverStrategy::new(symbol, 10, 30);
            Ok(Arc::new(Mutex::new(strategy)))
        }
        _ => {
            anyhow::bail!(
                "Unknown strategy: '{}'. Available: quad_ma, ma_crossover",
                name
            )
        }
    }
}
```

**Verification**: `cargo check -p algo-trade-strategy`
**Estimated LOC**: 30

---

### Task 4: Extend BotActor to Own TradingSystem

**File**: `/home/andrew/Projects/deep-algo/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Lines 6-77 (entire file needs structural changes)
**Action**: Add TradingSystem field, rewrite event loop to use `tokio::select!`

**Major Changes**:

1. **Add Imports**:
```rust
use algo_trade_core::{TradingSystem, MarketEvent};
use algo_trade_hyperliquid::{LiveDataProvider, LiveExecutionHandler, HyperliquidClient};
use tokio::task::JoinHandle;
use std::sync::Arc;
```

2. **Update BotActor Struct**:
```rust
struct BotActor {
    config: BotConfig,
    rx: mpsc::Receiver<BotCommand>,
    state: BotState,
    trading_system: Option<TradingSystem<LiveDataProvider, LiveExecutionHandler>>,
    ws_handle: Option<JoinHandle<()>>,
}
```

3. **Rewrite `run()` Method**:
```rust
pub async fn run(mut self) -> Result<()> {
    tracing::info!("BotActor started for bot_id={}", self.config.bot_id);

    loop {
        tokio::select! {
            // Handle commands from BotHandle
            Some(cmd) = self.rx.recv() => {
                match cmd {
                    BotCommand::Start => {
                        self.handle_start().await?;
                    }
                    BotCommand::Pause => {
                        self.handle_pause().await?;
                    }
                    BotCommand::Stop => {
                        self.handle_stop().await?;
                        break;
                    }
                    BotCommand::GetStatus(tx) => {
                        let status = BotStatus {
                            bot_id: self.config.bot_id.clone(),
                            state: self.state.clone(),
                        };
                        let _ = tx.send(status);
                    }
                }
            }

            // Process market events if system is running
            else if self.state == BotState::Running => {
                if let Some(ref mut system) = self.trading_system {
                    // Get next event from data provider
                    if let Some(event) = system.next_event().await? {
                        system.process_event(event).await?;
                    }
                }
            }

            else => {
                // No commands and not running, yield
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }

    tracing::info!("BotActor stopped for bot_id={}", self.config.bot_id);
    Ok(())
}

async fn handle_start(&mut self) -> Result<()> {
    // Implementation moved to Task 5
    Ok(())
}

async fn handle_pause(&mut self) -> Result<()> {
    if self.state == BotState::Running {
        self.state = BotState::Paused;
        tracing::info!("Bot paused: {}", self.config.bot_id);
    }
    Ok(())
}

async fn handle_stop(&mut self) -> Result<()> {
    self.state = BotState::Stopped;
    self.trading_system = None;

    if let Some(handle) = self.ws_handle.take() {
        handle.abort();
    }

    tracing::info!("Bot stopped: {}", self.config.bot_id);
    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 80

---

### Task 5: Wire Components in BotActor Start Command

**File**: `/home/andrew/Projects/deep-algo/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: `handle_start()` method (created in Task 4)
**Action**: Instantiate all TradingSystem components and wire together

**Code**:
```rust
async fn handle_start(&mut self) -> Result<()> {
    if self.state == BotState::Running {
        tracing::warn!("Bot already running: {}", self.config.bot_id);
        return Ok(());
    }

    tracing::info!("Starting bot: {}", self.config.bot_id);

    // Load environment variables for API endpoints
    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());
    let ws_url = std::env::var("HYPERLIQUID_WS_URL")
        .unwrap_or_else(|_| "wss://api.hyperliquid.xyz/ws".to_string());

    // Create Hyperliquid client
    let client = Arc::new(HyperliquidClient::new(&api_url));

    // Create LiveDataProvider
    let data_provider = LiveDataProvider::new(
        self.config.symbol.clone(),
        &ws_url,
    );

    // Warmup with historical data (200 candles = ~3.3 hours for 1m interval)
    tracing::info!("Fetching historical data for warmup...");
    let warmup_events = data_provider
        .warmup_with_historical(&client, 200)
        .await?;

    tracing::info!("Fetched {} warmup candles", warmup_events.len());

    // Create LiveExecutionHandler
    let execution_handler = LiveExecutionHandler::new(client.clone());

    // Create strategy using factory
    let strategy = algo_trade_strategy::create_strategy(
        &self.config.strategy,
        self.config.symbol.clone(),
    )?;

    // Create RiskManager (max position 5%, max drawdown 20%)
    let risk_manager = Arc::new(
        algo_trade_core::SimpleRiskManager::new(
            rust_decimal_macros::dec!(0.05),
            rust_decimal_macros::dec!(0.20),
        )
    );

    // Create TradingSystem
    let mut system = TradingSystem::new(
        data_provider,
        execution_handler,
        vec![strategy],
        risk_manager,
    );

    // Feed warmup events to initialize strategy state
    tracing::info!("Processing warmup events...");
    for event in warmup_events {
        system.process_event(event).await?;
    }

    tracing::info!("Warmup complete, starting live trading");

    // Store trading system
    self.trading_system = Some(system);
    self.state = BotState::Running;

    tracing::info!("Bot started successfully: {}", self.config.bot_id);

    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 70

---

### Task 6: Add Strategy Parameters to BotConfig

**File**: `/home/andrew/Projects/deep-algo/crates/bot-orchestrator/src/commands.rs`
**Location**: BotConfig struct (around line 15-21)
**Action**: Add interval and strategy_params fields for flexible strategy configuration

**Change**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
    pub enabled: bool,

    /// Candle interval (e.g., "1m", "5m", "1h")
    #[serde(default = "default_interval")]
    pub interval: String,

    /// Strategy-specific parameters (JSON object)
    /// Example for ma_crossover: {"fast_period": 10, "slow_period": 30}
    #[serde(default)]
    pub strategy_params: Option<serde_json::Value>,
}

fn default_interval() -> String {
    "1m".to_string()
}
```

**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated LOC**: 10

---

### Task 7: Create Live Bot TUI

**File**: NEW `/home/andrew/Projects/deep-algo/crates/cli/src/tui_live_bots/mod.rs`
**Action**: Create full TUI module for live bot management (reuse patterns from `tui_backtest`)

**Structure**:

```rust
//! Live bot management TUI
//!
//! Provides interactive terminal UI for:
//! - Viewing all running bots
//! - Creating new bots
//! - Starting/pausing/stopping bots
//! - Viewing bot details and status

use algo_trade_bot_orchestrator::{BotRegistry, BotConfig, BotCommand};
use algo_trade_core::Config;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::sync::Arc;
use std::time::Duration;

/// Application state
struct AppState {
    registry: Arc<BotRegistry>,
    config: Config,
    screen: Screen,
    selected_bot_index: usize,
    create_form: CreateBotForm,
    message: Option<String>,
}

/// Current screen
#[derive(Debug, Clone, PartialEq)]
enum Screen {
    BotList,
    CreateBot,
    BotDetail(String), // bot_id
}

/// Form for creating new bot
#[derive(Debug, Clone, Default)]
struct CreateBotForm {
    bot_id: String,
    symbol: String,
    strategy: String,
    interval: String,
    field_index: usize, // Currently selected field
}

impl CreateBotForm {
    fn fields_count() -> usize {
        4
    }
}

/// Main entry point
pub async fn run(registry: Arc<BotRegistry>, config: Config) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = AppState {
        registry,
        config,
        screen: Screen::BotList,
        selected_bot_index: 0,
        create_form: CreateBotForm::default(),
        message: None,
    };

    let result = run_app(&mut terminal, &mut app).await;

    // Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, app))?;

        // Poll for events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(key, app).await? {
                    break; // Exit requested
                }
            }
        }
    }

    Ok(())
}

fn render(f: &mut Frame, app: &AppState) {
    match &app.screen {
        Screen::BotList => render_bot_list(f, app),
        Screen::CreateBot => render_create_bot(f, app),
        Screen::BotDetail(bot_id) => render_bot_detail(f, app, bot_id),
    }
}

fn render_bot_list(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Live Bot Management")
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(title, chunks[0]);

    // Bot list
    let bots = app.registry.list_bots();
    let items: Vec<ListItem> = bots
        .iter()
        .enumerate()
        .map(|(i, bot_id)| {
            let style = if i == app.selected_bot_index {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![
                Span::styled(bot_id, style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Bots"))
        .highlight_style(Style::default().bg(Color::DarkGray));

    f.render_widget(list, chunks[1]);

    // Help
    let help_text = if let Some(msg) = &app.message {
        msg.clone()
    } else {
        "c: Create | Enter: Detail | s: Start | p: Pause | x: Stop | q: Quit".to_string()
    };

    let help = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

fn render_create_bot(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Create New Bot")
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Green));
    f.render_widget(title, chunks[0]);

    // Form
    let form = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(chunks[1]);

    let fields = [
        ("Bot ID", &app.create_form.bot_id),
        ("Symbol", &app.create_form.symbol),
        ("Strategy", &app.create_form.strategy),
        ("Interval", &app.create_form.interval),
    ];

    for (i, (label, value)) in fields.iter().enumerate() {
        let style = if i == app.create_form.field_index {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let text = format!("{}: {}", label, value);
        let widget = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL))
            .style(style);
        f.render_widget(widget, form[i]);
    }

    // Help
    let help = Paragraph::new("Tab: Next Field | Enter: Create | Esc: Cancel")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

fn render_bot_detail(f: &mut Frame, app: &AppState, bot_id: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!("Bot Details: {}", bot_id))
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(title, chunks[0]);

    // Details (fetch from registry)
    let details = format!("Bot ID: {}\n[Status information would go here]", bot_id);
    let content = Paragraph::new(details)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: true });
    f.render_widget(content, chunks[1]);

    // Help
    let help = Paragraph::new("s: Start | p: Pause | x: Stop | Esc: Back")
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

async fn handle_key_event(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    match &app.screen {
        Screen::BotList => handle_bot_list_key(key, app).await,
        Screen::CreateBot => handle_create_bot_key(key, app).await,
        Screen::BotDetail(_) => handle_bot_detail_key(key, app).await,
    }
}

async fn handle_bot_list_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true), // Quit
        KeyCode::Char('c') => {
            app.screen = Screen::CreateBot;
            app.create_form = CreateBotForm {
                interval: "1m".to_string(),
                ..Default::default()
            };
        }
        KeyCode::Up => {
            if app.selected_bot_index > 0 {
                app.selected_bot_index -= 1;
            }
        }
        KeyCode::Down => {
            let count = app.registry.list_bots().len();
            if app.selected_bot_index + 1 < count {
                app.selected_bot_index += 1;
            }
        }
        KeyCode::Enter => {
            let bots = app.registry.list_bots();
            if let Some(bot_id) = bots.get(app.selected_bot_index) {
                app.screen = Screen::BotDetail(bot_id.clone());
            }
        }
        KeyCode::Char('s') => {
            // Start selected bot
            let bots = app.registry.list_bots();
            if let Some(bot_id) = bots.get(app.selected_bot_index) {
                if let Some(handle) = app.registry.get_bot(bot_id) {
                    handle.send_command(BotCommand::Start).await?;
                    app.message = Some(format!("Started bot: {}", bot_id));
                }
            }
        }
        KeyCode::Char('p') => {
            // Pause selected bot
            let bots = app.registry.list_bots();
            if let Some(bot_id) = bots.get(app.selected_bot_index) {
                if let Some(handle) = app.registry.get_bot(bot_id) {
                    handle.send_command(BotCommand::Pause).await?;
                    app.message = Some(format!("Paused bot: {}", bot_id));
                }
            }
        }
        KeyCode::Char('x') => {
            // Stop selected bot
            let bots = app.registry.list_bots();
            if let Some(bot_id) = bots.get(app.selected_bot_index) {
                if let Some(handle) = app.registry.get_bot(bot_id) {
                    handle.send_command(BotCommand::Stop).await?;
                    app.registry.remove_bot(bot_id);
                    app.message = Some(format!("Stopped bot: {}", bot_id));
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

async fn handle_create_bot_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::BotList;
        }
        KeyCode::Tab => {
            app.create_form.field_index =
                (app.create_form.field_index + 1) % CreateBotForm::fields_count();
        }
        KeyCode::Char(c) => {
            // Add character to current field
            match app.create_form.field_index {
                0 => app.create_form.bot_id.push(c),
                1 => app.create_form.symbol.push(c),
                2 => app.create_form.strategy.push(c),
                3 => app.create_form.interval.push(c),
                _ => {}
            }
        }
        KeyCode::Backspace => {
            // Remove character from current field
            match app.create_form.field_index {
                0 => { app.create_form.bot_id.pop(); }
                1 => { app.create_form.symbol.pop(); }
                2 => { app.create_form.strategy.pop(); }
                3 => { app.create_form.interval.pop(); }
                _ => {}
            }
        }
        KeyCode::Enter => {
            // Create bot
            let config = BotConfig {
                bot_id: app.create_form.bot_id.clone(),
                symbol: app.create_form.symbol.clone(),
                strategy: app.create_form.strategy.clone(),
                enabled: true,
                interval: app.create_form.interval.clone(),
                strategy_params: None,
            };

            app.registry.create_bot(config).await?;
            app.message = Some(format!("Created bot: {}", app.create_form.bot_id));
            app.screen = Screen::BotList;
        }
        _ => {}
    }

    Ok(false)
}

async fn handle_bot_detail_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    let bot_id = match &app.screen {
        Screen::BotDetail(id) => id.clone(),
        _ => return Ok(false),
    };

    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::BotList;
        }
        KeyCode::Char('s') => {
            if let Some(handle) = app.registry.get_bot(&bot_id) {
                handle.send_command(BotCommand::Start).await?;
                app.message = Some(format!("Started bot: {}", bot_id));
            }
        }
        KeyCode::Char('p') => {
            if let Some(handle) = app.registry.get_bot(&bot_id) {
                handle.send_command(BotCommand::Pause).await?;
                app.message = Some(format!("Paused bot: {}", bot_id));
            }
        }
        KeyCode::Char('x') => {
            if let Some(handle) = app.registry.get_bot(&bot_id) {
                handle.send_command(BotCommand::Stop).await?;
                app.registry.remove_bot(&bot_id);
                app.message = Some(format!("Stopped bot: {}", bot_id));
                app.screen = Screen::BotList;
            }
        }
        _ => {}
    }

    Ok(false)
}
```

**Dependencies**:
- Add to `crates/cli/Cargo.toml`:
```toml
algo-trade-bot-orchestrator = { path = "../bot-orchestrator" }
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 450

---

### Task 8: Add TUI CLI Command

**File**: `/home/andrew/Projects/deep-algo/crates/cli/src/main.rs`
**Location**: Commands enum (around line 66) + match statement (around line 96)
**Action**: Add TuiLiveBots command variant and handler function

**Code**:

1. **Add to Commands enum** (after existing commands):
```rust
/// Interactive TUI for live bot management
TuiLiveBots {
    /// Config file path
    #[arg(short, long, default_value = "config/Config.toml")]
    config: String,
},
```

2. **Add to match statement** (in main function):
```rust
Commands::TuiLiveBots { config } => {
    run_tui_live_bots(&config).await?;
}
```

3. **Add handler function** (at end of file):
```rust
async fn run_tui_live_bots(config_path: &str) -> anyhow::Result<()> {
    // Load configuration
    let config = algo_trade_core::ConfigLoader::load()?;

    tracing::info!("Starting live bot TUI with config: {}", config_path);

    // Create bot registry
    let registry = Arc::new(algo_trade_bot_orchestrator::BotRegistry::new());

    // Run TUI
    tui_live_bots::run(registry, config).await
}
```

4. **Add module declaration** (at top of file with other modules):
```rust
mod tui_live_bots;
```

**Verification**:
```bash
cargo check -p algo-trade-cli
cargo run -p algo-trade-cli -- tui-live-bots --help
```

**Estimated LOC**: 25

---

## Verification Checklist

After all tasks complete:

### Build Verification

- [ ] `cargo build --workspace` - All crates compile
- [ ] `cargo clippy --workspace -- -D warnings` - Zero warnings
- [ ] `cargo check -p algo-trade-hyperliquid` - Data provider changes compile
- [ ] `cargo check -p algo-trade-bot-orchestrator` - BotActor changes compile
- [ ] `cargo check -p algo-trade-strategy` - Strategy factory compiles
- [ ] `cargo check -p algo-trade-cli` - TUI changes compile

### Functional Verification

- [ ] Run `cargo run -p algo-trade-cli -- tui-live-bots`
- [ ] Create bot: symbol=BTC, strategy=quad_ma, interval=1m
- [ ] Verify form validation and creation
- [ ] Start bot: Verify WebSocket connects
- [ ] Verify historical data warmup (200 candles loaded in logs)
- [ ] Verify live candle events received (check logs)
- [ ] Verify strategy generates signals (check TradingSystem output)
- [ ] Navigate to bot detail screen
- [ ] Pause bot and verify state change
- [ ] Resume bot and verify reconnection
- [ ] Stop bot gracefully and verify cleanup

### Karen Review (MANDATORY)

- [ ] Zero clippy warnings at all lint levels (default + pedantic + nursery)
- [ ] All public APIs documented with doc comments
- [ ] All `# Errors` sections documented for fallible functions
- [ ] All financial values use `rust_decimal::Decimal`
- [ ] No blocking I/O in async contexts
- [ ] No `f64` usage for prices/quantities/PnL
- [ ] Consistent error handling with `anyhow::Result`
- [ ] All imports organized and unused imports removed

---

## Dependencies Between Tasks

```
Task 1 (Candle Subscription)
    ↓
Task 2 (Historical Warmup) ← depends on Task 1 (MarketEvent::Bar)
    ↓
Task 3 (Strategy Factory) ← independent
    ↓
Task 4 (Extend BotActor) ← depends on Tasks 1-3 (needs all components)
    ↓
Task 5 (Wire Components) ← depends on Task 4 (implements handle_start)
    ↓
Task 6 (BotConfig params) ← independent but used by Task 5
    ↓
Task 7 (Live Bot TUI) ← depends on Task 6 (uses BotConfig)
    ↓
Task 8 (CLI Command) ← depends on Task 7 (invokes TUI module)
```

**Execution Order**: Sequential (Task 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8)

**Critical Path**: Tasks 1-5 must complete successfully for live trading to function

---

## Notes

### Technical Considerations

- **Task 4 is the largest refactor** (80 LOC) but represents a cohesive single unit (event loop redesign)
- **Task 7 is the largest new file** (450 LOC) but reuses proven patterns from `tui_backtest`
- All async operations use Tokio (no blocking I/O)
- All financial values use `rust_decimal::Decimal` (never f64)
- Strategy trait remains unchanged (backtest-live parity maintained)
- WebSocket reconnection handled by existing `HyperliquidWebSocket`

### Testing Strategy

1. **Unit Tests**: Each component tested in isolation
2. **Integration Test**: Full bot lifecycle (create → start → trade → stop)
3. **Manual TUI Test**: Interactive verification of all screens and commands

### Rollback Plan

If issues arise:
1. Tasks 1-3: Independent, can revert individually
2. Tasks 4-5: Tightly coupled, revert together
3. Tasks 7-8: UI only, safe to revert without affecting core

---

## Total Estimated LOC: ~710 lines

**Breakdown**:
- Task 1: 35 LOC
- Task 2: 40 LOC
- Task 3: 30 LOC
- Task 4: 80 LOC
- Task 5: 70 LOC
- Task 6: 10 LOC
- Task 7: 450 LOC
- Task 8: 25 LOC

**Total**: 740 LOC (revised from initial estimate)

---

## Success Criteria

Implementation complete when:

1. ✅ All 8 tasks pass verification
2. ✅ `cargo build --workspace` succeeds
3. ✅ Karen review passes with zero issues
4. ✅ TUI launches and displays bot list
5. ✅ Bot can be created via TUI form
6. ✅ Bot connects to Hyperliquid WebSocket
7. ✅ Historical warmup completes (200 candles)
8. ✅ Live candles received and processed
9. ✅ Strategy generates signals
10. ✅ Bot can be paused/resumed/stopped via TUI

---

**Playbook Status**: READY FOR EXECUTION
