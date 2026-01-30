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
        AppScreen::MetricsDetail => render_metrics_detail(f, app),
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
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
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
            let marker = if i == app.selected_strategy_index {
                "● "
            } else {
                "○ "
            };
            ListItem::new(format!("{marker}{strategy}")).style(style)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Strategies"));
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
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
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

        let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Tokens"));
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

/// Build date field line with component highlighting
fn build_date_line<'a>(
    prefix: &'static str,
    year: &'a str,
    month: &'a str,
    day: &'a str,
    hour: &'a str,
    minute: &'a str,
    editing_date_field: Option<super::DateField>,
) -> Line<'a> {
    use super::DateField;

    let cyan = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default();

    Line::from(vec![
        Span::raw(prefix),
        Span::styled(
            year,
            if editing_date_field == Some(DateField::Year) {
                cyan
            } else {
                normal
            },
        ),
        Span::raw("-"),
        Span::styled(
            month,
            if editing_date_field == Some(DateField::Month) {
                cyan
            } else {
                normal
            },
        ),
        Span::raw("-"),
        Span::styled(
            day,
            if editing_date_field == Some(DateField::Day) {
                cyan
            } else {
                normal
            },
        ),
        Span::raw(" "),
        Span::styled(
            hour,
            if editing_date_field == Some(DateField::Hour) {
                cyan
            } else {
                normal
            },
        ),
        Span::raw(":"),
        Span::styled(
            minute,
            if editing_date_field == Some(DateField::Minute) {
                cyan
            } else {
                normal
            },
        ),
    ])
}

/// Build interval text with option highlighting
fn build_interval_text(app: &App) -> String {
    if app.editing_field == Some(super::TimeframeField::Interval) {
        let options_str = app
            .interval_options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                if i == app.selected_interval_index {
                    format!("[{opt}]")
                } else {
                    (*opt).to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("Interval: {options_str}")
    } else {
        format!("Interval: {}", app.interval)
    }
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
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
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
    let start_is_editing = app.editing_field == Some(TimeframeField::StartDate);
    let start_line = if start_is_editing {
        build_date_line(
            "Start: ",
            &app.start_year,
            &app.start_month,
            &app.start_day,
            &app.start_hour,
            &app.start_minute,
            app.editing_date_field,
        )
    } else {
        Line::from(format!(
            "Start: {}-{}-{} {}:{}",
            app.start_year, app.start_month, app.start_day, app.start_hour, app.start_minute
        ))
    };
    let start_field = Paragraph::new(start_line).block(Block::default().borders(Borders::ALL));
    f.render_widget(start_field, form_chunks[0]);

    // End date field
    let end_is_editing = app.editing_field == Some(TimeframeField::EndDate);
    let end_line = if end_is_editing {
        build_date_line(
            "End:   ",
            &app.end_year,
            &app.end_month,
            &app.end_day,
            &app.end_hour,
            &app.end_minute,
            app.editing_date_field,
        )
    } else {
        Line::from(format!(
            "End:   {}-{}-{} {}:{}",
            app.end_year, app.end_month, app.end_day, app.end_hour, app.end_minute
        ))
    };
    let end_field = Paragraph::new(end_line).block(Block::default().borders(Borders::ALL));
    f.render_widget(end_field, form_chunks[1]);

    // Interval selector
    let interval_text = build_interval_text(app);
    let interval_style = if app.editing_field == Some(TimeframeField::Interval) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let interval_field = Paragraph::new(interval_text)
        .style(interval_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(interval_field, form_chunks[2]);

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from("Tab: Next Field/Component | Type: Edit | ↑↓: Change Interval"),
        Line::from("Backspace: Delete | Enter: Continue | Esc: Back"),
        Line::from("Date format: [YYYY]-[MM]-[DD] [HH]:[MM] (Tab through components)"),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

/// Build parameter lines for `MaCrossover` strategy in edit mode
fn build_ma_crossover_edit_lines(
    fast: usize,
    slow: usize,
    editing_field: super::ParamField,
    input_buffer: &str,
) -> Vec<Line<'static>> {
    use super::ParamField;

    let mut spans = vec![Span::raw("Fast: ")];
    if editing_field == ParamField::FastPeriod {
        let text = if input_buffer.is_empty() {
            format!("[{fast}_]")
        } else {
            format!("[{input_buffer}_]")
        };
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::raw(fast.to_string()));
    }

    spans.push(Span::raw(", Slow: "));
    if editing_field == ParamField::SlowPeriod {
        let text = if input_buffer.is_empty() {
            format!("[{slow}_]")
        } else {
            format!("[{input_buffer}_]")
        };
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::raw(slow.to_string()));
    }

    vec![Line::from(spans)]
}

/// Build parameter lines for `QuadMa` strategy in edit mode
#[allow(clippy::too_many_arguments)]
fn build_quad_ma_edit_lines(
    ma1: usize,
    ma2: usize,
    ma3: usize,
    ma4: usize,
    trend_period: usize,
    volume_factor: usize,
    take_profit: usize,
    stop_loss: usize,
    reversal_confirmation_bars: usize,
    editing_field: super::ParamField,
    input_buffer: &str,
) -> Vec<Line<'static>> {
    use super::ParamField;

    let params = [
        (ParamField::Ma1Period, ma1, "MA1: "),
        (ParamField::Ma2Period, ma2, ", MA2: "),
        (ParamField::Ma3Period, ma3, ", MA3: "),
        (ParamField::Ma4Period, ma4, ", MA4: "),
    ];

    let mut line1_spans = Vec::new();
    for (field, value, label) in &params {
        line1_spans.push(Span::raw(*label));
        if editing_field == *field {
            let text = if input_buffer.is_empty() {
                format!("[{value}_]")
            } else {
                format!("[{input_buffer}_]")
            };
            line1_spans.push(Span::styled(
                text,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            line1_spans.push(Span::raw(value.to_string()));
        }
    }

    let params2 = [
        (ParamField::TrendPeriod, trend_period, "Trend: "),
        (ParamField::VolumeFactor, volume_factor, ", Vol: "),
        (ParamField::TakeProfit, take_profit, ", TP: "),
        (ParamField::StopLoss, stop_loss, ", SL: "),
        (
            ParamField::ReversalConfirmBars,
            reversal_confirmation_bars,
            ", RevConf: ",
        ),
    ];

    let mut line2_spans = Vec::new();
    for (field, value, label) in &params2 {
        line2_spans.push(Span::raw(*label));
        if editing_field == *field {
            let text = if input_buffer.is_empty() {
                format!("[{value}_]")
            } else {
                format!("[{input_buffer}_]")
            };
            line2_spans.push(Span::styled(
                text,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            line2_spans.push(Span::raw(value.to_string()));
        }
    }

    vec![Line::from(line1_spans), Line::from(line2_spans)]
}

/// Build parameter list item for a configuration
fn build_param_list_item(i: usize, config: &super::ParamConfig, app: &App) -> ListItem<'static> {
    let is_selected = i == app.selected_param_index;
    let style = if is_selected {
        Style::default().bg(Color::Blue).fg(Color::White)
    } else {
        Style::default()
    };

    let params_lines = if is_selected && app.editing_param.is_some() {
        let editing_field = app.editing_param.unwrap();
        match &config.strategy {
            super::StrategyType::MaCrossover { fast, slow } => {
                build_ma_crossover_edit_lines(*fast, *slow, editing_field, &app.param_input_buffer)
            }
            super::StrategyType::QuadMa {
                ma1,
                ma2,
                ma3,
                ma4,
                trend_period,
                volume_factor,
                take_profit,
                stop_loss,
                reversal_confirmation_bars,
            } => build_quad_ma_edit_lines(
                *ma1,
                *ma2,
                *ma3,
                *ma4,
                *trend_period,
                *volume_factor,
                *take_profit,
                *stop_loss,
                *reversal_confirmation_bars,
                editing_field,
                &app.param_input_buffer,
            ),
        }
    } else {
        match &config.strategy {
            super::StrategyType::MaCrossover { fast, slow } => {
                vec![Line::from(format!("Fast: {fast}, Slow: {slow}"))]
            }
            super::StrategyType::QuadMa {
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
                    Line::from(format!("MA1: {ma1}, MA2: {ma2}, MA3: {ma3}, MA4: {ma4}")),
                    Line::from(format!("Trend: {trend_period}, Vol: {volume_factor}, TP: {take_profit}, SL: {stop_loss}, RevConf: {reversal_confirmation_bars}")),
                ]
            }
        }
    };

    let mut lines = vec![Line::from(Span::styled(
        config.name.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(params_lines);

    ListItem::new(lines).style(style)
}

fn render_parameter_config(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Config list
            Constraint::Length(6), // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!(
        "Parameter Configurations ({} configs)",
        app.param_configs.len()
    ))
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Config list
    let items: Vec<ListItem> = app
        .param_configs
        .iter()
        .enumerate()
        .map(|(i, config)| build_param_list_item(i, config, app))
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Configurations"),
    );
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
            Constraint::Length(3), // Title
            Constraint::Length(3), // Progress bar
            Constraint::Length(3), // Current backtest
            Constraint::Min(5),    // Status messages
            Constraint::Length(3), // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new("Running Backtests")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
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
    let status_lines: Vec<Line> = app
        .status_messages
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

/// Calculate Profit Factor from trade records
///
/// Profit Factor = Total Gross Profit / Total Gross Loss
/// Returns 999.0 for infinity case (all winning trades), 0.0 for no trades
fn calculate_profit_factor(trades: &[super::TradeRecord]) -> f64 {
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
        999.0 // All wins, no losses - display as "999+"
    } else {
        0.0 // No closed trades
    }
}

#[allow(clippy::too_many_lines)] // Table rendering with 12 columns requires comprehensive formatting
fn render_results(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Results table
            Constraint::Length(4), // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(format!("Results ({} backtests)", app.results.len()))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Results table
    let header_cells = [
        if app.sort_column == 0 {
            format!("Token {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Token".to_string()
        },
        if app.sort_column == 1 {
            format!("Config {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Config".to_string()
        },
        if app.sort_column == 2 {
            format!("Ret% {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Ret%".to_string()
        },
        if app.sort_column == 3 {
            format!("Ret$ {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Ret$".to_string()
        },
        if app.sort_column == 4 {
            format!("DD% {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "DD%".to_string()
        },
        if app.sort_column == 5 {
            format!("Shrp {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Shrp".to_string()
        },
        if app.sort_column == 6 {
            format!("#Tr {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "#Tr".to_string()
        },
        if app.sort_column == 7 {
            format!("Win% {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Win%".to_string()
        },
        if app.sort_column == 8 {
            format!("Fin$ {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Fin$".to_string()
        },
        if app.sort_column == 9 {
            format!("Peak$ {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "Peak$".to_string()
        },
        if app.sort_column == 10 {
            format!("B&H% {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "B&H%".to_string()
        },
        if app.sort_column == 11 {
            format!("PF {}", if app.sort_ascending { "↑" } else { "↓" })
        } else {
            "PF".to_string()
        },
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

            // Get extended metrics from embedded PerformanceMetrics
            let (return_dollars, final_capital, equity_peak, buy_hold_pct) =
                result.metrics.as_ref().map_or_else(
                    || {
                        (
                            "—".to_string(),
                            "—".to_string(),
                            "—".to_string(),
                            "—".to_string(),
                        )
                    },
                    |metrics| {
                        let ret_dollars = metrics.final_capital - metrics.initial_capital;
                        (
                            format!("{:.2}", ret_dollars.to_f64().unwrap_or(0.0)),
                            format!("{:.2}", metrics.final_capital.to_f64().unwrap_or(0.0)),
                            format!("{:.2}", metrics.equity_peak.to_f64().unwrap_or(0.0)),
                            format!(
                                "{:.2}",
                                metrics.buy_hold_return.to_f64().unwrap_or(0.0) * 100.0
                            ),
                        )
                    },
                );

            // Calculate Profit Factor from trades
            let profit_factor = calculate_profit_factor(&result.trades);
            let pf_str = if profit_factor >= 999.0 {
                "999+".to_string()
            } else if result.num_trades == 0 {
                "—".to_string()
            } else {
                format!("{profit_factor:.2}")
            };

            Row::new(vec![
                result.token.clone(),
                result.config_name.clone(),
                format!("{:.2}", result.total_return.to_f64().unwrap_or(0.0) * 100.0),
                return_dollars,
                format!("{:.2}", result.max_drawdown.to_f64().unwrap_or(0.0) * 100.0),
                format!("{:.2}", result.sharpe_ratio),
                format!("{}", result.num_trades),
                format!("{:.1}", result.win_rate * 100.0),
                final_capital,
                equity_peak,
                buy_hold_pct,
                pf_str,
            ])
            .style(Style::default().fg(return_color))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),  // Token
            Constraint::Length(12), // Config
            Constraint::Length(7),  // Ret%
            Constraint::Length(9),  // Ret$
            Constraint::Length(7),  // DD%
            Constraint::Length(6),  // Shrp
            Constraint::Length(5),  // #Tr
            Constraint::Length(6),  // Win%
            Constraint::Length(9),  // Fin$
            Constraint::Length(9),  // Peak$
            Constraint::Length(7),  // B&H%
            Constraint::Length(6),  // PF
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Backtest Results"),
    );

    f.render_widget(table, chunks[1]);

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from(
            "↑↓: Scroll | Enter: View Trades | m: View Metrics | s: Change Sort | r: Reverse Sort",
        ),
        Line::from("b: Back to Start | q: Quit"),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}

#[allow(clippy::too_many_lines)] // Trade detail table with position-aware coloring requires complex logic
fn render_trade_detail(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Trades table
            Constraint::Length(3), // Instructions
        ])
        .split(f.area());

    // Get selected result
    let result = app
        .selected_result_index
        .and_then(|idx| app.results.get(idx));

    let title_text = result.map_or_else(
        || "Trade History".to_string(),
        |r| {
            format!(
                "Trade History: {} - {} ({} trades)",
                r.token,
                r.config_name,
                r.trades.len()
            )
        },
    );

    // Title
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Trades table
    if let Some(r) = result {
        let header = Row::new(vec![
            "Timestamp",
            "Action",
            "Price",
            "Quantity",
            "Value (USDC)",
            "PnL",
            "Commission",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

        let rows: Vec<Row> = r
            .trades
            .iter()
            .skip(app.trade_scroll_offset)
            .map(|trade| {
                // Color based on action type
                let action_color = if trade.action.contains("LONG") {
                    if trade.action.starts_with("CLOSE") {
                        Color::Blue // Closing long
                    } else {
                        Color::Green // Open/Add long
                    }
                } else if trade.action.contains("SHORT") {
                    if trade.action.starts_with("CLOSE") {
                        Color::Blue // Closing short
                    } else {
                        Color::Red // Open/Add short
                    }
                } else {
                    Color::White
                };

                // Format PnL with color
                let (pnl_text, pnl_color) = trade.pnl.map_or_else(
                    || ("—".to_string(), Color::Gray),
                    |pnl_value| {
                        let pnl_f64 = pnl_value.to_f64().unwrap_or(0.0);
                        if pnl_f64 > 0.0 {
                            (format!("+${pnl_f64:.2}"), Color::Green)
                        } else if pnl_f64 < 0.0 {
                            (format!("-${:.2}", pnl_f64.abs()), Color::Red)
                        } else {
                            ("$0.00".to_string(), Color::White)
                        }
                    },
                );

                Row::new(vec![
                    Span::styled(
                        trade.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                        Style::default().fg(action_color),
                    ),
                    Span::styled(
                        trade.action.clone(),
                        Style::default()
                            .fg(action_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("${:.2}", trade.price.to_f64().unwrap_or(0.0)),
                        Style::default().fg(action_color),
                    ),
                    Span::styled(
                        format!("{:.8}", trade.quantity.to_f64().unwrap_or(0.0)),
                        Style::default().fg(action_color),
                    ),
                    Span::styled(
                        format!("${:.2}", trade.position_value.to_f64().unwrap_or(0.0)),
                        Style::default().fg(action_color),
                    ),
                    Span::styled(
                        pnl_text,
                        Style::default().fg(pnl_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("${:.4}", trade.commission.to_f64().unwrap_or(0.0)),
                        Style::default().fg(action_color),
                    ),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Length(14),
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

#[allow(clippy::too_many_lines)] // Metrics detail view with comprehensive performance statistics
fn render_metrics_detail(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Metrics content
            Constraint::Length(3), // Instructions
        ])
        .split(f.area());

    // Get selected result
    let result = app
        .selected_result_index
        .and_then(|idx| app.results.get(idx));

    let title_text = result.map_or_else(
        || "Performance Metrics".to_string(),
        |r| format!("Performance Metrics: {} - {}", r.token, r.config_name),
    );

    // Title
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Metrics content
    if let Some(r) = result {
        if let Some(metrics) = &r.metrics {
            // Format metrics similar to CLI MetricsFormatter
            let duration_days = metrics.duration.num_days();
            let duration_hours = metrics.duration.num_hours() % 24;

            let exposure_pct = metrics.exposure_time * 100.0;

            let total_return_pct = metrics.total_return.to_f64().unwrap_or(0.0) * 100.0;
            let buy_hold_pct = metrics.buy_hold_return.to_f64().unwrap_or(0.0) * 100.0;
            let max_dd_pct = metrics.max_drawdown.to_f64().unwrap_or(0.0) * 100.0;

            let metrics_text = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Duration: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{duration_days} days {duration_hours} hours")),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Exposure Time: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{exposure_pct:.1}%")),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Initial Capital: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        "${:.2}",
                        metrics.initial_capital.to_f64().unwrap_or(0.0)
                    )),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Final Capital: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        "${:.2}",
                        metrics.final_capital.to_f64().unwrap_or(0.0)
                    )),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Equity Peak: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        "${:.2}",
                        metrics.equity_peak.to_f64().unwrap_or(0.0)
                    )),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Total Return: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{total_return_pct:+.2}%"),
                        if total_return_pct >= 0.0 {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Buy & Hold: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{buy_hold_pct:+.2}%"),
                        if buy_hold_pct >= 0.0 {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Sharpe Ratio: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{:.2}", metrics.sharpe_ratio)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Max Drawdown: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{max_dd_pct:.2}%"), Style::default().fg(Color::Red)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Total Trades: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{}", metrics.num_trades)),
                ]),
                Line::from(vec![
                    Span::styled("Win Rate: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{:.1}%", metrics.win_rate * 100.0)),
                ]),
            ];

            let metrics_para = Paragraph::new(metrics_text)
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Left);
            f.render_widget(metrics_para, chunks[1]);
        } else {
            let empty = Paragraph::new("No metrics data available")
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(empty, chunks[1]);
        }
    } else {
        let empty = Paragraph::new("No result selected")
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(empty, chunks[1]);
    }

    // Instructions
    let instructions = Paragraph::new("Esc: Back to Results")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(instructions, chunks[2]);
}
