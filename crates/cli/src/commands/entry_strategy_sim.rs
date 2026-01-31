//! Entry strategy simulation CLI command.
//!
//! Simulates different entry timing strategies using real historical
//! price data to find optimal entry timing within betting windows.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::info;

use algo_trade_backtest::binary::{
    BetDirection, EdgeThresholdEntry, EntryContext, EntryDecision, EntryStrategy,
    FixedTimeEntry, PricePath, PricePoint,
};
use algo_trade_data::{OhlcvRecord, OhlcvRepository, SignalSnapshotRepository};

/// Arguments for the entry-strategy-sim command.
#[derive(Args, Debug, Clone)]
pub struct EntryStrategySimArgs {
    /// Start date for simulation (ISO 8601 format)
    #[arg(long)]
    pub start: String,

    /// End date for simulation (ISO 8601 format)
    #[arg(long)]
    pub end: String,

    /// Signal to use for probability estimation
    #[arg(long, default_value = "funding_rate")]
    pub signal: String,

    /// Minimum signal strength threshold
    #[arg(long, default_value = "0.3")]
    pub min_strength: f64,

    /// Symbol for price data (default: BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// Exchange for price data (default: binance)
    #[arg(long, default_value = "binance")]
    pub exchange: String,

    /// Window duration in minutes (default: 15)
    #[arg(long, default_value = "15")]
    pub window_minutes: i64,

    /// Fee rate as decimal (default: 0.02 = 2%)
    #[arg(long, default_value = "0.02")]
    pub fee_rate: f64,

    /// Database connection URL
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// Use simulated price paths instead of real data
    #[arg(long)]
    pub simulated: bool,

    /// Number of simulated paths per window (default: 1000)
    #[arg(long, default_value = "1000")]
    pub n_paths: usize,

    /// Output format: text, json (default: text)
    #[arg(long, default_value = "text")]
    pub format: String,
}

/// Entry strategy comparison result for a single window.
#[derive(Debug, Clone)]
struct WindowResult {
    timestamp: DateTime<Utc>,
    signal_direction: BetDirection,
    signal_strength: Decimal,
    signal_probability: Decimal,
    open_price: Decimal,
    close_price: Decimal,
    /// Results for each strategy
    strategy_results: Vec<StrategyWindowResult>,
}

#[derive(Debug, Clone)]
struct StrategyWindowResult {
    strategy_name: String,
    entered: bool,
    entry_offset_mins: Option<f64>,
    entry_price: Option<Decimal>,
    edge_at_entry: Option<Decimal>,
    price_improvement: Option<Decimal>,
    outcome_correct: bool,
}

/// Aggregate statistics across all windows.
#[derive(Debug, Clone)]
struct AggregateStats {
    strategy_name: String,
    total_windows: usize,
    windows_entered: usize,
    entry_rate: f64,
    avg_edge_at_entry: Option<Decimal>,
    avg_price_improvement: Option<Decimal>,
    wins: usize,
    win_rate: f64,
    total_edge: Decimal,
}

/// Run the entry strategy simulation command.
pub async fn run_entry_strategy_sim(args: EntryStrategySimArgs) -> Result<()> {
    let start: DateTime<Utc> = args
        .start
        .parse()
        .map_err(|e| anyhow!("Invalid start date: {}", e))?;
    let end: DateTime<Utc> = args
        .end
        .parse()
        .map_err(|e| anyhow!("Invalid end date: {}", e))?;

    info!(
        "Running entry strategy simulation from {} to {}",
        start.format("%Y-%m-%d %H:%M"),
        end.format("%Y-%m-%d %H:%M")
    );
    info!("Signal: {}", args.signal);
    info!("Min strength: {:.1}%", args.min_strength * 100.0);
    info!("Window: {} minutes", args.window_minutes);

    let db_url = args
        .db_url
        .clone()
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    let pool = sqlx::PgPool::connect(&db_url).await?;
    info!("Connected to database");

    // Load signal snapshots
    let signal_repo = SignalSnapshotRepository::new(pool.clone());
    let signals = signal_repo
        .query_by_signal_name(&args.signal, start, end)
        .await?;

    info!("Found {} signal snapshots", signals.len());

    // Filter by strength and direction
    let min_strength = Decimal::try_from(args.min_strength)?;
    let filtered_signals: Vec<_> = signals
        .into_iter()
        .filter(|s| {
            s.strength >= min_strength
                && (s.direction == "up" || s.direction == "down")
        })
        .collect();

    info!(
        "After filtering: {} signals with strength >= {:.1}%",
        filtered_signals.len(),
        args.min_strength * 100.0
    );

    if filtered_signals.is_empty() {
        println!("\nNo signals met the criteria. Try lowering --min-strength.");
        return Ok(());
    }

    // Load OHLCV data for price paths
    let ohlcv_repo = OhlcvRepository::new(pool.clone());

    // Define strategies to compare
    let strategies: Vec<Box<dyn EntryStrategy>> = vec![
        Box::new(FixedTimeEntry::new(
            Duration::zero(),
            Duration::minutes(args.window_minutes),
        )),
        Box::new(FixedTimeEntry::new(
            Duration::minutes(args.window_minutes / 4),
            Duration::minutes(args.window_minutes),
        )),
        Box::new(FixedTimeEntry::new(
            Duration::minutes(args.window_minutes / 2),
            Duration::minutes(args.window_minutes),
        )),
        Box::new(EdgeThresholdEntry::new(
            dec!(0.03),
            Duration::minutes(args.window_minutes - 1),
        )),
        Box::new(EdgeThresholdEntry::new(
            dec!(0.05),
            Duration::minutes(args.window_minutes - 1),
        )),
        Box::new(EdgeThresholdEntry::new(
            dec!(0.08),
            Duration::minutes(args.window_minutes - 1),
        )),
    ];

    let strategy_names = vec![
        "Immediate (t=0)",
        "Early (t=25%)",
        "Midpoint (t=50%)",
        "Edge >= 3%",
        "Edge >= 5%",
        "Edge >= 8%",
    ];

    let fee_rate = Decimal::try_from(args.fee_rate)?;
    let window_duration = Duration::minutes(args.window_minutes);

    let mut all_window_results: Vec<WindowResult> = Vec::new();

    // Process each signal window
    for signal in &filtered_signals {
        let window_start = signal.timestamp;
        let window_end = window_start + window_duration;

        // Get OHLCV candles for this window
        let candles = ohlcv_repo
            .query_by_time_range(&args.symbol, &args.exchange, window_start, window_end)
            .await?;

        if candles.is_empty() {
            continue;
        }

        // Build price path from real candles
        let price_path = build_price_path_from_candles(&candles, window_start, window_duration);

        if price_path.points.len() < 2 {
            continue;
        }

        let direction = match signal.direction.as_str() {
            "up" => BetDirection::Yes,
            "down" => BetDirection::No,
            _ => continue,
        };

        // Signal probability: convert strength to probability estimate
        // Higher strength = higher confidence in the direction
        let signal_probability = dec!(0.5) + signal.strength * dec!(0.2);

        // Determine if the signal was correct (price moved in predicted direction)
        let outcome_correct = match direction {
            BetDirection::Yes => price_path.close_price > price_path.open_price,
            BetDirection::No => price_path.close_price < price_path.open_price,
        };

        let mut strategy_results = Vec::new();

        // Evaluate each strategy on this window
        for (i, strategy) in strategies.iter().enumerate() {
            let result = evaluate_strategy_on_path(
                strategy.as_ref(),
                &price_path,
                signal_probability,
                direction,
                fee_rate,
            );

            let price_improvement = if result.entered {
                result.entry_price.map(|ep| {
                    match direction {
                        BetDirection::Yes => price_path.open_price - ep, // Lower entry = better for Yes
                        BetDirection::No => ep - price_path.open_price,  // Higher entry = better for No
                    }
                })
            } else {
                None
            };

            strategy_results.push(StrategyWindowResult {
                strategy_name: strategy_names[i].to_string(),
                entered: result.entered,
                entry_offset_mins: result.entry_offset.map(|d| d.num_seconds() as f64 / 60.0),
                entry_price: result.entry_price,
                edge_at_entry: result.edge_at_entry,
                price_improvement,
                outcome_correct,
            });
        }

        all_window_results.push(WindowResult {
            timestamp: signal.timestamp,
            signal_direction: direction,
            signal_strength: signal.strength,
            signal_probability,
            open_price: price_path.open_price,
            close_price: price_path.close_price,
            strategy_results,
        });
    }

    if all_window_results.is_empty() {
        println!("\nNo windows had sufficient price data for simulation.");
        return Ok(());
    }

    // Compute aggregate statistics
    let aggregate_stats = compute_aggregate_stats(&all_window_results, &strategy_names);

    // Display results
    print_results(&all_window_results, &aggregate_stats, &args);

    Ok(())
}

/// Result of evaluating a strategy on a price path.
struct StrategyEvalResult {
    entered: bool,
    entry_offset: Option<Duration>,
    entry_price: Option<Decimal>,
    edge_at_entry: Option<Decimal>,
}

/// Evaluate a single strategy on a price path.
fn evaluate_strategy_on_path(
    strategy: &dyn EntryStrategy,
    path: &PricePath,
    estimated_probability: Decimal,
    direction: BetDirection,
    fee_rate: Decimal,
) -> StrategyEvalResult {
    for point in &path.points {
        let ctx = EntryContext::new(
            point.price,
            estimated_probability,
            point.offset,
            path.duration,
            direction,
            fee_rate,
        );

        let decision = strategy.evaluate(&ctx);

        if let EntryDecision::Enter { offset, .. } = decision {
            return StrategyEvalResult {
                entered: true,
                entry_offset: Some(offset),
                entry_price: Some(point.price),
                edge_at_entry: Some(ctx.calculate_edge()),
            };
        }
    }

    StrategyEvalResult {
        entered: false,
        entry_offset: None,
        entry_price: None,
        edge_at_entry: None,
    }
}

/// Build a price path from OHLCV candles.
fn build_price_path_from_candles(
    candles: &[OhlcvRecord],
    window_start: DateTime<Utc>,
    window_duration: Duration,
) -> PricePath {
    let mut points = Vec::new();

    for candle in candles {
        let offset = candle.timestamp - window_start;
        if offset < Duration::zero() || offset > window_duration {
            continue;
        }

        // Use close price as the price at each minute
        let price = candle.close;
        points.push(PricePoint::new(offset, price));
    }

    // Sort by offset
    points.sort_by_key(|p| p.offset);

    // Normalize prices to 0-1 range for binary market simulation
    // We'll use price movements relative to open as a proxy for market odds
    if !points.is_empty() {
        let base_price = points[0].price;
        let max_movement = dec!(0.02); // 2% max price movement maps to full odds swing

        for point in &mut points {
            let movement = (point.price - base_price) / base_price;
            // Map to 0.3-0.7 range (reasonable binary market odds)
            let normalized = dec!(0.5) + (movement / max_movement) * dec!(0.2);
            point.price = normalized.max(dec!(0.30)).min(dec!(0.70));
        }
    }

    PricePath::new(points)
}

/// Compute aggregate statistics across all windows.
fn compute_aggregate_stats(
    results: &[WindowResult],
    strategy_names: &[&str],
) -> Vec<AggregateStats> {
    strategy_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let total_windows = results.len();
            let mut windows_entered = 0;
            let mut total_edge = Decimal::ZERO;
            let mut total_improvement = Decimal::ZERO;
            let mut wins = 0;
            let mut edge_count = 0;
            let mut improvement_count = 0;

            for window in results {
                let sr = &window.strategy_results[i];
                if sr.entered {
                    windows_entered += 1;
                    if let Some(edge) = sr.edge_at_entry {
                        total_edge += edge;
                        edge_count += 1;
                    }
                    if let Some(imp) = sr.price_improvement {
                        total_improvement += imp;
                        improvement_count += 1;
                    }
                    if sr.outcome_correct {
                        wins += 1;
                    }
                }
            }

            let entry_rate = if total_windows > 0 {
                windows_entered as f64 / total_windows as f64
            } else {
                0.0
            };

            let win_rate = if windows_entered > 0 {
                wins as f64 / windows_entered as f64
            } else {
                0.0
            };

            let avg_edge = if edge_count > 0 {
                Some(total_edge / Decimal::from(edge_count))
            } else {
                None
            };

            let avg_improvement = if improvement_count > 0 {
                Some(total_improvement / Decimal::from(improvement_count))
            } else {
                None
            };

            AggregateStats {
                strategy_name: name.to_string(),
                total_windows,
                windows_entered,
                entry_rate,
                avg_edge_at_entry: avg_edge,
                avg_price_improvement: avg_improvement,
                wins,
                win_rate,
                total_edge,
            }
        })
        .collect()
}

/// Print simulation results.
fn print_results(
    results: &[WindowResult],
    stats: &[AggregateStats],
    args: &EntryStrategySimArgs,
) {
    println!();
    println!("===============================================================");
    println!("              ENTRY STRATEGY SIMULATION RESULTS                ");
    println!("===============================================================");
    println!("Signal: {}", args.signal);
    println!("Windows analyzed: {}", results.len());
    println!("Window duration: {} minutes", args.window_minutes);
    println!("Fee rate: {:.1}%", args.fee_rate * 100.0);
    println!();

    println!("STRATEGY COMPARISON");
    println!("---------------------------------------------------------------");
    println!(
        "{:<20} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "Strategy", "Entries", "Entry %", "Win %", "Avg Edge", "Avg Impr"
    );
    println!("{}", "-".repeat(70));

    let mut best_edge: Option<(String, Decimal)> = None;
    let mut best_win_rate: Option<(String, f64)> = None;

    for stat in stats {
        let edge_str = stat
            .avg_edge_at_entry
            .map(|e| format!("{:.2}%", e * dec!(100)))
            .unwrap_or_else(|| "N/A".to_string());

        let impr_str = stat
            .avg_price_improvement
            .map(|e| format!("{:.4}", e))
            .unwrap_or_else(|| "N/A".to_string());

        println!(
            "{:<20} {:>8} {:>9.1}% {:>9.1}% {:>10} {:>10}",
            stat.strategy_name,
            stat.windows_entered,
            stat.entry_rate * 100.0,
            stat.win_rate * 100.0,
            edge_str,
            impr_str,
        );

        // Track best strategies
        if let Some(edge) = stat.avg_edge_at_entry {
            if best_edge.is_none() || edge > best_edge.as_ref().unwrap().1 {
                best_edge = Some((stat.strategy_name.clone(), edge));
            }
        }
        if stat.windows_entered > 0 {
            if best_win_rate.is_none() || stat.win_rate > best_win_rate.as_ref().unwrap().1 {
                best_win_rate = Some((stat.strategy_name.clone(), stat.win_rate));
            }
        }
    }

    println!();
    println!("KEY INSIGHTS");
    println!("---------------------------------------------------------------");

    if let Some((name, edge)) = best_edge {
        println!(
            "Highest Avg Edge:    {} ({:.2}%)",
            name,
            edge * dec!(100)
        );
    }

    if let Some((name, wr)) = best_win_rate {
        println!("Highest Win Rate:    {} ({:.1}%)", name, wr * 100.0);
    }

    // Compare immediate vs best edge-threshold
    let immediate = stats.first();
    let best_threshold = stats.iter().skip(3).max_by(|a, b| {
        a.avg_edge_at_entry
            .unwrap_or(Decimal::ZERO)
            .cmp(&b.avg_edge_at_entry.unwrap_or(Decimal::ZERO))
    });

    if let (Some(imm), Some(thresh)) = (immediate, best_threshold) {
        if let (Some(imm_edge), Some(thresh_edge)) =
            (imm.avg_edge_at_entry, thresh.avg_edge_at_entry)
        {
            let improvement = thresh_edge - imm_edge;
            println!();
            println!(
                "Edge improvement from waiting: {:.2}% (from {:.2}% to {:.2}%)",
                improvement * dec!(100),
                imm_edge * dec!(100),
                thresh_edge * dec!(100)
            );
        }

        println!(
            "Trade-off: {} entry rate vs {} entry rate",
            format!("{:.0}%", imm.entry_rate * 100.0),
            format!("{:.0}%", thresh.entry_rate * 100.0),
        );
    }

    println!();
    println!("RECOMMENDATION");
    println!("---------------------------------------------------------------");

    // Find best overall strategy (balance of edge and entry rate)
    let best_overall = stats
        .iter()
        .filter(|s| s.windows_entered > 0)
        .max_by(|a, b| {
            let score_a = a.avg_edge_at_entry.unwrap_or(Decimal::ZERO)
                * Decimal::try_from(a.entry_rate).unwrap_or(Decimal::ZERO);
            let score_b = b.avg_edge_at_entry.unwrap_or(Decimal::ZERO)
                * Decimal::try_from(b.entry_rate).unwrap_or(Decimal::ZERO);
            score_a.cmp(&score_b)
        });

    if let Some(best) = best_overall {
        println!(
            "Best balanced strategy: {} (edge * entry rate)",
            best.strategy_name
        );
        println!(
            "  - Entry rate: {:.1}%",
            best.entry_rate * 100.0
        );
        println!(
            "  - Avg edge: {:.2}%",
            best.avg_edge_at_entry.unwrap_or(Decimal::ZERO) * dec!(100)
        );
        println!(
            "  - Win rate: {:.1}%",
            best.win_rate * 100.0
        );
    }

    println!();
}
