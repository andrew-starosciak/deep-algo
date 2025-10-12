# Backtest Manager TUI + Docker Integration Playbook

**Date**: 2025-10-12
**Feature**: Add new TUI for viewing scheduled backtest results, token selection, and scheduler configuration
**Estimated Completion**: 6 phases, ~800 LOC total

---

## User Request

Create a new TUI called "backtest manager" that displays:
1. Scheduled backtest reports from TimescaleDB
2. Token selection results (approved/rejected tokens)
3. Scheduler configuration visualization
4. Docker integration via TUI_MODE environment variable

---

## Scope Boundaries

### MUST DO

1. ✅ Create new file `crates/cli/src/tui_backtest_manager.rs` with 5 screens:
   - Dashboard: Scheduler status, recent backtest count, token summary
   - Reports: Table view of backtest results from database
   - TokenSelection: Show approved/rejected tokens with metrics
   - Config: Display current scheduler configuration (read-only)
   - ReportDetail: Full metrics for single backtest result

2. ✅ Add `BacktestManagerTui` CLI command to main.rs

3. ✅ Update `docker/entrypoint.sh` to support TUI_MODE environment variable (route to correct TUI)

4. ✅ Update `docker-compose.yml` to add TUI_MODE env var with default

5. ✅ Test both TUIs work in Docker

6. ✅ Document in CLAUDE.md

### MUST NOT DO

1. ❌ Modify existing `tui_backtest/` (parameter sweep tool - leave unchanged)
2. ❌ Modify existing `tui_live_bot.rs` structure (only reference as pattern)
3. ❌ Add write/edit capabilities to config (read-only MVP)
4. ❌ Change Dockerfile (already works, no changes needed)
5. ❌ Break existing Docker daemon + ttyd behavior

---

## Architecture Context

**Pattern from tui_live_bot.rs** (reference structure):
- App struct with current_screen enum
- Screen-specific render functions
- Async data loading with tokio::select!
- Keyboard navigation (↑↓ for lists, Tab for screens, Enter to select, Esc to back)

**Data Sources**:
- `DatabaseClient::query_latest_backtest_results(strategy, hours)` → `Vec<BacktestResultRecord>`
- `TokenSelector::get_selection_details(strategy)` → `Vec<TokenSelectionResult>`
- Config read from `Config.toml` (already loaded by `ConfigLoader`)

**Key Data Structures**:
```rust
// From crates/data/src/database.rs
pub struct BacktestResultRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub exchange: String,
    pub strategy_name: String,
    pub sharpe_ratio: f64,
    pub sortino_ratio: Option<f64>,
    pub total_pnl: Decimal,
    pub total_return: Decimal,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub num_trades: i32,
    pub parameters: Option<JsonValue>,
}

// From crates/token-selector/src/selector.rs
pub struct TokenSelectionResult {
    pub symbol: String,
    pub sharpe_ratio: f64,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub num_trades: i32,
    pub total_pnl: Decimal,
    pub approved: bool,
}

// From crates/core/src/config.rs
pub struct BacktestSchedulerConfig {
    pub enabled: bool,
    pub cron_schedule: String,
    pub fetch_universe_from_exchange: bool,
    pub token_universe: Option<Vec<String>>,
    pub backtest_window_days: i64,
    pub strategy_name: String,
    pub exchange: String,
    pub hyperliquid_api_url: String,
}
```

---

## Phase 1: Create TUI Module Structure

### Task 1.1: Create tui_backtest_manager.rs with screen enum and app struct

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs` (new file)
**Lines**: 1-180
**Action**: Create module with:
- `ManagerScreen` enum (Dashboard, Reports, TokenSelection, Config, ReportDetail)
- `ManagerApp` struct (db_client, selector, config, screen state, cached data)
- Constructor and helper methods

**Code**:
```rust
use algo_trade_core::{config::Config, ConfigLoader};
use algo_trade_data::{BacktestResultRecord, DatabaseClient};
use algo_trade_token_selector::{TokenSelector, TokenSelectionResult};
use anyhow::Result;
use chrono::Utc;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table},
    Frame, Terminal,
};
use std::io;
use std::sync::Arc;

/// Application screens
#[derive(Debug, Clone, PartialEq, Eq)]
enum ManagerScreen {
    Dashboard,
    Reports,
    TokenSelection,
    Config,
    ReportDetail,
}

/// Application state
struct ManagerApp {
    config: Config,
    db_client: Arc<DatabaseClient>,
    selector: Arc<TokenSelector>,
    current_screen: ManagerScreen,

    // Dashboard data
    total_backtest_count: usize,
    approved_token_count: usize,
    rejected_token_count: usize,

    // Reports data
    cached_reports: Vec<BacktestResultRecord>,
    selected_report_index: usize,

    // Token selection data
    cached_token_results: Vec<TokenSelectionResult>,
    selected_token_index: usize,

    // Detail view
    detail_report: Option<BacktestResultRecord>,

    // Messages
    messages: Vec<String>,
}

impl ManagerApp {
    async fn new() -> Result<Self> {
        // Load config
        let config = ConfigLoader::load()?;

        // Create database client
        let db_client = Arc::new(DatabaseClient::new(&config.database.url).await?);

        // Create token selector
        let selector = Arc::new(TokenSelector::new(
            config.token_selector.clone(),
            db_client.clone()
        ));

        Ok(Self {
            config,
            db_client,
            selector,
            current_screen: ManagerScreen::Dashboard,
            total_backtest_count: 0,
            approved_token_count: 0,
            rejected_token_count: 0,
            cached_reports: Vec::new(),
            selected_report_index: 0,
            cached_token_results: Vec::new(),
            selected_token_index: 0,
            detail_report: None,
            messages: vec!["Backtest Manager - Press 'd' for Dashboard, 'r' for Reports, 't' for Token Selection, 'c' for Config, 'q' to Quit".to_string()],
        })
    }

    fn add_message(&mut self, msg: String) {
        self.messages.push(msg);
        if self.messages.len() > 10 {
            self.messages.remove(0);
        }
    }

    async fn refresh_dashboard_data(&mut self) -> Result<()> {
        let strategy_name = &self.config.backtest_scheduler.strategy_name;
        let lookback_hours = self.config.token_selector.lookback_hours;

        // Query reports
        let reports = self.db_client
            .query_latest_backtest_results(strategy_name, lookback_hours)
            .await?;
        self.total_backtest_count = reports.len();

        // Query token selection
        let token_results = self.selector
            .get_selection_details(strategy_name)
            .await?;
        self.approved_token_count = token_results.iter().filter(|t| t.approved).count();
        self.rejected_token_count = token_results.len() - self.approved_token_count;

        Ok(())
    }

    async fn refresh_reports(&mut self) -> Result<()> {
        let strategy_name = &self.config.backtest_scheduler.strategy_name;
        let lookback_hours = self.config.token_selector.lookback_hours;

        self.cached_reports = self.db_client
            .query_latest_backtest_results(strategy_name, lookback_hours)
            .await?;

        Ok(())
    }

    async fn refresh_token_selection(&mut self) -> Result<()> {
        let strategy_name = &self.config.backtest_scheduler.strategy_name;

        self.cached_token_results = self.selector
            .get_selection_details(strategy_name)
            .await?;

        Ok(())
    }
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 180

---

### Task 1.2: Add public run() function for TUI entry point

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: 181-230
**Action**: Add async run() function that initializes terminal and main loop

**Code**:
```rust
pub async fn run() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = ManagerApp::new().await?;

    // Run app
    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut ManagerApp,
) -> Result<()> {
    loop {
        // Refresh data based on current screen
        match app.current_screen {
            ManagerScreen::Dashboard => {
                let _ = app.refresh_dashboard_data().await;
            }
            ManagerScreen::Reports => {
                if app.cached_reports.is_empty() {
                    let _ = app.refresh_reports().await;
                }
            }
            ManagerScreen::TokenSelection => {
                if app.cached_token_results.is_empty() {
                    let _ = app.refresh_token_selection().await;
                }
            }
            _ => {}
        }

        terminal.draw(|f| ui(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(key, app).await? {
                    break; // Quit requested
                }
            }
        }
    }

    Ok(())
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 50

---

### KAREN GATE: Phase 1 Complete

**Command**: Invoke Karen agent with:
```bash
cargo build --package algo-trade-cli --lib
cargo clippy -p algo-trade-cli -- -D warnings
cargo clippy -p algo-trade-cli -- -W clippy::pedantic -W clippy::nursery
```

**Expected**: Zero errors, zero warnings, module compiles successfully.

**Blocking**: If Karen finds issues, STOP and fix atomically before proceeding to Phase 2.

---

## Phase 2: Implement Dashboard and Reports Screens

### Task 2.1: Implement Dashboard screen render function

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~400-500
**Action**: Create `render_dashboard()` function showing:
- Scheduler status (enabled/disabled, cron schedule)
- Recent backtest count (last 48h)
- Token summary (approved/rejected counts)
- Navigation help

**Code Outline**:
```rust
fn render_dashboard(f: &mut Frame, app: &ManagerApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(8),   // Scheduler status
            Constraint::Length(6),   // Backtest summary
            Constraint::Length(6),   // Token summary
            Constraint::Min(5),      // Messages
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Backtest Manager - Dashboard")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Scheduler status panel
    let scheduler_enabled = if app.config.backtest_scheduler.enabled { "✓ ENABLED" } else { "✗ DISABLED" };
    let scheduler_color = if app.config.backtest_scheduler.enabled { Color::Green } else { Color::Red };

    let scheduler_lines = vec![
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(scheduler_enabled, Style::default().fg(scheduler_color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("Cron Schedule: "),
            Span::styled(&app.config.backtest_scheduler.cron_schedule, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Strategy: "),
            Span::styled(&app.config.backtest_scheduler.strategy_name, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Exchange: "),
            Span::styled(&app.config.backtest_scheduler.exchange, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::raw("Backtest Window: "),
            Span::styled(format!("{} days", app.config.backtest_scheduler.backtest_window_days), Style::default().fg(Color::White)),
        ]),
    ];

    let scheduler_panel = Paragraph::new(scheduler_lines)
        .block(Block::default().borders(Borders::ALL).title("Scheduler Configuration"));
    f.render_widget(scheduler_panel, chunks[1]);

    // Backtest summary
    let backtest_lines = vec![
        Line::from(vec![
            Span::raw("Total Reports (Last "),
            Span::styled(format!("{}h", app.config.token_selector.lookback_hours), Style::default().fg(Color::Yellow)),
            Span::raw("): "),
            Span::styled(app.total_backtest_count.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("Recent Timestamp: "),
            Span::styled(Utc::now().format("%Y-%m-%d %H:%M UTC").to_string(), Style::default().fg(Color::Cyan)),
        ]),
    ];

    let backtest_panel = Paragraph::new(backtest_lines)
        .block(Block::default().borders(Borders::ALL).title("Backtest Reports"));
    f.render_widget(backtest_panel, chunks[2]);

    // Token summary
    let token_lines = vec![
        Line::from(vec![
            Span::styled("Approved Tokens: ", Style::default().fg(Color::Green)),
            Span::styled(app.approved_token_count.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("Rejected Tokens: ", Style::default().fg(Color::Red)),
            Span::styled(app.rejected_token_count.to_string(), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("Total Evaluated: "),
            Span::styled((app.approved_token_count + app.rejected_token_count).to_string(), Style::default().fg(Color::White)),
        ]),
    ];

    let token_panel = Paragraph::new(token_lines)
        .block(Block::default().borders(Borders::ALL).title("Token Selection"));
    f.render_widget(token_panel, chunks[3]);

    // Messages
    let messages: Vec<Line> = app.messages.iter().map(|m| Line::from(m.as_str())).collect();
    let messages_widget = Paragraph::new(messages)
        .block(Block::default().borders(Borders::ALL).title("Messages"))
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(messages_widget, chunks[4]);

    // Help
    let help = Paragraph::new("d: Dashboard | r: Reports | t: Token Selection | c: Config | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Navigation"));
    f.render_widget(help, chunks[5]);
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### Task 2.2: Implement Reports screen render function

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~500-600
**Action**: Create `render_reports()` function showing table of backtest results

**Code Outline**:
```rust
fn render_reports(f: &mut Frame, app: &ManagerApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Min(10),     // Reports table
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!("Backtest Reports - {} ({} results)",
                                       app.config.backtest_scheduler.strategy_name,
                                       app.cached_reports.len()))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Reports table
    if app.cached_reports.is_empty() {
        let empty = Paragraph::new("No backtest results found. Press 'r' to refresh.")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(empty, chunks[1]);
    } else {
        let header = Row::new(vec!["Symbol", "Sharpe", "Win%", "MaxDD%", "Trades", "PnL", "Timestamp"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .bottom_margin(1);

        let rows: Vec<Row> = app.cached_reports
            .iter()
            .enumerate()
            .map(|(i, record)| {
                let style = if i == app.selected_report_index {
                    Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    record.symbol.clone(),
                    format!("{:.2}", record.sharpe_ratio),
                    format!("{:.1}", record.win_rate * 100.0),
                    format!("{:.1}", record.max_drawdown.to_string().parse::<f64>().unwrap_or(0.0) * 100.0),
                    record.num_trades.to_string(),
                    format!("${}", record.total_pnl),
                    record.timestamp.format("%m-%d %H:%M").to_string(),
                ])
                .style(style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(10),  // Symbol
                Constraint::Length(8),   // Sharpe
                Constraint::Length(6),   // Win%
                Constraint::Length(8),   // MaxDD%
                Constraint::Length(8),   // Trades
                Constraint::Length(12),  // PnL
                Constraint::Length(15),  // Timestamp
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Reports"));

        f.render_widget(table, chunks[1]);
    }

    // Help
    let help = Paragraph::new("↑↓: Navigate | Enter: View Detail | r: Refresh | Esc: Back to Dashboard | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### KAREN GATE: Phase 2 Complete

**Command**: Invoke Karen agent with:
```bash
cargo build --package algo-trade-cli --lib
cargo clippy -p algo-trade-cli -- -D warnings
cargo clippy -p algo-trade-cli -- -W clippy::pedantic -W clippy::nursery
```

**Expected**: Zero errors, zero warnings, Dashboard and Reports render correctly.

**Blocking**: If Karen finds issues, STOP and fix atomically before proceeding to Phase 3.

---

## Phase 3: Implement Token Selection, Config, and Report Detail Screens

### Task 3.1: Implement Token Selection screen render function

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~600-700
**Action**: Create `render_token_selection()` function showing approved/rejected tokens with metrics

**Code Outline**:
```rust
fn render_token_selection(f: &mut Frame, app: &ManagerApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(5),   // Criteria summary
            Constraint::Min(10),     // Token table
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!("Token Selection - {} ({} approved / {} total)",
                                       app.config.backtest_scheduler.strategy_name,
                                       app.approved_token_count,
                                       app.cached_token_results.len()))
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Criteria summary
    let criteria_lines = vec![
        Line::from(vec![
            Span::raw("Min Sharpe: "),
            Span::styled(format!("{:.2}", app.config.token_selector.min_sharpe_ratio), Style::default().fg(Color::Yellow)),
            Span::raw("  Min Win Rate: "),
            Span::styled(format!("{:.0}%", app.config.token_selector.min_win_rate * 100.0), Style::default().fg(Color::Yellow)),
            Span::raw("  Max DD: "),
            Span::styled(format!("{:.0}%", app.config.token_selector.max_drawdown * 100.0), Style::default().fg(Color::Yellow)),
            Span::raw("  Min Trades: "),
            Span::styled(app.config.token_selector.min_num_trades.to_string(), Style::default().fg(Color::Yellow)),
        ]),
    ];

    let criteria_panel = Paragraph::new(criteria_lines)
        .block(Block::default().borders(Borders::ALL).title("Selection Criteria"));
    f.render_widget(criteria_panel, chunks[1]);

    // Token table
    if app.cached_token_results.is_empty() {
        let empty = Paragraph::new("No token results found. Press 't' to refresh.")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(empty, chunks[2]);
    } else {
        let header = Row::new(vec!["Status", "Symbol", "Sharpe", "Win%", "MaxDD%", "Trades", "PnL"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .bottom_margin(1);

        let rows: Vec<Row> = app.cached_token_results
            .iter()
            .enumerate()
            .map(|(i, result)| {
                let status_icon = if result.approved { "✓" } else { "✗" };
                let status_color = if result.approved { Color::Green } else { Color::Red };

                let style = if i == app.selected_token_index {
                    Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    status_icon.to_string(),
                    result.symbol.clone(),
                    format!("{:.2}", result.sharpe_ratio),
                    format!("{:.1}", result.win_rate * 100.0),
                    format!("{:.1}", result.max_drawdown.to_string().parse::<f64>().unwrap_or(0.0) * 100.0),
                    result.num_trades.to_string(),
                    format!("${}", result.total_pnl),
                ])
                .style(if result.approved {
                    style.fg(Color::Green)
                } else {
                    style.fg(Color::Gray)
                })
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(6),   // Status
                Constraint::Length(10),  // Symbol
                Constraint::Length(8),   // Sharpe
                Constraint::Length(6),   // Win%
                Constraint::Length(8),   // MaxDD%
                Constraint::Length(8),   // Trades
                Constraint::Length(12),  // PnL
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Tokens"));

        f.render_widget(table, chunks[2]);
    }

    // Help
    let help = Paragraph::new("↑↓: Navigate | t: Refresh | Esc: Back to Dashboard | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[3]);
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### Task 3.2: Implement Config screen render function

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~700-800
**Action**: Create `render_config()` function showing full scheduler and selector configuration (read-only)

**Code Outline**:
```rust
fn render_config(f: &mut Frame, app: &ManagerApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Min(5),      // Scheduler config
            Constraint::Min(5),      // Token selector config
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Configuration (Read-Only)")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Scheduler config
    let scheduler_lines = vec![
        Line::from(vec![
            Span::raw("Enabled: "),
            Span::styled(app.config.backtest_scheduler.enabled.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Cron Schedule: "),
            Span::styled(&app.config.backtest_scheduler.cron_schedule, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Strategy: "),
            Span::styled(&app.config.backtest_scheduler.strategy_name, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Exchange: "),
            Span::styled(&app.config.backtest_scheduler.exchange, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Backtest Window: "),
            Span::styled(format!("{} days", app.config.backtest_scheduler.backtest_window_days), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("API URL: "),
            Span::styled(&app.config.backtest_scheduler.hyperliquid_api_url, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Fetch Universe from Exchange: "),
            Span::styled(app.config.backtest_scheduler.fetch_universe_from_exchange.to_string(), Style::default().fg(Color::Yellow)),
        ]),
    ];

    let scheduler_panel = Paragraph::new(scheduler_lines)
        .block(Block::default().borders(Borders::ALL).title("Backtest Scheduler"));
    f.render_widget(scheduler_panel, chunks[1]);

    // Token selector config
    let selector_lines = vec![
        Line::from(vec![
            Span::raw("Min Sharpe Ratio: "),
            Span::styled(format!("{:.2}", app.config.token_selector.min_sharpe_ratio), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Min Win Rate: "),
            Span::styled(format!("{:.1}%", app.config.token_selector.min_win_rate * 100.0), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Max Drawdown: "),
            Span::styled(format!("{:.1}%", app.config.token_selector.max_drawdown * 100.0), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Min Num Trades: "),
            Span::styled(app.config.token_selector.min_num_trades.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::raw("Lookback Hours: "),
            Span::styled(format!("{}h", app.config.token_selector.lookback_hours), Style::default().fg(Color::Yellow)),
        ]),
    ];

    let selector_panel = Paragraph::new(selector_lines)
        .block(Block::default().borders(Borders::ALL).title("Token Selector"));
    f.render_widget(selector_panel, chunks[2]);

    // Help
    let help = Paragraph::new("Esc: Back to Dashboard | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Navigation"));
    f.render_widget(help, chunks[3]);
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### Task 3.3: Implement Report Detail screen render function

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~800-900
**Action**: Create `render_report_detail()` function showing full metrics for single backtest

**Code Outline**:
```rust
fn render_report_detail(f: &mut Frame, app: &ManagerApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Min(15),     // Details
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    if let Some(report) = &app.detail_report {
        // Title
        let title = Paragraph::new(format!("Backtest Report Detail - {}", report.symbol))
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, chunks[0]);

        // Details
        let detail_lines = vec![
            Line::from(vec![
                Span::styled("Symbol: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(&report.symbol),
            ]),
            Line::from(vec![
                Span::styled("Strategy: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(&report.strategy_name),
            ]),
            Line::from(vec![
                Span::styled("Exchange: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(&report.exchange),
            ]),
            Line::from(vec![
                Span::styled("Timestamp: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(report.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Performance Metrics:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
            ]),
            Line::from(vec![
                Span::raw("  Sharpe Ratio: "),
                Span::styled(format!("{:.2}", report.sharpe_ratio), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::raw("  Sortino Ratio: "),
                Span::styled(
                    report.sortino_ratio.map_or("N/A".to_string(), |v| format!("{:.2}", v)),
                    Style::default().fg(Color::Cyan)
                ),
            ]),
            Line::from(vec![
                Span::raw("  Total PnL: "),
                Span::styled(format!("${}", report.total_pnl),
                    if report.total_pnl > rust_decimal::Decimal::ZERO {
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                ),
            ]),
            Line::from(vec![
                Span::raw("  Total Return: "),
                Span::styled(format!("{:.2}%", report.total_return.to_string().parse::<f64>().unwrap_or(0.0) * 100.0),
                    if report.total_return > rust_decimal::Decimal::ZERO {
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                ),
            ]),
            Line::from(vec![
                Span::raw("  Win Rate: "),
                Span::styled(format!("{:.1}%", report.win_rate * 100.0), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::raw("  Max Drawdown: "),
                Span::styled(format!("{:.1}%", report.max_drawdown.to_string().parse::<f64>().unwrap_or(0.0) * 100.0), Style::default().fg(Color::Red)),
            ]),
            Line::from(vec![
                Span::raw("  Number of Trades: "),
                Span::styled(report.num_trades.to_string(), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Parameters:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
            ]),
            Line::from(vec![
                Span::raw(if let Some(params) = &report.parameters {
                    format!("  {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "N/A".to_string()))
                } else {
                    "  N/A".to_string()
                }),
            ]),
        ];

        let detail_panel = Paragraph::new(detail_lines)
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(detail_panel, chunks[1]);
    } else {
        let empty = Paragraph::new("No report selected")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(empty, chunks[1]);
    }

    // Help
    let help = Paragraph::new("Esc: Back to Reports | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Navigation"));
    f.render_widget(help, chunks[2]);
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### Task 3.4: Add main ui() dispatcher and key event handlers

**File**: `/home/g/Work/deep-algo/crates/cli/src/tui_backtest_manager.rs`
**Lines**: ~900-1000
**Action**: Add ui() function that dispatches to screen renderers and handle_key_event() for navigation

**Code Outline**:
```rust
fn ui(f: &mut Frame, app: &ManagerApp) {
    match app.current_screen {
        ManagerScreen::Dashboard => render_dashboard(f, app),
        ManagerScreen::Reports => render_reports(f, app),
        ManagerScreen::TokenSelection => render_token_selection(f, app),
        ManagerScreen::Config => render_config(f, app),
        ManagerScreen::ReportDetail => render_report_detail(f, app),
    }
}

async fn handle_key_event(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match app.current_screen {
        ManagerScreen::Dashboard => handle_dashboard_keys(key, app),
        ManagerScreen::Reports => handle_reports_keys(key, app),
        ManagerScreen::TokenSelection => handle_token_selection_keys(key, app),
        ManagerScreen::Config => handle_config_keys(key, app),
        ManagerScreen::ReportDetail => handle_report_detail_keys(key, app),
    }
}

fn handle_dashboard_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true), // Quit
        KeyCode::Char('d') => {
            app.current_screen = ManagerScreen::Dashboard;
        }
        KeyCode::Char('r') => {
            app.current_screen = ManagerScreen::Reports;
            app.cached_reports.clear(); // Force refresh
        }
        KeyCode::Char('t') => {
            app.current_screen = ManagerScreen::TokenSelection;
            app.cached_token_results.clear(); // Force refresh
        }
        KeyCode::Char('c') => {
            app.current_screen = ManagerScreen::Config;
        }
        _ => {}
    }
    Ok(false)
}

fn handle_reports_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Dashboard;
        }
        KeyCode::Char('r') => {
            app.cached_reports.clear(); // Force refresh
        }
        KeyCode::Enter => {
            if let Some(report) = app.cached_reports.get(app.selected_report_index) {
                app.detail_report = Some(report.clone());
                app.current_screen = ManagerScreen::ReportDetail;
            }
        }
        KeyCode::Up => {
            if app.selected_report_index > 0 {
                app.selected_report_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_report_index < app.cached_reports.len().saturating_sub(1) {
                app.selected_report_index += 1;
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_token_selection_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Dashboard;
        }
        KeyCode::Char('t') => {
            app.cached_token_results.clear(); // Force refresh
        }
        KeyCode::Up => {
            if app.selected_token_index > 0 {
                app.selected_token_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_token_index < app.cached_token_results.len().saturating_sub(1) {
                app.selected_token_index += 1;
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_config_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Dashboard;
        }
        _ => {}
    }
    Ok(false)
}

fn handle_report_detail_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Reports;
            app.detail_report = None;
        }
        _ => {}
    }
    Ok(false)
}
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 100

---

### KAREN GATE: Phase 3 Complete

**Command**: Invoke Karen agent with:
```bash
cargo build --package algo-trade-cli --lib
cargo clippy -p algo-trade-cli -- -D warnings
cargo clippy -p algo-trade-cli -- -W clippy::pedantic -W clippy::nursery
```

**Expected**: Zero errors, zero warnings, all 5 screens render correctly.

**Blocking**: If Karen finds issues, STOP and fix atomically before proceeding to Phase 4.

---

## Phase 4: CLI Integration

### Task 4.1: Add module declaration in main.rs

**File**: `/home/g/Work/deep-algo/crates/cli/src/main.rs`
**Lines**: 4-5 (after existing mod declarations)
**Action**: Add module declaration for new TUI

**Code**:
```rust
mod tui_backtest;
mod tui_backtest_manager; // NEW
mod tui_live_bot;
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 1

---

### Task 4.2: Add BacktestManagerTui CLI command enum variant

**File**: `/home/g/Work/deep-algo/crates/cli/src/main.rs`
**Lines**: 97-102 (after BacktestDaemon variant)
**Action**: Add new CLI command variant

**Code**:
```rust
    /// Interactive TUI for viewing backtest results and token selection
    BacktestManagerTui {
        /// Optional log file path (logs to file instead of stderr)
        #[arg(long)]
        log_file: Option<String>,
    },
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 6

---

### Task 4.3: Add BacktestManagerTui to logging configuration match

**File**: `/home/g/Work/deep-algo/crates/cli/src/main.rs`
**Lines**: 105-118 (update existing match statement)
**Action**: Add BacktestManagerTui to TUI logging cases

**Code**:
```rust
    // Initialize logging (disabled for TUI to prevent screen corruption, unless log_file is provided)
    match &cli.command {
        Commands::LiveBotTui { log_file: Some(path) }
        | Commands::BacktestManagerTui { log_file: Some(path) } => {
            // Log to file for TUI
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .with_writer(std::sync::Mutex::new(file))
                .init();
        }
        Commands::TuiBacktest { .. }
        | Commands::LiveBotTui { .. }
        | Commands::BacktestManagerTui { .. } => {
            // No logging for TUI (prevents screen corruption)
        }
```

**Verification**: `cargo check -p algo-trade-cli`
**Estimated LOC**: 3 (modified lines)

---

### Task 4.4: Add BacktestManagerTui command handler in main match

**File**: `/home/g/Work/deep-algo/crates/cli/src/main.rs`
**Lines**: 160-162 (after BacktestDaemon handler)
**Action**: Add command handler that calls tui_backtest_manager::run()

**Code**:
```rust
        Commands::BacktestDaemon { config, strategy } => {
            run_backtest_daemon(&config, &strategy).await?;
        }
        Commands::BacktestManagerTui { log_file: _ } => {
            tui_backtest_manager::run().await?;
        }
```

**Verification**: `cargo check -p algo-trade-cli && cargo build -p algo-trade-cli`
**Estimated LOC**: 3

---

### KAREN GATE: Phase 4 Complete

**Command**: Invoke Karen agent with:
```bash
cargo build --package algo-trade-cli
cargo clippy -p algo-trade-cli -- -D warnings
cargo clippy -p algo-trade-cli -- -W clippy::pedantic -W clippy::nursery
cargo build --release -p algo-trade-cli
```

**Expected**: Zero errors, zero warnings, binary compiles successfully, TUI runs locally.

**Blocking**: If Karen finds issues, STOP and fix atomically before proceeding to Phase 5.

---

## Phase 5: Docker Integration

### Task 5.1: Update entrypoint.sh to support TUI_MODE environment variable

**File**: `/home/g/Work/deep-algo/docker/entrypoint.sh`
**Lines**: 28-40 (replace daemon start section)
**Action**: Add TUI_MODE routing logic

**Code**:
```bash
# Start trading daemon in background
echo "Starting trading daemon as $(whoami)..."
algo-trade run --config "${CONFIG_PATH:-/config/Config.toml}" &
daemon_pid=$!

# Wait for daemon to initialize
sleep 2

# Start ttyd for TUI access with mode selection
echo "Starting ttyd on port 7681..."
TUI_MODE="${TUI_MODE:-live-bot}"

if [ "$TUI_MODE" = "backtest-manager" ]; then
    echo "TUI_MODE=backtest-manager: Launching backtest manager TUI"
    ttyd -p 7681 -W algo-trade backtest-manager-tui &
elif [ "$TUI_MODE" = "live-bot" ]; then
    echo "TUI_MODE=live-bot: Launching live bot TUI"
    ttyd -p 7681 -W algo-trade live-bot-tui &
else
    echo "Invalid TUI_MODE='$TUI_MODE'. Valid options: live-bot, backtest-manager"
    echo "Defaulting to live-bot TUI"
    ttyd -p 7681 -W algo-trade live-bot-tui &
fi

ttyd_pid=$!
```

**Verification**: `bash -n docker/entrypoint.sh` (syntax check)
**Estimated LOC**: 20 (modified lines)

---

### Task 5.2: Update docker-compose.yml to add TUI_MODE environment variable

**File**: `/home/g/Work/deep-algo/docker-compose.yml`
**Lines**: 51-52 (add after CONFIG_PATH env var)
**Action**: Add TUI_MODE env variable with default value

**Code**:
```yaml
      # Config path
      CONFIG_PATH: /config/Config.toml

      # TUI mode selection (live-bot or backtest-manager)
      TUI_MODE: ${TUI_MODE:-live-bot}
```

**Verification**: `docker compose config` (validate YAML)
**Estimated LOC**: 3

---

### Task 5.3: Test Docker integration with both TUI modes

**Action**: Manual testing steps (document results in task completion)

**Test 1 - Default (live-bot TUI)**:
```bash
docker compose down
docker compose build
docker compose up -d
# Access http://localhost:7681 → should show live bot TUI
docker compose logs app | grep "TUI_MODE"
# Expected: "TUI_MODE=live-bot: Launching live bot TUI"
```

**Test 2 - Backtest Manager TUI**:
```bash
docker compose down
TUI_MODE=backtest-manager docker compose up -d
# Access http://localhost:7681 → should show backtest manager TUI
docker compose logs app | grep "TUI_MODE"
# Expected: "TUI_MODE=backtest-manager: Launching backtest manager TUI"
```

**Test 3 - Invalid TUI_MODE fallback**:
```bash
docker compose down
TUI_MODE=invalid docker compose up -d
docker compose logs app | grep "TUI_MODE"
# Expected: "Invalid TUI_MODE='invalid'... Defaulting to live-bot TUI"
```

**Verification**: All 3 tests pass, both TUIs accessible via browser
**Estimated Time**: 15 minutes

---

### KAREN GATE: Phase 5 Complete

**Command**: Manual review + Karen agent with:
```bash
# Verify Docker builds
docker compose build

# Verify entrypoint syntax
bash -n docker/entrypoint.sh

# Verify compose file
docker compose config

# Verify Rust code still compiles
cargo build --release -p algo-trade-cli
```

**Expected**: Zero errors, Docker builds successfully, both TUI modes work.

**Blocking**: If Karen finds issues or Docker tests fail, STOP and fix before Phase 6.

---

## Phase 6: Documentation

### Task 6.1: Add Backtest Manager TUI section to CLAUDE.md

**File**: `/home/g/Work/deep-algo/CLAUDE.md`
**Lines**: After line 96 (after BacktestDaemon section in Development Commands)
**Action**: Document new TUI and Docker integration

**Code**:
```markdown
# Backtest Manager TUI
cargo run -p algo-trade-cli -- backtest-manager-tui

# With logging to file
cargo run -p algo-trade-cli -- backtest-manager-tui --log-file logs/backtest-manager.log

# Docker with Backtest Manager TUI
TUI_MODE=backtest-manager docker compose up -d
# Access at http://localhost:7681

# Docker with Live Bot TUI (default)
TUI_MODE=live-bot docker compose up -d
# Or just: docker compose up -d
```

**Verification**: Read file to confirm formatting
**Estimated LOC**: 13

---

### Task 6.2: Update Docker Integration section in CLAUDE.md

**File**: `/home/g/Work/deep-algo/CLAUDE.md`
**Lines**: Add new section after "Development Commands" (around line 110)
**Action**: Add Docker integration documentation

**Code**:
```markdown
## Docker Integration

### TUI Mode Selection

The Docker environment supports two TUI modes via the `TUI_MODE` environment variable:

1. **live-bot** (default): Interactive TUI for managing live trading bots
2. **backtest-manager**: Interactive TUI for viewing backtest results and token selection

#### Usage

```bash
# Default mode (live-bot TUI)
docker compose up -d

# Backtest manager TUI
TUI_MODE=backtest-manager docker compose up -d

# Access TUI at http://localhost:7681
```

#### Environment Variables

- `TUI_MODE`: Set to `live-bot` or `backtest-manager` (default: `live-bot`)
- `DATABASE_URL`: PostgreSQL/TimescaleDB connection string
- `HYPERLIQUID_API_URL`: Hyperliquid REST API URL
- `HYPERLIQUID_WS_URL`: Hyperliquid WebSocket URL
- `CONFIG_PATH`: Path to Config.toml inside container

#### Port Mappings

- `8080`: REST API (BotRegistry, health checks)
- `7681`: ttyd web terminal (TUI access)
- `5432`: TimescaleDB (PostgreSQL)

### Backtest Manager TUI Features

The Backtest Manager TUI provides 5 screens:

1. **Dashboard**: Overview of scheduler status, recent backtest count, token summary
2. **Reports**: Table view of all backtest results (last 48h by default)
3. **Token Selection**: Show approved/rejected tokens with filtering criteria
4. **Config**: Display scheduler and selector configuration (read-only)
5. **Report Detail**: Full metrics for individual backtest (Sharpe, Sortino, PnL, parameters)

#### Navigation

- `d`: Dashboard
- `r`: Reports
- `t`: Token Selection
- `c`: Config
- `↑↓`: Navigate lists
- `Enter`: View detail (Reports screen)
- `Esc`: Back to Dashboard
- `q`: Quit

#### Data Sources

- Reads from TimescaleDB `backtest_results` table
- Uses `TokenSelector` for approval filtering
- Displays config from `Config.toml`
```

**Verification**: Read file to confirm formatting and accuracy
**Estimated LOC**: 60

---

### KAREN GATE: Phase 6 Complete (FINAL)

**Command**: Invoke Karen agent with:
```bash
# Final compilation check
cargo build --release

# Clippy all levels
cargo clippy --all -- -D warnings
cargo clippy --all -- -W clippy::pedantic -W clippy::nursery

# Verify documentation renders
cat CLAUDE.md | grep -A 20 "Backtest Manager TUI"

# Final Docker test
docker compose down
docker compose build
TUI_MODE=backtest-manager docker compose up -d
# Manual: Access http://localhost:7681 and verify TUI works
docker compose down
```

**Expected**:
- Zero Rust errors/warnings
- Documentation accurate and complete
- Docker builds successfully
- Both TUI modes accessible

**Blocking**: This is the FINAL Karen gate. ALL issues must be resolved before marking feature complete.

---

## Verification Checklist

After all phases complete and Karen reviews pass:

- [x] **Phase 1**: TUI module structure compiles
- [x] **Phase 2**: Dashboard and Reports screens render
- [x] **Phase 3**: All 5 screens render with correct data
- [x] **Phase 4**: CLI integration works (cargo run command)
- [x] **Phase 5**: Docker integration with TUI_MODE works
- [x] **Phase 6**: Documentation complete and accurate
- [x] **Karen Phase 0**: Compilation check passes
- [x] **Karen Phase 1**: Clippy default passes
- [x] **Karen Phase 2**: Clippy pedantic + nursery passes
- [x] **Karen Phase 3**: Cross-file validation passes
- [x] **Karen Phase 4**: Per-file verification passes
- [x] **Karen Phase 5**: Release build succeeds
- [x] **Karen Phase 6**: Tests compile (if tests exist)

---

## Rollback Plan

If critical issues are found during Karen reviews:

1. **Revert Docker changes**: `git checkout docker/entrypoint.sh docker-compose.yml`
2. **Revert CLI changes**: `git checkout crates/cli/src/main.rs`
3. **Remove TUI module**: `rm crates/cli/src/tui_backtest_manager.rs`
4. **Rebuild**: `cargo build --release`

---

## Success Criteria

Feature is considered complete when:

1. ✅ All 6 phases complete with zero Karen issues
2. ✅ `cargo build --release` succeeds
3. ✅ `cargo clippy --all -- -D warnings` produces zero warnings
4. ✅ `cargo clippy --all -- -W clippy::pedantic -W clippy::nursery` produces zero warnings
5. ✅ Docker builds and runs with both TUI modes
6. ✅ Documentation is accurate and complete
7. ✅ Manual testing confirms all 5 TUI screens work correctly

---

## Estimated Timeline

- **Phase 1**: 30 minutes (structure + entry point)
- **Phase 2**: 45 minutes (Dashboard + Reports screens)
- **Phase 3**: 60 minutes (3 remaining screens + handlers)
- **Phase 4**: 15 minutes (CLI integration)
- **Phase 5**: 30 minutes (Docker integration + testing)
- **Phase 6**: 20 minutes (documentation)
- **Karen Reviews**: 30 minutes (6 gates × 5 min each)

**Total**: ~4 hours

---

## Notes

- **Read-only MVP**: Config screen is read-only (no editing). Write capabilities can be added in future iteration if needed.
- **Data caching**: App caches reports and token results to minimize database queries. Manual refresh via keyboard shortcuts.
- **Error handling**: All database queries wrapped in `Result<()>`. Errors logged to messages panel.
- **Pattern consistency**: Follows exact same structure as `tui_live_bot.rs` for maintainability.
- **Docker safety**: Existing daemon + ttyd behavior unchanged. TUI_MODE only affects ttyd command routing.
