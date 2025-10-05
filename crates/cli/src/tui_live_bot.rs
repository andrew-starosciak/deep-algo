use algo_trade_bot_orchestrator::{BotConfig, BotHandle, BotRegistry};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::io;
use std::sync::Arc;
use tokio::sync::RwLock;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

struct App {
    registry: Arc<BotRegistry>,
    input: Input,
    input_mode: InputMode,
    messages: Vec<String>,
    selected_bot: usize,
}

enum InputMode {
    Normal,
    AddBot,
}

impl App {
    fn new(registry: Arc<BotRegistry>) -> Self {
        Self {
            registry,
            input: Input::default(),
            input_mode: InputMode::Normal,
            messages: vec!["TUI started. Press 'a' to add bot, 'q' to quit".to_string()],
            selected_bot: 0,
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
        terminal.draw(|f| ui(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('a') => {
                            app.input_mode = InputMode::AddBot;
                            app.add_message("Enter bot config: bot_id,symbol,strategy,interval (e.g., bot1,BTC,quad_ma,1m)".to_string());
                        }
                        KeyCode::Char('s') => {
                            // Start selected bot
                            let bots = app.registry.list_bots();
                            if let Some(bot_id) = bots.get(app.selected_bot) {
                                if let Some(handle) = app.registry.get_bot(bot_id) {
                                    handle.start().await?;
                                    app.add_message(format!("Started bot: {bot_id}"));
                                }
                            }
                        }
                        KeyCode::Char('x') => {
                            // Stop selected bot
                            let bots = app.registry.list_bots();
                            if let Some(bot_id) = bots.get(app.selected_bot) {
                                if let Some(handle) = app.registry.get_bot(bot_id) {
                                    handle.stop().await?;
                                    app.add_message(format!("Stopped bot: {bot_id}"));
                                }
                            }
                        }
                        KeyCode::Char('r') => {
                            // Remove selected bot
                            let bots = app.registry.list_bots();
                            if let Some(bot_id) = bots.get(app.selected_bot) {
                                app.registry.remove_bot(bot_id).await?;
                                app.add_message(format!("Removed bot: {bot_id}"));
                                if app.selected_bot > 0 {
                                    app.selected_bot -= 1;
                                }
                            }
                        }
                        KeyCode::Down => {
                            let bots = app.registry.list_bots();
                            if app.selected_bot < bots.len().saturating_sub(1) {
                                app.selected_bot += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.selected_bot > 0 {
                                app.selected_bot -= 1;
                            }
                        }
                        _ => {}
                    },
                    InputMode::AddBot => match key.code {
                        KeyCode::Enter => {
                            let input_str = app.input.value();
                            if let Err(e) = parse_and_add_bot(&app.registry, input_str).await {
                                app.add_message(format!("Error adding bot: {e}"));
                            } else {
                                app.add_message(format!("Added bot: {input_str}"));
                            }
                            app.input.reset();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Esc => {
                            app.input.reset();
                            app.input_mode = InputMode::Normal;
                        }
                        _ => {
                            app.input.handle_event(&Event::Key(key));
                        }
                    },
                }
            }
        }
    }
}

async fn parse_and_add_bot(registry: &BotRegistry, input: &str) -> Result<()> {
    let parts: Vec<&str> = input.split(',').collect();
    if parts.len() != 4 {
        anyhow::bail!("Invalid format. Expected: bot_id,symbol,strategy,interval");
    }

    let config = BotConfig {
        bot_id: parts[0].to_string(),
        symbol: parts[1].to_string(),
        strategy: parts[2].to_string(),
        enabled: true,
        interval: parts[3].to_string(),
        ws_url: std::env::var("HYPERLIQUID_WS_URL")
            .unwrap_or_else(|_| "wss://api.hyperliquid.xyz/ws".to_string()),
        api_url: std::env::var("HYPERLIQUID_API_URL")
            .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string()),
        warmup_periods: 100,
        strategy_config: None,
    };

    registry.add_bot(config).await?;
    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Live Bot Manager")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Bot list
    let bots = app.registry.list_bots();
    let items: Vec<ListItem> = bots
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

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Bots"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(list, chunks[1]);

    // Messages
    let messages: Vec<ListItem> = app
        .messages
        .iter()
        .map(|m| ListItem::new(m.as_str()))
        .collect();
    let messages_widget = List::new(messages)
        .block(Block::default().borders(Borders::ALL).title("Messages"));
    f.render_widget(messages_widget, chunks[2]);

    // Input/Help
    let (msg, style) = match app.input_mode {
        InputMode::Normal => (
            vec![
                Span::raw("Press "),
                Span::styled("a", Style::default().fg(Color::Yellow)),
                Span::raw(" to add bot, "),
                Span::styled("s", Style::default().fg(Color::Green)),
                Span::raw(" to start, "),
                Span::styled("x", Style::default().fg(Color::Red)),
                Span::raw(" to stop, "),
                Span::styled("r", Style::default().fg(Color::Red)),
                Span::raw(" to remove, "),
                Span::styled("q", Style::default().fg(Color::Red)),
                Span::raw(" to quit"),
            ],
            Style::default(),
        ),
        InputMode::AddBot => (
            vec![
                Span::raw("Enter config: "),
                Span::styled(app.input.value(), Style::default().fg(Color::Yellow)),
            ],
            Style::default().fg(Color::Yellow),
        ),
    };
    let mut text = Text::from(Line::from(msg));
    text.patch_style(style);
    let help = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[3]);
}
