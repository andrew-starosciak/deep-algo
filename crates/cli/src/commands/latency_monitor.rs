//! CLI command for latency arbitrage monitoring.
//!
//! This command connects to both Binance (BTC spot) and Polymarket (order books)
//! to detect latency arbitrage opportunities in real-time.

use algo_trade_polymarket::arbitrage::{
    LatencyConfig, LatencyDirection, LatencyRunner, LatencyRunnerConfig, LatencySignal,
};
use algo_trade_polymarket::{Coin, GammaClient};
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{error, info, warn};

/// Arguments for the latency monitor command.
#[derive(Args, Debug)]
pub struct LatencyMonitorArgs {
    /// Duration to run in minutes (default: 15 for one market window).
    #[arg(short, long, default_value = "15")]
    pub duration_mins: u64,

    /// Minimum spot price change threshold (percent, e.g., 0.2).
    #[arg(long, default_value = "0.2")]
    pub min_spot_change_pct: f64,

    /// Maximum entry price for signals (e.g., 0.45).
    #[arg(long, default_value = "0.45")]
    pub max_entry_price: f64,

    /// Lookback period for spot change in minutes.
    #[arg(long, default_value = "5")]
    pub lookback_mins: u64,

    /// Signal cooldown in seconds.
    #[arg(long, default_value = "5")]
    pub cooldown_secs: u64,

    /// Check interval in milliseconds.
    #[arg(long, default_value = "100")]
    pub check_interval_ms: u64,

    /// Use aggressive config (lower thresholds).
    #[arg(long)]
    pub aggressive: bool,

    /// Use conservative config (higher thresholds).
    #[arg(long)]
    pub conservative: bool,

    /// Show verbose output (every check, not just signals).
    #[arg(short, long)]
    pub verbose: bool,
}

/// Runs the latency arbitrage monitor.
pub async fn run(args: LatencyMonitorArgs) -> Result<()> {
    info!("=== Latency Arbitrage Monitor ===");
    info!(
        "Duration: {} minutes | Min spot change: {}% | Max entry: ${}",
        args.duration_mins, args.min_spot_change_pct, args.max_entry_price
    );

    // Build latency config
    let latency_config = if args.aggressive {
        info!("Using AGGRESSIVE config");
        LatencyConfig::aggressive()
    } else if args.conservative {
        info!("Using CONSERVATIVE config");
        LatencyConfig::conservative()
    } else {
        LatencyConfig {
            min_spot_change: args.min_spot_change_pct / 100.0,
            max_entry_price: Decimal::try_from(args.max_entry_price)?,
            lookback_ms: (args.lookback_mins * 60 * 1000) as i64,
            min_staleness_ms: 1000,
        }
    };

    info!(
        "Config: min_change={:.2}%, max_price=${}, lookback={}min",
        latency_config.min_spot_change * 100.0,
        latency_config.max_entry_price,
        latency_config.lookback_ms / 60000
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
    let yes_token = market.up_token().ok_or_else(|| {
        anyhow::anyhow!("Market missing UP token")
    })?;
    let no_token = market.down_token().ok_or_else(|| {
        anyhow::anyhow!("Market missing DOWN token")
    })?;

    info!("YES token: {}", yes_token.token_id);
    info!("NO token: {}", no_token.token_id);

    // Create runner config
    let runner_config = LatencyRunnerConfig {
        yes_token_id: yes_token.token_id.clone(),
        no_token_id: no_token.token_id.clone(),
        latency_config,
        check_interval_ms: args.check_interval_ms,
        ..Default::default()
    };

    // Create runner
    let (runner, mut signal_rx) = LatencyRunner::new(runner_config);
    let stop_handle = runner.stop_handle();
    let stats = runner.stats();
    let spot_tracker = runner.spot_tracker();

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
    let mut signals: Vec<LatencySignal> = Vec::new();

    info!("");
    info!("📡 Monitoring for latency signals...");
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
                    let t = spot_tracker.read().await;

                    if let (Some(spot), Some(yes), Some(no)) = (
                        t.current_price(),
                        s.current_yes_ask,
                        s.current_no_ask,
                    ) {
                        let change_5m = t.change_5min().unwrap_or(0.0);
                        info!(
                            "Status: BTC ${:.2} ({:+.2}% 5m) | YES ${} | NO ${} | Checks: {}",
                            spot,
                            change_5m * 100.0,
                            yes,
                            no,
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
    println!("Signals generated: {}", final_stats.signals_generated);
    println!("  - BUY YES: {}", final_stats.buy_yes_signals);
    println!("  - BUY NO: {}", final_stats.buy_no_signals);

    if !signals.is_empty() {
        println!();
        println!("=== Signal Details ===");
        for (i, sig) in signals.iter().enumerate() {
            println!(
                "{}. {} @ ${} | Spot {:+.2}% | Strength {:.2}",
                i + 1,
                match sig.direction {
                    LatencyDirection::BuyYes => "BUY YES",
                    LatencyDirection::BuyNo => "BUY NO",
                },
                sig.entry_price,
                sig.spot_change_pct * 100.0,
                sig.strength
            );
        }

        // Calculate theoretical P&L if all signals were followed
        println!();
        println!("=== Theoretical Analysis ===");
        println!(
            "If each signal was a $50 trade at 95% win rate (gabagool's claimed rate):"
        );
        let wins = (signals.len() as f64 * 0.95).round() as usize;
        let losses = signals.len() - wins;

        // Average entry price
        let avg_entry: f64 = signals
            .iter()
            .map(|s| s.entry_price.to_string().parse::<f64>().unwrap_or(0.35))
            .sum::<f64>()
            / signals.len() as f64;

        let profit_per_win = 50.0 * ((1.0 - avg_entry) / avg_entry);
        let total_wins = wins as f64 * profit_per_win;
        let total_losses = losses as f64 * 50.0;
        let net_pnl = total_wins - total_losses;

        println!("  Signals: {}", signals.len());
        println!("  Assumed wins: {} (95%)", wins);
        println!("  Assumed losses: {} (5%)", losses);
        println!("  Avg entry price: ${:.2}", avg_entry);
        println!("  Profit per win: ${:.2}", profit_per_win);
        println!("  Theoretical P&L: ${:.2}", net_pnl);
    } else {
        println!();
        println!("No signals detected during monitoring period.");
        println!("This could mean:");
        println!("  - Market is efficiently priced (no < ${} opportunities)", args.max_entry_price);
        println!("  - BTC spot didn't move enough (need {}%+)", args.min_spot_change_pct);
        println!("  - Try running during higher volatility periods");
    }

    Ok(())
}

/// Prints a signal to stdout.
fn print_signal(signal: &LatencySignal) {
    let direction = match signal.direction {
        LatencyDirection::BuyYes => "🟢 BUY YES",
        LatencyDirection::BuyNo => "🔴 BUY NO",
    };

    println!(
        "\n🎯 SIGNAL: {} @ ${} | BTC ${:.2} ({:+.2}%) | Strength: {:.2}",
        direction,
        signal.entry_price,
        signal.spot_price,
        signal.spot_change_pct * 100.0,
        signal.strength
    );
    println!(
        "   Hedge target: ${} | Time: {}",
        signal.hedge_target(),
        signal.timestamp.format("%H:%M:%S UTC")
    );
}
