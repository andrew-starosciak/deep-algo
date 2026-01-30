use anyhow::{Context, Result};
use ethers::signers::LocalWallet;
use std::str::FromStr;

/// Create wallet from private key (without 0x prefix)
///
/// # Errors
/// Returns error if private key format is invalid
pub fn create_wallet_from_private_key(private_key: &str) -> Result<LocalWallet> {
    // Remove 0x prefix if present
    let key = private_key.strip_prefix("0x").unwrap_or(private_key);

    LocalWallet::from_str(key).context("Failed to create wallet from private key")
}

/// Load wallet from environment variable
///
/// Expected env var: `HYPERLIQUID_PRIVATE_KEY` (64 hex chars, with or without 0x)
///
/// # Errors
/// Returns error if environment variable is missing or invalid
pub fn load_wallet_from_env() -> Result<LocalWallet> {
    let private_key = std::env::var("HYPERLIQUID_PRIVATE_KEY")
        .context("Missing HYPERLIQUID_PRIVATE_KEY env var")?;

    create_wallet_from_private_key(&private_key)
}
