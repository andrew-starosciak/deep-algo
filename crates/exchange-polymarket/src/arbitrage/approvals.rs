//! Polymarket exchange contract approval transactions.
//!
//! Sets ERC-20 and ERC-1155 allowances so the Polymarket CTF Exchange contracts
//! can spend the wallet's USDCe and conditional tokens.

use reqwest::Client;
use tracing::{info, warn};

use super::polygon_tx::{self, LegacyTx, TxError};
use super::signer::Wallet;

// =============================================================================
// Contract Addresses (Polygon Mainnet)
// =============================================================================

/// USDCe (PoS bridged USDC) on Polygon.
const USDCE: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Conditional Tokens Framework (ERC-1155).
const CTF: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";

/// Polymarket CTF Exchange.
const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

/// Polymarket Neg Risk CTF Exchange.
const NEG_RISK_CTF_EXCHANGE: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";

/// Polymarket Neg Risk Adapter.
const NEG_RISK_ADAPTER: &str = "0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296";

/// Polygon chain ID.
const POLYGON_CHAIN_ID: u64 = 137;

/// Gas limit for approval transactions (approvals use ~46k, 100k is safe).
const APPROVAL_GAS_LIMIT: u64 = 100_000;

// =============================================================================
// ABI Encoding Helpers
// =============================================================================

/// ERC-20 `approve(address spender, uint256 amount)` selector.
const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// ERC-1155 `setApprovalForAll(address operator, bool approved)` selector.
const SET_APPROVAL_FOR_ALL_SELECTOR: [u8; 4] = [0xa2, 0x2c, 0xb4, 0x65];

/// MAX_UINT256 for unlimited approval.
const MAX_UINT256: [u8; 32] = [0xff; 32];

/// Builds ERC-20 `approve(spender, MAX_UINT256)` calldata.
fn build_erc20_approve(spender: &[u8; 20]) -> Vec<u8> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&APPROVE_SELECTOR);
    // spender address (left-padded to 32 bytes)
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(spender);
    // amount = MAX_UINT256
    data.extend_from_slice(&MAX_UINT256);
    data
}

/// Builds ERC-1155 `setApprovalForAll(operator, true)` calldata.
fn build_set_approval_for_all(operator: &[u8; 20]) -> Vec<u8> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SET_APPROVAL_FOR_ALL_SELECTOR);
    // operator address (left-padded to 32 bytes)
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(operator);
    // approved = true (uint256 value 1)
    let mut true_val = [0u8; 32];
    true_val[31] = 1;
    data.extend_from_slice(&true_val);
    data
}

/// Parses a hex address string to 20 bytes.
fn parse_address(addr: &str) -> Result<[u8; 20], TxError> {
    let stripped = addr.strip_prefix("0x").unwrap_or(addr);
    let bytes = hex::decode(stripped)
        .map_err(|e| TxError::Rlp(format!("Invalid address '{}': {}", addr, e)))?;
    if bytes.len() != 20 {
        return Err(TxError::Rlp(format!(
            "Address wrong length: {} bytes",
            bytes.len()
        )));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// =============================================================================
// Main Approval Function
// =============================================================================

/// Approval targets and their descriptions.
const APPROVAL_TARGETS: [(&str, &str); 3] = [
    (CTF_EXCHANGE, "CTF Exchange"),
    (NEG_RISK_CTF_EXCHANGE, "Neg Risk CTF Exchange"),
    (NEG_RISK_ADAPTER, "Neg Risk Adapter"),
];

/// Sets all Polymarket exchange allowances for the given wallet.
///
/// Sends 6 transactions:
/// - 3x ERC-20 `approve(target, MAX_UINT256)` on USDCe
/// - 3x ERC-1155 `setApprovalForAll(target, true)` on CTF
///
/// Returns a list of transaction hashes.
pub async fn set_polymarket_allowances(
    wallet: &Wallet,
    rpc_url: &str,
) -> Result<Vec<String>, TxError> {
    let http = Client::new();

    let usdce_addr = parse_address(USDCE)?;
    let ctf_addr = parse_address(CTF)?;

    // Get nonce and gas price
    let mut nonce = polygon_tx::get_nonce(&http, rpc_url, wallet.address()).await?;
    let gas_price = polygon_tx::get_gas_price(&http, rpc_url).await?;

    // Add 20% buffer to gas price for faster inclusion
    let gas_price = gas_price + gas_price / 5;

    info!(
        nonce,
        gas_price_gwei = gas_price / 1_000_000_000,
        "Starting approval transactions"
    );

    let mut tx_hashes = Vec::new();

    for (target_addr_str, target_name) in &APPROVAL_TARGETS {
        let target_addr = parse_address(target_addr_str)?;

        // 1. ERC-20 approve USDCe
        let approve_data = build_erc20_approve(&target_addr);
        let tx = LegacyTx {
            nonce,
            gas_price,
            gas_limit: APPROVAL_GAS_LIMIT,
            to: usdce_addr,
            value: [0u8; 32],
            data: approve_data,
        };

        let signed = polygon_tx::sign_legacy_tx(&tx, POLYGON_CHAIN_ID, wallet.expose_private_key())?;
        let hash = polygon_tx::broadcast_tx(&http, rpc_url, &signed).await?;
        info!(tx_hash = %hash, target = target_name, "ERC-20 approve sent");
        tx_hashes.push(hash);
        nonce += 1;

        // 2. ERC-1155 setApprovalForAll on CTF
        let approval_data = build_set_approval_for_all(&target_addr);
        let tx = LegacyTx {
            nonce,
            gas_price,
            gas_limit: APPROVAL_GAS_LIMIT,
            to: ctf_addr,
            value: [0u8; 32],
            data: approval_data,
        };

        let signed = polygon_tx::sign_legacy_tx(&tx, POLYGON_CHAIN_ID, wallet.expose_private_key())?;
        let hash = polygon_tx::broadcast_tx(&http, rpc_url, &signed).await?;
        info!(tx_hash = %hash, target = target_name, "ERC-1155 setApprovalForAll sent");
        tx_hashes.push(hash);
        nonce += 1;
    }

    // Wait for all receipts
    info!("Waiting for {} transactions to confirm...", tx_hashes.len());
    let mut all_success = true;

    for hash in &tx_hashes {
        match polygon_tx::wait_for_receipt(&http, rpc_url, hash, 60).await {
            Ok(true) => {}
            Ok(false) => {
                warn!(tx_hash = %hash, "Transaction reverted!");
                all_success = false;
            }
            Err(e) => {
                warn!(tx_hash = %hash, error = %e, "Failed to get receipt");
                all_success = false;
            }
        }
    }

    if all_success {
        info!("All {} approval transactions confirmed", tx_hashes.len());
    } else {
        warn!("Some approval transactions failed — check hashes on Polygonscan");
    }

    Ok(tx_hashes)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erc20_approve_calldata_format() {
        let spender = [0xAA; 20];
        let data = build_erc20_approve(&spender);

        assert_eq!(data.len(), 68); // 4 + 32 + 32
        assert_eq!(&data[0..4], &APPROVE_SELECTOR);
        // Check spender is at bytes 16..36 (12 zero-padding + 20 address)
        assert_eq!(&data[4..16], &[0u8; 12]);
        assert_eq!(&data[16..36], &[0xAA; 20]);
        // Check amount is MAX_UINT256
        assert_eq!(&data[36..68], &MAX_UINT256);
    }

    #[test]
    fn set_approval_for_all_calldata_format() {
        let operator = [0xBB; 20];
        let data = build_set_approval_for_all(&operator);

        assert_eq!(data.len(), 68); // 4 + 32 + 32
        assert_eq!(&data[0..4], &SET_APPROVAL_FOR_ALL_SELECTOR);
        assert_eq!(&data[4..16], &[0u8; 12]);
        assert_eq!(&data[16..36], &[0xBB; 20]);
        // approved = true (1)
        assert_eq!(data[67], 1);
        assert_eq!(&data[36..67], &[0u8; 31]);
    }

    #[test]
    fn parse_valid_address() {
        let addr = parse_address("0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174").unwrap();
        assert_eq!(addr[0], 0x27);
        assert_eq!(addr[19], 0x74);
    }

    #[test]
    fn parse_address_without_prefix() {
        let addr = parse_address("2791Bca1f2de4661ED88A30C99A7a9449Aa84174").unwrap();
        assert_eq!(addr[0], 0x27);
    }

    #[test]
    fn parse_invalid_address() {
        assert!(parse_address("0xinvalid").is_err());
        assert!(parse_address("0x1234").is_err()); // too short
    }
}
