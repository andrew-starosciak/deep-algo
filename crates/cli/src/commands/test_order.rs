//! Submit a micro test order to verify EIP-712 signing and amount precision.
//!
//! Tests with realistic prices ($0.82) that exercise the maker_amount rounding path,
//! not just trivial $0.01 orders that happen to be 2dp naturally.

use algo_trade_polymarket::arbitrage::execution::{OrderParams, OrderType, Side};
use algo_trade_polymarket::arbitrage::sdk_client::{ClobClient, ClobClientConfig};
use algo_trade_polymarket::arbitrage::signer::{Wallet, WalletConfig};
use algo_trade_polymarket::gamma::GammaClient;
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::str::FromStr;

/// Arguments for the test-order command.
#[derive(Args, Debug)]
pub struct TestOrderArgs {
    /// Coin to test with (btc, eth, sol, xrp).
    #[arg(long, default_value = "btc")]
    pub coin: String,

    /// Price to test at (use a realistic price like 0.82 to exercise rounding).
    #[arg(long, default_value = "0.82")]
    pub price: String,

    /// Size in shares.
    #[arg(long, default_value = "6.54")]
    pub size: String,
}

/// Runs the test order flow.
pub async fn run(args: TestOrderArgs) -> Result<()> {
    println!("=== Test Order: Signing + Amount Precision ===\n");

    let coin = match args.coin.to_lowercase().as_str() {
        "btc" => Coin::Btc,
        "eth" => Coin::Eth,
        "sol" => Coin::Sol,
        "xrp" => Coin::Xrp,
        _ => anyhow::bail!("Unknown coin: {}", args.coin),
    };

    let price = Decimal::from_str(&args.price)?;
    let size = Decimal::from_str(&args.size)?;

    // 1. Discover a live 15-min market
    println!("[1/5] Discovering 15-min {} market...", coin.slug_prefix());
    let gamma = GammaClient::new();
    let markets = gamma.get_15min_markets_for_coins(&[coin]).await;
    let market = markets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No active 15-min {} market found", coin.slug_prefix()))?;
    let token_id = market
        .up_token()
        .ok_or_else(|| anyhow::anyhow!("No Up token for market"))?
        .token_id
        .clone();
    println!("  Market: {}", market.question);
    println!("  Token:  {}...{}", &token_id[..8], &token_id[token_id.len()-8..]);

    // 2. Create CLOB client with correct fee config
    println!("\n[2/5] Creating CLOB client (fee_rate_bps=1000, neg_risk=false)...");
    let wallet = Wallet::from_env(WalletConfig::mainnet())?;
    let mut config = ClobClientConfig::mainnet();
    config.taker_fee_bps = 1000;
    let mut client = ClobClient::new(wallet, config)?;
    client.authenticate().await?;
    println!("  Authenticated OK");

    // 3. Submit a GTC BUY at a realistic price that exercises rounding
    let raw_product = price * size;
    let floored = (raw_product * dec!(100)).floor() / dec!(100);
    println!("\n[3/5] Submitting test GTC BUY: {} shares @ ${} (maker={} -> floor 2dp={})...",
        size, price, raw_product, floored);

    let params = OrderParams {
        token_id: token_id.clone(),
        side: Side::Buy,
        price,
        size,
        order_type: OrderType::Gtc,
        neg_risk: false,
        presigned: None,
    };

    let result = client.submit_order(&params).await;
    match &result {
        Ok(r) => {
            println!("  ACCEPTED! order_id={}", r.order_id);
            println!("  status={:?}", r.status);

            // 4. Cancel it immediately
            println!("\n[4/5] Cancelling test order...");
            match client.cancel_order(&r.order_id).await {
                Ok(()) => println!("  Cancelled OK"),
                Err(e) => println!("  Cancel failed (non-critical): {e}"),
            }
        }
        Err(e) => {
            let err_str = format!("{e}");
            println!("  REJECTED: {err_str}");
            if err_str.contains("invalid signature") {
                println!("\n  !!! SIGNING BROKEN — check neg_risk and fee_rate_bps !!!");
            } else if err_str.contains("invalid amounts") {
                println!("\n  !!! AMOUNT PRECISION BROKEN — check maker_amount rounding !!!");
            }
            println!("\n[4/5] No order to cancel (submission failed).");
        }
    }

    // 5. Summary
    println!("\n[5/5] Summary:");
    if result.is_ok() {
        println!("  PASS: Order accepted with price={}, size={}, maker_amount={}",
            price, size, floored);
    } else {
        println!("  FAIL: Order was rejected. See error above.");
    }
    println!();

    Ok(())
}
