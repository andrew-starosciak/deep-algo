//! CLI command for settling unsettled directional trades.
//!
//! Connects to the database, loads all unsettled directional trades, and
//! settles them using the three-path cascade:
//!   1. CLOB fast-settle (token price ≥0.90 / ≤0.10)
//!   2. Gamma API (official UMA resolution)
//!   3. Chainlink oracle fallback (start vs end price)

use algo_trade_data::chainlink::ChainlinkWindowTracker;
use algo_trade_polymarket::arbitrage::sdk_client::{ClobClient, ClobClientConfig};
use algo_trade_polymarket::arbitrage::signer::{Wallet, WalletConfig};
use algo_trade_polymarket::models::Coin;
use algo_trade_polymarket::GammaClient;
use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Arguments for the directional settlement command.
#[derive(Args, Debug)]
pub struct DirectionalSettleArgs {
    /// Only show pending trades without settling them.
    #[arg(long)]
    pub dry_run: bool,

    /// Filter by coin (e.g., BTC, ETH). Settles all coins if not specified.
    #[arg(long)]
    pub coin: Option<String>,

    /// Filter by session ID.
    #[arg(long)]
    pub session: Option<String>,

    /// Maximum age in hours — only settle trades newer than this (default: no limit).
    #[arg(long)]
    pub max_age_hours: Option<u64>,

    /// Skip on-chain redemption of resolved positions.
    #[arg(long)]
    pub no_redeem: bool,

    /// Show verbose output.
    #[arg(short, long)]
    pub verbose: bool,
}

/// An unsettled trade loaded from the database.
struct UnsettledTrade {
    id: i32,
    trade_id: String,
    coin: String,
    direction: String,
    token_id: String,
    entry_price: Decimal,
    shares: Decimal,
    cost: Decimal,
    signal_timestamp: DateTime<Utc>,
    session_id: Option<String>,
}

/// Runs the directional settlement command.
pub async fn run(args: DirectionalSettleArgs) -> Result<()> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required"))?;

    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    // ── On-chain redemption (runs regardless of DB state) ──
    // The $USDC may be locked in resolved on-chain positions even if the DB
    // has no unsettled rows (e.g. trades were placed outside this system).
    if !args.no_redeem && !args.dry_run {
        redeem_resolved_positions().await;
    }

    // Load unsettled trades
    let trades = load_unsettled_trades(&pool, &args).await?;

    if trades.is_empty() {
        println!("No unsettled directional trades found.");
        return Ok(());
    }

    println!("Found {} unsettled directional trades:\n", trades.len());
    println!(
        "  {:<12} {:<6} {:<5} {:>10} {:>10} {:>10}  {}",
        "TRADE_ID", "COIN", "DIR", "PRICE", "SHARES", "COST", "SIGNAL_TIME"
    );
    println!("  {}", "-".repeat(75));
    for t in &trades {
        println!(
            "  {:<12} {:<6} {:<5} {:>10} {:>10} {:>10}  {}",
            t.trade_id,
            t.coin.to_uppercase(),
            t.direction,
            t.entry_price.round_dp(4),
            t.shares.round_dp(2),
            t.cost.round_dp(2),
            t.signal_timestamp.format("%Y-%m-%d %H:%M:%S"),
        );
    }
    println!();

    if args.dry_run {
        println!("Dry run — no settlements executed.");
        return Ok(());
    }

    // Settlement infrastructure
    let gamma_client = GammaClient::new();
    let http_client = reqwest::Client::new();
    let rpc_url = std::env::var("POLYGON_RPC_URL")
        .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());
    let mut chainlink_tracker = ChainlinkWindowTracker::new(&rpc_url);

    // Poll Chainlink a few times to populate window data
    info!("Polling Chainlink oracle for window prices...");
    for _ in 0..3 {
        chainlink_tracker.poll().await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let fee_rate = dec!(0.02);
    let mut settled = 0u32;
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut total_pnl = Decimal::ZERO;
    let mut failed = 0u32;

    println!("Settling trades...\n");

    for trade in &trades {
        let window_start_ms =
            algo_trade_polymarket::arbitrage::reference_tracker::ReferenceTracker::window_start_for_time(
                trade.signal_timestamp.timestamp_millis(),
            );
        let window_end_ms = window_start_ms + 15 * 60 * 1000;
        let now_ms = Utc::now().timestamp_millis();
        let age_ms = now_ms - window_end_ms;

        if age_ms < 0 {
            println!(
                "  {} {}: window still open, skipping",
                trade.trade_id, trade.coin
            );
            continue;
        }

        // ── Path 1: CLOB fast-settle ──
        if let Some(won) = try_clob_settle(&trade.token_id, &http_client).await {
            let pnl = calculate_pnl(won, trade.entry_price, trade.shares, trade.cost, fee_rate);
            persist_settlement(&pool, &trade.trade_id, won, pnl).await;
            print_settlement(&trade.trade_id, &trade.coin, &trade.direction, won, pnl, "CLOB");
            settled += 1;
            total_pnl += pnl;
            if won { wins += 1; } else { losses += 1; }
            continue;
        }

        // ── Path 2: Gamma API ──
        if let Some(coin) = Coin::from_slug(&trade.coin) {
            if let Some(window_end_dt) = DateTime::from_timestamp_millis(window_end_ms) {
                match gamma_client.get_market_outcome(coin, window_end_dt).await {
                    Ok(Some(outcome)) => {
                        let won = (trade.direction == "UP" && outcome == "UP")
                            || (trade.direction == "DOWN" && outcome == "DOWN");
                        let pnl = calculate_pnl(won, trade.entry_price, trade.shares, trade.cost, fee_rate);
                        persist_settlement(&pool, &trade.trade_id, won, pnl).await;
                        print_settlement(&trade.trade_id, &trade.coin, &trade.direction, won, pnl, "Gamma");
                        settled += 1;
                        total_pnl += pnl;
                        if won { wins += 1; } else { losses += 1; }
                        continue;
                    }
                    Ok(None) => {
                        debug!(trade_id = trade.trade_id, "Gamma: not yet resolved");
                    }
                    Err(e) => {
                        debug!(trade_id = trade.trade_id, error = %e, "Gamma API error");
                    }
                }
            }
        }

        // ── Path 3: Chainlink fallback ──
        let window_start_secs = window_start_ms / 1000;
        if let Some(outcome) = chainlink_tracker.get_outcome(&trade.coin.to_uppercase(), window_start_secs) {
            let won = trade.direction == outcome;
            let pnl = calculate_pnl(won, trade.entry_price, trade.shares, trade.cost, fee_rate);
            persist_settlement(&pool, &trade.trade_id, won, pnl).await;
            print_settlement(&trade.trade_id, &trade.coin, &trade.direction, won, pnl, "Chainlink");
            settled += 1;
            total_pnl += pnl;
            if won { wins += 1; } else { losses += 1; }
            continue;
        }

        warn!(
            trade_id = trade.trade_id,
            coin = trade.coin,
            age_mins = age_ms / 60_000,
            "Could not settle via any path"
        );
        println!(
            "  {} {:<4} {}: UNSETTLED (age {}m)",
            trade.trade_id, trade.coin.to_uppercase(), trade.direction, age_ms / 60_000,
        );
        failed += 1;
    }

    // Summary
    println!();
    println!("=== Settlement Summary ===");
    println!("  Total:    {}", trades.len());
    println!("  Settled:  {}", settled);
    println!("  Failed:   {}", failed);
    if settled > 0 {
        let win_rate = wins as f64 / settled as f64 * 100.0;
        let pnl_color = if total_pnl >= Decimal::ZERO { "\x1b[32m" } else { "\x1b[31m" };
        println!("  Wins:     {} ({:.1}%)", wins, win_rate);
        println!("  Losses:   {}", losses);
        println!("  P&L:      {}${}\x1b[0m", pnl_color, total_pnl.round_dp(2));
    }

    Ok(())
}

/// Loads unsettled trades from the database, applying optional filters.
async fn load_unsettled_trades(pool: &PgPool, args: &DirectionalSettleArgs) -> Result<Vec<UnsettledTrade>> {
    let mut query = String::from(
        "SELECT id, trade_id, coin, direction, token_id, entry_price, shares, cost, \
         signal_timestamp, session_id \
         FROM directional_trades WHERE settled = FALSE",
    );

    if let Some(ref coin) = args.coin {
        query.push_str(&format!(" AND UPPER(coin) = '{}'", coin.to_uppercase()));
    }
    if let Some(ref session) = args.session {
        query.push_str(&format!(" AND session_id = '{}'", session));
    }
    if let Some(max_hours) = args.max_age_hours {
        query.push_str(&format!(
            " AND signal_timestamp > NOW() - INTERVAL '{} hours'",
            max_hours
        ));
    }
    query.push_str(" ORDER BY signal_timestamp ASC");

    let rows = sqlx::query_as::<_, (
        i32,
        String,
        String,
        String,
        String,
        Decimal,
        Decimal,
        Decimal,
        DateTime<Utc>,
        Option<String>,
    )>(&query)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UnsettledTrade {
            id: r.0,
            trade_id: r.1,
            coin: r.2,
            direction: r.3,
            token_id: r.4,
            entry_price: r.5,
            shares: r.6,
            cost: r.7,
            signal_timestamp: r.8,
            session_id: r.9,
        })
        .collect())
}

/// Attempts CLOB fast-settle by checking if token price is decisive.
async fn try_clob_settle(token_id: &str, http_client: &reqwest::Client) -> Option<bool> {
    let url = format!("https://clob.polymarket.com/prices?token_ids={}", token_id);

    let response = match http_client
        .get(&url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return None,
    };

    #[derive(Deserialize)]
    struct PriceResponse {
        price: String,
    }

    let body = response.text().await.ok()?;
    let prices: std::collections::HashMap<String, PriceResponse> =
        serde_json::from_str(&body).ok()?;

    let price = prices
        .get(token_id)
        .and_then(|p| Decimal::from_str(&p.price).ok())?;

    if price >= dec!(0.90) {
        Some(true)
    } else if price <= dec!(0.10) {
        Some(false)
    } else {
        None
    }
}

/// Calculates P&L for a directional trade.
fn calculate_pnl(won: bool, entry_price: Decimal, shares: Decimal, cost: Decimal, fee_rate: Decimal) -> Decimal {
    if won {
        let gross = (Decimal::ONE - entry_price) * shares;
        gross * (Decimal::ONE - fee_rate)
    } else {
        -cost
    }
}

/// Persists settlement result to the database.
async fn persist_settlement(pool: &PgPool, trade_id: &str, won: bool, pnl: Decimal) {
    let result = sqlx::query(
        "UPDATE directional_trades SET settled = TRUE, won = $1, pnl = $2, settled_at = NOW() WHERE trade_id = $3",
    )
    .bind(won)
    .bind(pnl)
    .bind(trade_id)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {}
        Ok(_) => warn!(trade_id = trade_id, "Settlement update matched no rows"),
        Err(e) => warn!(trade_id = trade_id, error = %e, "Failed to persist settlement"),
    }
}

/// Prints a single settlement result.
fn print_settlement(trade_id: &str, coin: &str, direction: &str, won: bool, pnl: Decimal, source: &str) {
    let status = if won {
        format!("\x1b[32mWIN  +${}\x1b[0m", pnl.round_dp(2))
    } else {
        format!("\x1b[31mLOSS ${}\x1b[0m", pnl.round_dp(2))
    };
    println!(
        "  {} {:<4} {:<4} → {} (via {})",
        trade_id,
        coin.to_uppercase(),
        direction,
        status,
        source,
    );
}

/// Redeems resolved positions on-chain (converts winning tokens → USDC).
///
/// Requires POLYMARKET_PRIVATE_KEY env var. Silently skips if not available.
async fn redeem_resolved_positions() {
    let wallet = match Wallet::from_env(WalletConfig::mainnet()) {
        Ok(w) => w,
        Err(_) => {
            println!();
            println!("  \x1b[2mSkipping on-chain redemption (no POLYMARKET_PRIVATE_KEY)\x1b[0m");
            return;
        }
    };

    println!();
    println!("=== On-Chain Redemption ===");
    println!("  Wallet: {}", wallet.address());

    let client = match ClobClient::new(wallet, ClobClientConfig::mainnet()) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "Failed to create CLOB client for redemption");
            println!("  \x1b[31mFailed to create CLOB client: {}\x1b[0m", e);
            return;
        }
    };

    let rpc_url = std::env::var("POLYGON_RPC_URL")
        .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

    match client.redeem_resolved_positions(&rpc_url).await {
        Ok(0) => {
            println!("  No redeemable positions found.");
        }
        Ok(n) => {
            println!("  \x1b[32mRedeemed {} resolved positions!\x1b[0m", n);
        }
        Err(e) => {
            warn!(error = %e, "Redemption failed");
            println!("  \x1b[31mRedemption failed: {}\x1b[0m", e);
        }
    }
}
