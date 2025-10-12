// Allow dead code warnings in Phase 1 - will be used in Phase 4 when wired to CLI
#![allow(dead_code)]

use algo_trade_core::{AppConfig, ConfigLoader};
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
    widgets::{Block, Borders, Paragraph, Row, Table},
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
    config: AppConfig,
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
                if handle_key_event(key, app).await {
                    break; // Quit requested
                }
            }
        }
    }

    Ok(())
}

fn ui(f: &mut Frame, app: &ManagerApp) {
    match app.current_screen {
        ManagerScreen::Dashboard => render_dashboard(f, app),
        ManagerScreen::Reports => render_reports(f, app),
        ManagerScreen::TokenSelection => render_token_selection(f, app),
        ManagerScreen::Config => render_config(f, app),
        ManagerScreen::ReportDetail => render_report_detail(f, app),
    }
}

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
                    report.sortino_ratio.map_or_else(|| "N/A".to_string(), |v| format!("{v:.2}")),
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
                Span::raw(
                    report.parameters.as_ref().map_or_else(
                        || "  N/A".to_string(),
                        |params| format!("  {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "N/A".to_string()))
                    )
                ),
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

async fn handle_key_event(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match app.current_screen {
        ManagerScreen::Dashboard => handle_dashboard_keys(key, app),
        ManagerScreen::Reports => handle_reports_keys(key, app),
        ManagerScreen::TokenSelection => handle_token_selection_keys(key, app),
        ManagerScreen::Config => handle_config_keys(key, app),
        ManagerScreen::ReportDetail => handle_report_detail_keys(key, app),
    }
}

fn handle_dashboard_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match key.code {
        KeyCode::Char('q') => return true, // Quit
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
    false
}

fn handle_reports_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
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
    false
}

fn handle_token_selection_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
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
    false
}

fn handle_config_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Dashboard;
        }
        _ => {}
    }
    false
}

fn handle_report_detail_keys(key: crossterm::event::KeyEvent, app: &mut ManagerApp) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            app.current_screen = ManagerScreen::Reports;
            app.detail_report = None;
        }
        _ => {}
    }
    false
}
