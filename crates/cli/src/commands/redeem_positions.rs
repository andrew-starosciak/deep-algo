//! CLI command to redeem winning positions from resolved Polymarket markets.

use anyhow::Result;
use clap::Args;

use algo_trade_polymarket::arbitrage::{approvals, Wallet, WalletConfig};

/// Redeem winning positions from resolved Polymarket markets.
///
/// Burns conditional tokens for resolved markets and returns USDCe to your wallet.
#[derive(Args, Debug)]
pub struct RedeemPositionsArgs {
    /// Polygon RPC URL
    #[arg(long, default_value = "https://polygon-rpc.com")]
    pub rpc_url: String,

    /// Condition IDs to redeem (hex, comma-separated).
    /// Each entry is "conditionId:indexSet" where indexSet is 1 (outcome 0) or 2 (outcome 1).
    #[arg(long, value_delimiter = ',')]
    pub conditions: Vec<String>,
}

/// Runs the redeem-positions command.
pub async fn run(args: RedeemPositionsArgs) -> Result<()> {
    let wallet = Wallet::from_env(WalletConfig::mainnet())?;

    println!("Wallet:  {}", wallet.address());
    println!("RPC:     {}", args.rpc_url);
    println!();

    // Parse conditions: "conditionId:indexSet1:indexSet2..."
    let mut conditions: Vec<(&str, Vec<u32>)> = Vec::new();
    let parsed: Vec<(String, Vec<u32>)> = args
        .conditions
        .iter()
        .map(|s| {
            let parts: Vec<&str> = s.split(':').collect();
            let cid = parts[0].to_string();
            let index_sets: Vec<u32> = parts[1..]
                .iter()
                .filter_map(|p| p.parse().ok())
                .collect();
            (cid, if index_sets.is_empty() { vec![1, 2] } else { index_sets })
        })
        .collect();

    for (cid, idx) in &parsed {
        conditions.push((cid.as_str(), idx.clone()));
        println!("  Redeem: {} indexSets={:?}", cid, idx);
    }
    println!();

    println!("Sending {} redemption transactions...", conditions.len());
    println!();

    let tx_hashes = approvals::redeem_positions(&wallet, &args.rpc_url, &conditions)
        .await
        .map_err(|e| anyhow::anyhow!("Redemption failed: {}", e))?;

    println!();
    println!("Completed! Transaction hashes:");
    for (i, hash) in tx_hashes.iter().enumerate() {
        println!("  {}. https://polygonscan.com/tx/{}", i + 1, hash);
    }

    Ok(())
}
