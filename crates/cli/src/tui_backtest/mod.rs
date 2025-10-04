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
use rust_decimal::prelude::ToPrimitive;
use std::collections::HashSet;
use std::io;

/// Individual trade record for drill-down view
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp: DateTime<Utc>,
    pub action: String,  // "OPEN LONG" | "ADD LONG" | "CLOSE LONG" | "OPEN SHORT" | "ADD SHORT" | "CLOSE SHORT"
    pub price: rust_decimal::Decimal,
    pub quantity: rust_decimal::Decimal,
    pub commission: rust_decimal::Decimal,
    pub pnl: Option<rust_decimal::Decimal>,  // PnL for closing trades, None for opening
    pub position_value: rust_decimal::Decimal,  // quantity × price in USDC
}

/// Strategy configuration
#[derive(Debug, Clone)]
pub enum StrategyType {
    MaCrossover { fast: usize, slow: usize },
    QuadMa {
        ma1: usize,
        ma2: usize,
        ma3: usize,
        ma4: usize,
        trend_period: usize,
        volume_factor: usize,  // hundredths (150 = 1.5x)
        take_profit: usize,    // basis points (200 = 2.0%)
        stop_loss: usize,      // basis points (100 = 1.0%)
        reversal_confirmation_bars: usize,  // bars to confirm reversal (2 = default)
    },
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
    MetricsDetail,
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
            name: "Default (5/10/20/50)".to_string(),
            strategy: StrategyType::QuadMa {
                ma1: 5,
                ma2: 10,
                ma3: 20,
                ma4: 50,
                trend_period: 100,
                volume_factor: 150,  // 1.5x
                take_profit: 200,    // 2.0%
                stop_loss: 100,      // 1.0%
                reversal_confirmation_bars: 2,  // 2 bars confirmation
            },
        }
    }
}

/// Which field is being edited in `TimeframeConfig` screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeField {
    StartDate,
    EndDate,
    Interval,
}

/// Which date component is being edited
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateField {
    Year,
    Month,
    Day,
    Hour,
    Minute,
}

/// Which parameter is being edited in `ParameterConfig` screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum ParamField {
    FastPeriod,
    SlowPeriod,
    Ma1Period,
    Ma2Period,
    Ma3Period,
    Ma4Period,
    TrendPeriod,
    VolumeFactor,
    TakeProfit,
    StopLoss,
    ReversalConfirmBars,
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
    pub metrics: Option<algo_trade_core::engine::PerformanceMetrics>,
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
    pub editing_param: Option<ParamField>,
    pub param_input_buffer: String,

    // Timeframe configuration
    pub editing_field: Option<TimeframeField>,
    pub editing_date_field: Option<DateField>,
    pub start_year: String,
    pub start_month: String,
    pub start_day: String,
    pub start_hour: String,
    pub start_minute: String,
    pub end_year: String,
    pub end_month: String,
    pub end_day: String,
    pub end_hour: String,
    pub end_minute: String,
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

/// Calculate profit factor from trade records
/// Profit Factor = Total Gross Profit / Total Gross Loss
fn calculate_profit_factor(trades: &[TradeRecord]) -> f64 {
    use rust_decimal::Decimal;

    let mut gross_profit = Decimal::ZERO;
    let mut gross_loss = Decimal::ZERO;

    for trade in trades {
        if let Some(pnl) = trade.pnl {
            if pnl > Decimal::ZERO {
                gross_profit += pnl;
            } else if pnl < Decimal::ZERO {
                gross_loss += pnl.abs();
            }
        }
    }

    if gross_loss > Decimal::ZERO {
        (gross_profit / gross_loss).to_f64().unwrap_or(0.0)
    } else if gross_profit > Decimal::ZERO {
        999.0  // All wins, no losses - display as "999+"
    } else {
        0.0  // No closed trades
    }
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
            editing_param: None,
            param_input_buffer: String::new(),

            editing_field: None,
            editing_date_field: None,
            start_year: start.format("%Y").to_string(),
            start_month: start.format("%m").to_string(),
            start_day: start.format("%d").to_string(),
            start_hour: start.format("%H").to_string(),
            start_minute: start.format("%M").to_string(),
            end_year: end.format("%Y").to_string(),
            end_month: end.format("%m").to_string(),
            end_day: end.format("%d").to_string(),
            end_hour: end.format("%H").to_string(),
            end_minute: end.format("%M").to_string(),
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
            AppScreen::MetricsDetail => self.handle_metrics_detail_key(key),
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

    /// Get mutable reference to date field based on editing state
    #[allow(clippy::missing_const_for_fn)] // Can't be const - returns mutable reference
    fn get_date_field_mut(&mut self) -> Option<&mut String> {
        match (self.editing_field, self.editing_date_field) {
            (Some(TimeframeField::StartDate), Some(DateField::Year)) => Some(&mut self.start_year),
            (Some(TimeframeField::StartDate), Some(DateField::Month)) => Some(&mut self.start_month),
            (Some(TimeframeField::StartDate), Some(DateField::Day)) => Some(&mut self.start_day),
            (Some(TimeframeField::StartDate), Some(DateField::Hour)) => Some(&mut self.start_hour),
            (Some(TimeframeField::StartDate), Some(DateField::Minute)) => Some(&mut self.start_minute),
            (Some(TimeframeField::EndDate), Some(DateField::Year)) => Some(&mut self.end_year),
            (Some(TimeframeField::EndDate), Some(DateField::Month)) => Some(&mut self.end_month),
            (Some(TimeframeField::EndDate), Some(DateField::Day)) => Some(&mut self.end_day),
            (Some(TimeframeField::EndDate), Some(DateField::Hour)) => Some(&mut self.end_hour),
            (Some(TimeframeField::EndDate), Some(DateField::Minute)) => Some(&mut self.end_minute),
            _ => None,
        }
    }

    /// Handle Tab key for cycling fields
    #[allow(clippy::missing_const_for_fn)] // Can't be const - mutates self
    fn handle_tab_key(&mut self) {
        if let Some(TimeframeField::StartDate | TimeframeField::EndDate) = self.editing_field {
            // Cycle through date components
            self.editing_date_field = Some(match self.editing_date_field {
                None | Some(DateField::Minute) => DateField::Year,
                Some(DateField::Year) => DateField::Month,
                Some(DateField::Month) => DateField::Day,
                Some(DateField::Day) => DateField::Hour,
                Some(DateField::Hour) => DateField::Minute,
            });
        } else {
            // Cycle through main fields
            let new_field = Some(match self.editing_field {
                None | Some(TimeframeField::Interval) => TimeframeField::StartDate,
                Some(TimeframeField::StartDate) => TimeframeField::EndDate,
                Some(TimeframeField::EndDate) => TimeframeField::Interval,
            });

            // When entering a date field, auto-initialize to Year component
            if matches!(new_field, Some(TimeframeField::StartDate | TimeframeField::EndDate)) {
                self.editing_date_field = Some(DateField::Year);
            } else {
                self.editing_date_field = None;
            }

            self.editing_field = new_field;
        }
    }

    /// Handle up/down navigation for interval or date increment
    fn handle_timeframe_navigation(&mut self, up: bool) {
        match self.editing_field {
            Some(TimeframeField::Interval) => {
                if up && self.selected_interval_index > 0 {
                    self.selected_interval_index -= 1;
                    self.interval = self.interval_options[self.selected_interval_index].to_string();
                } else if !up && self.selected_interval_index < self.interval_options.len() - 1 {
                    self.selected_interval_index += 1;
                    self.interval = self.interval_options[self.selected_interval_index].to_string();
                }
            }
            Some(TimeframeField::StartDate | TimeframeField::EndDate) => {
                self.increment_date_field(up);
            }
            _ => {}
        }
    }

    /// Try to parse and commit datetime from components
    fn try_commit_datetime(&mut self) {
        use chrono::DateTime;

        let start_str = format!(
            "{}-{}-{}T{}:{}:00Z",
            self.start_year, self.start_month, self.start_day,
            self.start_hour, self.start_minute
        );
        let end_str = format!(
            "{}-{}-{}T{}:{}:00Z",
            self.end_year, self.end_month, self.end_day,
            self.end_hour, self.end_minute
        );

        if let (Ok(start), Ok(end)) = (
            start_str.parse::<DateTime<chrono::Utc>>(),
            end_str.parse::<DateTime<chrono::Utc>>(),
        ) {
            if start < end {
                self.start_date = start;
                self.end_date = end;
                self.current_screen = AppScreen::ParameterConfig;
            }
        }
    }

    fn handle_timeframe_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Tab => self.handle_tab_key(),
            KeyCode::Up | KeyCode::Char('k') => self.handle_timeframe_navigation(true),
            KeyCode::Down | KeyCode::Char('j') => self.handle_timeframe_navigation(false),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let max_len = match self.editing_date_field {
                    Some(DateField::Year) => 4,
                    _ => 2,
                };
                if let Some(field) = self.get_date_field_mut() {
                    if field.len() < max_len {
                        field.push(c);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(field) = self.get_date_field_mut() {
                    field.pop();
                }
            }
            KeyCode::Enter => self.try_commit_datetime(),
            KeyCode::Esc => {
                if self.editing_date_field.is_some() {
                    self.editing_date_field = None;
                } else if self.editing_field.is_some() {
                    self.editing_field = None;
                } else {
                    self.current_screen = AppScreen::TokenSelection;
                }
            }
            _ => {}
        }
    }

    fn increment_date_field(&mut self, increment: bool) {
        let (field_ref, max_val) = match (self.editing_field, self.editing_date_field) {
            (Some(TimeframeField::StartDate), Some(DateField::Year)) => (Some(&mut self.start_year), 9999),
            (Some(TimeframeField::StartDate), Some(DateField::Month)) => (Some(&mut self.start_month), 12),
            (Some(TimeframeField::StartDate), Some(DateField::Day)) => (Some(&mut self.start_day), 31),
            (Some(TimeframeField::StartDate), Some(DateField::Hour)) => (Some(&mut self.start_hour), 23),
            (Some(TimeframeField::StartDate), Some(DateField::Minute)) => (Some(&mut self.start_minute), 59),
            (Some(TimeframeField::EndDate), Some(DateField::Year)) => (Some(&mut self.end_year), 9999),
            (Some(TimeframeField::EndDate), Some(DateField::Month)) => (Some(&mut self.end_month), 12),
            (Some(TimeframeField::EndDate), Some(DateField::Day)) => (Some(&mut self.end_day), 31),
            (Some(TimeframeField::EndDate), Some(DateField::Hour)) => (Some(&mut self.end_hour), 23),
            (Some(TimeframeField::EndDate), Some(DateField::Minute)) => (Some(&mut self.end_minute), 59),
            _ => (None, 0),
        };

        if let Some(field) = field_ref {
            if let Ok(mut val) = field.parse::<usize>() {
                if increment {
                    val = (val + 1).min(max_val);
                } else {
                    let min_val = usize::from(matches!(self.editing_date_field, Some(DateField::Month | DateField::Day)));
                    val = val.saturating_sub(1).max(min_val);
                }
                *field = format!("{:0width$}", val, width = field.len().max(if matches!(self.editing_date_field, Some(DateField::Year)) { 4 } else { 2 }));
            }
        }
    }

    fn handle_param_key(&mut self, key: KeyCode) {
        // If in edit mode, handle editing keys
        if self.editing_param.is_some() {
            self.handle_param_edit_key(key);
            return;
        }

        // Normal navigation mode
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
            KeyCode::Char('e') => {
                // Enter edit mode for selected config
                if let Some(config) = self.param_configs.get(self.selected_param_index) {
                    self.editing_param = Some(match &config.strategy {
                        StrategyType::MaCrossover { .. } => ParamField::FastPeriod,
                        StrategyType::QuadMa { .. } => ParamField::Ma1Period,
                    });
                    self.param_input_buffer.clear();
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

    fn handle_param_edit_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Tab => {
                // Save current field before cycling to next
                if !self.param_input_buffer.is_empty() {
                    if let Ok(value) = self.param_input_buffer.parse::<usize>() {
                        if value > 0 {
                            if let Some(config) = self.param_configs.get_mut(self.selected_param_index) {
                                match (&mut config.strategy, self.editing_param.unwrap()) {
                                    (StrategyType::MaCrossover { fast, .. }, ParamField::FastPeriod) => *fast = value,
                                    (StrategyType::MaCrossover { slow, .. }, ParamField::SlowPeriod) => *slow = value,
                                    (StrategyType::QuadMa { ma1, .. }, ParamField::Ma1Period) => *ma1 = value,
                                    (StrategyType::QuadMa { ma2, .. }, ParamField::Ma2Period) => *ma2 = value,
                                    (StrategyType::QuadMa { ma3, .. }, ParamField::Ma3Period) => *ma3 = value,
                                    (StrategyType::QuadMa { ma4, .. }, ParamField::Ma4Period) => *ma4 = value,
                                    (StrategyType::QuadMa { trend_period, .. }, ParamField::TrendPeriod) => *trend_period = value,
                                    (StrategyType::QuadMa { volume_factor, .. }, ParamField::VolumeFactor) => *volume_factor = value,
                                    (StrategyType::QuadMa { take_profit, .. }, ParamField::TakeProfit) => *take_profit = value,
                                    (StrategyType::QuadMa { stop_loss, .. }, ParamField::StopLoss) => *stop_loss = value,
                                    (StrategyType::QuadMa { reversal_confirmation_bars, .. }, ParamField::ReversalConfirmBars) => *reversal_confirmation_bars = value,
                                    _ => {}
                                }
                            }
                        }
                    }
                }

                // Cycle to next parameter field
                if let Some(config) = self.param_configs.get(self.selected_param_index) {
                    self.editing_param = Some(match (&config.strategy, self.editing_param.unwrap()) {
                        (StrategyType::MaCrossover { .. }, ParamField::FastPeriod) => ParamField::SlowPeriod,
                        (StrategyType::MaCrossover { .. }, ParamField::SlowPeriod) => ParamField::FastPeriod,
                        (StrategyType::QuadMa { .. }, ParamField::Ma1Period) => ParamField::Ma2Period,
                        (StrategyType::QuadMa { .. }, ParamField::Ma2Period) => ParamField::Ma3Period,
                        (StrategyType::QuadMa { .. }, ParamField::Ma3Period) => ParamField::Ma4Period,
                        (StrategyType::QuadMa { .. }, ParamField::Ma4Period) => ParamField::TrendPeriod,
                        (StrategyType::QuadMa { .. }, ParamField::TrendPeriod) => ParamField::VolumeFactor,
                        (StrategyType::QuadMa { .. }, ParamField::VolumeFactor) => ParamField::TakeProfit,
                        (StrategyType::QuadMa { .. }, ParamField::TakeProfit) => ParamField::StopLoss,
                        (StrategyType::QuadMa { .. }, ParamField::StopLoss) => ParamField::ReversalConfirmBars,
                        (StrategyType::QuadMa { .. }, ParamField::ReversalConfirmBars) => ParamField::Ma1Period,
                        _ => return, // Invalid combination
                    });
                    self.param_input_buffer.clear();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                // Append digit to buffer
                self.param_input_buffer.push(c);
            }
            KeyCode::Backspace => {
                // Remove last character
                self.param_input_buffer.pop();
            }
            KeyCode::Enter => {
                // Save the edit
                if let Ok(value) = self.param_input_buffer.parse::<usize>() {
                    if value > 0 {
                        if let Some(config) = self.param_configs.get_mut(self.selected_param_index) {
                            match (&mut config.strategy, self.editing_param.unwrap()) {
                                (StrategyType::MaCrossover { fast, .. }, ParamField::FastPeriod) => *fast = value,
                                (StrategyType::MaCrossover { slow, .. }, ParamField::SlowPeriod) => *slow = value,
                                (StrategyType::QuadMa { ma1, .. }, ParamField::Ma1Period) => *ma1 = value,
                                (StrategyType::QuadMa { ma2, .. }, ParamField::Ma2Period) => *ma2 = value,
                                (StrategyType::QuadMa { ma3, .. }, ParamField::Ma3Period) => *ma3 = value,
                                (StrategyType::QuadMa { ma4, .. }, ParamField::Ma4Period) => *ma4 = value,
                                (StrategyType::QuadMa { trend_period, .. }, ParamField::TrendPeriod) => *trend_period = value,
                                (StrategyType::QuadMa { volume_factor, .. }, ParamField::VolumeFactor) => *volume_factor = value,
                                (StrategyType::QuadMa { take_profit, .. }, ParamField::TakeProfit) => *take_profit = value,
                                (StrategyType::QuadMa { stop_loss, .. }, ParamField::StopLoss) => *stop_loss = value,
                                (StrategyType::QuadMa { reversal_confirmation_bars, .. }, ParamField::ReversalConfirmBars) => *reversal_confirmation_bars = value,
                                _ => {} // Invalid combination
                            }
                        }
                    }
                }
                self.editing_param = None;
                self.param_input_buffer.clear();
            }
            KeyCode::Esc => {
                // Cancel edit
                self.editing_param = None;
                self.param_input_buffer.clear();
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
                self.sort_column = (self.sort_column + 1) % 12;
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
            KeyCode::Char('m') => {
                // View metrics detail
                if !self.results.is_empty() {
                    self.selected_result_index = Some(self.results_scroll_offset);
                    self.current_screen = AppScreen::MetricsDetail;
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
                3 => {
                    // Ret$ = final_capital - initial_capital
                    let ret_a = a.metrics.as_ref().map(|m| m.final_capital - m.initial_capital);
                    let ret_b = b.metrics.as_ref().map(|m| m.final_capital - m.initial_capital);
                    ret_a.cmp(&ret_b)
                }
                4 => a.max_drawdown.cmp(&b.max_drawdown),
                5 => a.sharpe_ratio.partial_cmp(&b.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal),
                6 => a.num_trades.cmp(&b.num_trades),
                7 => a.win_rate.partial_cmp(&b.win_rate).unwrap_or(std::cmp::Ordering::Equal),
                8 => {
                    // Fin$ = final_capital
                    let fin_a = a.metrics.as_ref().map(|m| m.final_capital);
                    let fin_b = b.metrics.as_ref().map(|m| m.final_capital);
                    fin_a.cmp(&fin_b)
                }
                9 => {
                    // Peak$ = equity_peak
                    let peak_a = a.metrics.as_ref().map(|m| m.equity_peak);
                    let peak_b = b.metrics.as_ref().map(|m| m.equity_peak);
                    peak_a.cmp(&peak_b)
                }
                10 => {
                    // B&H% = buy_hold_return
                    let bh_a = a.metrics.as_ref().map(|m| m.buy_hold_return);
                    let bh_b = b.metrics.as_ref().map(|m| m.buy_hold_return);
                    bh_a.cmp(&bh_b)
                }
                11 => {
                    // PF = profit_factor (calculated from trades)
                    let pf_a = calculate_profit_factor(&a.trades);
                    let pf_b = calculate_profit_factor(&b.trades);
                    pf_a.partial_cmp(&pf_b).unwrap_or(std::cmp::Ordering::Equal)
                }
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

    fn handle_metrics_detail_key(&mut self, key: KeyCode) {
        if key == KeyCode::Esc {
            self.current_screen = AppScreen::Results;
            self.selected_result_index = None;
        }
    }
}

/// Find index of interval in options list, defaults to 0 (1m) if not found
fn interval_options_index(interval: &str) -> usize {
    let options = ["1m", "5m", "15m", "30m", "1h", "4h", "1d"];
    options.iter().position(|&i| i == interval).unwrap_or(0)
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
