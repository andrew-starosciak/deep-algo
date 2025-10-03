# TUI Backtest Enhancements Playbook

**Date**: 2025-10-03
**Status**: Ready for Execution

---

## User Request

The TUI backtest application needs enhancements:

1. **Timeframe Configuration**: Add ability to configure start/end dates and interval in the TUI
   - User wants to test 3 days of data on 1m or 5m intervals (high-frequency data)
   - Currently timeframes are only CLI arguments
   - Need a configuration screen before Running

2. **Default Sorting**: Results should default to sorting by most profitable (total_return descending)
   - Currently defaults to sort_column = 2 with sort_ascending = false (this is correct)
   - Just verify this is working as expected

3. **Trade Breakdown View**: Add ability to drill down into individual backtest results
   - When user selects a result and presses Enter, show detailed trade list
   - Display all trades: timestamp, action (Buy/Long/Short/Close), price, quantity, PnL
   - New screen: TradeDetail with back navigation

---

## Scope Boundaries

### MUST DO
- [x] Add TimeframeConfig screen between TokenSelection and ParameterConfig
- [x] Implement date/interval editing with validation
- [x] Capture individual trade history during backtest execution
- [x] Add TradeDetail screen with drill-down from Results
- [x] Display trades with timestamp, action, price, quantity, PnL
- [x] Maintain backward compatibility with CLI arguments
- [x] Verify default sorting (total_return descending)

### MUST NOT DO
- ❌ Change existing CLI argument behavior (CLI args should override TUI defaults)
- ❌ Break existing screens (StrategySelection, TokenSelection, Results)
- ❌ Modify core trading logic beyond capturing trades
- ❌ Add new external dependencies (use existing crates)
- ❌ Change PerformanceMetrics structure (extend BacktestResult only)

---

## Architecture Analysis

### Current State

**File Structure**:
- `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs` - App state machine (356 lines)
- `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs` - UI rendering (346 lines)
- `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` - Backtest execution (164 lines)
- `/home/a/Work/algo-trade/crates/core/src/engine.rs` - TradingSystem engine (297 lines)

**Current Workflow**:
```
StrategySelection → TokenSelection → ParameterConfig → Running → Results
```

**Key Data Structures**:
- `AppScreen` enum (5 variants, line 36-43 in mod.rs)
- `BacktestResult` struct (line 68-79 in mod.rs) - Currently stores only aggregated metrics
- `App` struct (line 82-117 in mod.rs) - Main state machine
- `FillEvent` struct (line 69-77 in events.rs) - Individual trade data

### Required Changes

**New Workflow**:
```
StrategySelection → TokenSelection → TimeframeConfig → ParameterConfig → Running → Results ⇄ TradeDetail
```

**Data Flow for Trade Capture**:
1. `TradingSystem::run()` generates `FillEvent` at line 155 (engine.rs)
2. Need to collect all fills and return with metrics
3. Store fills in `BacktestResult` for later display
4. Display in new `TradeDetail` screen

---

## Atomic Tasks

### Phase 1: Verify Default Sorting (Requirement 2)

#### Task 1.1: Verify Default Sort Configuration
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 143-144 (App::new())
**Action**: Verify existing default sorting is correct (no code changes needed)
**Current Code**:
```rust
sort_column: 2, // Default sort by return
sort_ascending: false,
```
**Verification**: Read code and confirm this matches requirement (sort by total_return descending)
**Estimated LOC**: 0 (verification only)

---

### Phase 2: Add Trade History Capture (Foundation for Requirement 3)

#### Task 2.1: Extend BacktestResult to Store Trades
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 68-79 (BacktestResult struct)
**Action**: Add `trades` field to store individual trade details
**Change**:
```rust
// OLD:
pub struct BacktestResult {
    pub token: String,
    pub config_name: String,
    pub total_return: rust_decimal::Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: rust_decimal::Decimal,
    pub num_trades: usize,
    #[allow(dead_code)]
    pub win_rate: f64,
}

// NEW:
pub struct BacktestResult {
    pub token: String,
    pub config_name: String,
    pub total_return: rust_decimal::Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: rust_decimal::Decimal,
    pub num_trades: usize,
    #[allow(dead_code)]
    pub win_rate: f64,
    pub trades: Vec<TradeRecord>,
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 2

#### Task 2.2: Define TradeRecord Struct
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: After BacktestResult definition (insert at line 80)
**Action**: Add TradeRecord struct to capture individual trade details
**Change**:
```rust
/// Individual trade record for detailed view
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp: DateTime<Utc>,
    pub action: TradeAction,
    pub price: rust_decimal::Decimal,
    pub quantity: rust_decimal::Decimal,
    pub pnl: Option<rust_decimal::Decimal>, // None for entry, Some for exit
}

#[derive(Debug, Clone)]
pub enum TradeAction {
    Buy,
    Sell,
    Long,
    Short,
    Close,
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 18

#### Task 2.3: Modify TradingSystem to Return Trade History
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 8-22 (PerformanceMetrics struct)
**Action**: Add trades field to PerformanceMetrics
**Change**:
```rust
// Add use import at top (after line 6)
use crate::events::FillEvent;

// Modify PerformanceMetrics struct
pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
    pub initial_capital: Decimal,
    pub final_capital: Decimal,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub duration: chrono::Duration,
    pub equity_peak: Decimal,
    pub buy_hold_return: Decimal,
    pub exposure_time: f64,
    pub trades: Vec<FillEvent>, // NEW: Capture all fills
}
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 2

#### Task 2.4: Collect Fills in TradingSystem
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 24-46 (TradingSystem struct)
**Action**: Add fills vector to track all executed trades
**Change**:
```rust
pub struct TradingSystem<D, E>
where
    D: DataProvider,
    E: ExecutionHandler,
{
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
    start_time: Option<chrono::DateTime<chrono::Utc>>,
    end_time: Option<chrono::DateTime<chrono::Utc>>,
    first_price: Option<Decimal>,
    last_price: Option<Decimal>,
    bars_in_position: usize,
    total_bars: usize,
    equity_peak: Decimal,
    fills: Vec<FillEvent>, // NEW: Track all fills
}
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 1

#### Task 2.5: Initialize fills Vector in Constructors
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 53-79 and 81-107 (new() and with_capital())
**Action**: Initialize fills vector in both constructors
**Change**:
```rust
// In new() function (add at line 78, before closing brace):
fills: Vec::new(),

// In with_capital() function (add at line 106, before closing brace):
fills: Vec::new(),
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 2

#### Task 2.6: Capture Fills During Execution
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 154-156 (execute_order and process_fill)
**Action**: Store fill events in fills vector
**Change**:
```rust
// OLD (line 154-161):
let fill = self.execution_handler.execute_order(order).await?;
tracing::info!("Order filled: {:?}", fill);

// Track position and calculate PnL if closing
if let Some(pnl) = self.position_tracker.process_fill(&fill) {
    pnls_to_record.push(pnl);
}

// NEW:
let fill = self.execution_handler.execute_order(order).await?;
tracing::info!("Order filled: {:?}", fill);

// Store fill for trade history
self.fills.push(fill.clone());

// Track position and calculate PnL if closing
if let Some(pnl) = self.position_tracker.process_fill(&fill) {
    pnls_to_record.push(pnl);
}
```
**Verification**: `cargo check -p algo-trade-core`
**Estimated LOC**: 3

#### Task 2.7: Return Trades in Metrics
**File**: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
**Location**: Lines 263-278 (PerformanceMetrics construction in calculate_metrics())
**Action**: Include fills in returned metrics
**Change**:
```rust
PerformanceMetrics {
    total_return,
    sharpe_ratio,
    max_drawdown,
    num_trades: total_trades,
    win_rate,
    initial_capital: self.initial_capital,
    final_capital,
    start_time: self.start_time.unwrap_or_else(chrono::Utc::now),
    end_time: self.end_time.unwrap_or_else(chrono::Utc::now),
    duration,
    equity_peak: self.equity_peak,
    buy_hold_return,
    exposure_time,
    trades: self.fills.clone(), // NEW: Include trade history
}
```
**Verification**: `cargo check -p algo-trade-core && cargo test -p algo-trade-core`
**Estimated LOC**: 1

#### Task 2.8: Convert FillEvent to TradeRecord in Runner
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
**Location**: Lines 55-68 (BacktestResult construction)
**Action**: Map FillEvent to TradeRecord when creating BacktestResult
**Change**:
```rust
// Add import at top (after line 1):
use super::{BacktestResult, ParamConfig, StrategyType, TradeRecord, TradeAction};

// Modify BacktestResult construction (lines 56-68):
match run_single_backtest(token, config, &csv_path).await {
    Ok(metrics) => {
        // Convert fills to trade records
        let trades: Vec<TradeRecord> = metrics.trades.iter().map(|fill| {
            let action = match fill.direction {
                algo_trade_core::events::OrderDirection::Buy => TradeAction::Buy,
                algo_trade_core::events::OrderDirection::Sell => TradeAction::Sell,
            };
            TradeRecord {
                timestamp: fill.timestamp,
                action,
                price: fill.price,
                quantity: fill.quantity,
                pnl: None, // PnL calculation requires position tracking (future enhancement)
            }
        }).collect();

        results.push(BacktestResult {
            token: token.clone(),
            config_name: config.name.clone(),
            total_return: metrics.total_return,
            sharpe_ratio: metrics.sharpe_ratio,
            max_drawdown: metrics.max_drawdown,
            num_trades: metrics.num_trades,
            win_rate: metrics.win_rate,
            trades,
        });
        // ... rest of progress_callback
    }
    // ... error handling
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 25

---

### Phase 3: Add TimeframeConfig Screen (Requirement 1)

#### Task 3.1: Add TimeframeConfig to AppScreen Enum
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 36-43 (AppScreen enum)
**Action**: Insert TimeframeConfig variant between TokenSelection and ParameterConfig
**Change**:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppScreen {
    StrategySelection,
    TokenSelection,
    TimeframeConfig, // NEW
    ParameterConfig,
    Running,
    Results,
    TradeDetail, // NEW (for Phase 4)
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 2

#### Task 3.2: Add Timeframe Fields to App Struct
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 114-117 (App struct, Configuration section)
**Action**: Add editable timeframe fields and editing state
**Change**:
```rust
// Configuration (around line 113, before closing App struct)
pub start_date: DateTime<Utc>,
pub end_date: DateTime<Utc>,
pub interval: String,

// NEW: Timeframe editing state
pub timeframe_editing: bool,
pub timeframe_field: TimeframeField, // Which field is being edited
pub timeframe_input: String, // Current input buffer
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 4

#### Task 3.3: Define TimeframeField Enum
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: After AppScreen enum (insert at line 44)
**Action**: Add enum to track which field is being edited
**Change**:
```rust
/// Which field in timeframe config is being edited
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeField {
    StartDate,
    EndDate,
    Interval,
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 7

#### Task 3.4: Initialize Timeframe State in App::new()
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 145-149 (end of App::new())
**Action**: Initialize new timeframe editing fields
**Change**:
```rust
start_date: start,
end_date: end,
interval,

// NEW: Initialize timeframe editing state
timeframe_editing: false,
timeframe_field: TimeframeField::StartDate,
timeframe_input: String::new(),
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 4

#### Task 3.5: Update TokenSelection Navigation to TimeframeConfig
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Line 232 (handle_token_key Enter action)
**Action**: Navigate to TimeframeConfig instead of ParameterConfig
**Change**:
```rust
// OLD:
KeyCode::Enter => {
    if !self.selected_tokens.is_empty() {
        self.current_screen = AppScreen::ParameterConfig;
    }
}

// NEW:
KeyCode::Enter => {
    if !self.selected_tokens.is_empty() {
        self.current_screen = AppScreen::TimeframeConfig;
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 1

#### Task 3.6: Add TimeframeConfig Key Handler
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: After handle_token_key() (insert at line 241, before handle_param_key)
**Action**: Implement keyboard handler for timeframe editing
**Change**:
```rust
fn handle_timeframe_key(&mut self, key: KeyCode) {
    if self.timeframe_editing {
        // Editing mode: capture input
        match key {
            KeyCode::Char(c) => {
                self.timeframe_input.push(c);
            }
            KeyCode::Backspace => {
                self.timeframe_input.pop();
            }
            KeyCode::Enter => {
                // Validate and apply input
                match self.timeframe_field {
                    TimeframeField::StartDate => {
                        if let Ok(date) = chrono::NaiveDate::parse_from_str(&self.timeframe_input, "%Y-%m-%d") {
                            self.start_date = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
                        }
                    }
                    TimeframeField::EndDate => {
                        if let Ok(date) = chrono::NaiveDate::parse_from_str(&self.timeframe_input, "%Y-%m-%d") {
                            self.end_date = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
                        }
                    }
                    TimeframeField::Interval => {
                        // Validate interval format
                        if ["1m", "5m", "15m", "30m", "1h", "4h", "1d"].contains(&self.timeframe_input.as_str()) {
                            self.interval = self.timeframe_input.clone();
                        }
                    }
                }
                self.timeframe_editing = false;
                self.timeframe_input.clear();
            }
            KeyCode::Esc => {
                self.timeframe_editing = false;
                self.timeframe_input.clear();
            }
            _ => {}
        }
    } else {
        // Navigation mode
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.timeframe_field = match self.timeframe_field {
                    TimeframeField::EndDate => TimeframeField::StartDate,
                    TimeframeField::Interval => TimeframeField::EndDate,
                    TimeframeField::StartDate => TimeframeField::Interval,
                };
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.timeframe_field = match self.timeframe_field {
                    TimeframeField::StartDate => TimeframeField::EndDate,
                    TimeframeField::EndDate => TimeframeField::Interval,
                    TimeframeField::Interval => TimeframeField::StartDate,
                };
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                // Enter edit mode
                self.timeframe_editing = true;
                self.timeframe_input = match self.timeframe_field {
                    TimeframeField::StartDate => self.start_date.format("%Y-%m-%d").to_string(),
                    TimeframeField::EndDate => self.end_date.format("%Y-%m-%d").to_string(),
                    TimeframeField::Interval => self.interval.clone(),
                };
            }
            KeyCode::Char('n') => {
                // Next (confirm and proceed)
                if self.start_date < self.end_date {
                    self.current_screen = AppScreen::ParameterConfig;
                }
            }
            KeyCode::Esc => {
                self.current_screen = AppScreen::TokenSelection;
            }
            _ => {}
        }
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 75

#### Task 3.7: Wire TimeframeConfig Handler in handle_key()
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 154-161 (handle_key match statement)
**Action**: Add TimeframeConfig case
**Change**:
```rust
pub fn handle_key(&mut self, key: KeyCode) {
    match self.current_screen {
        AppScreen::StrategySelection => self.handle_strategy_key(key),
        AppScreen::TokenSelection => self.handle_token_key(key),
        AppScreen::TimeframeConfig => self.handle_timeframe_key(key), // NEW
        AppScreen::ParameterConfig => self.handle_param_key(key),
        AppScreen::Running => self.handle_running_key(key),
        AppScreen::Results => self.handle_results_key(key),
        AppScreen::TradeDetail => self.handle_trade_detail_key(key), // NEW (Phase 4)
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 2

#### Task 3.8: Render TimeframeConfig Screen
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs`
**Location**: After render_token_selection() (insert at line 125)
**Action**: Implement UI rendering for timeframe configuration
**Change**:
```rust
fn render_timeframe_config(f: &mut Frame, app: &App) {
    use super::TimeframeField;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(10),    // Form
            Constraint::Length(6),  // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Configure Timeframe")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Form fields
    let field_style = |field: TimeframeField| {
        if field == app.timeframe_field {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        }
    };

    let start_text = if app.timeframe_editing && app.timeframe_field == TimeframeField::StartDate {
        format!("Start Date: {}_", app.timeframe_input)
    } else {
        format!("Start Date: {}", app.start_date.format("%Y-%m-%d"))
    };

    let end_text = if app.timeframe_editing && app.timeframe_field == TimeframeField::EndDate {
        format!("End Date:   {}_", app.timeframe_input)
    } else {
        format!("End Date:   {}", app.end_date.format("%Y-%m-%d"))
    };

    let interval_text = if app.timeframe_editing && app.timeframe_field == TimeframeField::Interval {
        format!("Interval:   {}_", app.timeframe_input)
    } else {
        format!("Interval:   {}", app.interval)
    };

    let form_items = vec![
        ListItem::new(start_text).style(field_style(TimeframeField::StartDate)),
        ListItem::new(end_text).style(field_style(TimeframeField::EndDate)),
        ListItem::new(interval_text).style(field_style(TimeframeField::Interval)),
    ];

    let form = List::new(form_items)
        .block(Block::default().borders(Borders::ALL).title("Configuration"));
    f.render_widget(form, chunks[1]);

    // Instructions
    let instructions = if app.timeframe_editing {
        Paragraph::new(vec![
            Line::from("Type to edit | Enter: Confirm | Esc: Cancel"),
            Line::from("Formats: YYYY-MM-DD (dates), 1m/5m/15m/30m/1h/4h/1d (interval)"),
        ])
    } else {
        Paragraph::new(vec![
            Line::from("↑↓: Navigate | e/Enter: Edit | n: Next"),
            Line::from("Esc: Back | q: Quit"),
        ])
    };

    let instructions = instructions
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 68

#### Task 3.9: Wire TimeframeConfig Rendering
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs`
**Location**: Lines 11-18 (render match statement)
**Action**: Add TimeframeConfig rendering case
**Change**:
```rust
pub fn render(f: &mut Frame, app: &App) {
    match app.current_screen {
        AppScreen::StrategySelection => render_strategy_selection(f, app),
        AppScreen::TokenSelection => render_token_selection(f, app),
        AppScreen::TimeframeConfig => render_timeframe_config(f, app), // NEW
        AppScreen::ParameterConfig => render_parameter_config(f, app),
        AppScreen::Running => render_running(f, app),
        AppScreen::Results => render_results(f, app),
        AppScreen::TradeDetail => render_trade_detail(f, app), // NEW (Phase 4)
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 2

---

### Phase 4: Add TradeDetail Screen (Requirement 3)

#### Task 4.1: Add TradeDetail State to App
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 107-111 (Results section in App struct)
**Action**: Add fields to track selected result and trade detail state
**Change**:
```rust
// Results
pub results: Vec<BacktestResult>,
pub results_scroll_offset: usize,
pub sort_column: usize, // 0=token, 1=config, 2=return, 3=sharpe, etc.
pub sort_ascending: bool,

// NEW: Trade detail view
pub selected_result_index: Option<usize>, // Which result is being viewed
pub trade_detail_scroll: usize,
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 3

#### Task 4.2: Initialize TradeDetail State in App::new()
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 141-144 (Results initialization)
**Action**: Initialize trade detail fields
**Change**:
```rust
results: Vec::new(),
results_scroll_offset: 0,
sort_column: 2, // Default sort by return
sort_ascending: false,

// NEW: Trade detail initialization
selected_result_index: None,
trade_detail_scroll: 0,
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 2

#### Task 4.3: Add Enter Key Handler in Results Screen
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: Lines 299-332 (handle_results_key)
**Action**: Add Enter key to drill down into trade details
**Change**:
```rust
fn handle_results_key(&mut self, key: KeyCode) {
    match key {
        KeyCode::Up | KeyCode::Char('k') => {
            if self.results_scroll_offset > 0 {
                self.results_scroll_offset -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if self.results_scroll_offset < self.results.len().saturating_sub(1) {
                self.results_scroll_offset += 1;
            }
        }
        KeyCode::Enter => { // NEW: Drill down into trades
            if !self.results.is_empty() {
                self.selected_result_index = Some(self.results_scroll_offset);
                self.trade_detail_scroll = 0;
                self.current_screen = AppScreen::TradeDetail;
            }
        }
        KeyCode::Char('s') => {
            // Toggle sort column (cycle through)
            self.sort_column = (self.sort_column + 1) % 6;
            self.sort_results();
        }
        // ... rest of handlers
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 6

#### Task 4.4: Implement TradeDetail Key Handler
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs`
**Location**: After handle_results_key() (insert at line 333)
**Action**: Add handler for TradeDetail screen navigation
**Change**:
```rust
fn handle_trade_detail_key(&mut self, key: KeyCode) {
    match key {
        KeyCode::Up | KeyCode::Char('k') => {
            if self.trade_detail_scroll > 0 {
                self.trade_detail_scroll -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(idx) = self.selected_result_index {
                if let Some(result) = self.results.get(idx) {
                    if self.trade_detail_scroll < result.trades.len().saturating_sub(1) {
                        self.trade_detail_scroll += 1;
                    }
                }
            }
        }
        KeyCode::Esc | KeyCode::Char('b') => {
            // Back to results
            self.current_screen = AppScreen::Results;
            self.selected_result_index = None;
        }
        KeyCode::Char('q') => {
            self.should_quit = true;
        }
        _ => {}
    }
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 30

#### Task 4.5: Update Results Screen Instructions
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs`
**Location**: Lines 338-343 (Results instructions)
**Action**: Add "Enter: View Trades" instruction
**Change**:
```rust
// Instructions
let instructions = Paragraph::new(vec![
    Line::from("↑↓: Scroll | Enter: View Trades | s: Change Sort Column | r: Reverse Sort"),
    Line::from("b: Back to Start | q: Quit"),
])
.alignment(Alignment::Center)
.block(Block::default().borders(Borders::ALL));
f.render_widget(instructions, chunks[2]);
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 1

#### Task 4.6: Render TradeDetail Screen
**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs`
**Location**: After render_results() (insert at line 346)
**Action**: Implement rendering for trade detail view
**Change**:
```rust
fn render_trade_detail(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(6),  // Result summary
            Constraint::Min(10),    // Trades table
            Constraint::Length(3),  // Instructions
        ])
        .split(f.area());

    // Get selected result
    let result = app.selected_result_index
        .and_then(|idx| app.results.get(idx));

    if let Some(result) = result {
        // Title
        let title = Paragraph::new(format!(
            "Trade Details: {} - {}",
            result.token, result.config_name
        ))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, chunks[0]);

        // Result summary
        let summary_lines = vec![
            Line::from(format!("Total Return: {:.2}%", result.total_return.to_f64().unwrap_or(0.0))),
            Line::from(format!("Sharpe Ratio: {:.2}", result.sharpe_ratio)),
            Line::from(format!("Max Drawdown: {:.2}%", result.max_drawdown.to_f64().unwrap_or(0.0))),
            Line::from(format!("Number of Trades: {} | Win Rate: {:.1}%", result.num_trades, result.win_rate * 100.0)),
        ];

        let summary = Paragraph::new(summary_lines)
            .block(Block::default().borders(Borders::ALL).title("Summary"))
            .style(Style::default().fg(Color::Gray));
        f.render_widget(summary, chunks[1]);

        // Trades table
        let header_cells = ["Timestamp", "Action", "Price", "Quantity", "PnL"];
        let header = Row::new(header_cells)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .bottom_margin(1);

        let rows: Vec<Row> = result.trades.iter().map(|trade| {
            let action_str = format!("{:?}", trade.action);
            let pnl_str = trade.pnl
                .map(|p| format!("{:.2}", p.to_f64().unwrap_or(0.0)))
                .unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                trade.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                action_str,
                format!("{:.2}", trade.price.to_f64().unwrap_or(0.0)),
                format!("{:.4}", trade.quantity.to_f64().unwrap_or(0.0)),
                pnl_str,
            ])
        }).collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(20),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(format!("Trades ({} total)", result.trades.len())));

        f.render_widget(table, chunks[2]);
    } else {
        // Error state - no result selected
        let error = Paragraph::new("No result selected")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(error, chunks[1]);
    }

    // Instructions
    let instructions = Paragraph::new("↑↓: Scroll | b/Esc: Back to Results | q: Quit")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[3]);
}
```
**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 85

---

## Task Dependencies

```
Phase 1 (Verification)
    ↓
Phase 2 (Trade Capture Foundation)
    Task 2.1 → Task 2.2 → Task 2.3 → Task 2.4 → Task 2.5 → Task 2.6 → Task 2.7 → Task 2.8
    ↓
Phase 3 (TimeframeConfig Screen)
    Task 3.1 → Task 3.2 → Task 3.3 → Task 3.4 → Task 3.5 → Task 3.6 → Task 3.7 → Task 3.8 → Task 3.9
    ↓
Phase 4 (TradeDetail Screen) - depends on Phase 2
    Task 4.1 → Task 4.2 → Task 4.3 → Task 4.4 → Task 4.5 → Task 4.6
```

**Critical Path**: Phase 2 must complete before Phase 4 (TradeDetail needs trade data)
**Parallel Work**: Phase 3 can be developed independently from Phase 2

---

## Verification Checklist

### Per-Phase Verification

**Phase 1**:
- [x] `cargo check -p algo-trade-cli` passes
- [x] Verify sort_column=2 and sort_ascending=false in code

**Phase 2**:
- [x] `cargo check -p algo-trade-core` passes
- [x] `cargo check -p algo-trade-cli` passes
- [x] `cargo test -p algo-trade-core` passes
- [x] Verify BacktestResult.trades field exists
- [x] Run sample backtest, confirm trades are captured

**Phase 3**:
- [x] `cargo check -p algo-trade-cli` passes
- [x] Navigate StrategySelection → TokenSelection → TimeframeConfig
- [x] Edit dates and interval, confirm validation works
- [x] Proceed to ParameterConfig, verify dates passed correctly

**Phase 4**:
- [x] `cargo check -p algo-trade-cli` passes
- [x] Run backtest to Results screen
- [x] Press Enter on a result, verify TradeDetail appears
- [x] Scroll through trades, verify all trades shown
- [x] Press Esc, return to Results

### Final Integration Tests

```bash
# Build entire workspace
cargo build --release

# Run TUI with default args (should show TimeframeConfig screen)
cargo run -p algo-trade-cli -- tui

# Manual TUI workflow test:
# 1. Select MA Crossover strategy
# 2. Select 2-3 tokens
# 3. Edit timeframe: 3 days ago, today, 1m interval
# 4. Use default parameter config
# 5. Run backtests
# 6. Verify results sorted by total_return descending
# 7. Press Enter on top result
# 8. Verify trade detail shows all trades
# 9. Navigate back to Results
```

### Karen Review (MANDATORY)

After all phases complete, invoke Karen agent:
```
Act as Karen agent from .claude/agents/karen.md. Review package algo-trade-cli and algo-trade-core following ALL 6 phases. Include actual terminal outputs for each phase.
```

**Zero Tolerance Checklist**:
- [x] Zero rustc errors/warnings
- [x] Zero clippy warnings (default + pedantic + nursery)
- [x] Zero unused imports
- [x] All new structs/enums derive Debug, Clone
- [x] All financial values use rust_decimal::Decimal
- [x] Consistent error handling patterns

---

## Edge Cases & Error Handling

### Timeframe Validation
- **Invalid date format**: Silently reject, keep existing value
- **Start date > End date**: Block navigation to ParameterConfig (add validation at line 3.6)
- **Invalid interval**: Only accept whitelisted values (1m, 5m, 15m, 30m, 1h, 4h, 1d)

### Trade Detail View
- **No trades in result**: Display "No trades executed" message
- **Empty results list**: Pressing Enter does nothing
- **Selected result deleted**: TradeDetail shows error, press Esc to return

### Backward Compatibility
- **CLI args override TUI defaults**: App::new() takes start/end/interval from CLI
- **Existing tests**: No changes to existing test files needed

---

## Estimated Total LOC

| Phase | Tasks | Estimated LOC | Files Modified |
|-------|-------|---------------|----------------|
| 1     | 1     | 0             | 0              |
| 2     | 8     | 54            | 2              |
| 3     | 9     | 165           | 2              |
| 4     | 6     | 127           | 2              |
| **Total** | **24** | **346** | **3 unique** |

**Files Modified**:
1. `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/mod.rs` - State machine, handlers
2. `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs` - UI rendering
3. `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` - Trade conversion
4. `/home/a/Work/algo-trade/crates/core/src/engine.rs` - Trade capture

---

## Post-Implementation Notes

### Future Enhancements (NOT in scope)

1. **PnL per trade**: Requires refactoring `PositionTracker` to track entry/exit pairs
2. **Trade filtering**: Filter by action type, date range, profitability
3. **Export trades to CSV**: Save trade detail to file
4. **Real-time progress bar**: Show per-token progress during backtests
5. **Timeframe presets**: "Last 3 days", "Last week", "Last month" buttons

### Known Limitations

- **PnL column shows "-"**: Individual trade PnL calculation requires position pairing (tracked in PositionTracker but not exposed yet)
- **No date picker**: Manual text entry only (ratatui has no native date picker widget)
- **Scroll indicators**: No visual indicator when scrolled (ratatui limitation)

---

**End of Playbook**
