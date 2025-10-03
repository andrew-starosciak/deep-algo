mod runner;
mod screens;

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::collections::HashSet;
use std::io;

/// Individual trade record for drill-down view
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp: DateTime<Utc>,
    pub action: String,  // "LONG" | "SHORT" | "CLOSE LONG" | "CLOSE SHORT"
    pub price: rust_decimal::Decimal,
    pub quantity: rust_decimal::Decimal,
    pub commission: rust_decimal::Decimal,
}

/// Strategy configuration
#[derive(Debug, Clone)]
pub enum StrategyType {
    MaCrossover { fast: usize, slow: usize },
    QuadMa { ma1: usize, ma2: usize, ma3: usize, ma4: usize },
}

impl StrategyType {
    #[allow(dead_code)]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::MaCrossover { .. } => "MA Crossover",
            Self::QuadMa { .. } => "Quad MA",
        }
    }
}

/// Application state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppScreen {
    StrategySelection,
    TokenSelection,
    TimeframeConfig,
    ParameterConfig,
    Running,
    Results,
    TradeDetail,
}

/// Parameter configuration set
#[derive(Debug, Clone)]
pub struct ParamConfig {
    pub name: String,
    pub strategy: StrategyType,
}

impl ParamConfig {
    pub fn default_ma_crossover() -> Self {
        Self {
            name: "Default (10/30)".to_string(),
            strategy: StrategyType::MaCrossover { fast: 10, slow: 30 },
        }
    }

    pub fn default_quad_ma() -> Self {
        Self {
            name: "Fibonacci (5/8/13/21)".to_string(),
            strategy: StrategyType::QuadMa { ma1: 5, ma2: 8, ma3: 13, ma4: 21 },
        }
    }
}

/// Which field is being edited in TimeframeConfig screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeField {
    StartDate,
    EndDate,
    Interval,
}

/// Backtest result for a single token/config combination
#[derive(Debug, Clone)]
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

/// Main application state
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub current_screen: AppScreen,
    pub should_quit: bool,

    // Strategy selection
    pub selected_strategy_index: usize,
    pub available_strategies: Vec<&'static str>,

    // Token selection
    pub available_tokens: Vec<String>,
    pub selected_tokens: HashSet<String>,
    pub token_scroll_offset: usize,
    pub loading_tokens: bool,

    // Parameter configuration
    pub param_configs: Vec<ParamConfig>,
    pub selected_param_index: usize,

    // Timeframe configuration
    pub editing_field: Option<TimeframeField>,
    pub start_date_input: String,
    pub end_date_input: String,
    pub interval_options: Vec<&'static str>,
    pub selected_interval_index: usize,

    // Running state
    pub total_backtests: usize,
    pub completed_backtests: usize,
    pub current_backtest: Option<(String, String)>, // (token, config)
    pub status_messages: Vec<String>,

    // Results
    pub results: Vec<BacktestResult>,
    pub results_scroll_offset: usize,
    pub sort_column: usize, // 0=token, 1=config, 2=return, 3=sharpe, etc.
    pub sort_ascending: bool,

    // Trade detail
    pub selected_result_index: Option<usize>,
    pub trade_scroll_offset: usize,

    // Configuration
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub interval: String,
}

impl App {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>, interval: String) -> Self {
        Self {
            current_screen: AppScreen::StrategySelection,
            should_quit: false,

            selected_strategy_index: 0,
            available_strategies: vec!["MA Crossover", "Quad MA"],

            available_tokens: Vec::new(),
            selected_tokens: HashSet::new(),
            token_scroll_offset: 0,
            loading_tokens: true,

            param_configs: Vec::new(),
            selected_param_index: 0,

            editing_field: None,
            start_date_input: start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            end_date_input: end.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            interval_options: vec!["1m", "5m", "15m", "30m", "1h", "4h", "1d"],
            selected_interval_index: interval_options_index(&interval),

            total_backtests: 0,
            completed_backtests: 0,
            current_backtest: None,
            status_messages: Vec::new(),

            results: Vec::new(),
            results_scroll_offset: 0,
            sort_column: 2, // Default sort by return
            sort_ascending: false,

            selected_result_index: None,
            trade_scroll_offset: 0,

            start_date: start,
            end_date: end,
            interval,
        }
    }

    /// Handle keyboard input based on current screen
    pub fn handle_key(&mut self, key: KeyCode) {
        match self.current_screen {
            AppScreen::StrategySelection => self.handle_strategy_key(key),
            AppScreen::TokenSelection => self.handle_token_key(key),
            AppScreen::TimeframeConfig => self.handle_timeframe_key(key),
            AppScreen::ParameterConfig => self.handle_param_key(key),
            AppScreen::Running => self.handle_running_key(key),
            AppScreen::Results => self.handle_results_key(key),
            AppScreen::TradeDetail => self.handle_trade_detail_key(key),
        }
    }

    /// Add a status message (keeps last 10 messages)
    pub fn add_status(&mut self, message: String) {
        self.status_messages.push(message);
        if self.status_messages.len() > 10 {
            self.status_messages.remove(0);
        }
    }

    fn handle_strategy_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_strategy_index > 0 {
                    self.selected_strategy_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_strategy_index < self.available_strategies.len() - 1 {
                    self.selected_strategy_index += 1;
                }
            }
            KeyCode::Enter => {
                // Initialize default param config for selected strategy
                let default_config = if self.selected_strategy_index == 0 {
                    ParamConfig::default_ma_crossover()
                } else {
                    ParamConfig::default_quad_ma()
                };
                self.param_configs = vec![default_config];
                self.current_screen = AppScreen::TokenSelection;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn handle_token_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.token_scroll_offset > 0 {
                    self.token_scroll_offset -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.token_scroll_offset < self.available_tokens.len().saturating_sub(1) {
                    self.token_scroll_offset += 1;
                }
            }
            KeyCode::Char(' ') => {
                // Toggle selection of current token
                if let Some(token) = self.available_tokens.get(self.token_scroll_offset) {
                    if self.selected_tokens.contains(token) {
                        self.selected_tokens.remove(token);
                    } else {
                        self.selected_tokens.insert(token.clone());
                    }
                }
            }
            KeyCode::Char('a') => {
                // Select all
                self.selected_tokens = self.available_tokens.iter().cloned().collect();
            }
            KeyCode::Char('n') => {
                // Deselect all
                self.selected_tokens.clear();
            }
            KeyCode::Enter => {
                if !self.selected_tokens.is_empty() {
                    self.current_screen = AppScreen::TimeframeConfig;
                }
            }
            KeyCode::Esc => {
                self.current_screen = AppScreen::StrategySelection;
            }
            _ => {}
        }
    }

    fn handle_timeframe_key(&mut self, key: KeyCode) {
        use chrono::DateTime;

        match key {
            KeyCode::Tab => {
                // Cycle through fields
                self.editing_field = Some(match self.editing_field {
                    None | Some(TimeframeField::Interval) => TimeframeField::StartDate,
                    Some(TimeframeField::StartDate) => TimeframeField::EndDate,
                    Some(TimeframeField::EndDate) => TimeframeField::Interval,
                });
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(TimeframeField::Interval) = self.editing_field {
                    if self.selected_interval_index > 0 {
                        self.selected_interval_index -= 1;
                        self.interval = self.interval_options[self.selected_interval_index].to_string();
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(TimeframeField::Interval) = self.editing_field {
                    if self.selected_interval_index < self.interval_options.len() - 1 {
                        self.selected_interval_index += 1;
                        self.interval = self.interval_options[self.selected_interval_index].to_string();
                    }
                }
            }
            KeyCode::Char(c) => {
                // Text input for date fields
                match self.editing_field {
                    Some(TimeframeField::StartDate) => {
                        self.start_date_input.push(c);
                    }
                    Some(TimeframeField::EndDate) => {
                        self.end_date_input.push(c);
                    }
                    _ => {}
                }
            }
            KeyCode::Backspace => {
                // Delete character from date fields
                match self.editing_field {
                    Some(TimeframeField::StartDate) => {
                        self.start_date_input.pop();
                    }
                    Some(TimeframeField::EndDate) => {
                        self.end_date_input.pop();
                    }
                    _ => {}
                }
            }
            KeyCode::Enter => {
                // Parse dates and proceed to parameter config
                if let (Ok(start), Ok(end)) = (
                    self.start_date_input.parse::<DateTime<chrono::Utc>>(),
                    self.end_date_input.parse::<DateTime<chrono::Utc>>(),
                ) {
                    if start < end {
                        self.start_date = start;
                        self.end_date = end;
                        self.current_screen = AppScreen::ParameterConfig;
                    }
                }
            }
            KeyCode::Esc => {
                if self.editing_field.is_some() {
                    self.editing_field = None;
                } else {
                    self.current_screen = AppScreen::TokenSelection;
                }
            }
            _ => {}
        }
    }

    fn handle_param_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_param_index > 0 {
                    self.selected_param_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_param_index < self.param_configs.len().saturating_sub(1) {
                    self.selected_param_index += 1;
                }
            }
            KeyCode::Char('a') => {
                // Add new config (copy of selected or default)
                let new_config = if let Some(config) = self.param_configs.get(self.selected_param_index) {
                    let mut new = config.clone();
                    new.name = format!("Config {}", self.param_configs.len() + 1);
                    new
                } else if self.selected_strategy_index == 0 {
                    ParamConfig::default_ma_crossover()
                } else {
                    ParamConfig::default_quad_ma()
                };
                self.param_configs.push(new_config);
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                // Delete selected config (keep at least one)
                if self.param_configs.len() > 1 {
                    self.param_configs.remove(self.selected_param_index);
                    if self.selected_param_index >= self.param_configs.len() {
                        self.selected_param_index = self.param_configs.len().saturating_sub(1);
                    }
                }
            }
            KeyCode::Enter => {
                // Proceed to running
                self.total_backtests = self.selected_tokens.len() * self.param_configs.len();
                self.completed_backtests = 0;
                self.current_screen = AppScreen::Running;
            }
            KeyCode::Esc => {
                self.current_screen = AppScreen::TimeframeConfig;
            }
            _ => {}
        }
    }

    const fn handle_running_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                // Cancel backtest (in real implementation, stop runner)
                self.current_screen = AppScreen::Results;
            }
            _ => {}
        }
    }

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
            KeyCode::Char('s') => {
                // Toggle sort column (cycle through)
                self.sort_column = (self.sort_column + 1) % 6;
                self.sort_results();
            }
            KeyCode::Char('r') => {
                // Reverse sort order
                self.sort_ascending = !self.sort_ascending;
                self.sort_results();
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('b') => {
                // Back to start
                self.current_screen = AppScreen::StrategySelection;
                self.results.clear();
                self.selected_tokens.clear();
            }
            KeyCode::Enter => {
                // Drill down to trade details
                if !self.results.is_empty() {
                    self.selected_result_index = Some(self.results_scroll_offset);
                    self.trade_scroll_offset = 0;
                    self.current_screen = AppScreen::TradeDetail;
                }
            }
            _ => {}
        }
    }

    fn sort_results(&mut self) {
        self.results.sort_by(|a, b| {
            let cmp = match self.sort_column {
                0 => a.token.cmp(&b.token),
                1 => a.config_name.cmp(&b.config_name),
                2 => a.total_return.cmp(&b.total_return),
                3 => a.sharpe_ratio.partial_cmp(&b.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal),
                4 => a.max_drawdown.cmp(&b.max_drawdown),
                5 => a.num_trades.cmp(&b.num_trades),
                _ => std::cmp::Ordering::Equal,
            };

            if self.sort_ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });
    }

    fn handle_trade_detail_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.trade_scroll_offset > 0 {
                    self.trade_scroll_offset -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(result_idx) = self.selected_result_index {
                    if let Some(result) = self.results.get(result_idx) {
                        if self.trade_scroll_offset < result.trades.len().saturating_sub(1) {
                            self.trade_scroll_offset += 1;
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.current_screen = AppScreen::Results;
                self.selected_result_index = None;
            }
            _ => {}
        }
    }
}

/// Find index of interval in options list, defaults to 4 (1h) if not found
fn interval_options_index(interval: &str) -> usize {
    let options = ["1m", "5m", "15m", "30m", "1h", "4h", "1d"];
    options.iter().position(|&i| i == interval).unwrap_or(4)
}

/// Main entry point for TUI application
pub async fn run(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: String,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new(start, end, interval);

    // Fetch available tokens
    match fetch_tokens().await {
        Ok(tokens) => {
            app.available_tokens = tokens;
            app.loading_tokens = false;
        }
        Err(e) => {
            // Cleanup and return error
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;
            return Err(e);
        }
    }

    // Run app loop
    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| screens::render(f, app))?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }

        // Check for screen-specific async tasks
        if app.current_screen == AppScreen::Running && !app.results.is_empty() {
            // If we have results, transition to results screen
            if app.completed_backtests >= app.total_backtests {
                app.current_screen = AppScreen::Results;
                app.sort_results();
            }
        }

        // Run backtests when in Running screen
        if app.current_screen == AppScreen::Running && app.results.is_empty() {
            // Clone values needed for async function to avoid borrow conflicts
            let tokens: Vec<String> = app.selected_tokens.iter().cloned().collect();
            let configs = app.param_configs.clone();
            let start_date = app.start_date;
            let end_date = app.end_date;
            let interval = app.interval.clone();

            // Execute all backtests
            let results = runner::run_all_backtests(
                &tokens,
                &configs,
                start_date,
                end_date,
                &interval,
                |completed, _total, token, config, status_msg| {
                    app.completed_backtests = completed;
                    app.current_backtest = Some((token.to_string(), config.to_string()));

                    // Add status message if provided
                    if let Some(msg) = status_msg {
                        app.add_status(msg);
                    }

                    // Force redraw
                    let _ = terminal.draw(|f| screens::render(f, app));
                }
            ).await?;

            app.results = results;
            app.current_screen = AppScreen::Results;
            app.sort_results();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

async fn fetch_tokens() -> Result<Vec<String>> {
    use algo_trade_hyperliquid::HyperliquidClient;

    let api_url = std::env::var("HYPERLIQUID_API_URL")
        .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());

    let client = HyperliquidClient::new(api_url);
    client.fetch_available_symbols().await
}
