# Playbook: Interactive TUI for Multi-Token Parameter Sweep Backtesting

**Date**: 2025-10-03
**TaskMaster**: Claude (Sonnet 4.5)
**Context Report**: `/home/a/Work/algo-trade/.claude/context/2025-10-03_multi-token-param-sweep-tui.md`
**Status**: ✅ Ready for Execution

---

## User Request (Verbatim from Context Report)

```
"I want an interactive TUI that lets me:
1. Select a strategy (MA Crossover or Quad MA)
2. Select tokens from a list of all available Hyperliquid tokens (multiple selection, or select all)
3. Input strategy parameters with defaults shown, but allow adding multiple parameter configurations
4. Backtest the selected strategy against EVERY token × EVERY parameter configuration
5. Display results in a comparison view"
```

---

## Scope Boundaries

### MUST DO

1. ✅ Add `ratatui`, `crossterm`, `tui-input` dependencies to `crates/cli/Cargo.toml`
2. ✅ Create `crates/cli/src/tui_backtest.rs` - Main TUI application (~800 lines)
3. ✅ Create `crates/cli/src/tui_backtest/screens.rs` - Screen rendering (~400 lines)
4. ✅ Create `crates/cli/src/tui_backtest/runner.rs` - Backtest execution (~300 lines)
5. ✅ Add `TuiBacktest` command to `crates/cli/src/main.rs:11` (Commands enum)
6. ✅ Add `run_tui_backtest()` handler to `crates/cli/src/main.rs:66` (match statement)
7. ✅ Add `fetch_available_symbols()` method to `crates/exchange-hyperliquid/src/client.rs:217`
8. ✅ Implement 5-screen state machine: Strategy → Tokens → Params → Running → Results
9. ✅ Implement data caching in `cache/` directory with format `symbol_interval_start_end.csv`
10. ✅ Implement progress tracking with ratatui Gauge widget
11. ✅ Implement results table with sorting by Return, Sharpe, Win%, Trades
12. ✅ Implement CSV export in results screen (S key)
13. ✅ Add parameter validation: MA1 < MA2 < MA3 < MA4, periods > 0
14. ✅ Add edge case handling: Empty selection, network errors, insufficient data
15. ✅ Preserve backward compatibility: Existing `backtest` command unchanged

### MUST NOT DO

1. ❌ DO NOT modify existing `run_backtest()` function (line 103-156 in `cli/src/main.rs`)
2. ❌ DO NOT change `PerformanceMetrics` struct (used by existing code)
3. ❌ DO NOT use `f64` for financial values (use `rust_decimal::Decimal`)
4. ❌ DO NOT add parallel execution in MVP (keep sequential for simplicity)
5. ❌ DO NOT use `indicatif` for progress (conflicts with ratatui)
6. ❌ DO NOT hardcode token list (must fetch from Hyperliquid API)
7. ❌ DO NOT break existing CLI commands (`run`, `server`, `fetch-data`)
8. ❌ DO NOT modify `TradingSystem` or `Strategy` traits (public API)
9. ❌ DO NOT add authentication (TUI uses public data only)
10. ❌ DO NOT auto-clean cache directory (let user manage disk space)

---

## Atomic Tasks

### Phase 0: Setup Dependencies and Module Structure

#### Task 0.1: Add TUI Dependencies to CLI Crate

**File**: `/home/a/Work/algo-trade/crates/cli/Cargo.toml`
**Location**: ~Line 15 (in `[dependencies]` section)
**Action**: Add ratatui, crossterm, and tui-input dependencies

**Changes**:
```toml
# Add these lines to [dependencies] section
ratatui = "0.28"
crossterm = "0.28"
tui-input = "0.10"
```

**Verification**: `cargo tree -p algo-trade-cli | grep ratatui`
**Estimated LOC**: 3
**Complexity**: LOW
**Dependencies**: None

---

#### Task 0.2: Declare TUI Backtest Module

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 1 (top of file, after existing module declarations)
**Action**: Add module declaration for tui_backtest

**Changes**:
```rust
// Add after existing mod declarations
mod tui_backtest;
```

**Verification**: `cargo check -p algo-trade-cli` (should fail until module exists)
**Estimated LOC**: 1
**Complexity**: LOW
**Dependencies**: None

---

### Phase 1: Hyperliquid API Integration

#### Task 1.1: Add fetch_available_symbols Method

**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: Line 217 (after `fetch_candles_chunk()` method)
**Action**: Add method to fetch list of tradeable symbols from Hyperliquid

**Changes**:
```rust
/// Fetches list of all tradeable perpetual symbols
///
/// # Errors
/// Returns error if API request fails or response parsing fails
pub async fn fetch_available_symbols(&self) -> Result<Vec<String>> {
    let request_body = serde_json::json!({ "type": "meta" });
    let response = self.post("/info", request_body).await?;

    let universe = response["universe"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing universe in meta response"))?;

    let symbols: Vec<String> = universe
        .iter()
        .filter_map(|item| item["name"].as_str().map(String::from))
        .collect();

    tracing::info!("Fetched {} available symbols from Hyperliquid", symbols.len());

    Ok(symbols)
}
```

**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated LOC**: 20
**Complexity**: MEDIUM
**Dependencies**: Task 0.1

---

### Phase 2: CLI Command Integration

#### Task 2.1: Add TuiBacktest Command Variant

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 11 (in `Commands` enum, after last existing variant)
**Action**: Add new command variant for TUI backtest

**Changes**:
```rust
/// Interactive TUI for multi-token parameter sweep backtesting
TuiBacktest {
    /// Candle interval (1h, 1d, etc.)
    #[arg(short, long, default_value = "1h")]
    interval: String,

    /// Start time in ISO 8601 format
    #[arg(long, default_value = "2025-01-01T00:00:00Z")]
    start: String,

    /// End time in ISO 8601 format
    #[arg(long, default_value = "2025-03-01T00:00:00Z")]
    end: String,
},
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 12
**Complexity**: LOW
**Dependencies**: Task 0.2

---

#### Task 2.2: Add Command Match Arm

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 66 (in `match cli.command` block, after last existing arm)
**Action**: Add match arm to handle TuiBacktest command

**Changes**:
```rust
Commands::TuiBacktest { interval, start, end } => {
    run_tui_backtest(&interval, &start, &end).await?;
}
```

**Verification**: `cargo check -p algo-trade-cli` (will fail until handler exists)
**Estimated LOC**: 3
**Complexity**: LOW
**Dependencies**: Task 2.1

---

#### Task 2.3: Add TUI Backtest Handler Function

**File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
**Location**: Line 234 (after `run_fetch_data()` function)
**Action**: Add async function to launch TUI application

**Changes**:
```rust
async fn run_tui_backtest(
    interval: &str,
    start_str: &str,
    end_str: &str,
) -> anyhow::Result<()> {
    use tui_backtest::TuiApp;

    // Parse and validate date range
    let start: DateTime<Utc> = start_str.parse()
        .context("Invalid start time. Use ISO 8601 format (e.g., 2025-01-01T00:00:00Z)")?;
    let end: DateTime<Utc> = end_str.parse()
        .context("Invalid end time. Use ISO 8601 format (e.g., 2025-03-01T00:00:00Z)")?;

    if start >= end {
        anyhow::bail!("Start time must be before end time");
    }

    let days = (end - start).num_days();
    if days < 7 {
        anyhow::bail!("Date range must be at least 7 days for meaningful backtests");
    }
    if days > 365 {
        anyhow::bail!("Date range cannot exceed 365 days (performance limitation)");
    }

    // Launch TUI application
    let mut app = TuiApp::new(interval.to_string(), start, end).await?;
    app.run().await?;

    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-cli` (will fail until TuiApp exists)
**Estimated LOC**: 28
**Complexity**: MEDIUM
**Dependencies**: Task 2.2

---

### Phase 3: Core TUI Application Structure

#### Task 3.1: Create TUI Backtest Module with Types

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs` (NEW)
**Location**: N/A (new file)
**Action**: Create main TUI module with state types and basic structure

**Changes**:
```rust
//! Interactive TUI for multi-token parameter sweep backtesting

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tui_input::Input;

use algo_trade_backtest::{HistoricalDataProvider, PerformanceMetrics};
use algo_trade_core::{SimpleRiskManager, TradingSystem};
use algo_trade_execution::SimulatedExecutionHandler;
use algo_trade_hyperliquid::HyperliquidClient;
use algo_trade_strategy::{MaCrossoverStrategy, QuadMaStrategy, Strategy};

mod runner;
mod screens;

pub use runner::BacktestRunner;

/// Main TUI application state machine
pub struct TuiApp {
    state: AppState,
    hyperliquid_client: Arc<HyperliquidClient>,
    interval: String,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
}

/// Application state variants
enum AppState {
    StrategySelection {
        selected: StrategyType,
    },
    TokenSelection {
        strategy: StrategyType,
        tokens: Vec<String>,
        selected: HashSet<usize>,
        list_state: ListState,
    },
    ParameterConfig {
        strategy: StrategyType,
        tokens: Vec<String>,
        configs: Vec<ParamConfig>,
        selected_index: usize,
    },
    Running {
        matrix: Vec<BacktestJob>,
        completed: usize,
        total: usize,
        current_job: String,
        results: Vec<BacktestResult>,
    },
    Results {
        results: Vec<BacktestResult>,
        sort_by: SortColumn,
        table_state: TableState,
    },
}

/// Strategy type selection
#[derive(Clone, Debug)]
enum StrategyType {
    MaCrossover,
    QuadMa,
}

impl StrategyType {
    fn name(&self) -> &str {
        match self {
            Self::MaCrossover => "MA Crossover",
            Self::QuadMa => "Quad MA",
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::MaCrossover => "2 moving averages - Simple crossover strategy",
            Self::QuadMa => "4 moving averages - Fibonacci sequence (5/8/13/21)",
        }
    }

    fn toggle(&mut self) {
        *self = match self {
            Self::MaCrossover => Self::QuadMa,
            Self::QuadMa => Self::MaCrossover,
        };
    }
}

/// Parameter configuration for a strategy
#[derive(Clone, Debug)]
struct ParamConfig {
    name: String,
    params: StrategyParams,
}

/// Strategy-specific parameters
#[derive(Clone, Debug)]
enum StrategyParams {
    MaCrossover { fast: usize, slow: usize },
    QuadMa { ma1: usize, ma2: usize, ma3: usize, ma4: usize },
}

impl StrategyParams {
    /// Validate parameter constraints
    fn validate(&self) -> Result<()> {
        match self {
            Self::MaCrossover { fast, slow } => {
                if *fast == 0 || *slow == 0 {
                    anyhow::bail!("Periods must be greater than 0");
                }
                if fast >= slow {
                    anyhow::bail!("Fast period must be less than slow period");
                }
                Ok(())
            }
            Self::QuadMa { ma1, ma2, ma3, ma4 } => {
                if *ma1 == 0 || *ma2 == 0 || *ma3 == 0 || *ma4 == 0 {
                    anyhow::bail!("Periods must be greater than 0");
                }
                if !(*ma1 < *ma2 && *ma2 < *ma3 && *ma3 < *ma4) {
                    anyhow::bail!("Periods must be in ascending order: MA1 < MA2 < MA3 < MA4");
                }
                Ok(())
            }
        }
    }
}

/// Backtest job specification
#[derive(Clone, Debug)]
struct BacktestJob {
    symbol: String,
    config_name: String,
    params: StrategyParams,
}

/// Backtest result
#[derive(Clone, Debug)]
struct BacktestResult {
    symbol: String,
    config_name: String,
    metrics: PerformanceMetrics,
}

/// Result table sort column
#[derive(Clone, Copy, Debug)]
enum SortColumn {
    Return,
    Sharpe,
    WinRate,
    Trades,
}

impl SortColumn {
    fn next(&self) -> Self {
        match self {
            Self::Return => Self::Sharpe,
            Self::Sharpe => Self::WinRate,
            Self::WinRate => Self::Trades,
            Self::Trades => Self::Return,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Return => "Return",
            Self::Sharpe => "Sharpe",
            Self::WinRate => "Win%",
            Self::Trades => "Trades",
        }
    }
}

impl TuiApp {
    /// Create new TUI application
    pub async fn new(interval: String, start_time: DateTime<Utc>, end_time: DateTime<Utc>) -> Result<Self> {
        let api_url = "https://api.hyperliquid.xyz".to_string();
        let client = HyperliquidClient::new(api_url);

        Ok(Self {
            state: AppState::StrategySelection {
                selected: StrategyType::MaCrossover,
            },
            hyperliquid_client: Arc::new(client),
            interval,
            start_time,
            end_time,
        })
    }

    /// Main event loop
    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        stdout()
            .execute(EnterAlternateScreen)
            .context("Failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout());
        let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

        // Run event loop
        let result = self.run_event_loop(&mut terminal).await;

        // Restore terminal
        disable_raw_mode().context("Failed to disable raw mode")?;
        stdout()
            .execute(LeaveAlternateScreen)
            .context("Failed to leave alternate screen")?;

        result
    }

    async fn run_event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if !self.handle_key(key.code).await? {
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Main UI dispatcher
    fn ui(&self, frame: &mut Frame) {
        match &self.state {
            AppState::StrategySelection { selected } => {
                self.ui_strategy_selection(frame, selected);
            }
            AppState::TokenSelection { strategy, tokens, selected, list_state } => {
                self.ui_token_selection(frame, strategy, tokens, selected, list_state);
            }
            AppState::ParameterConfig { strategy, configs, selected_index, .. } => {
                self.ui_parameter_config(frame, strategy, configs, *selected_index);
            }
            AppState::Running { completed, total, current_job, .. } => {
                self.ui_running(frame, *completed, *total, current_job);
            }
            AppState::Results { results, sort_by, table_state } => {
                self.ui_results(frame, results, *sort_by, table_state);
            }
        }
    }

    /// Main key handler dispatcher
    async fn handle_key(&mut self, key: KeyCode) -> Result<bool> {
        // Global quit
        if matches!(key, KeyCode::Char('q') | KeyCode::Char('Q')) {
            return Ok(false);
        }

        match &mut self.state {
            AppState::StrategySelection { selected } => {
                self.handle_strategy_selection_key(key, selected).await?;
            }
            AppState::TokenSelection { .. } => {
                self.handle_token_selection_key(key).await?;
            }
            AppState::ParameterConfig { .. } => {
                self.handle_parameter_config_key(key).await?;
            }
            AppState::Running { .. } => {
                // Only allow Esc to cancel
                if key == KeyCode::Esc {
                    // TODO: Implement cancellation
                }
            }
            AppState::Results { .. } => {
                self.handle_results_key(key).await?;
            }
        }

        Ok(true)
    }
}
```

**Verification**: `cargo check -p algo-trade-cli` (should compile with stubs)
**Estimated LOC**: 300
**Complexity**: HIGH
**Dependencies**: Task 0.1, Task 2.3

---

#### Task 3.2: Create Screens Module

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs` (NEW)
**Location**: N/A (new file)
**Action**: Create screen rendering implementations

**Changes**:
```rust
//! TUI screen rendering implementations

use super::*;

impl TuiApp {
    /// Screen 1: Strategy Selection
    pub fn ui_strategy_selection(&self, frame: &mut Frame, selected: &StrategyType) {
        let area = frame.area();

        let strategies = vec![StrategyType::MaCrossover, StrategyType::QuadMa];
        let items: Vec<ListItem> = strategies
            .iter()
            .map(|st| {
                let symbol = if st.name() == selected.name() { "> " } else { "  " };
                let text = format!("{}{} - {}", symbol, st.name(), st.description());
                ListItem::new(text)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title("Select Strategy")
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().bg(Color::Blue));

        let help = Paragraph::new("↑/↓: Navigate | Enter: Continue | Q: Quit")
            .style(Style::default().fg(Color::DarkGray));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(1)])
            .split(area);

        frame.render_widget(list, chunks[0]);
        frame.render_widget(help, chunks[1]);
    }

    /// Screen 2: Token Selection
    pub fn ui_token_selection(
        &self,
        frame: &mut Frame,
        strategy: &StrategyType,
        tokens: &[String],
        selected: &HashSet<usize>,
        list_state: &ListState,
    ) {
        let area = frame.area();

        let title = format!("Select Tokens for {} Strategy", strategy.name());
        let selected_count = format!("Selected: {}/{}", selected.len(), tokens.len());

        let items: Vec<ListItem> = tokens
            .iter()
            .enumerate()
            .map(|(i, token)| {
                let checkbox = if selected.contains(&i) { "[x]" } else { "[ ]" };
                ListItem::new(format!("{} {}", checkbox, token))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(title.as_str())
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD));

        let info = Paragraph::new(selected_count.as_str())
            .style(Style::default().fg(Color::Green));

        let help = Paragraph::new("↑/↓: Navigate | Space: Toggle | A: All | N: None | Enter: Continue | Esc: Back | Q: Quit")
            .style(Style::default().fg(Color::DarkGray));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        let mut list_state_mut = list_state.clone();
        frame.render_stateful_widget(list, chunks[0], &mut list_state_mut);
        frame.render_widget(info, chunks[1]);
        frame.render_widget(help, chunks[2]);
    }

    /// Screen 3: Parameter Configuration
    pub fn ui_parameter_config(
        &self,
        frame: &mut Frame,
        strategy: &StrategyType,
        configs: &[ParamConfig],
        selected_index: usize,
    ) {
        let area = frame.area();

        let title = format!("{} - Parameter Configurations", strategy.name());

        let mut items = Vec::new();
        for (i, config) in configs.iter().enumerate() {
            let symbol = if i == selected_index { "> " } else { "  " };
            let params_str = match &config.params {
                StrategyParams::MaCrossover { fast, slow } => {
                    format!("Fast: {} | Slow: {}", fast, slow)
                }
                StrategyParams::QuadMa { ma1, ma2, ma3, ma4 } => {
                    format!("MA1: {} | MA2: {} | MA3: {} | MA4: {}", ma1, ma2, ma3, ma4)
                }
            };
            items.push(ListItem::new(format!("{}{} - {}", symbol, config.name, params_str)));
        }

        if configs.len() < 10 {
            items.push(ListItem::new("  [+] Add Configuration").style(Style::default().fg(Color::Green)));
        }

        let list = List::new(items)
            .block(Block::default().title(title.as_str()).borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::Blue));

        let help = Paragraph::new("↑/↓: Navigate | Enter: Run | E: Edit | D: Delete | A: Add | Esc: Back | Q: Quit")
            .style(Style::default().fg(Color::DarkGray));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(1)])
            .split(area);

        frame.render_widget(list, chunks[0]);
        frame.render_widget(help, chunks[1]);
    }

    /// Screen 4: Running Backtests
    pub fn ui_running(&self, frame: &mut Frame, completed: usize, total: usize, current_job: &str) {
        let area = frame.area();

        let title = format!("Running Backtests ({}/{})", completed, total);
        let progress = if total > 0 {
            (completed as f64 / total as f64 * 100.0) as u16
        } else {
            0
        };

        let gauge = Gauge::default()
            .block(Block::default().title(title.as_str()).borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .percent(progress)
            .label(format!("{}%", progress));

        let current = Paragraph::new(format!("Current: {}", current_job))
            .style(Style::default().fg(Color::Cyan));

        let help = Paragraph::new("Esc: Cancel | Q: Quit")
            .style(Style::default().fg(Color::DarkGray));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        frame.render_widget(gauge, chunks[0]);
        frame.render_widget(current, chunks[1]);
        frame.render_widget(help, chunks[3]);
    }

    /// Screen 5: Results
    pub fn ui_results(
        &self,
        frame: &mut Frame,
        results: &[BacktestResult],
        sort_by: SortColumn,
        table_state: &TableState,
    ) {
        let area = frame.area();

        let header = Row::new(vec![
            "Token",
            "Config",
            "Return ▼",
            "Sharpe",
            "Win%",
            "Trades",
        ])
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

        let rows = results.iter().map(|r| {
            let total_return = r.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
            Row::new(vec![
                r.symbol.clone(),
                r.config_name.clone(),
                format!("{:.2}%", total_return * 100.0),
                format!("{:.2}", r.metrics.sharpe_ratio),
                format!("{:.1}%", r.metrics.win_rate * 100.0),
                r.metrics.num_trades.to_string(),
            ])
        });

        let widths = [
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default().title("Backtest Results").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::DarkGray));

        let sort_info = Paragraph::new(format!("Sort by: {} | Total: {}", sort_by.name(), results.len()))
            .style(Style::default().fg(Color::Cyan));

        let help = Paragraph::new("↑/↓: Navigate | S: Sort | X: Export CSV | R: Restart | Q: Quit")
            .style(Style::default().fg(Color::DarkGray));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        let mut table_state_mut = table_state.clone();
        frame.render_stateful_widget(table, chunks[0], &mut table_state_mut);
        frame.render_widget(sort_info, chunks[1]);
        frame.render_widget(help, chunks[2]);
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 200
**Complexity**: MEDIUM
**Dependencies**: Task 3.1

---

#### Task 3.3: Create Runner Module

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` (NEW)
**Location**: N/A (new file)
**Action**: Create backtest execution and data caching logic

**Changes**:
```rust
//! Backtest execution engine with caching

use super::*;
use anyhow::Context;
use std::path::Path;
use algo_trade_data::CsvStorage;

pub struct BacktestRunner {
    hyperliquid_client: Arc<HyperliquidClient>,
    interval: String,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
}

impl BacktestRunner {
    pub fn new(
        hyperliquid_client: Arc<HyperliquidClient>,
        interval: String,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Self> {
        // Ensure cache directory exists
        std::fs::create_dir_all("cache").context("Failed to create cache directory")?;

        Ok(Self {
            hyperliquid_client,
            interval,
            start_time,
            end_time,
        })
    }

    /// Run batch of backtests sequentially
    pub async fn run_batch(&self, jobs: Vec<BacktestJob>) -> Result<Vec<BacktestResult>> {
        let mut results = Vec::new();

        for job in jobs {
            match self.run_single_backtest(&job).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::warn!("Backtest failed for {} with {}: {}", job.symbol, job.config_name, e);
                    // Continue with other backtests
                }
            }
        }

        Ok(results)
    }

    /// Get cached data path or fetch from API
    async fn get_or_fetch_data(&self, symbol: &str) -> Result<String> {
        let cache_path = format!(
            "cache/{}_{}_{}_{}.csv",
            symbol,
            self.interval,
            self.start_time.timestamp(),
            self.end_time.timestamp()
        );

        if Path::new(&cache_path).exists() {
            tracing::info!("Using cached data for {}", symbol);
            Ok(cache_path)
        } else {
            tracing::info!("Fetching data for {}", symbol);
            let records = self
                .hyperliquid_client
                .fetch_candles(symbol, &self.interval, self.start_time, self.end_time)
                .await
                .context(format!("Failed to fetch candles for {}", symbol))?;

            if records.is_empty() {
                anyhow::bail!("No data available for {} in specified date range", symbol);
            }

            CsvStorage::write_ohlcv(&cache_path, &records)
                .context("Failed to write cached data")?;
            Ok(cache_path)
        }
    }

    /// Run single backtest
    async fn run_single_backtest(&self, job: &BacktestJob) -> Result<BacktestResult> {
        // Get or fetch data
        let csv_path = self.get_or_fetch_data(&job.symbol).await?;

        // Load data
        let data_provider = HistoricalDataProvider::from_csv(&csv_path)
            .context("Failed to load historical data")?;

        // Validate sufficient data for strategy parameters
        self.validate_data_length(&job.params, &csv_path)?;

        // Create strategy
        let strategy: Arc<Mutex<dyn Strategy>> = match &job.params {
            StrategyParams::MaCrossover { fast, slow } => Arc::new(Mutex::new(
                MaCrossoverStrategy::new(job.symbol.clone(), *fast, *slow),
            )),
            StrategyParams::QuadMa { ma1, ma2, ma3, ma4 } => Arc::new(Mutex::new(
                QuadMaStrategy::with_periods(job.symbol.clone(), *ma1, *ma2, *ma3, *ma4),
            )),
        };

        // Create trading system
        let mut system = TradingSystem::new(
            data_provider,
            SimulatedExecutionHandler::new(0.001, 5.0),
            vec![strategy],
            Arc::new(SimpleRiskManager::new(1000.0, 0.1)),
        );

        // Run backtest
        let metrics = system.run().await.context("Backtest execution failed")?;

        Ok(BacktestResult {
            symbol: job.symbol.clone(),
            config_name: job.config_name.clone(),
            metrics,
        })
    }

    /// Validate sufficient data for strategy
    fn validate_data_length(&self, params: &StrategyParams, csv_path: &str) -> Result<()> {
        let file = std::fs::File::open(csv_path)?;
        let reader = std::io::BufReader::new(file);
        let line_count = std::io::BufRead::lines(reader).count().saturating_sub(1); // Subtract header

        let required = match params {
            StrategyParams::MaCrossover { slow, .. } => *slow,
            StrategyParams::QuadMa { ma4, .. } => *ma4,
        };

        if line_count < required {
            anyhow::bail!(
                "Insufficient data: strategy requires {} candles, got {}",
                required,
                line_count
            );
        }

        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 150
**Complexity**: HIGH
**Dependencies**: Task 3.1, Task 1.1

---

### Phase 4: Event Handlers

#### Task 4.1: Implement Strategy Selection Handler

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs`
**Location**: After `handle_key()` method (~line 250)
**Action**: Add handler for strategy selection screen

**Changes**:
```rust
async fn handle_strategy_selection_key(
    &mut self,
    key: KeyCode,
    selected: &mut StrategyType,
) -> Result<()> {
    match key {
        KeyCode::Up | KeyCode::Down => {
            selected.toggle();
        }
        KeyCode::Enter => {
            // Fetch available tokens from Hyperliquid
            let tokens = self
                .hyperliquid_client
                .fetch_available_symbols()
                .await
                .context("Failed to fetch available symbols from Hyperliquid")?;

            if tokens.is_empty() {
                anyhow::bail!("No tokens available from Hyperliquid");
            }

            self.state = AppState::TokenSelection {
                strategy: selected.clone(),
                tokens,
                selected: HashSet::new(),
                list_state: ListState::default(),
            };
        }
        _ => {}
    }
    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 30
**Complexity**: MEDIUM
**Dependencies**: Task 3.1, Task 1.1

---

#### Task 4.2: Implement Token Selection Handler

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs`
**Location**: After `handle_strategy_selection_key()` method
**Action**: Add handler for token selection screen

**Changes**:
```rust
async fn handle_token_selection_key(&mut self, key: KeyCode) -> Result<()> {
    if let AppState::TokenSelection {
        strategy,
        tokens,
        selected,
        list_state,
    } = &mut self.state
    {
        match key {
            KeyCode::Up => {
                let current = list_state.selected().unwrap_or(0);
                if current > 0 {
                    list_state.select(Some(current - 1));
                }
            }
            KeyCode::Down => {
                let current = list_state.selected().unwrap_or(0);
                if current < tokens.len().saturating_sub(1) {
                    list_state.select(Some(current + 1));
                }
            }
            KeyCode::Char(' ') => {
                let current = list_state.selected().unwrap_or(0);
                if selected.contains(&current) {
                    selected.remove(&current);
                } else {
                    selected.insert(current);
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                *selected = (0..tokens.len()).collect();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                selected.clear();
            }
            KeyCode::Enter => {
                if selected.is_empty() {
                    // Show error (for now, just do nothing)
                    return Ok(());
                }

                let selected_tokens: Vec<String> = selected
                    .iter()
                    .map(|&i| tokens[i].clone())
                    .collect();

                // Create default configuration for selected strategy
                let default_config = self.create_default_config(&strategy);

                self.state = AppState::ParameterConfig {
                    strategy: strategy.clone(),
                    tokens: selected_tokens,
                    configs: vec![default_config],
                    selected_index: 0,
                };
            }
            KeyCode::Esc => {
                self.state = AppState::StrategySelection {
                    selected: strategy.clone(),
                };
            }
            _ => {}
        }
    }
    Ok(())
}

fn create_default_config(&self, strategy: &StrategyType) -> ParamConfig {
    match strategy {
        StrategyType::MaCrossover => ParamConfig {
            name: "Default".to_string(),
            params: StrategyParams::MaCrossover { fast: 10, slow: 30 },
        },
        StrategyType::QuadMa => ParamConfig {
            name: "Default".to_string(),
            params: StrategyParams::QuadMa {
                ma1: 5,
                ma2: 8,
                ma3: 13,
                ma4: 21,
            },
        },
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 80
**Complexity**: MEDIUM
**Dependencies**: Task 3.1

---

#### Task 4.3: Implement Parameter Config Handler

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs`
**Location**: After `handle_token_selection_key()` method
**Action**: Add handler for parameter configuration screen

**Changes**:
```rust
async fn handle_parameter_config_key(&mut self, key: KeyCode) -> Result<()> {
    if let AppState::ParameterConfig {
        strategy,
        tokens,
        configs,
        selected_index,
    } = &mut self.state
    {
        match key {
            KeyCode::Up => {
                if *selected_index > 0 {
                    *selected_index -= 1;
                }
            }
            KeyCode::Down => {
                let max_index = configs.len(); // +1 for "Add Config" option
                if *selected_index < max_index {
                    *selected_index += 1;
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if configs.len() < 10 {
                    let new_config = self.create_numbered_config(strategy, configs.len() + 1);
                    configs.push(new_config);
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if !configs.is_empty() && *selected_index < configs.len() {
                    configs.remove(*selected_index);
                    if *selected_index >= configs.len() && *selected_index > 0 {
                        *selected_index -= 1;
                    }
                }
            }
            KeyCode::Enter => {
                if configs.is_empty() {
                    return Ok(());
                }

                // Build backtest matrix
                let mut jobs = Vec::new();
                for token in tokens.iter() {
                    for config in configs.iter() {
                        jobs.push(BacktestJob {
                            symbol: token.clone(),
                            config_name: config.name.clone(),
                            params: config.params.clone(),
                        });
                    }
                }

                let total = jobs.len();
                self.state = AppState::Running {
                    matrix: jobs,
                    completed: 0,
                    total,
                    current_job: String::new(),
                    results: Vec::new(),
                };

                // Start backtests
                self.run_backtests().await?;
            }
            KeyCode::Esc => {
                // Go back to token selection
                let all_tokens = self.hyperliquid_client.fetch_available_symbols().await?;
                let selected_indices: HashSet<usize> = all_tokens
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| tokens.contains(t))
                    .map(|(i, _)| i)
                    .collect();

                self.state = AppState::TokenSelection {
                    strategy: strategy.clone(),
                    tokens: all_tokens,
                    selected: selected_indices,
                    list_state: ListState::default(),
                };
            }
            _ => {}
        }
    }
    Ok(())
}

fn create_numbered_config(&self, strategy: &StrategyType, number: usize) -> ParamConfig {
    match strategy {
        StrategyType::MaCrossover => ParamConfig {
            name: format!("Config {}", number),
            params: StrategyParams::MaCrossover {
                fast: 10 + (number * 5),
                slow: 30 + (number * 10),
            },
        },
        StrategyType::QuadMa => ParamConfig {
            name: format!("Config {}", number),
            params: StrategyParams::QuadMa {
                ma1: 5,
                ma2: 8 + number,
                ma3: 13 + (number * 2),
                ma4: 21 + (number * 3),
            },
        },
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100
**Complexity**: MEDIUM
**Dependencies**: Task 3.1, Task 3.3

---

#### Task 4.4: Implement Backtest Execution

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs`
**Location**: After `handle_parameter_config_key()` method
**Action**: Add backtest execution logic

**Changes**:
```rust
async fn run_backtests(&mut self) -> Result<()> {
    if let AppState::Running { matrix, completed, total, current_job, results } = &mut self.state {
        let runner = BacktestRunner::new(
            self.hyperliquid_client.clone(),
            self.interval.clone(),
            self.start_time,
            self.end_time,
        )?;

        for (i, job) in matrix.iter().enumerate() {
            *current_job = format!("{} - {}", job.symbol, job.config_name);
            *completed = i;

            match runner.run_single_backtest(job).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::warn!("Backtest failed: {}", e);
                }
            }
        }

        *completed = *total;

        // Transition to results screen
        let mut results_sorted = results.clone();
        results_sorted.sort_by(|a, b| {
            let a_return = a.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
            let b_return = b.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
            b_return.partial_cmp(&a_return).unwrap_or(std::cmp::Ordering::Equal)
        });

        self.state = AppState::Results {
            results: results_sorted,
            sort_by: SortColumn::Return,
            table_state: TableState::default(),
        };
    }

    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 40
**Complexity**: MEDIUM
**Dependencies**: Task 3.3

---

#### Task 4.5: Implement Results Handler

**File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs`
**Location**: After `run_backtests()` method
**Action**: Add handler for results screen

**Changes**:
```rust
async fn handle_results_key(&mut self, key: KeyCode) -> Result<()> {
    if let AppState::Results {
        results,
        sort_by,
        table_state,
    } = &mut self.state
    {
        match key {
            KeyCode::Up => {
                let current = table_state.selected().unwrap_or(0);
                if current > 0 {
                    table_state.select(Some(current - 1));
                }
            }
            KeyCode::Down => {
                let current = table_state.selected().unwrap_or(0);
                if current < results.len().saturating_sub(1) {
                    table_state.select(Some(current + 1));
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                *sort_by = sort_by.next();
                self.sort_results(results, *sort_by);
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                self.export_results_csv(results)?;
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.state = AppState::StrategySelection {
                    selected: StrategyType::MaCrossover,
                };
            }
            _ => {}
        }
    }
    Ok(())
}

fn sort_results(&self, results: &mut [BacktestResult], sort_by: SortColumn) {
    results.sort_by(|a, b| {
        let cmp = match sort_by {
            SortColumn::Return => {
                let a_val = a.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
                let b_val = b.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
                b_val.partial_cmp(&a_val).unwrap_or(std::cmp::Ordering::Equal)
            }
            SortColumn::Sharpe => {
                b.metrics.sharpe_ratio.partial_cmp(&a.metrics.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal)
            }
            SortColumn::WinRate => {
                b.metrics.win_rate.partial_cmp(&a.metrics.win_rate).unwrap_or(std::cmp::Ordering::Equal)
            }
            SortColumn::Trades => {
                b.metrics.num_trades.cmp(&a.metrics.num_trades)
            }
        };
        cmp
    });
}

fn export_results_csv(&self, results: &[BacktestResult]) -> Result<()> {
    use std::io::Write;

    let filename = format!("backtest_results_{}.csv", chrono::Utc::now().timestamp());
    let mut file = std::fs::File::create(&filename)
        .context("Failed to create CSV file")?;

    // Write header
    writeln!(file, "Symbol,Config,Return,Sharpe,WinRate,Trades,MaxDrawdown")?;

    // Write rows
    for r in results {
        let total_return = r.metrics.total_return.to_string().parse::<f64>().unwrap_or(0.0);
        writeln!(
            file,
            "{},{},{:.4},{:.4},{:.4},{},{:.4}",
            r.symbol,
            r.config_name,
            total_return,
            r.metrics.sharpe_ratio,
            r.metrics.win_rate,
            r.metrics.num_trades,
            r.metrics.max_drawdown,
        )?;
    }

    tracing::info!("Results exported to {}", filename);
    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 80
**Complexity**: MEDIUM
**Dependencies**: Task 3.1

---

### Phase 5: Integration and Testing

#### Task 5.1: Fix Compilation Errors

**File**: Various
**Location**: Throughout codebase
**Action**: Fix any compilation errors from incomplete implementations

**Changes**: Address missing imports, type mismatches, visibility issues

**Verification**: `cargo build -p algo-trade-cli`
**Estimated LOC**: 20-50 (adjustments)
**Complexity**: MEDIUM
**Dependencies**: All previous tasks

---

#### Task 5.2: Add Integration Test

**File**: `/home/a/Work/algo-trade/crates/cli/tests/tui_backtest_test.rs` (NEW)
**Location**: N/A (new file)
**Action**: Create basic integration test

**Changes**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_backtest_runner_caching() {
        // Test that cache directory is created
        // Test that second fetch uses cached data
        // (Requires mock Hyperliquid client)
    }

    #[tokio::test]
    async fn test_parameter_validation() {
        // Test MA1 < MA2 < MA3 < MA4
        let params = StrategyParams::QuadMa {
            ma1: 5,
            ma2: 8,
            ma3: 13,
            ma4: 21,
        };
        assert!(params.validate().is_ok());

        let invalid = StrategyParams::QuadMa {
            ma1: 10,
            ma2: 8,
            ma3: 13,
            ma4: 21,
        };
        assert!(invalid.validate().is_err());
    }
}
```

**Verification**: `cargo test -p algo-trade-cli tui_backtest`
**Estimated LOC**: 50
**Complexity**: LOW
**Dependencies**: Task 5.1

---

## Task Dependencies Graph

```
Phase 0: Setup
Task 0.1 (Cargo deps) ──┬─→ Task 3.1
                        └─→ Task 1.1
Task 0.2 (mod decl) ────→ Task 2.1

Phase 1: API
Task 1.1 (Hyperliquid API) ──→ Task 4.1

Phase 2: CLI
Task 2.1 (Command) ──→ Task 2.2 (Match arm)
Task 2.2 ──→ Task 2.3 (Handler)

Phase 3: Core TUI
Task 3.1 (tui_backtest.rs) ──┬─→ Task 3.2 (screens.rs)
                              ├─→ Task 3.3 (runner.rs)
                              └─→ Task 4.1

Task 3.2 ──→ Task 4.2
Task 3.3 ──→ Task 4.3

Phase 4: Handlers
Task 4.1 ──→ Task 4.2
Task 4.2 ──→ Task 4.3
Task 4.3 ──→ Task 4.4
Task 4.4 ──→ Task 4.5

Phase 5: Integration
Task 4.5 ──→ Task 5.1 (Fix compilation)
Task 5.1 ──→ Task 5.2 (Tests)
```

---

## Verification Checklist

### Per-Task Verification

- [ ] **Task 0.1**: `cargo tree -p algo-trade-cli | grep ratatui` shows dependency
- [ ] **Task 0.2**: `cargo check -p algo-trade-cli` (expected to fail until module exists)
- [ ] **Task 1.1**: `cargo check -p algo-trade-hyperliquid` succeeds
- [ ] **Task 2.1**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 2.2**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 2.3**: `cargo check -p algo-trade-cli` (expected to fail until TuiApp exists)
- [ ] **Task 3.1**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 3.2**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 3.3**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 4.1**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 4.2**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 4.3**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 4.4**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 4.5**: `cargo check -p algo-trade-cli` succeeds
- [ ] **Task 5.1**: `cargo build -p algo-trade-cli` succeeds
- [ ] **Task 5.2**: `cargo test -p algo-trade-cli` succeeds

### Integration Verification

- [ ] `cargo run -p algo-trade-cli -- tui-backtest` launches TUI
- [ ] Strategy selection screen displays with up/down navigation
- [ ] Token selection fetches symbols from Hyperliquid API
- [ ] Space key toggles token selection
- [ ] 'A' key selects all tokens
- [ ] Parameter config allows adding/deleting configurations
- [ ] Enter on parameter screen starts backtests
- [ ] Progress gauge updates during backtest execution
- [ ] Results table displays with correct data
- [ ] 'S' key cycles through sort columns
- [ ] 'X' key exports CSV file
- [ ] Cached data is reused on second run (check logs)
- [ ] Existing commands still work: `cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv --strategy quad_ma`

### Karen Quality Gates (MANDATORY)

- [ ] **Phase 0**: `cargo build --package algo-trade-cli --lib` succeeds
- [ ] **Phase 1**: `cargo clippy -p algo-trade-cli -- -D warnings` (zero warnings)
- [ ] **Phase 1b**: `cargo clippy -p algo-trade-cli -- -W clippy::pedantic` (zero pedantic warnings)
- [ ] **Phase 1c**: `cargo clippy -p algo-trade-cli -- -W clippy::nursery` (zero nursery warnings)
- [ ] **Phase 2**: Zero rust-analyzer diagnostics in VSCode/editor
- [ ] **Phase 3**: No broken cross-file references
- [ ] **Phase 4**: All files individually compile
- [ ] **Phase 5**: Run Karen agent for full review
- [ ] **Phase 6**: `cargo build --release -p algo-trade-cli` succeeds

---

## Edge Cases & Error Handling

### Implemented in Tasks

1. **Empty token selection** (Task 4.2): Prevent advancing with 0 tokens
2. **Parameter validation** (Task 3.1): MA1 < MA2 < MA3 < MA4
3. **Insufficient data** (Task 3.3): Validate candle count vs required periods
4. **Network errors** (Task 4.1): Context on API failures
5. **Cache directory** (Task 3.3): Create if not exists
6. **Date range validation** (Task 2.3): 7-365 day constraint
7. **Failed backtests** (Task 3.3): Continue with remaining jobs
8. **CSV export errors** (Task 4.5): Contextual error messages
9. **Max configs limit** (Task 4.3): Prevent >10 configurations
10. **Terminal restore** (Task 3.1): Ensure cleanup on error

---

## Complexity Estimate

| Phase | Tasks | Total LOC | Est. Time | Risk Level |
|-------|-------|-----------|-----------|------------|
| 0 | 2 | 4 | 15m | LOW |
| 1 | 1 | 20 | 30m | MEDIUM |
| 2 | 3 | 43 | 30m | LOW |
| 3 | 3 | 650 | 8h | HIGH |
| 4 | 5 | 330 | 4h | MEDIUM |
| 5 | 2 | 70 | 1.5h | MEDIUM |

**Total**: ~1117 LOC, ~14.5 hours

**High-Risk Tasks**:
- Task 3.1: Core TUI state machine (300 LOC, complex async)
- Task 3.3: Backtest runner with caching (150 LOC, file I/O)
- Task 4.3: Parameter config with matrix generation (100 LOC, logic heavy)

---

## Manual Testing Checklist

**After Phase 5 completion**:

- [ ] Launch TUI: `cargo run -p algo-trade-cli -- tui-backtest`
- [ ] Navigate strategies with ↑/↓, verify toggle works
- [ ] Press Enter, verify token list fetches from API
- [ ] Select 3 tokens with Space, verify checkboxes
- [ ] Press 'A', verify all selected
- [ ] Press 'N', verify all deselected
- [ ] Select 2 tokens, press Enter
- [ ] On parameter screen, press 'A' to add config
- [ ] Verify second config appears
- [ ] Press 'D' to delete first config
- [ ] Press Enter to run backtests
- [ ] Verify progress gauge animates
- [ ] Verify current job updates
- [ ] Wait for completion, verify results table
- [ ] Press 'S' multiple times, verify sort changes
- [ ] Press ↑/↓ to navigate results
- [ ] Press 'X', verify CSV file created
- [ ] Press 'R', verify returns to strategy selection
- [ ] Press 'Q', verify clean exit
- [ ] Run again with same params, verify "Using cached data" in logs
- [ ] Test terminal resize during each screen
- [ ] Test Esc key on each screen (except running)

---

## Final Notes

**Backward Compatibility**: All existing CLI commands preserved. New `TuiBacktest` command is opt-in.

**Performance**: Sequential execution for MVP. Parallel execution can be added in future iteration using `tokio::spawn` and `JoinSet`.

**Extensibility**: Easy to add new strategies by:
1. Adding variant to `StrategyType` enum
2. Adding case to `create_default_config()`
3. Adding case in `BacktestRunner::run_single_backtest()`

**Karen Review**: MANDATORY after all tasks complete. Block any phase progression until zero issues.

---

**Playbook Status**: ✅ Ready for Execution
**Generated**: 2025-10-03
**TaskMaster**: Claude (Sonnet 4.5)
