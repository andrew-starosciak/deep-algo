//! CLI command for Gabagool-style hybrid arbitrage monitoring.
//!
//! This command connects to Binance (BTC spot) and Polymarket (order books)
//! to detect hybrid arbitrage opportunities using the gabagool strategy:
//! - Entry: Wait for cheap side + spot confirmation
//! - Hedge: Lock in pair arbitrage when both sides are cheap
//! - Scratch: Exit before expiry if unhedged

use algo_trade_polymarket::arbitrage::{
    GabagoolConfig, GabagoolDirection, GabagoolRunner, GabagoolRunnerConfig, GabagoolSignal,
    GabagoolSignalType,
};
use algo_trade_polymarket::{Coin, GammaClient};
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{error, info, warn};

/// Arguments for the gabagool monitor command.
#[derive(Args, Debug)]
pub struct GabagoolMonitorArgs {
    /// Duration to run in minutes (default: 15 for one market window).
    #[arg(short, long, default_value = "15")]
    pub duration_mins: u64,

    /// Maximum price to consider "cheap" for entry (e.g., 0.42).
    /// Entry only happens when one side is below this threshold.
    #[arg(long, default_value = "0.42")]
    pub cheap_threshold: f64,

    /// Minimum delta vs window reference (percent, e.g., 0.03 = 0.03%).
    /// BTC must move this much from "price to beat" to confirm direction.
    #[arg(long, default_value = "0.03")]
    pub min_delta_pct: f64,

    /// Maximum pair cost for hedge (e.g., 0.97).
    /// If YES + NO < this, we can lock in guaranteed profit.
    #[arg(long, default_value = "0.97")]
    pub pair_cost_threshold: f64,

    /// Time before window close to scratch (seconds).
    #[arg(long, default_value = "180")]
    pub scratch_time_secs: i64,

    /// Maximum loss to accept for scratch (e.g., 0.02).
    #[arg(long, default_value = "0.02")]
    pub scratch_loss_limit: f64,

    /// Minimum time into window before signaling (seconds).
    #[arg(long, default_value = "30")]
    pub min_elapsed_secs: u64,

    /// Check interval in milliseconds.
    #[arg(long, default_value = "100")]
    pub check_interval_ms: u64,

    /// Use aggressive config.
    #[arg(long)]
    pub aggressive: bool,

    /// Use conservative config.
    #[arg(long)]
    pub conservative: bool,

    /// Path to export signal history (JSONL format).
    #[arg(long)]
    pub export_history: Option<PathBuf>,

    /// Show verbose output (every check, not just signals).
    #[arg(short, long)]
    pub verbose: bool,
}

/// Runs the gabagool hybrid arbitrage monitor.
pub async fn run(args: GabagoolMonitorArgs) -> Result<()> {
    info!("=== Gabagool Hybrid Arbitrage Monitor ===");
    info!(
        "Duration: {} min | Cheap threshold: ${} | Pair threshold: ${}",
        args.duration_mins, args.cheap_threshold, args.pair_cost_threshold
    );

    // Build gabagool config
    let detector_config = if args.aggressive {
        info!("Using AGGRESSIVE config");
        GabagoolConfig::aggressive()
    } else if args.conservative {
        info!("Using CONSERVATIVE config");
        GabagoolConfig::conservative()
    } else {
        GabagoolConfig {
            cheap_threshold: Decimal::try_from(args.cheap_threshold)?,
            min_reference_delta: args.min_delta_pct / 100.0,
            pair_cost_threshold: Decimal::try_from(args.pair_cost_threshold)?,
            scratch_time_secs: args.scratch_time_secs,
            scratch_loss_limit: Decimal::try_from(args.scratch_loss_limit)?,
            min_window_elapsed_ms: (args.min_elapsed_secs * 1000) as i64,
            ..GabagoolConfig::default()
        }
    };

    info!(
        "Config: cheap<${} | delta>{:.3}% | pair<${} | scratch@{}s",
        detector_config.cheap_threshold,
        detector_config.min_reference_delta * 100.0,
        detector_config.pair_cost_threshold,
        detector_config.scratch_time_secs
    );

    // Fetch current BTC 15-min market
    info!("Fetching active BTC 15-minute market from Gamma API...");
    let gamma_client = GammaClient::new();
    let market = gamma_client.get_current_15min_market(Coin::Btc).await?;

    let end_time = market
        .end_date
        .map(|d| d.format("%H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    info!("Found market: {} (expires {})", market.question, end_time);

    // Get token IDs
    let yes_token = market
        .up_token()
        .ok_or_else(|| anyhow::anyhow!("Market missing UP token"))?;
    let no_token = market
        .down_token()
        .ok_or_else(|| anyhow::anyhow!("Market missing DOWN token"))?;

    info!("YES token: {}", yes_token.token_id);
    info!("NO token: {}", no_token.token_id);

    // Create runner config
    let runner_config = GabagoolRunnerConfig {
        yes_token_id: yes_token.token_id.clone(),
        no_token_id: no_token.token_id.clone(),
        detector_config,
        check_interval_ms: args.check_interval_ms,
        history_export_path: args.export_history.clone(),
        ..Default::default()
    };

    // Create runner
    let (runner, mut signal_rx) = GabagoolRunner::new(runner_config);
    let stop_handle = runner.stop_handle();
    let stats = runner.stats();
    let history = runner.history();

    // Spawn runner
    let runner_handle = tokio::spawn(async move {
        if let Err(e) = runner.run().await {
            error!("Runner error: {}", e);
        }
    });

    // Set up Ctrl+C handler
    let stop_on_ctrl_c = stop_handle.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received Ctrl+C, stopping...");
            stop_on_ctrl_c.store(true, Ordering::SeqCst);
        }
    });

    // Collect signals
    let duration = Duration::from_secs(args.duration_mins * 60);
    let deadline = tokio::time::Instant::now() + duration;
    let mut signals: Vec<GabagoolSignal> = Vec::new();

    info!("");
    info!("Monitoring for gabagool signals...");
    info!("   Entry: cheap side + spot confirms direction");
    info!("   Hedge: both sides cheap enough to lock profit");
    info!("   Scratch: exit before expiry if unhedged");
    info!("   Press Ctrl+C to stop early");
    info!("");

    // Status update interval
    let mut last_status = tokio::time::Instant::now();
    let status_interval = Duration::from_secs(10);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                info!("Duration elapsed");
                break;
            }
            signal = signal_rx.recv() => {
                match signal {
                    Some(sig) => {
                        print_signal(&sig);
                        signals.push(sig);
                    }
                    None => {
                        warn!("Signal channel closed");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                // Periodic status update
                if args.verbose && last_status.elapsed() >= status_interval {
                    let s = stats.read().await;

                    if let (Some(spot), Some(yes), Some(no), Some(ref_price)) = (
                        s.current_spot_price,
                        s.current_yes_ask,
                        s.current_no_ask,
                        s.current_reference_price,
                    ) {
                        let delta_pct = ((spot - ref_price) / ref_price) * 100.0;
                        let pair_cost = yes + no;
                        info!(
                            "Status: BTC ${:.2} vs ref ${:.2} ({:+.3}%) | YES ${} | NO ${} | Pair ${} | Checks: {}",
                            spot,
                            ref_price,
                            delta_pct,
                            yes,
                            no,
                            pair_cost,
                            s.checks_performed
                        );
                    }
                    last_status = tokio::time::Instant::now();
                }
            }
        }

        if stop_handle.load(Ordering::SeqCst) {
            break;
        }
    }

    // Stop runner
    stop_handle.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(Duration::from_secs(5), runner_handle).await;

    // Print summary
    println!();
    println!("=== Session Summary ===");

    let final_stats = stats.read().await;
    println!("Duration: {} minutes", args.duration_mins);
    println!("Checks performed: {}", final_stats.checks_performed);
    println!("Windows processed: {}", final_stats.windows_processed);
    println!();
    println!("Signals:");
    println!("  - Entry: {} (YES: {}, NO: {})",
        final_stats.entry_signals,
        final_stats.yes_entries,
        final_stats.no_entries
    );
    println!("  - Hedge: {}", final_stats.hedge_signals);
    println!("  - Scratch: {}", final_stats.scratch_signals);
    println!("  - Total: {}", final_stats.total_signals());

    // Signal details
    if !signals.is_empty() {
        println!();
        println!("=== Signal Details ===");
        for (i, sig) in signals.iter().enumerate() {
            let signal_type = match sig.signal_type {
                GabagoolSignalType::Entry => "ENTRY",
                GabagoolSignalType::Hedge => "HEDGE",
                GabagoolSignalType::Scratch => "SCRATCH",
            };
            let direction = match sig.direction {
                GabagoolDirection::Yes => "YES",
                GabagoolDirection::No => "NO",
            };
            println!(
                "{}. [{}] {} @ ${} | Spot {:+.3}% vs ref | Pair ${} | {}s left",
                i + 1,
                signal_type,
                direction,
                sig.entry_price,
                sig.spot_delta_pct * 100.0,
                sig.current_pair_cost,
                sig.time_remaining_secs
            );
        }
    }

    // History export info
    if let Some(ref path) = args.export_history {
        let history_len = history.read().await.len();
        if history_len > 0 {
            println!();
            println!("Signal history exported to: {}", path.display());
            println!("  Records: {}", history_len);
        }
    }

    // Analysis
    if !signals.is_empty() {
        println!();
        println!("=== Strategy Analysis ===");

        let entry_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.signal_type == GabagoolSignalType::Entry)
            .collect();
        let hedge_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.signal_type == GabagoolSignalType::Hedge)
            .collect();

        if !entry_signals.is_empty() {
            let avg_entry: f64 = entry_signals
                .iter()
                .map(|s| s.entry_price.to_string().parse::<f64>().unwrap_or(0.35))
                .sum::<f64>()
                / entry_signals.len() as f64;

            println!("Entry signals: {}", entry_signals.len());
            println!("  Avg entry price: ${:.3}", avg_entry);

            // If we had hedges, calculate locked profit
            if !hedge_signals.is_empty() {
                let avg_hedge: f64 = hedge_signals
                    .iter()
                    .map(|s| s.entry_price.to_string().parse::<f64>().unwrap_or(0.60))
                    .sum::<f64>()
                    / hedge_signals.len() as f64;

                let avg_pair_cost = avg_entry + avg_hedge;
                let locked_profit = 1.0 - avg_pair_cost;

                println!("Hedge signals: {}", hedge_signals.len());
                println!("  Avg hedge price: ${:.3}", avg_hedge);
                println!("  Avg pair cost: ${:.3}", avg_pair_cost);
                println!("  Locked profit per pair: ${:.3} ({:.1}%)", locked_profit, locked_profit * 100.0);
            }
        }

        // Theoretical P&L
        println!();
        println!("=== Theoretical P&L (95% win rate assumption) ===");
        let trades = entry_signals.len();
        if trades > 0 {
            let avg_entry: f64 = entry_signals
                .iter()
                .map(|s| s.entry_price.to_string().parse::<f64>().unwrap_or(0.35))
                .sum::<f64>()
                / trades as f64;

            let wins = (trades as f64 * 0.95).round() as usize;
            let losses = trades - wins;
            let profit_per_win = 50.0 * ((1.0 - avg_entry) / avg_entry);
            let loss_per_loss = 50.0;
            let net = (wins as f64 * profit_per_win) - (losses as f64 * loss_per_loss);

            println!("Trades: {} | Wins: {} | Losses: {}", trades, wins, losses);
            println!("Avg entry: ${:.3} | Profit/win: ${:.2} | Loss/loss: ${:.2}", avg_entry, profit_per_win, loss_per_loss);
            println!("Theoretical net P&L ($50/trade): ${:.2}", net);
        }
    } else {
        println!();
        println!("No signals detected during monitoring period.");
        println!("This could mean:");
        println!("  - No side dropped below ${:.2}", args.cheap_threshold);
        println!("  - BTC didn't confirm direction (need {:.2}%+ delta)", args.min_delta_pct);
        println!("  - Market is efficiently priced");
        println!("  - Try during higher volatility periods");
        if final_stats.checks_performed == 0 {
            println!();
            println!("WARNING: No checks were performed!");
            println!("  - Check that Binance WebSocket is connecting");
            println!("  - Check that Polymarket order books are available");
            println!("  - Run with RUST_LOG=info for more details");
        }
    }

    Ok(())
}

/// Prints a signal to stdout.
fn print_signal(signal: &GabagoolSignal) {
    let (emoji, label) = match signal.signal_type {
        GabagoolSignalType::Entry => match signal.direction {
            GabagoolDirection::Yes => ("", "ENTRY YES"),
            GabagoolDirection::No => ("", "ENTRY NO"),
        },
        GabagoolSignalType::Hedge => ("", "HEDGE"),
        GabagoolSignalType::Scratch => ("", "SCRATCH"),
    };

    println!(
        "\n{} {}: ${} | BTC ${:.2} vs ref ${:.2} ({:+.3}%) | Pair ${} | {}s left",
        emoji,
        label,
        signal.entry_price,
        signal.spot_price,
        signal.reference_price,
        signal.spot_delta_pct * 100.0,
        signal.current_pair_cost,
        signal.time_remaining_secs
    );

    match signal.signal_type {
        GabagoolSignalType::Entry => {
            println!(
                "   Edge: {:.2}% | Confidence: {:?} | Time: {}",
                signal.estimated_edge * 100.0,
                signal.confidence,
                signal.timestamp.format("%H:%M:%S UTC")
            );
        }
        GabagoolSignalType::Hedge => {
            if let Some(existing) = signal.existing_entry_price {
                let total = existing + signal.entry_price;
                let profit = Decimal::ONE - total;
                println!(
                    "   Entry was ${} + Hedge ${} = ${} pair | Locked profit: ${}",
                    existing, signal.entry_price, total, profit
                );
            }
        }
        GabagoolSignalType::Scratch => {
            if let Some(existing) = signal.existing_entry_price {
                let loss = existing - signal.entry_price;
                println!(
                    "   Entry was ${} | Exit at ${} | Loss: ${}",
                    existing, signal.entry_price, loss
                );
            }
        }
    }
}
