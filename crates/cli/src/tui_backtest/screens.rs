use super::{App, AppScreen};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Row, Table},
    Frame,
};
use rust_decimal::{prelude::ToPrimitive, Decimal};

pub fn render(f: &mut Frame, app: &App) {
    match app.current_screen {
        AppScreen::StrategySelection => render_strategy_selection(f, app),
        AppScreen::TokenSelection => render_token_selection(f, app),
        AppScreen::TimeframeConfig => render_timeframe_config(f, app),
        AppScreen::ParameterConfig => render_parameter_config(f, app),
        AppScreen::Running => render_running(f, app),
        AppScreen::Results => render_results(f, app),
        AppScreen::TradeDetail => render_trade_detail(f, app),
    }
}

fn render_strategy_selection(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(0),    // Content
            Constraint::Length(3), // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Select Strategy")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Strategy list
    let items: Vec<ListItem> = app
        .available_strategies
        .iter()
        .enumerate()
        .map(|(i, strategy)| {
            let style = if i == app.selected_strategy_index {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            let marker = if i == app.selected_strategy_index { "● " } else { "○ " };
            ListItem::new(format!("{marker}{strategy}")).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Strategies"));
    f.render_widget(list, chunks[1]);

    // Instructions
    let instructions = Paragraph::new("↑↓: Navigate | Enter: Select | q: Quit")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

fn render_token_selection(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(0),    // Token list
            Constraint::Length(5), // Stats + Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!(
        "Select Tokens ({} selected)",
        app.selected_tokens.len()
    ))
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Token list
    if app.loading_tokens {
        let loading = Paragraph::new("Loading tokens from Hyperliquid...")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(loading, chunks[1]);
    } else {
        let items: Vec<ListItem> = app
            .available_tokens
            .iter()
            .enumerate()
            .map(|(i, token)| {
                let is_selected = app.selected_tokens.contains(token);
                let is_highlighted = i == app.token_scroll_offset;

                let style = if is_highlighted {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default()
                };

                let checkbox = if is_selected { "[x]" } else { "[ ]" };
                ListItem::new(format!("{checkbox} {token}")).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Tokens"));
        f.render_widget(list, chunks[1]);
    }

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from("↑↓: Navigate | Space: Toggle | a: Select All | n: Deselect All"),
        Line::from("Enter: Continue | Esc: Back | q: Quit"),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

fn render_timeframe_config(f: &mut Frame, app: &App) {
    use super::TimeframeField;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(8),    // Form
            Constraint::Length(5), // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Configure Timeframe")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Form fields
    let form_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Start date
            Constraint::Length(3), // End date
            Constraint::Length(3), // Interval
        ])
        .margin(1)
        .split(chunks[1]);

    // Start date field
    let start_style = if app.editing_field == Some(TimeframeField::StartDate) {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let start_field = Paragraph::new(format!("Start: {}", app.start_date_input))
        .style(start_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(start_field, form_chunks[0]);

    // End date field
    let end_style = if app.editing_field == Some(TimeframeField::EndDate) {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let end_field = Paragraph::new(format!("End:   {}", app.end_date_input))
        .style(end_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(end_field, form_chunks[1]);

    // Interval selector
    let interval_text = if app.editing_field == Some(TimeframeField::Interval) {
        let options_str = app.interval_options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                if i == app.selected_interval_index {
                    format!("[{opt}]")
                } else {
                    opt.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("Interval: {options_str}")
    } else {
        format!("Interval: {}", app.interval)
    };

    let interval_style = if app.editing_field == Some(TimeframeField::Interval) {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let interval_field = Paragraph::new(interval_text)
        .style(interval_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(interval_field, form_chunks[2]);

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from("Tab: Select Field | Type: Edit Date | ↑↓: Change Interval"),
        Line::from("Backspace: Delete | Enter: Continue | Esc: Back"),
        Line::from(format!("Date format: YYYY-MM-DDTHH:MM:SSZ (e.g., {})", app.start_date.format("%Y-%m-%dT%H:%M:%SZ"))),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

fn render_parameter_config(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(10),    // Config list
            Constraint::Length(6),  // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!(
        "Parameter Configurations ({} configs)",
        app.param_configs.len()
    ))
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Config list
    let items: Vec<ListItem> = app
        .param_configs
        .iter()
        .enumerate()
        .map(|(i, config)| {
            let is_selected = i == app.selected_param_index;
            let style = if is_selected {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };

            // Build params string with highlighting for edit mode
            let params_line = if is_selected && app.editing_param.is_some() {
                use super::ParamField;
                let editing_field = app.editing_param.unwrap();

                match &config.strategy {
                    super::StrategyType::MaCrossover { fast, slow } => {
                        let mut spans = vec![Span::raw("Fast: ")];
                        if editing_field == ParamField::FastPeriod {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{fast}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(fast.to_string()));
                        }
                        spans.push(Span::raw(", Slow: "));
                        if editing_field == ParamField::SlowPeriod {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{slow}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(slow.to_string()));
                        }
                        Line::from(spans)
                    }
                    super::StrategyType::QuadMa { ma1, ma2, ma3, ma4 } => {
                        let mut spans = vec![Span::raw("MA1: ")];
                        if editing_field == ParamField::Ma1Period {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{ma1}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(ma1.to_string()));
                        }
                        spans.push(Span::raw(", MA2: "));
                        if editing_field == ParamField::Ma2Period {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{ma2}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(ma2.to_string()));
                        }
                        spans.push(Span::raw(", MA3: "));
                        if editing_field == ParamField::Ma3Period {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{ma3}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(ma3.to_string()));
                        }
                        spans.push(Span::raw(", MA4: "));
                        if editing_field == ParamField::Ma4Period {
                            spans.push(Span::styled(
                                if app.param_input_buffer.is_empty() {
                                    format!("[{ma4}_]")
                                } else {
                                    format!("[{}_]", app.param_input_buffer)
                                },
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            ));
                        } else {
                            spans.push(Span::raw(ma4.to_string()));
                        }
                        Line::from(spans)
                    }
                }
            } else {
                // Normal mode - no edit highlighting
                let params_str = match &config.strategy {
                    super::StrategyType::MaCrossover { fast, slow } => {
                        format!("Fast: {fast}, Slow: {slow}")
                    }
                    super::StrategyType::QuadMa { ma1, ma2, ma3, ma4 } => {
                        format!("MA1: {ma1}, MA2: {ma2}, MA3: {ma3}, MA4: {ma4}")
                    }
                };
                Line::from(params_str)
            };

            ListItem::new(vec![
                Line::from(Span::styled(&config.name, Style::default().add_modifier(Modifier::BOLD))),
                params_line,
            ])
            .style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Configurations"));
    f.render_widget(list, chunks[1]);

    // Instructions
    let instructions = if app.editing_param.is_some() {
        Paragraph::new(vec![
            Line::from("EDIT MODE: Type digits to change value | Tab: Next Field"),
            Line::from("Enter: Save | Esc: Cancel"),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL))
    } else {
        Paragraph::new(vec![
            Line::from("↑↓: Navigate | e: Edit | a: Add Config | d: Delete"),
            Line::from(format!(
                "Backtest Matrix: {} tokens × {} configs = {} backtests",
                app.selected_tokens.len(),
                app.param_configs.len(),
                app.selected_tokens.len() * app.param_configs.len()
            )),
            Line::from("Enter: Run Backtests | Esc: Back | q: Quit"),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL))
    };
    f.render_widget(instructions, chunks[2]);
}

fn render_running(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(3),  // Progress bar
            Constraint::Length(3),  // Current backtest
            Constraint::Min(5),     // Status messages
            Constraint::Length(3),  // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Running Backtests")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Progress bar
    #[allow(clippy::cast_precision_loss)]
    let progress = if app.total_backtests > 0 {
        (app.completed_backtests as f64 / app.total_backtests as f64) * 100.0
    } else {
        0.0
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Green).bg(Color::Black))
        .percent(progress as u16)
        .label(format!(
            "{}/{} ({:.1}%)",
            app.completed_backtests, app.total_backtests, progress
        ));
    f.render_widget(gauge, chunks[1]);

    // Current backtest info
    let current_info = if let Some((token, config)) = &app.current_backtest {
        format!("Current: {token} - {config}")
    } else {
        "Initializing...".to_string()
    };

    let info = Paragraph::new(current_info)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Current"));
    f.render_widget(info, chunks[2]);

    // Status messages (last 5)
    let status_lines: Vec<Line> = app.status_messages
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(|msg| Line::from(msg.as_str()))
        .collect();

    let status = Paragraph::new(status_lines)
        .block(Block::default().borders(Borders::ALL).title("Status Log"))
        .style(Style::default().fg(Color::Gray));
    f.render_widget(status, chunks[3]);

    // Instructions
    let instructions = Paragraph::new("Esc: Cancel | q: Quit")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[4]);
}

fn render_results(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(10),    // Results table
            Constraint::Length(4),  // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!("Results ({} backtests)", app.results.len()))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Results table
    let sort_indicators = ["↓", "↓", "↓", "↓", "↓", "↓"];
    let header_cells = [
        if app.sort_column == 0 { format!("Token {}", sort_indicators[0]) } else { "Token".to_string() },
        if app.sort_column == 1 { format!("Config {}", sort_indicators[1]) } else { "Config".to_string() },
        if app.sort_column == 2 { format!("Return% {}", if app.sort_ascending { "↑" } else { "↓" }) } else { "Return%".to_string() },
        if app.sort_column == 3 { format!("Sharpe {}", if app.sort_ascending { "↑" } else { "↓" }) } else { "Sharpe".to_string() },
        if app.sort_column == 4 { format!("MaxDD% {}", if app.sort_ascending { "↑" } else { "↓" }) } else { "MaxDD%".to_string() },
        if app.sort_column == 5 { format!("Trades {}", if app.sort_ascending { "↑" } else { "↓" }) } else { "Trades".to_string() },
    ];

    let header = Row::new(header_cells)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .results
        .iter()
        .map(|result| {
            let return_color = match result.total_return.cmp(&Decimal::ZERO) {
                std::cmp::Ordering::Greater => Color::Green,
                std::cmp::Ordering::Less => Color::Red,
                std::cmp::Ordering::Equal => Color::White,
            };

            Row::new(vec![
                result.token.clone(),
                result.config_name.clone(),
                format!("{:.2}", result.total_return.to_f64().unwrap_or(0.0)),
                format!("{:.2}", result.sharpe_ratio),
                format!("{:.2}", result.max_drawdown.to_f64().unwrap_or(0.0)),
                format!("{}", result.num_trades),
            ])
            .style(Style::default().fg(return_color))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(15),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title("Backtest Results"));

    f.render_widget(table, chunks[1]);

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from("↑↓: Scroll | Enter: View Trades | s: Change Sort | r: Reverse Sort"),
        Line::from("b: Back to Start | q: Quit"),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

fn render_trade_detail(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(10),    // Trades table
            Constraint::Length(3),  // Instructions
        ])
        .split(f.area());

    // Get selected result
    let result = app.selected_result_index
        .and_then(|idx| app.results.get(idx));

    let title_text = if let Some(r) = result {
        format!("Trade History: {} - {} ({} trades)", r.token, r.config_name, r.trades.len())
    } else {
        "Trade History".to_string()
    };

    // Title
    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Trades table
    if let Some(r) = result {
        let header = Row::new(vec!["Timestamp", "Action", "Price", "Quantity", "Commission"])
            .style(Style::default().add_modifier(Modifier::BOLD))
            .bottom_margin(1);

        let rows: Vec<Row> = r.trades
            .iter()
            .skip(app.trade_scroll_offset)
            .map(|trade| {
                let action_color = match trade.action.as_str() {
                    "BUY" => Color::Green,
                    "SELL" => Color::Red,
                    _ => Color::White,
                };

                Row::new(vec![
                    trade.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                    trade.action.clone(),
                    format!("{:.2}", trade.price.to_f64().unwrap_or(0.0)),
                    format!("{:.4}", trade.quantity.to_f64().unwrap_or(0.0)),
                    format!("{:.4}", trade.commission.to_f64().unwrap_or(0.0)),
                ])
                .style(Style::default().fg(action_color))
            })
            .collect();

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
        .block(Block::default().borders(Borders::ALL).title("Trades"));

        f.render_widget(table, chunks[1]);
    } else {
        let empty = Paragraph::new("No result selected")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(empty, chunks[1]);
    }

    // Instructions
    let instructions = Paragraph::new("↑↓: Scroll | Esc: Back to Results")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}
