use algo_trade_bot_orchestrator::{BotConfig, BotEvent, BotRegistry, BotState, EnhancedBotStatus, ExecutionMode};
use algo_trade_hyperliquid::HyperliquidClient;
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
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

/// Strategy configuration (copied from backtest TUI)
#[derive(Debug, Clone)]
enum StrategyType {
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
        reversal_confirmation_bars: usize,
    },
}

impl StrategyType {
    const fn name(&self) -> &'static str {
        match self {
            Self::MaCrossover { .. } => "MA Crossover",
            Self::QuadMa { .. } => "Quad MA",
        }
    }
}

/// Parameter configuration
#[derive(Debug, Clone)]
struct ParamConfig {
    #[allow(dead_code)]
    name: String,
    strategy: StrategyType,
}

impl ParamConfig {
    fn default_ma_crossover() -> Self {
        Self {
            name: "Default (10/30)".to_string(),
            strategy: StrategyType::MaCrossover { fast: 10, slow: 30 },
        }
    }

    fn default_quad_ma() -> Self {
        Self {
            name: "Default (5/10/20/50)".to_string(),
            strategy: StrategyType::QuadMa {
                ma1: 5,
                ma2: 10,
                ma3: 20,
                ma4: 50,
                trend_period: 100,
                volume_factor: 150,
                take_profit: 200,
                stop_loss: 100,
                reversal_confirmation_bars: 2,
            },
        }
    }
}

/// Which parameter is being edited
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamField {
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

/// Application screens
#[derive(Debug, Clone, PartialEq, Eq)]
enum BotScreen {
    BotList,
    BotMonitor,
    StrategySelection,
    ParameterConfig,
    TokenSelection,
}

/// Application state
struct App {
    registry: Arc<BotRegistry>,
    current_screen: BotScreen,

    // Bot list
    cached_bots: Vec<String>,
    cached_bot_statuses: HashMap<String, EnhancedBotStatus>,
    selected_bot: usize,
    messages: Vec<String>,

    // Bot monitor
    monitored_bot_id: Option<String>,
    bot_events: Vec<BotEvent>,
    bot_status: Option<EnhancedBotStatus>,
    event_rx: Option<tokio::sync::broadcast::Receiver<BotEvent>>,

    // Strategy selection
    selected_strategy_index: usize,
    available_strategies: Vec<&'static str>,

    // Parameter config
    param_config: ParamConfig,
    editing_param: Option<ParamField>,
    param_input_buffer: String,

    // Token selection
    available_tokens: Vec<String>,
    selected_token_index: usize,
    loading_tokens: bool,
}

impl App {
    fn new(registry: Arc<BotRegistry>) -> Self {
        Self {
            registry,
            current_screen: BotScreen::BotList,
            cached_bots: Vec::new(),
            cached_bot_statuses: HashMap::new(),
            selected_bot: 0,
            messages: vec!["Live Bot Manager - Press 'a' to add bot, 'v' to view bot, 'q' to quit".to_string()],
            monitored_bot_id: None,
            bot_events: Vec::new(),
            bot_status: None,
            event_rx: None,
            selected_strategy_index: 0,
            available_strategies: vec!["Quad MA", "MA Crossover"],
            param_config: ParamConfig::default_quad_ma(),
            editing_param: None,
            param_input_buffer: String::new(),
            available_tokens: Vec::new(),
            selected_token_index: 0,
            loading_tokens: false,
        }
    }

    fn add_message(&mut self, msg: String) {
        self.messages.push(msg);
        if self.messages.len() > 10 {
            self.messages.remove(0);
        }
    }
}

pub async fn run() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create registry
    let registry = Arc::new(BotRegistry::new());
    let mut app = App::new(registry.clone());

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
    app: &mut App,
) -> Result<()> {
    loop {
        // Refresh cached bot list and statuses before drawing
        if app.current_screen == BotScreen::BotList {
            app.cached_bots = app.registry.list_bots().await;
            app.cached_bot_statuses.clear();
            for bot_id in &app.cached_bots {
                if let Some(handle) = app.registry.get_bot(bot_id).await {
                    app.cached_bot_statuses.insert(bot_id.clone(), handle.latest_status());
                }
            }
        }

        // Proactively load tokens when entering token selection screen
        if app.current_screen == BotScreen::TokenSelection && app.loading_tokens {
            let api_url = std::env::var("HYPERLIQUID_API_URL")
                .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string());

            match load_tokens(&api_url).await {
                Ok(tokens) => {
                    app.available_tokens = tokens;
                    app.loading_tokens = false;
                    app.selected_token_index = 0;
                    app.add_message(format!("Loaded {} tokens", app.available_tokens.len()));
                }
                Err(e) => {
                    app.add_message(format!("Error loading tokens: {e}"));
                    app.current_screen = BotScreen::ParameterConfig;
                    app.loading_tokens = false;
                }
            }
        }

        // Poll for bot events when on monitor screen
        if app.current_screen == BotScreen::BotMonitor {
            if let Some(ref mut event_rx) = app.event_rx {
                // Try to receive events (non-blocking)
                while let Ok(event) = event_rx.try_recv() {
                    app.bot_events.push(event);
                    // Keep only last 100 events
                    if app.bot_events.len() > 100 {
                        app.bot_events.remove(0);
                    }
                }
            }

            // Update status from handle
            if let Some(bot_id) = &app.monitored_bot_id {
                if let Some(handle) = app.registry.get_bot(bot_id).await {
                    app.bot_status = Some(handle.latest_status());
                }
            }
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

async fn handle_key_event(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match app.current_screen {
        BotScreen::BotList => handle_bot_list_keys(key, app).await,
        BotScreen::BotMonitor => handle_bot_monitor_keys(key, app).await,
        BotScreen::StrategySelection => handle_strategy_selection_keys(key, app),
        BotScreen::ParameterConfig => handle_parameter_config_keys(key, app),
        BotScreen::TokenSelection => handle_token_selection_keys(key, app).await,
    }
}

async fn handle_bot_list_keys(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true), // Quit
        KeyCode::Char('a') => {
            // Add new bot - go to strategy selection
            app.current_screen = BotScreen::StrategySelection;
            app.selected_strategy_index = 0;
            app.add_message("Select strategy for new bot".to_string());
        }
        KeyCode::Char('v') => {
            // View selected bot
            if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
                if let Some(handle) = app.registry.get_bot(bot_id).await {
                    app.monitored_bot_id = Some(bot_id.clone());
                    app.bot_events.clear();
                    app.bot_status = Some(handle.latest_status());
                    app.event_rx = Some(handle.subscribe_events());
                    app.current_screen = BotScreen::BotMonitor;
                    app.add_message(format!("Monitoring bot: {bot_id}"));
                }
            }
        }
        KeyCode::Char('s') => {
            // Start selected bot
            if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
                if let Some(handle) = app.registry.get_bot(bot_id).await {
                    handle.start().await?;
                    app.add_message(format!("Started bot: {bot_id}"));
                }
            }
        }
        KeyCode::Char('x') => {
            // Stop selected bot
            if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
                if let Some(handle) = app.registry.get_bot(bot_id).await {
                    handle.stop().await?;
                    app.add_message(format!("Stopped bot: {bot_id}"));
                }
            }
        }
        KeyCode::Char('r') => {
            // Remove selected bot
            if let Some(bot_id) = app.cached_bots.get(app.selected_bot) {
                app.registry.remove_bot(bot_id).await?;
                app.add_message(format!("Removed bot: {bot_id}"));
                if app.selected_bot > 0 {
                    app.selected_bot -= 1;
                }
            }
        }
        KeyCode::Down => {
            if app.selected_bot < app.cached_bots.len().saturating_sub(1) {
                app.selected_bot += 1;
            }
        }
        KeyCode::Up => {
            if app.selected_bot > 0 {
                app.selected_bot -= 1;
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_strategy_selection_keys(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.current_screen = BotScreen::BotList;
            app.add_message("Cancelled bot creation".to_string());
        }
        KeyCode::Enter => {
            // Select strategy and go to parameter config
            app.param_config = if app.selected_strategy_index == 0 {
                ParamConfig::default_quad_ma()
            } else {
                ParamConfig::default_ma_crossover()
            };
            app.current_screen = BotScreen::ParameterConfig;
            app.editing_param = None;
            app.param_input_buffer.clear();
            app.add_message("Configure strategy parameters (Tab to edit, Enter when done)".to_string());
        }
        KeyCode::Up => {
            if app.selected_strategy_index > 0 {
                app.selected_strategy_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_strategy_index < app.available_strategies.len() - 1 {
                app.selected_strategy_index += 1;
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_parameter_config_keys(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.current_screen = BotScreen::StrategySelection;
            app.add_message("Back to strategy selection".to_string());
        }
        KeyCode::Enter if app.editing_param.is_none() => {
            // Done configuring, go to token selection
            app.current_screen = BotScreen::TokenSelection;
            app.loading_tokens = true;
            app.add_message("Loading available tokens...".to_string());
        }
        KeyCode::Tab => {
            // Cycle through editable fields
            app.editing_param = Some(next_param_field(&app.param_config.strategy, app.editing_param));
            app.param_input_buffer.clear();
        }
        KeyCode::Char(c) if c.is_ascii_digit() && app.editing_param.is_some() => {
            app.param_input_buffer.push(c);
        }
        KeyCode::Backspace if app.editing_param.is_some() => {
            app.param_input_buffer.pop();
        }
        KeyCode::Enter if app.editing_param.is_some() => {
            // Apply edited value
            if let Ok(value) = app.param_input_buffer.parse::<usize>() {
                apply_param_value(&mut app.param_config.strategy, app.editing_param.unwrap(), value);
            }
            app.editing_param = None;
            app.param_input_buffer.clear();
        }
        _ => {}
    }
    Ok(false)
}

async fn handle_token_selection_keys(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            app.current_screen = BotScreen::ParameterConfig;
            app.add_message("Back to parameter configuration".to_string());
        }
        KeyCode::Enter => {
            // Create bot with selected token
            if let Some(token) = app.available_tokens.get(app.selected_token_index) {
                match create_bot(app, token).await {
                    Ok(bot_id) => {
                        app.add_message(format!("Created bot: {bot_id}"));
                        app.current_screen = BotScreen::BotList;
                    }
                    Err(e) => {
                        app.add_message(format!("Error creating bot: {e}"));
                    }
                }
            }
        }
        KeyCode::Up => {
            if app.selected_token_index > 0 {
                app.selected_token_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_token_index < app.available_tokens.len().saturating_sub(1) {
                app.selected_token_index += 1;
            }
        }
        KeyCode::PageUp => {
            app.selected_token_index = app.selected_token_index.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.selected_token_index = (app.selected_token_index + 10)
                .min(app.available_tokens.len().saturating_sub(1));
        }
        _ => {}
    }
    Ok(false)
}

async fn handle_bot_monitor_keys(key: crossterm::event::KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            // Return to bot list
            app.current_screen = BotScreen::BotList;
            app.monitored_bot_id = None;
            app.bot_events.clear();
            app.bot_status = None;
            app.event_rx = None;
        }
        _ => {}
    }
    Ok(false)
}

fn next_param_field(strategy: &StrategyType, current: Option<ParamField>) -> ParamField {
    match strategy {
        StrategyType::MaCrossover { .. } => match current {
            None | Some(ParamField::SlowPeriod) => ParamField::FastPeriod,
            Some(ParamField::FastPeriod) => ParamField::SlowPeriod,
            _ => ParamField::FastPeriod,
        },
        StrategyType::QuadMa { .. } => match current {
            None | Some(ParamField::ReversalConfirmBars) => ParamField::Ma1Period,
            Some(ParamField::Ma1Period) => ParamField::Ma2Period,
            Some(ParamField::Ma2Period) => ParamField::Ma3Period,
            Some(ParamField::Ma3Period) => ParamField::Ma4Period,
            Some(ParamField::Ma4Period) => ParamField::TrendPeriod,
            Some(ParamField::TrendPeriod) => ParamField::VolumeFactor,
            Some(ParamField::VolumeFactor) => ParamField::TakeProfit,
            Some(ParamField::TakeProfit) => ParamField::StopLoss,
            Some(ParamField::StopLoss) => ParamField::ReversalConfirmBars,
            _ => ParamField::Ma1Period,
        },
    }
}

fn apply_param_value(strategy: &mut StrategyType, field: ParamField, value: usize) {
    match (strategy, field) {
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
        (StrategyType::QuadMa { reversal_confirmation_bars, .. }, ParamField::ReversalConfirmBars) => {
            *reversal_confirmation_bars = value;
        }
        _ => {} // Mismatched strategy/field combination
    }
}

async fn load_tokens(api_url: &str) -> Result<Vec<String>> {
    let client = HyperliquidClient::new(api_url.to_string());
    client.fetch_available_symbols().await
}

async fn create_bot(app: &App, token: &str) -> Result<String> {
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let bot_id = format!("bot_{}_{token}", timestamp);

    let (strategy_name, strategy_config) = match &app.param_config.strategy {
        StrategyType::MaCrossover { fast, slow } => (
            "ma_crossover",
            serde_json::json!({
                "fast": fast,
                "slow": slow,
            })
            .to_string(),
        ),
        StrategyType::QuadMa {
            ma1,
            ma2,
            ma3,
            ma4,
            trend_period,
            volume_factor,
            take_profit,
            stop_loss,
            reversal_confirmation_bars,
        } => (
            "quad_ma",
            serde_json::json!({
                "ma1": ma1,
                "ma2": ma2,
                "ma3": ma3,
                "ma4": ma4,
                "trend_period": trend_period,
                "volume_factor": volume_factor,
                "take_profit": take_profit,
                "stop_loss": stop_loss,
                "reversal_confirmation_bars": reversal_confirmation_bars,
            })
            .to_string(),
        ),
    };

    let config = BotConfig {
        bot_id: bot_id.clone(),
        symbol: token.to_string(),
        strategy: strategy_name.to_string(),
        enabled: true,
        interval: "1m".to_string(),
        ws_url: std::env::var("HYPERLIQUID_WS_URL")
            .unwrap_or_else(|_| "wss://api.hyperliquid.xyz/ws".to_string()),
        api_url: std::env::var("HYPERLIQUID_API_URL")
            .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string()),
        warmup_periods: 100,
        strategy_config: Some(strategy_config),
        initial_capital: rust_decimal::Decimal::from(10000),
        risk_per_trade_pct: 0.05,
        max_position_pct: 0.20,
        leverage: 1,
        margin_mode: algo_trade_bot_orchestrator::MarginMode::Cross,
        execution_mode: algo_trade_bot_orchestrator::ExecutionMode::Paper,
        paper_slippage_bps: 10.0,
        paper_commission_rate: 0.00025,
        wallet: None, // Loaded from env at runtime
    };

    app.registry.spawn_bot(config).await?;

    Ok(bot_id)
}

fn ui(f: &mut Frame, app: &App) {
    match app.current_screen {
        BotScreen::BotList => render_bot_list(f, app),
        BotScreen::BotMonitor => render_bot_monitor(f, app),
        BotScreen::StrategySelection => render_strategy_selection(f, app),
        BotScreen::ParameterConfig => render_parameter_config(f, app),
        BotScreen::TokenSelection => render_token_selection(f, app),
    }
}

/// Returns color for bot state
fn state_color(state: &BotState) -> Color {
    match state {
        BotState::Running => Color::Green,
        BotState::Stopped => Color::Gray,
        BotState::Paused => Color::Yellow,
        BotState::Error => Color::Red,
    }
}

/// Returns icon for bot state
fn state_icon(state: &BotState) -> &'static str {
    match state {
        BotState::Running => "▶",
        BotState::Stopped => "■",
        BotState::Paused => "⏸",
        BotState::Error => "✖",
    }
}

/// Returns icon for execution mode
fn execution_icon(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Live => "💰",
        ExecutionMode::Paper => "📄",
    }
}

/// Formats PnL with color (green for profit, red for loss)
fn format_pnl(pnl_pct: f64) -> (String, Color) {
    let color = if pnl_pct >= 0.0 { Color::Green } else { Color::Red };
    let sign = if pnl_pct >= 0.0 { "+" } else { "" };
    (format!("{sign}{pnl_pct:.2}%"), color)
}

/// Formats duration in human-readable format (e.g., "2h 15m", "45m", "3d 5h")
fn format_duration(started_at: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(started_at);

    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

fn render_bot_list(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(3),  // Summary
            Constraint::Min(10),    // Bot list
            Constraint::Length(10), // Messages
            Constraint::Length(3),  // Help
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Live Bot Manager")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Summary header - count bots by state
    let mut running = 0;
    let mut stopped = 0;
    let mut paused = 0;
    let mut error = 0;

    for status in app.cached_bot_statuses.values() {
        match status.state {
            BotState::Running => running += 1,
            BotState::Stopped => stopped += 1,
            BotState::Paused => paused += 1,
            BotState::Error => error += 1,
        }
    }

    let summary_text = vec![
        Span::styled(format!("{running} Running"), Style::default().fg(Color::Green)),
        Span::raw(" | "),
        Span::styled(format!("{stopped} Stopped"), Style::default().fg(Color::Gray)),
        Span::raw(" | "),
        Span::styled(format!("{paused} Paused"), Style::default().fg(Color::Yellow)),
        Span::raw(" | "),
        Span::styled(format!("{error} Error"), Style::default().fg(Color::Red)),
    ];

    let summary = Paragraph::new(Line::from(summary_text))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Summary"));
    f.render_widget(summary, chunks[1]);

    // Bot list with color coding, icons, and compact metrics
    let items: Vec<ListItem> = app
        .cached_bots
        .iter()
        .enumerate()
        .map(|(i, bot_id)| {
            // Get bot status for color, icons, and metrics from cache
            let (state_col, display_text) = if let Some(status) = app.cached_bot_statuses.get(bot_id) {
                let state_col = state_color(&status.state);
                let state_ico = state_icon(&status.state);
                let exec_ico = execution_icon(status.execution_mode);

                // Runtime display (only for running bots)
                let runtime_str = if matches!(status.state, BotState::Running) && status.started_at.is_some() {
                    format!(" | {}", format_duration(status.started_at.unwrap()))
                } else {
                    String::new()
                };

                // Metrics display (trades and return)
                let (pnl_str, _pnl_color) = format_pnl(status.total_return_pct);
                let metrics_str = format!(" | {} trades | {}", status.num_trades, pnl_str);

                let display_text = format!("{state_ico} {exec_ico} {bot_id}{runtime_str}{metrics_str}");
                (state_col, display_text)
            } else {
                (Color::Gray, format!("? ? {bot_id}"))
            };

            // Highlight selected bot
            let style = if i == app.selected_bot {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(state_col)
            };

            ListItem::new(display_text).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Bots"))
        .highlight_style(Style::default().bg(Color::DarkGray));

    f.render_widget(list, chunks[2]);

    // Messages
    let messages: Vec<Line> = app.messages.iter().map(|m| Line::from(m.as_str())).collect();
    let messages_widget = Paragraph::new(messages)
        .block(Block::default().borders(Borders::ALL).title("Messages"))
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(messages_widget, chunks[3]);

    // Help
    let help = Paragraph::new("a: Add Bot | v: View Bot | s: Start | x: Stop | r: Remove | ↑↓: Navigate | q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[4]);
}

fn render_bot_monitor(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Title
            Constraint::Length(8),   // Metrics panel
            Constraint::Min(8),      // Trade history
            Constraint::Min(8),      // Events log
            Constraint::Length(3),   // Help
        ])
        .split(f.area());

    // Title
    let bot_id = app.monitored_bot_id.as_ref().map_or("Unknown", String::as_str);
    let title = Paragraph::new(format!("Bot Monitor - {bot_id}"))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Metrics panel
    let metrics_text = if let Some(status) = &app.bot_status {
        vec![
            Line::from(vec![
                Span::raw("Equity: "),
                Span::styled(format!("${}", status.current_equity), Style::default().fg(Color::Green)),
                Span::raw("  Return: "),
                Span::styled(format!("{:.2}%", status.total_return_pct * 100.0),
                    if status.total_return_pct >= 0.0 { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Red) }),
            ]),
            Line::from(vec![
                Span::raw("Sharpe: "),
                Span::styled(format!("{:.2}", status.sharpe_ratio), Style::default().fg(Color::Yellow)),
                Span::raw("  Max DD: "),
                Span::styled(format!("{:.2}%", status.max_drawdown * 100.0), Style::default().fg(Color::Red)),
                Span::raw("  Win Rate: "),
                Span::styled(format!("{:.1}%", status.win_rate * 100.0), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::raw("Trades: "),
                Span::styled(status.num_trades.to_string(), Style::default().fg(Color::White)),
                Span::raw("  Open Positions: "),
                Span::styled(status.open_positions.len().to_string(), Style::default().fg(Color::Magenta)),
            ]),
            Line::from(vec![
                Span::raw("State: "),
                Span::styled(format!("{:?}", status.state), Style::default().fg(Color::Green)),
            ]),
        ]
    } else {
        vec![
            Line::from("Waiting for bot data..."),
        ]
    };

    let metrics_panel = Paragraph::new(metrics_text)
        .block(Block::default().borders(Borders::ALL).title("Performance Metrics"));
    f.render_widget(metrics_panel, chunks[1]);

    // Trade history
    let trade_lines: Vec<ListItem> = if let Some(status) = &app.bot_status {
        status.closed_trades
            .iter()
            .rev()
            .take(10)
            .map(|trade| {
                let direction_icon = match trade.direction {
                    algo_trade_core::events::TradeDirection::Long => "📈",
                    algo_trade_core::events::TradeDirection::Short => "📉",
                };
                let pnl_color = if trade.pnl > rust_decimal::Decimal::ZERO {
                    Color::Green
                } else {
                    Color::Red
                };
                let pnl_sign = if trade.pnl > rust_decimal::Decimal::ZERO { "+" } else { "" };

                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} {} ", direction_icon, trade.symbol)),
                    Span::styled(
                        format!("{}{:.2}% ", pnl_sign, trade.pnl_pct),
                        Style::default().fg(pnl_color).add_modifier(Modifier::BOLD)
                    ),
                    Span::raw(format!("(${} → ${})", trade.entry_price, trade.exit_price)),
                ]))
            })
            .collect()
    } else {
        vec![ListItem::new("No trades yet")]
    };

    let trades_list = List::new(trade_lines)
        .block(Block::default().borders(Borders::ALL).title("Trade History (Last 10)"));
    f.render_widget(trades_list, chunks[2]);

    // Events log
    let event_lines: Vec<ListItem> = app.bot_events
        .iter()
        .rev()
        .take(20)
        .map(|event| {
            let text = format_bot_event(event);
            ListItem::new(text)
        })
        .collect();

    let events_list = List::new(event_lines)
        .block(Block::default().borders(Borders::ALL).title("Recent Events"));
    f.render_widget(events_list, chunks[3]);

    // Help
    let help = Paragraph::new("q/Esc: Back to Bot List")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[4]);
}

fn format_bot_event(event: &BotEvent) -> String {
    match event {
        BotEvent::MarketUpdate { symbol, price, timestamp, .. } => {
            format!("[{:}] Market: {} @ ${}", timestamp.format("%H:%M:%S"), symbol, price)
        }
        BotEvent::SignalGenerated(signal) => {
            format!("[{:}] Signal: {:?} {} @ ${}", signal.timestamp.format("%H:%M:%S"), signal.direction, signal.symbol, signal.price)
        }
        BotEvent::OrderPlaced(order) => {
            format!("[{:}] Order: {:?} {} qty={}", order.timestamp.format("%H:%M:%S"), order.direction, order.symbol, order.quantity)
        }
        BotEvent::OrderFilled(fill) => {
            format!("[{:}] Fill: {:?} {} qty={} @ ${}", fill.timestamp.format("%H:%M:%S"), fill.direction, fill.symbol, fill.quantity, fill.price)
        }
        BotEvent::PositionUpdate { symbol, quantity, avg_price, unrealized_pnl } => {
            format!("Position: {} qty={} avg=${} PnL=${}", symbol, quantity, avg_price, unrealized_pnl)
        }
        BotEvent::TradeClosed { symbol, pnl, win } => {
            let status = if *win { "WIN" } else { "LOSS" };
            format!("Trade Closed: {} {} PnL=${}", symbol, status, pnl)
        }
        BotEvent::Error { message, timestamp } => {
            format!("[{:}] ERROR: {}", timestamp.format("%H:%M:%S"), message)
        }
    }
}

fn render_strategy_selection(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new("Select Strategy")
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = app
        .available_strategies
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let style = if i == app.selected_strategy_index {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(*name).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray));

    f.render_widget(list, chunks[1]);

    let help = Paragraph::new("↑↓: Navigate | Enter: Select | Esc: Cancel")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}

fn render_parameter_config(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new(format!("Configure {} Strategy", app.param_config.strategy.name()))
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Build parameter display based on strategy type
    let param_lines = build_param_display(&app.param_config.strategy, app.editing_param, &app.param_input_buffer);

    let params = Paragraph::new(param_lines)
        .block(Block::default().borders(Borders::ALL).title("Parameters"));
    f.render_widget(params, chunks[1]);

    let help = Paragraph::new("Tab: Edit Field | 0-9: Type Value | Enter: Confirm/Done | Esc: Back")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}

fn build_param_display(strategy: &StrategyType, editing: Option<ParamField>, buffer: &str) -> Vec<Line<'static>> {
    let highlight_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    match strategy {
        StrategyType::MaCrossover { fast, slow } => {
            vec![
                Line::from(vec![
                    Span::raw("Fast Period: "),
                    if editing == Some(ParamField::FastPeriod) {
                        Span::styled(
                            if buffer.is_empty() { format!("[{fast}_]") } else { format!("[{buffer}_]") },
                            highlight_style
                        )
                    } else {
                        Span::raw(fast.to_string())
                    },
                ]),
                Line::from(vec![
                    Span::raw("Slow Period: "),
                    if editing == Some(ParamField::SlowPeriod) {
                        Span::styled(
                            if buffer.is_empty() { format!("[{slow}_]") } else { format!("[{buffer}_]") },
                            highlight_style
                        )
                    } else {
                        Span::raw(slow.to_string())
                    },
                ]),
            ]
        }
        StrategyType::QuadMa {
            ma1,
            ma2,
            ma3,
            ma4,
            trend_period,
            volume_factor,
            take_profit,
            stop_loss,
            reversal_confirmation_bars,
        } => {
            vec![
                Line::from(vec![
                    Span::raw("MA1: "),
                    if editing == Some(ParamField::Ma1Period) {
                        Span::styled(if buffer.is_empty() { format!("[{ma1}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(ma1.to_string())
                    },
                    Span::raw("  MA2: "),
                    if editing == Some(ParamField::Ma2Period) {
                        Span::styled(if buffer.is_empty() { format!("[{ma2}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(ma2.to_string())
                    },
                    Span::raw("  MA3: "),
                    if editing == Some(ParamField::Ma3Period) {
                        Span::styled(if buffer.is_empty() { format!("[{ma3}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(ma3.to_string())
                    },
                    Span::raw("  MA4: "),
                    if editing == Some(ParamField::Ma4Period) {
                        Span::styled(if buffer.is_empty() { format!("[{ma4}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(ma4.to_string())
                    },
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("Trend Period: "),
                    if editing == Some(ParamField::TrendPeriod) {
                        Span::styled(if buffer.is_empty() { format!("[{trend_period}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(trend_period.to_string())
                    },
                ]),
                Line::from(vec![
                    Span::raw("Volume Factor: "),
                    if editing == Some(ParamField::VolumeFactor) {
                        Span::styled(if buffer.is_empty() { format!("[{volume_factor}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(format!("{volume_factor} ({:.2}x)", *volume_factor as f64 / 100.0))
                    },
                ]),
                Line::from(vec![
                    Span::raw("Take Profit: "),
                    if editing == Some(ParamField::TakeProfit) {
                        Span::styled(if buffer.is_empty() { format!("[{take_profit}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(format!("{take_profit} ({:.2}%)", *take_profit as f64 / 100.0))
                    },
                ]),
                Line::from(vec![
                    Span::raw("Stop Loss: "),
                    if editing == Some(ParamField::StopLoss) {
                        Span::styled(if buffer.is_empty() { format!("[{stop_loss}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(format!("{stop_loss} ({:.2}%)", *stop_loss as f64 / 100.0))
                    },
                ]),
                Line::from(vec![
                    Span::raw("Reversal Confirmation: "),
                    if editing == Some(ParamField::ReversalConfirmBars) {
                        Span::styled(if buffer.is_empty() { format!("[{reversal_confirmation_bars}_]") } else { format!("[{buffer}_]") }, highlight_style)
                    } else {
                        Span::raw(format!("{reversal_confirmation_bars} bars"))
                    },
                ]),
            ]
        }
    }
}

fn render_token_selection(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new("Select Token")
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    if app.loading_tokens {
        let loading = Paragraph::new("Loading tokens...")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(loading, chunks[1]);
    } else {
        let items: Vec<ListItem> = app
            .available_tokens
            .iter()
            .enumerate()
            .map(|(i, token)| {
                let style = if i == app.selected_token_index {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(token.as_str()).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!("{} tokens available", app.available_tokens.len())))
            .highlight_style(Style::default().bg(Color::DarkGray));

        f.render_widget(list, chunks[1]);
    }

    let help = Paragraph::new("↑↓: Navigate | PgUp/PgDn: Jump 10 | Enter: Create Bot | Esc: Back")
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}
