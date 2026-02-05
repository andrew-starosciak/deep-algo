//! CLI command for cross-market correlation scanner.
//!
//! This command scans all 4 coin markets (BTC, ETH, SOL, XRP) for cross-market
//! correlation arbitrage opportunities where the combined cost of buying
//! opposite/same directions on two different coins is less than $1.00.

use algo_trade_data::models::CrossMarketOpportunityRecord;
use algo_trade_data::repositories::CrossMarketRepository;
use algo_trade_polymarket::arbitrage::{
    CrossMarketConfig, CrossMarketOpportunity, CrossMarketRunner, CrossMarketRunnerConfig,
};
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{error, info, warn};

/// Arguments for the cross-market scanner command.
#[derive(Args, Debug)]
pub struct CrossMarketScannerArgs {
    /// Duration to run in minutes (default: 60 for one hour).
    #[arg(short, long, default_value = "60")]
    pub duration_mins: u64,

    /// Maximum total cost threshold (sum of both legs must be < this).
    /// Default: 0.80 means only signal when cost < $0.80 (spread > 20%).
    /// Backtest showed cost<0.80 gives best avg P&L ($0.51 vs $0.40 for all signals).
    #[arg(long, default_value = "0.80")]
    pub max_cost: f64,

    /// Minimum spread (guaranteed profit if either wins).
    /// Default: 0.20 means $0.20 minimum spread (20% ROI if either wins).
    /// Backtest showed 20%+ spread entries have highest avg P&L.
    #[arg(long, default_value = "0.20")]
    pub min_spread: f64,

    /// Assumed correlation between crypto assets (0.0 to 1.0).
    /// Default: 0.85 means 85% correlation (BTC/ETH/SOL/XRP move together).
    #[arg(long, default_value = "0.85")]
    pub correlation: f64,

    /// Minimum expected value to signal (in dollars).
    /// Default: 0.01 means at least 1 cent expected profit.
    #[arg(long, default_value = "0.01")]
    pub min_ev: f64,

    /// Comma-separated list of coins to scan.
    /// Default: btc,eth,sol,xrp (all 4 coins).
    #[arg(long, default_value = "btc,eth,sol,xrp")]
    pub coins: String,

    /// Scan interval in milliseconds.
    /// Default: 1000 (1 second).
    #[arg(long, default_value = "1000")]
    pub scan_interval_ms: u64,

    /// Signal cooldown in milliseconds.
    /// Default: 5000 (5 seconds between same pair/combo signals).
    #[arg(long, default_value = "5000")]
    pub cooldown_ms: u64,

    /// Use aggressive config (faster scanning, lower thresholds).
    #[arg(long)]
    pub aggressive: bool,

    /// Show verbose output (status updates, not just signals).
    #[arg(short, long)]
    pub verbose: bool,

    /// Persist opportunities to database (requires DATABASE_URL env var).
    #[arg(long)]
    pub persist: bool,

    /// Session ID for grouping opportunities (auto-generated if not provided).
    #[arg(long)]
    pub session_id: Option<String>,

    /// Only detect Coin1Up/Coin2Down combinations (89% win rate strategy).
    #[arg(long)]
    pub only_up_down: bool,

    /// Only detect real arbitrage (opposing directions on different coins).
    /// Includes Coin1Up/Coin2Down and Coin1Down/Coin2Up.
    /// Excludes directional bets (BothUp, BothDown).
    #[arg(long)]
    pub arbitrage_only: bool,

    /// Use optimal backtest-proven settings:
    /// - Only Coin1UpCoin2Down (89% win rate)
    /// - Min spread 20% (cost < $0.80)
    /// - Best pairs: SOL/XRP, BTC/SOL, BTC/XRP
    #[arg(long)]
    pub optimal: bool,

    /// Track order book depth via WebSocket.
    /// This adds latency but captures liquidity data for fill analysis.
    #[arg(long)]
    pub track_depth: bool,

    /// Minimum order book depth (shares) required on both legs.
    /// Opportunities with less liquidity are filtered out.
    /// Default: 100 shares when --track-depth is enabled, 0 otherwise.
    #[arg(long)]
    pub min_depth: Option<f64>,
}

/// Runs the cross-market correlation scanner.
pub async fn run(args: CrossMarketScannerArgs) -> Result<()> {
    info!("=== Cross-Market Correlation Scanner ===");
    info!(
        "Duration: {} minutes | Max cost: ${} | Min spread: ${} | Correlation: {}",
        args.duration_mins, args.max_cost, args.min_spread, args.correlation
    );

    // Set up database persistence if requested
    let repo: Option<CrossMarketRepository> = if args.persist {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required for --persist"))?;
        info!("Connecting to database for persistence...");
        let pool = PgPool::connect(&database_url).await?;
        info!("Database connected");
        Some(CrossMarketRepository::new(pool))
    } else {
        None
    };

    // Generate session ID
    let session_id = args.session_id.unwrap_or_else(|| {
        format!("cross-market-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    });
    if args.persist {
        info!("Session ID: {}", session_id);
    }

    // Parse coins
    let coins: Vec<Coin> = args
        .coins
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim().to_lowercase();
            match trimmed.as_str() {
                "btc" | "bitcoin" => Some(Coin::Btc),
                "eth" | "ethereum" => Some(Coin::Eth),
                "sol" | "solana" => Some(Coin::Sol),
                "xrp" | "ripple" => Some(Coin::Xrp),
                _ => {
                    warn!("Unknown coin '{}', skipping", s);
                    None
                }
            }
        })
        .collect();

    if coins.len() < 2 {
        anyhow::bail!("Need at least 2 coins for cross-market scanning. Got: {:?}", coins);
    }

    info!("Scanning coins: {:?}", coins);

    // Apply flags (priority: optimal > only_up_down > arbitrage_only)
    let use_only_up_down = args.only_up_down || args.optimal;
    let use_arbitrage_only = args.arbitrage_only && !use_only_up_down;
    let max_cost = if args.optimal { 0.80 } else { args.max_cost };
    let min_spread = if args.optimal { 0.20 } else { args.min_spread };
    let correlation = if args.optimal { 0.64 } else { args.correlation }; // 64% observed correlation

    // Calculate number of pairs
    let num_pairs = coins.len() * (coins.len() - 1) / 2;
    let num_combos = if use_only_up_down {
        1
    } else if use_arbitrage_only {
        2
    } else {
        4
    };
    info!("Monitoring {} coin pairs with {} combination(s) each = {} total checks per scan",
          num_pairs, num_combos, num_pairs * num_combos);

    if args.optimal {
        info!("OPTIMAL MODE: Coin1UpCoin2Down only, cost<$0.80, 64% correlation");
        info!("  Backtest: 89% win rate, $0.51 avg P&L per trade");
    } else if use_only_up_down {
        info!("FILTERED: Only Coin1Up/Coin2Down combinations (89% win rate strategy)");
    } else if use_arbitrage_only {
        info!("ARBITRAGE MODE: Opposing directions only (Coin1Up/Coin2Down + Coin1Down/Coin2Up)");
        info!("  Excludes directional bets (BothUp, BothDown)");
    }

    // Determine minimum depth requirement
    // Default to 100 shares when tracking depth, 0 otherwise
    let min_depth = args.min_depth.unwrap_or(if args.track_depth { 100.0 } else { 0.0 });
    let min_depth_decimal = Decimal::from_str(&min_depth.to_string())
        .unwrap_or(Decimal::ZERO);

    // Build config
    let detector_config = if args.aggressive {
        info!("Using AGGRESSIVE config");
        let mut config = CrossMarketConfig::aggressive();
        config.coins = coins;
        config.min_depth = min_depth_decimal;
        if use_only_up_down {
            config = config.only_up_down();
        } else if use_arbitrage_only {
            config = config.arbitrage_only();
        }
        config
    } else {
        let mut config = CrossMarketConfig {
            max_total_cost: Decimal::from_str(&max_cost.to_string())
                .unwrap_or_else(|_| Decimal::from_str("0.80").unwrap()),
            min_spread: Decimal::from_str(&min_spread.to_string())
                .unwrap_or_else(|_| Decimal::from_str("0.20").unwrap()),
            assumed_correlation: correlation,
            min_expected_value: Decimal::from_str(&args.min_ev.to_string())
                .unwrap_or_else(|_| Decimal::from_str("0.01").unwrap()),
            signal_cooldown_ms: args.cooldown_ms as i64,
            coins,
            combinations: None,
            min_depth: min_depth_decimal,
        };
        if use_only_up_down {
            config = config.only_up_down();
        } else if use_arbitrage_only {
            config = config.arbitrage_only();
        }
        config
    };

    let mut runner_config = CrossMarketRunnerConfig {
        detector_config: detector_config.clone(),
        scan_interval_ms: args.scan_interval_ms,
        signal_buffer_size: 200,
        gamma_rate_limit: if args.aggressive { 60 } else { 30 },
        track_depth: args.track_depth,
    };

    if args.track_depth {
        info!("Order book depth tracking: ENABLED");
        // Slow down scan interval to allow WebSocket to populate books
        if runner_config.scan_interval_ms < 2000 {
            info!("  Increasing scan interval to 2s for depth tracking");
            runner_config.scan_interval_ms = 2000;
        }
        if min_depth > 0.0 {
            info!("  Min depth filter: {} shares (opportunities with less liquidity will be skipped)", min_depth);
        }
    }

    info!(
        "Config: max_cost=${}, min_spread=${}, correlation={:.0}%, min_ev=${}",
        runner_config.detector_config.max_total_cost,
        runner_config.detector_config.min_spread,
        runner_config.detector_config.assumed_correlation * 100.0,
        runner_config.detector_config.min_expected_value
    );

    // Create runner
    let (runner, mut opp_rx) = CrossMarketRunner::new(runner_config);
    let stop_handle = runner.stop_handle();
    let stats = runner.stats();

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

    // Collect opportunities
    let duration = Duration::from_secs(args.duration_mins * 60);
    let deadline = tokio::time::Instant::now() + duration;
    let mut opportunities: Vec<CrossMarketOpportunity> = Vec::new();

    info!("");
    info!("Scanning for cross-market opportunities...");
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
            opp = opp_rx.recv() => {
                match opp {
                    Some(o) => {
                        print_opportunity(&o);

                        // Persist to database if enabled
                        if let Some(ref repo) = repo {
                            let record = opportunity_to_record(&o, &session_id);
                            match repo.insert(&record).await {
                                Ok(id) => {
                                    info!("Persisted opportunity #{}", id);
                                }
                                Err(e) => {
                                    error!("Failed to persist opportunity: {}", e);
                                }
                            }
                        }

                        opportunities.push(o);
                    }
                    None => {
                        warn!("Opportunity channel closed");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                // Periodic status update
                if args.verbose && last_status.elapsed() >= status_interval {
                    let s = stats.read().await;

                    // Build prices string
                    let prices_str: String = s.current_prices
                        .iter()
                        .map(|(coin, (up, down))| format!("{}:${}/{}", coin, up, down))
                        .collect::<Vec<_>>()
                        .join(" | ");

                    info!(
                        "Status: Scans {} | Opps {} | Errors {} | {}",
                        s.scans_performed,
                        s.opportunities_detected,
                        s.errors,
                        if prices_str.is_empty() { "No prices yet".to_string() } else { prices_str }
                    );
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
    println!("Scans performed: {}", final_stats.scans_performed);
    println!("Opportunities detected: {}", final_stats.opportunities_detected);
    println!("Errors: {}", final_stats.errors);

    if let Some(best) = final_stats.best_spread {
        println!("Best spread seen: ${}", best);
    }
    if let Some(lowest) = final_stats.lowest_cost {
        println!("Lowest cost seen: ${}", lowest);
    }

    if !final_stats.by_pair.is_empty() {
        println!();
        println!("Opportunities by pair:");
        for (pair, count) in &final_stats.by_pair {
            println!("  {}: {}", pair, count);
        }
    }

    if args.persist {
        println!();
        println!("=== Database Persistence ===");
        println!("Session ID: {}", session_id);
        println!("Opportunities persisted: {}", opportunities.len());
    }

    if !opportunities.is_empty() {
        println!();
        println!("=== Opportunity Details ===");
        for (i, opp) in opportunities.iter().enumerate() {
            println!(
                "{}. {}/{} {:?} | ${} + ${} = ${} | Spread ${} | EV ${:.4} | P(win) {:.0}%",
                i + 1,
                opp.coin1,
                opp.coin2,
                opp.combination,
                opp.leg1_price,
                opp.leg2_price,
                opp.total_cost,
                opp.spread,
                opp.expected_value,
                opp.win_probability * 100.0
            );
        }

        // Calculate theoretical P&L
        println!();
        println!("=== Theoretical Analysis ===");

        let avg_spread: Decimal = opportunities.iter().map(|o| o.spread).sum::<Decimal>()
            / Decimal::from(opportunities.len() as u32);
        let avg_win_prob: f64 = opportunities.iter().map(|o| o.win_probability).sum::<f64>()
            / opportunities.len() as f64;
        let avg_ev: Decimal = opportunities.iter().map(|o| o.expected_value).sum::<Decimal>()
            / Decimal::from(opportunities.len() as u32);

        println!("If each opportunity was a $100 trade:");
        println!("  Total opportunities: {}", opportunities.len());
        println!("  Average spread: ${}", avg_spread);
        println!("  Average win probability: {:.1}%", avg_win_prob * 100.0);
        println!("  Average expected value: ${}", avg_ev);

        // With correlation, loss scenario is rare
        let total_ev: f64 = opportunities.iter()
            .map(|o| {
                let ev_f64: f64 = o.expected_value.to_string().parse().unwrap_or(0.0);
                ev_f64 * 100.0 // $100 per trade
            })
            .sum();
        println!("  Total theoretical EV: ${:.2}", total_ev);
    } else {
        println!();
        println!("No opportunities detected during monitoring period.");
        println!("This could mean:");
        println!("  - Markets are efficiently priced (no < ${} combined cost)", args.max_cost);
        println!("  - Spread is too tight (need ${}+ minimum)", args.min_spread);
        println!("  - Try running during higher volatility periods");
        println!("  - Try with --aggressive flag for lower thresholds");
    }

    Ok(())
}

/// Converts an opportunity to a database record.
fn opportunity_to_record(opp: &CrossMarketOpportunity, session_id: &str) -> CrossMarketOpportunityRecord {
    let mut record = CrossMarketOpportunityRecord::new(
        opp.detected_at,
        opp.coin1.clone(),
        opp.coin2.clone(),
        format!("{:?}", opp.combination),
        opp.leg1_direction.clone(),
        opp.leg1_price,
        opp.leg1_token_id.clone(),
        opp.leg2_direction.clone(),
        opp.leg2_price,
        opp.leg2_token_id.clone(),
        opp.total_cost,
        opp.spread,
        opp.expected_value,
        opp.win_probability,
        opp.assumed_correlation,
    )
    .with_session(session_id.to_string());

    // Add depth data if available
    if opp.has_depth_data() {
        record = record.with_depth(
            opp.leg1_bid_depth.unwrap_or(Decimal::ZERO),
            opp.leg1_ask_depth.unwrap_or(Decimal::ZERO),
            opp.leg1_spread_bps.unwrap_or(Decimal::ZERO),
            opp.leg2_bid_depth.unwrap_or(Decimal::ZERO),
            opp.leg2_ask_depth.unwrap_or(Decimal::ZERO),
            opp.leg2_spread_bps.unwrap_or(Decimal::ZERO),
        );
    }

    record
}

/// Prints an opportunity to stdout.
fn print_opportunity(opp: &CrossMarketOpportunity) {
    println!(
        "\n OPPORTUNITY: {}/{} {:?}",
        opp.coin1, opp.coin2, opp.combination
    );
    println!(
        "   {} {} @ ${} + {} {} @ ${} = ${} total",
        opp.coin1, opp.leg1_direction, opp.leg1_price,
        opp.coin2, opp.leg2_direction, opp.leg2_price,
        opp.total_cost
    );
    println!(
        "   Spread: ${} | EV: ${:.4} | P(win): {:.1}% | Correlation: {:.0}%",
        opp.spread,
        opp.expected_value,
        opp.win_probability * 100.0,
        opp.assumed_correlation * 100.0
    );

    // Show depth data if available
    if opp.has_depth_data() {
        println!(
            "   Depth L1: {} bid / {} ask @ {} bps spread",
            opp.leg1_bid_depth.unwrap_or(Decimal::ZERO),
            opp.leg1_ask_depth.unwrap_or(Decimal::ZERO),
            opp.leg1_spread_bps.unwrap_or(Decimal::ZERO).round()
        );
        println!(
            "   Depth L2: {} bid / {} ask @ {} bps spread",
            opp.leg2_bid_depth.unwrap_or(Decimal::ZERO),
            opp.leg2_ask_depth.unwrap_or(Decimal::ZERO),
            opp.leg2_spread_bps.unwrap_or(Decimal::ZERO).round()
        );
        if let Some(min_depth) = opp.min_depth() {
            println!("   Min depth: {} shares", min_depth);
        }
    }

    println!("   Time: {}", opp.detected_at.format("%H:%M:%S UTC"));
}
