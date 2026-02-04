//! CLI command for cross-market opportunity settlement.
//!
//! This command polls for pending cross-market opportunities and settles them
//! when the 15-minute windows close and outcomes are known.
//!
//! # Settlement Modes
//!
//! - **Paper mode** (default): Uses CLOB prices, then Chainlink/Binance fallback
//! - **Live mode** (`--live`): Queries wallet positions from Polymarket Data API

use algo_trade_data::repositories::CrossMarketRepository;
use algo_trade_polymarket::arbitrage::{
    CrossMarketSettlementConfig, CrossMarketSettlementHandler, SettlementMode,
};
use algo_trade_polymarket::arbitrage::sdk_client::{ClobClient, ClobClientConfig};
use algo_trade_polymarket::arbitrage::signer::{Wallet, WalletConfig};
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Arguments for the cross-market settlement command.
#[derive(Args, Debug)]
pub struct CrossMarketSettleArgs {
    /// Enable live mode (use wallet positions for settlement).
    /// Requires POLYMARKET_PRIVATE_KEY environment variable.
    #[arg(long)]
    pub live: bool,

    /// Duration to run in minutes (default: 0 for continuous).
    #[arg(short, long, default_value = "0")]
    pub duration_mins: u64,

    /// Settlement delay after window close in seconds (default: 120).
    #[arg(long, default_value = "120")]
    pub delay_secs: u64,

    /// Poll interval in seconds (default: 30).
    #[arg(long, default_value = "30")]
    pub poll_secs: u64,

    /// Fee rate on winnings (default: 0.02 for 2%).
    #[arg(long, default_value = "0.02")]
    pub fee_rate: f64,

    /// Process one batch and exit (don't run continuously).
    #[arg(long)]
    pub once: bool,

    /// Show verbose output.
    #[arg(short, long)]
    pub verbose: bool,

    /// Manually settle a specific opportunity by ID.
    #[arg(long)]
    pub settle_id: Option<i32>,

    /// Coin1 outcome for manual settlement (UP or DOWN).
    #[arg(long)]
    pub coin1_outcome: Option<String>,

    /// Coin2 outcome for manual settlement (UP or DOWN).
    #[arg(long)]
    pub coin2_outcome: Option<String>,
}

/// Runs the cross-market settlement handler.
pub async fn run(args: CrossMarketSettleArgs) -> Result<()> {
    info!("=== Cross-Market Settlement Handler ===");

    // Connect to database
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required"))?;
    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    info!("Database connected");

    // Create repository
    let repo = CrossMarketRepository::new(pool.clone());

    // Build config with mode
    let mode = if args.live {
        SettlementMode::Live
    } else {
        SettlementMode::Paper
    };

    let config = CrossMarketSettlementConfig {
        mode,
        settlement_delay_ms: (args.delay_secs * 1000) as i64,
        batch_size: 50,
        poll_interval_ms: args.poll_secs * 1000,
        fee_rate: Decimal::from_str(&args.fee_rate.to_string())
            .unwrap_or_else(|_| Decimal::from_str("0.02").unwrap()),
        max_pending_age_ms: 60 * 60 * 1000, // 1 hour
    };

    info!(
        "Config: mode={:?}, delay={}s, poll={}s, fee={:.1}%",
        mode,
        args.delay_secs,
        args.poll_secs,
        args.fee_rate * 100.0
    );

    // Create handler based on mode
    let handler = if args.live {
        // Live mode: use wallet-based settlement
        info!("Live mode: Using wallet positions for settlement (source of truth)");

        match Wallet::from_env(WalletConfig::mainnet()) {
            Ok(wallet) => {
                info!("Wallet loaded: {}", wallet.address());

                match ClobClient::new(wallet, ClobClientConfig::mainnet()) {
                    Ok(client) => {
                        CrossMarketSettlementHandler::new_live(repo.clone(), Arc::new(client), config)
                    }
                    Err(e) => {
                        warn!("Failed to create CLOB client: {}. Falling back to paper mode.", e);
                        let mut paper_config = config;
                        paper_config.mode = SettlementMode::Paper;
                        CrossMarketSettlementHandler::new(repo.clone(), paper_config)
                    }
                }
            }
            Err(e) => {
                warn!("Failed to load wallet: {}. Falling back to paper mode.", e);
                let mut paper_config = config;
                paper_config.mode = SettlementMode::Paper;
                CrossMarketSettlementHandler::new(repo.clone(), paper_config)
            }
        }
    } else {
        // Paper mode: use fallback settlement
        CrossMarketSettlementHandler::new(repo.clone(), config)
    };
    let stop_handle = handler.stop_handle();
    let stats = handler.stats();

    // Handle manual settlement
    if let Some(settle_id) = args.settle_id {
        info!("Manual settlement for opportunity #{}", settle_id);

        match (&args.coin1_outcome, &args.coin2_outcome) {
            (Some(c1), Some(c2)) => {
                let c1_upper = c1.to_uppercase();
                let c2_upper = c2.to_uppercase();

                if !["UP", "DOWN"].contains(&c1_upper.as_str())
                    || !["UP", "DOWN"].contains(&c2_upper.as_str())
                {
                    anyhow::bail!("Outcomes must be UP or DOWN");
                }

                handler.settle_manually(settle_id, &c1_upper, &c2_upper).await?;
                info!("Successfully settled opportunity #{}", settle_id);
                return Ok(());
            }
            _ => {
                // Try automatic settlement
                handler.settle_by_id(settle_id).await?;
                info!("Successfully settled opportunity #{}", settle_id);
                return Ok(());
            }
        }
    }

    // Single batch mode
    if args.once {
        info!("Processing single batch...");

        // Get pending count first
        let pending = repo.get_pending_settlement(1000).await?;
        info!("Found {} pending opportunities", pending.len());

        // Run one batch
        let stop_on_ctrl_c = stop_handle.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stop_on_ctrl_c.store(true, Ordering::SeqCst);
            }
        });

        // Process for a short time
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                stop_handle.store(true, Ordering::SeqCst);
            }
            result = handler.run() => {
                if let Err(e) = result {
                    error!("Handler error: {}", e);
                }
            }
        }

        let final_stats = stats.read().await;
        println!();
        println!("=== Batch Results ===");
        println!("Processed: {}", final_stats.total_processed);
        println!("Settled: {}", final_stats.settled);
        println!("Expired: {}", final_stats.expired);
        println!("Errors: {}", final_stats.errors);
        if final_stats.settled > 0 {
            println!("Wins: {} ({:.1}%)", final_stats.wins, final_stats.win_rate() * 100.0);
            println!("Losses: {}", final_stats.losses);
            println!("Double Wins: {}", final_stats.double_wins);
            println!("Total P&L: ${}", final_stats.total_pnl);
            println!("Correlation Accuracy: {:.1}%", final_stats.correlation_accuracy() * 100.0);
        }

        return Ok(());
    }

    // Continuous mode
    let duration = if args.duration_mins > 0 {
        Some(Duration::from_secs(args.duration_mins * 60))
    } else {
        None
    };

    info!("Running settlement handler...");
    if duration.is_some() {
        info!("Duration: {} minutes", args.duration_mins);
    } else {
        info!("Duration: continuous (Ctrl+C to stop)");
    }

    // Set up Ctrl+C handler
    let stop_on_ctrl_c = stop_handle.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received Ctrl+C, stopping...");
            stop_on_ctrl_c.store(true, Ordering::SeqCst);
        }
    });

    // Set up status updates if verbose
    let stats_clone = stats.clone();
    if args.verbose {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let s = stats_clone.read().await;
                info!(
                    "Status: processed={}, settled={}, wins={}, losses={}, P&L=${}",
                    s.total_processed,
                    s.settled,
                    s.wins,
                    s.losses,
                    s.total_pnl
                );
            }
        });
    }

    // Run handler
    if let Some(d) = duration {
        let deadline = tokio::time::Instant::now() + d;
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                info!("Duration elapsed");
                stop_handle.store(true, Ordering::SeqCst);
            }
            result = handler.run() => {
                if let Err(e) = result {
                    error!("Handler error: {}", e);
                }
            }
        }
    } else {
        if let Err(e) = handler.run().await {
            error!("Handler error: {}", e);
        }
    }

    // Print final summary
    let final_stats = stats.read().await;
    println!();
    println!("=== Settlement Summary ===");
    println!("Total Processed: {}", final_stats.total_processed);
    println!("Settled: {}", final_stats.settled);
    println!("Expired: {}", final_stats.expired);
    println!("Errors: {}", final_stats.errors);

    if final_stats.settled > 0 {
        println!();
        println!("=== Performance ===");
        println!("Wins: {} ({:.1}%)", final_stats.wins, final_stats.win_rate() * 100.0);
        println!("  - Regular Wins: {}", final_stats.wins - final_stats.double_wins);
        println!("  - Double Wins: {}", final_stats.double_wins);
        println!("Losses: {}", final_stats.losses);
        println!();
        println!("Total P&L: ${}", final_stats.total_pnl);
        if final_stats.settled > 0 {
            println!("Avg P&L per Trade: ${:.4}", final_stats.total_pnl / Decimal::from(final_stats.settled as u32));
        }
        println!();
        println!("=== Model Validation ===");
        println!("Actual Win Rate: {:.1}%", final_stats.win_rate() * 100.0);
        println!("Expected Win Rate: ~96%");
        println!("Correlation Accuracy: {:.1}%", final_stats.correlation_accuracy() * 100.0);
        println!("Expected Correlation: ~85%");
    }

    Ok(())
}
