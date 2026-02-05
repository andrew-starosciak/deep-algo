//! CLI command to check order book depth for 15-minute crypto markets.
//!
//! Connects to Polymarket CLOB WebSocket to get real-time order book data
//! and displays liquidity available at various price levels.

use algo_trade_polymarket::models::Coin;
use algo_trade_polymarket::websocket::{BookEvent, PolymarketWebSocket, WebSocketConfig};
use algo_trade_polymarket::GammaClient;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use std::time::Duration;

/// Arguments for the check-depth command.
#[derive(Args, Debug)]
pub struct CheckDepthArgs {
    /// Coins to check (comma-separated). Default: btc,eth,sol,xrp
    #[arg(long, default_value = "btc,eth,sol,xrp")]
    pub coins: String,

    /// Timeout in seconds to wait for order book data.
    #[arg(long, default_value = "15")]
    pub timeout_secs: u64,
}

/// Runs the depth check.
pub async fn run(args: CheckDepthArgs) -> Result<()> {
    let coins: Vec<Coin> = args
        .coins
        .split(',')
        .filter_map(|s| match s.trim().to_lowercase().as_str() {
            "btc" => Some(Coin::Btc),
            "eth" => Some(Coin::Eth),
            "sol" => Some(Coin::Sol),
            "xrp" => Some(Coin::Xrp),
            _ => None,
        })
        .collect();

    if coins.is_empty() {
        anyhow::bail!("No valid coins specified");
    }

    println!("\n=== Order Book Depth Check ===\n");

    // First, get current markets from Gamma API
    let gamma = GammaClient::new();

    for coin in &coins {
        println!("Checking {}...", coin.slug_prefix().to_uppercase());

        match gamma.get_current_15min_market(*coin).await {
            Ok(market) => {
                let up_token = market.up_token().map(|t| t.token_id.as_str()).unwrap_or("?");
                let down_token = market.down_token().map(|t| t.token_id.as_str()).unwrap_or("?");
                let up_price = market.up_price().unwrap_or(Decimal::ZERO);
                let down_price = market.down_price().unwrap_or(Decimal::ZERO);

                let question = &market.question;
                let question_display = &question[..50.min(question.len())];
                let condition_display = &market.condition_id[..10.min(market.condition_id.len())];

                println!("  Market: {} (condition: {})", question_display, condition_display);
                println!("  Prices: UP=${:.3} DOWN=${:.3}", up_price, down_price);
                println!("  Liquidity (from Gamma): ${}", market.liquidity.unwrap_or(Decimal::ZERO));

                // Try to get order book via WebSocket
                match fetch_order_book(up_token, down_token, args.timeout_secs).await {
                    Ok((up_book, down_book)) => {
                        println!("  UP Order Book:");
                        print_book_summary(&up_book);
                        println!("  DOWN Order Book:");
                        print_book_summary(&down_book);
                    }
                    Err(e) => {
                        println!("  Order book fetch failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("  Failed to fetch market: {}", e);
            }
        }
        println!();
    }

    Ok(())
}

/// Book summary data
struct BookSummary {
    best_bid: Option<Decimal>,
    best_ask: Option<Decimal>,
    bid_depth_10c: Decimal,  // Depth within 10 cents of best bid
    ask_depth_10c: Decimal,  // Depth within 10 cents of best ask
    total_bid_depth: Decimal,
    total_ask_depth: Decimal,
    spread_bps: Option<Decimal>,
}

async fn fetch_order_book(
    up_token: &str,
    down_token: &str,
    timeout_secs: u64,
) -> Result<(BookSummary, BookSummary)> {
    let config = WebSocketConfig::default();
    let token_ids = vec![up_token.to_string(), down_token.to_string()];

    let (ws, mut rx) = PolymarketWebSocket::connect(token_ids, config).await?;

    let timeout = Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();

    let mut up_summary: Option<BookSummary> = None;
    let mut down_summary: Option<BookSummary> = None;

    while start.elapsed() < timeout {
        match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(event)) => {
                if let BookEvent::Snapshot { asset_id, book } = event {
                    let summary = BookSummary {
                        best_bid: book.best_bid(),
                        best_ask: book.best_ask(),
                        bid_depth_10c: calculate_depth_within(&book, true, Decimal::new(10, 2)),
                        ask_depth_10c: calculate_depth_within(&book, false, Decimal::new(10, 2)),
                        total_bid_depth: book.bids.values().sum(),
                        total_ask_depth: book.asks.values().sum(),
                        spread_bps: calculate_spread_bps(book.best_bid(), book.best_ask()),
                    };

                    if asset_id == up_token {
                        up_summary = Some(summary);
                    } else if asset_id == down_token {
                        down_summary = Some(summary);
                    }

                    if up_summary.is_some() && down_summary.is_some() {
                        break;
                    }
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    // Clean disconnect
    drop(ws);

    match (up_summary, down_summary) {
        (Some(up), Some(down)) => Ok((up, down)),
        _ => anyhow::bail!("Timeout waiting for order book snapshots"),
    }
}

fn calculate_depth_within(
    book: &algo_trade_polymarket::arbitrage::L2OrderBook,
    is_bid: bool,
    range: Decimal,
) -> Decimal {
    if is_bid {
        let best = match book.best_bid() {
            Some(b) => b,
            None => return Decimal::ZERO,
        };
        book.bids
            .iter()
            .filter(|(price, _)| price.0 >= best - range)
            .map(|(_, size)| *size)
            .sum()
    } else {
        let best = match book.best_ask() {
            Some(a) => a,
            None => return Decimal::ZERO,
        };
        let max_price = best + range;
        book.asks
            .iter()
            .filter(|(price, _)| **price <= max_price)
            .map(|(_, size)| *size)
            .sum()
    }
}

fn calculate_spread_bps(best_bid: Option<Decimal>, best_ask: Option<Decimal>) -> Option<Decimal> {
    match (best_bid, best_ask) {
        (Some(bid), Some(ask)) if bid > Decimal::ZERO => {
            Some((ask - bid) / bid * Decimal::new(10000, 0))
        }
        _ => None,
    }
}

fn print_book_summary(summary: &BookSummary) {
    println!("    Best Bid: ${:.4}  Best Ask: ${:.4}  Spread: {} bps",
        summary.best_bid.unwrap_or(Decimal::ZERO),
        summary.best_ask.unwrap_or(Decimal::ZERO),
        summary.spread_bps.map(|s| format!("{:.0}", s)).unwrap_or_else(|| "?".to_string())
    );
    println!("    Bid Depth (10c): {} shares  Ask Depth (10c): {} shares",
        summary.bid_depth_10c,
        summary.ask_depth_10c
    );
    println!("    Total Bid: {} shares  Total Ask: {} shares",
        summary.total_bid_depth,
        summary.total_ask_depth
    );
}
