//! CLI command to set Polymarket exchange contract allowances.

use anyhow::Result;
use clap::Args;

use algo_trade_polymarket::arbitrage::{approvals, Wallet, WalletConfig};

/// Set Polymarket exchange contract allowances.
///
/// Sends 6 on-chain transactions to approve the CTF Exchange, Neg Risk Exchange,
/// and Neg Risk Adapter contracts to spend your USDCe and conditional tokens.
/// This is required once before live trading.
#[derive(Args, Debug)]
pub struct ApproveAllowancesArgs {
    /// Polygon RPC URL
    #[arg(long, default_value = "https://polygon-rpc.com")]
    pub rpc_url: String,
}

/// Runs the approve-allowances command.
pub async fn run(args: ApproveAllowancesArgs) -> Result<()> {
    let wallet = Wallet::from_env(WalletConfig::mainnet())?;

    println!("Wallet:  {}", wallet.address());
    println!("RPC:     {}", args.rpc_url);
    println!();
    println!("Sending 6 approval transactions...");
    println!();

    let tx_hashes = approvals::set_polymarket_allowances(&wallet, &args.rpc_url).await
        .map_err(|e| anyhow::anyhow!("Approval failed: {}", e))?;

    println!();
    println!("Completed! Transaction hashes:");
    for (i, hash) in tx_hashes.iter().enumerate() {
        println!("  {}. https://polygonscan.com/tx/{}", i + 1, hash);
    }
    println!();
    println!("Run 'preflight --coins btc,eth' to verify allowances.");

    Ok(())
}
