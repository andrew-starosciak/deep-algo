//! Preflight validation for live trading.
//!
//! Runs comprehensive checks before going live:
//! - Wallet connectivity and authentication
//! - USDC balance verification
//! - Current positions listing
//! - Market data connectivity

use algo_trade_polymarket::arbitrage::sdk_client::{ClobClient, ClobClientConfig};
use algo_trade_polymarket::arbitrage::signer::{Wallet, WalletConfig};
use algo_trade_polymarket::gamma::GammaClient;
use algo_trade_polymarket::models::Coin;
use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;

/// Arguments for the preflight validation command.
#[derive(Args, Debug)]
pub struct PreflightArgs {
    /// Minimum required USDC balance.
    #[arg(long, default_value = "10")]
    pub min_balance: f64,

    /// Coins to check market data for.
    #[arg(long, default_value = "btc,eth")]
    pub coins: String,

    /// Show verbose output.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Preflight check result.
#[derive(Debug)]
struct CheckResult {
    name: &'static str,
    passed: bool,
    message: String,
}

impl CheckResult {
    fn pass(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            message: message.into(),
        }
    }

    fn fail(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            passed: false,
            message: message.into(),
        }
    }
}

/// Runs preflight validation checks.
pub async fn run(args: PreflightArgs) -> Result<()> {
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           PREFLIGHT VALIDATION FOR LIVE TRADING              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut results: Vec<CheckResult> = Vec::new();
    let min_balance = Decimal::try_from(args.min_balance)?;

    // 1. Check environment variables
    println!("┌─ Environment Variables ─────────────────────────────────────┐");

    let pk_result = std::env::var("POLYMARKET_PRIVATE_KEY");
    if pk_result.is_ok() {
        results.push(CheckResult::pass("ENV: POLYMARKET_PRIVATE_KEY", "Set (not shown)"));
        println!("│  ✓ POLYMARKET_PRIVATE_KEY: Set                              │");
    } else {
        results.push(CheckResult::fail("ENV: POLYMARKET_PRIVATE_KEY", "Missing"));
        println!("│  ✗ POLYMARKET_PRIVATE_KEY: Missing                          │");
    }

    let db_result = std::env::var("DATABASE_URL");
    if db_result.is_ok() {
        results.push(CheckResult::pass("ENV: DATABASE_URL", "Set"));
        println!("│  ✓ DATABASE_URL: Set                                        │");
    } else {
        results.push(CheckResult::pass("ENV: DATABASE_URL", "Missing (optional for live)"));
        println!("│  ⚠ DATABASE_URL: Missing (optional)                         │");
    }
    println!("└──────────────────────────────────────────────────────────────┘");
    println!();

    // 2. Load wallet
    println!("┌─ Wallet ─────────────────────────────────────────────────────┐");
    let wallet = match Wallet::from_env(WalletConfig::mainnet()) {
        Ok(w) => {
            let addr = w.address();
            results.push(CheckResult::pass("Wallet Load", format!("Address: {}", addr)));
            println!("│  ✓ Loaded wallet: {}  │", addr);
            Some(w)
        }
        Err(e) => {
            results.push(CheckResult::fail("Wallet Load", format!("{}", e)));
            println!("│  ✗ Failed to load wallet: {:38} │", truncate(&e.to_string(), 38));
            None
        }
    };
    println!("└──────────────────────────────────────────────────────────────┘");
    println!();

    // 3. CLOB API authentication and balance
    println!("┌─ CLOB API ──────────────────────────────────────────────────┐");
    let client = if let Some(wallet) = wallet {
        match ClobClient::new(wallet, ClobClientConfig::mainnet()) {
            Ok(mut c) => {
                // Test authentication
                match c.authenticate().await {
                    Ok(()) => {
                        results.push(CheckResult::pass("CLOB Auth", "Authenticated"));
                        println!("│  ✓ Authentication: Success                                  │");
                    }
                    Err(e) => {
                        results.push(CheckResult::fail("CLOB Auth", format!("{}", e)));
                        println!("│  ✗ Authentication: {:42} │", truncate(&e.to_string(), 42));
                    }
                }
                Some(c)
            }
            Err(e) => {
                results.push(CheckResult::fail("CLOB Client", format!("{}", e)));
                println!("│  ✗ Client creation: {:41} │", truncate(&e.to_string(), 41));
                None
            }
        }
    } else {
        println!("│  ⊘ Skipped (no wallet)                                      │");
        None
    };

    // Check balance
    if let Some(ref client) = client {
        match client.get_balance().await {
            Ok(balance) => {
                let passed = balance >= min_balance;
                let status = if passed { "✓" } else { "✗" };
                let msg = format!("{} USDC (min: {})", balance, min_balance);
                if passed {
                    results.push(CheckResult::pass("Balance", msg.clone()));
                } else {
                    results.push(CheckResult::fail("Balance", msg.clone()));
                }
                println!("│  {} Balance: {:49} │", status, msg);
            }
            Err(e) => {
                results.push(CheckResult::fail("Balance", format!("{}", e)));
                println!("│  ✗ Balance check failed: {:36} │", truncate(&e.to_string(), 36));
            }
        }
    }
    println!("└──────────────────────────────────────────────────────────────┘");
    println!();

    // 4. Current positions
    println!("┌─ Current Positions ─────────────────────────────────────────┐");
    if let Some(ref client) = client {
        match client.get_positions().await {
            Ok(positions) => {
                let active: Vec<_> = positions.iter()
                    .filter(|p| p.size > 0.0)
                    .collect();

                results.push(CheckResult::pass("Positions", format!("{} active", active.len())));
                println!("│  ✓ Found {} active position(s)                               │", active.len());

                if args.verbose && !active.is_empty() {
                    for pos in active.iter().take(5) {
                        let title = truncate(&pos.title, 40);
                        println!("│    • {} {}│", pos.outcome, title);
                    }
                    if active.len() > 5 {
                        println!("│    ... and {} more                                          │", active.len() - 5);
                    }
                }
            }
            Err(e) => {
                results.push(CheckResult::fail("Positions", format!("{}", e)));
                println!("│  ✗ Position fetch failed: {:35} │", truncate(&e.to_string(), 35));
            }
        }
    } else {
        println!("│  ⊘ Skipped (no client)                                      │");
    }
    println!("└──────────────────────────────────────────────────────────────┘");
    println!();

    // 5. Market data (Gamma API)
    println!("┌─ Market Data (Gamma API) ────────────────────────────────────┐");
    let gamma = GammaClient::new();

    // Parse coins
    let coins: Vec<Coin> = args.coins
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
        println!("│  ⚠ No valid coins specified                                 │");
    } else {
        let markets = gamma.get_15min_markets_for_coins(&coins).await;

        if markets.is_empty() {
            results.push(CheckResult::fail("Market Data", "No active 15-min markets found"));
            println!("│  ⚠ No active 15-min markets found                           │");
        } else {
            results.push(CheckResult::pass("Market Data", format!("{} markets found", markets.len())));
            println!("│  ✓ Found {} active 15-min market(s)                          │", markets.len());

            if args.verbose {
                for m in markets.iter().take(3) {
                    let q = truncate(&m.question, 50);
                    println!("│    • {}│", q);
                }
                if markets.len() > 3 {
                    println!("│    ... and {} more                                       │", markets.len() - 3);
                }
            }
        }
    }
    println!("└──────────────────────────────────────────────────────────────┘");
    println!();

    // Summary
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                         SUMMARY                              ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();

    println!("║  Passed: {:3}                                                 ║", passed);
    println!("║  Failed: {:3}                                                 ║", failed);
    println!("╠══════════════════════════════════════════════════════════════╣");

    let critical_failures: Vec<_> = results.iter()
        .filter(|r| !r.passed && is_critical(r.name))
        .collect();

    if critical_failures.is_empty() {
        println!("║  ✓ READY FOR LIVE TRADING                                    ║");
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
        println!("Recommended next steps:");
        println!("  1. Start with small bet sizes (--bet-size 1)");
        println!("  2. Monitor first few trades closely");
        println!("  3. Use --persist to save trades to database");
        println!();
        println!("Example live command:");
        println!("  cargo run -p algo-trade-cli -- cross-market-auto \\");
        println!("    --pair btc,eth \\");
        println!("    --combination coin1down_coin2up \\");
        println!("    --mode live \\");
        println!("    --bet-size 5 \\");
        println!("    --duration 1h \\");
        println!("    --persist");
    } else {
        println!("║  ✗ NOT READY - Fix critical issues first:                    ║");
        for failure in &critical_failures {
            println!("║    • {:55} ║", truncate(failure.name, 55));
        }
        println!("╚══════════════════════════════════════════════════════════════╝");
    }
    println!();

    if failed > 0 && !critical_failures.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        format!("{:width$}", s, width = max_len)
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

fn is_critical(name: &str) -> bool {
    matches!(name,
        "ENV: POLYMARKET_PRIVATE_KEY" |
        "Wallet Load" |
        "CLOB Auth" |
        "Balance"
    )
}
