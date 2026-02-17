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
// Redemption
// =============================================================================

/// Gas limit for redeemPositions (uses ~80-120k gas).
const REDEEM_GAS_LIMIT: u64 = 200_000;

/// Gas limit for NegRiskAdapter redeemPositions (wrapping overhead, ~150-200k gas).
const NEG_RISK_REDEEM_GAS_LIMIT: u64 = 300_000;

/// Conditional token decimals (matches USDCe's 6 decimals).
pub const CTF_DECIMALS: u32 = 6;

/// Builds `redeemPositions(address collateralToken, bytes32 parentCollectionId, bytes32 conditionId, uint256[] indexSets)` calldata.
///
/// The function selector is computed from keccak256 of the canonical signature.
fn build_redeem_positions(condition_id: &[u8; 32], index_sets: &[u32]) -> Vec<u8> {
    use sha3::{Digest, Keccak256};

    // Compute function selector: keccak256("redeemPositions(address,bytes32,bytes32,uint256[])")
    let selector = {
        let mut hasher = Keccak256::new();
        hasher.update(b"redeemPositions(address,bytes32,bytes32,uint256[])");
        let hash = hasher.finalize();
        [hash[0], hash[1], hash[2], hash[3]]
    };

    // ABI encode:
    // [0]  selector (4 bytes)
    // [4]  collateralToken (address, padded to 32 bytes) - USDCe
    // [36] parentCollectionId (bytes32) - all zeros
    // [68] conditionId (bytes32)
    // [100] offset to indexSets array (0x80 = 128 from start of params)
    // [132] length of indexSets
    // [164+] indexSets elements (each 32 bytes)

    let usdce_bytes = hex::decode("2791Bca1f2de4661ED88A30C99A7a9449Aa84174")
        .expect("valid USDCe hex");

    let mut data = Vec::with_capacity(164 + index_sets.len() * 32);

    // Function selector
    data.extend_from_slice(&selector);

    // collateralToken (USDCe address, left-padded)
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(&usdce_bytes);

    // parentCollectionId (bytes32 zero)
    data.extend_from_slice(&[0u8; 32]);

    // conditionId
    data.extend_from_slice(condition_id);

    // offset to dynamic array (4 params * 32 = 128 = 0x80)
    let mut offset = [0u8; 32];
    offset[31] = 0x80;
    data.extend_from_slice(&offset);

    // array length (uint256)
    let mut len = [0u8; 32];
    let len_bytes = (index_sets.len() as u64).to_be_bytes();
    len[24..32].copy_from_slice(&len_bytes);
    data.extend_from_slice(&len);

    // array elements
    for &idx in index_sets {
        let mut val = [0u8; 32];
        val[28..32].copy_from_slice(&(idx as u32).to_be_bytes());
        data.extend_from_slice(&val);
    }

    data
}

/// Builds NegRiskAdapter `redeemPositions(bytes32 conditionId, uint256[] amounts)` calldata.
///
/// The NegRiskAdapter has a simpler interface than the CTF — it handles the
/// collateral token and parent collection internally.
fn build_neg_risk_redeem_positions(condition_id: &[u8; 32], amounts: &[u128]) -> Vec<u8> {
    use sha3::{Digest, Keccak256};

    // Compute function selector: keccak256("redeemPositions(bytes32,uint256[])")
    let selector = {
        let mut hasher = Keccak256::new();
        hasher.update(b"redeemPositions(bytes32,uint256[])");
        let hash = hasher.finalize();
        [hash[0], hash[1], hash[2], hash[3]]
    };

    // ABI encode:
    // [0]  selector (4 bytes)
    // [4]  conditionId (bytes32) - static param
    // [36] offset to amounts array (0x40 = 64, past the 2 static slots)
    // [68] length of amounts
    // [100+] amounts elements (each uint256 = 32 bytes)

    let mut data = Vec::with_capacity(100 + amounts.len() * 32);

    // Function selector
    data.extend_from_slice(&selector);

    // conditionId (bytes32)
    data.extend_from_slice(condition_id);

    // offset to dynamic array: conditionId (32) + offset slot (32) = 64 = 0x40
    let mut offset = [0u8; 32];
    offset[31] = 0x40;
    data.extend_from_slice(&offset);

    // array length (uint256)
    let mut len = [0u8; 32];
    let len_bytes = (amounts.len() as u64).to_be_bytes();
    len[24..32].copy_from_slice(&len_bytes);
    data.extend_from_slice(&len);

    // array elements (uint256, big-endian)
    for &amount in amounts {
        let mut val = [0u8; 32];
        val[16..32].copy_from_slice(&amount.to_be_bytes());
        data.extend_from_slice(&val);
    }

    data
}

/// Redeems winning positions from resolved Polymarket markets.
///
/// For each condition ID, calls `CTF.redeemPositions(USDCe, 0x0, conditionId, indexSets)`
/// which burns the conditional tokens and returns USDCe.
///
/// # Arguments
/// * `wallet` - Wallet to sign and send from
/// * `rpc_url` - Polygon RPC URL
/// * `conditions` - List of (conditionId_hex, indexSets) to redeem
///
/// # Returns
/// Transaction hashes for each redemption.
pub async fn redeem_positions(
    wallet: &Wallet,
    rpc_url: &str,
    conditions: &[(&str, Vec<u32>)],
) -> Result<Vec<String>, TxError> {
    let http = Client::new();
    let ctf_addr = parse_address(CTF)?;

    let mut nonce = polygon_tx::get_nonce(&http, rpc_url, wallet.address()).await?;
    let gas_price = polygon_tx::get_gas_price(&http, rpc_url).await?;
    let gas_price = gas_price + gas_price / 5; // 20% buffer

    info!(
        nonce,
        gas_price_gwei = gas_price / 1_000_000_000,
        count = conditions.len(),
        "Starting redemption transactions"
    );

    let mut tx_hashes = Vec::new();

    for (condition_id_hex, index_sets) in conditions {
        let stripped = condition_id_hex.strip_prefix("0x").unwrap_or(condition_id_hex);
        let cid_bytes = hex::decode(stripped)
            .map_err(|e| TxError::Rlp(format!("Invalid conditionId '{}': {}", condition_id_hex, e)))?;
        if cid_bytes.len() != 32 {
            return Err(TxError::Rlp(format!(
                "conditionId wrong length: {} bytes",
                cid_bytes.len()
            )));
        }
        let mut cid = [0u8; 32];
        cid.copy_from_slice(&cid_bytes);

        let calldata = build_redeem_positions(&cid, index_sets);

        let tx = LegacyTx {
            nonce,
            gas_price,
            gas_limit: REDEEM_GAS_LIMIT,
            to: ctf_addr,
            value: [0u8; 32],
            data: calldata,
        };

        let signed = polygon_tx::sign_legacy_tx(&tx, POLYGON_CHAIN_ID, wallet.expose_private_key())?;
        let hash = polygon_tx::broadcast_tx(&http, rpc_url, &signed).await?;
        info!(
            tx_hash = %hash,
            condition_id = %condition_id_hex,
            index_sets = ?index_sets,
            "Redemption tx sent"
        );
        tx_hashes.push(hash);
        nonce += 1;
    }

    // Wait for receipts
    info!("Waiting for {} redemption transactions to confirm...", tx_hashes.len());
    let mut all_success = true;

    for hash in &tx_hashes {
        match polygon_tx::wait_for_receipt(&http, rpc_url, hash, 60).await {
            Ok(true) => info!(tx_hash = %hash, "Redemption confirmed"),
            Ok(false) => {
                warn!(tx_hash = %hash, "Redemption reverted!");
                all_success = false;
            }
            Err(e) => {
                warn!(tx_hash = %hash, error = %e, "Failed to get receipt");
                all_success = false;
            }
        }
    }

    if all_success {
        info!("All {} redemptions confirmed", tx_hashes.len());
    } else {
        warn!("Some redemptions failed — check hashes on Polygonscan");
    }

    Ok(tx_hashes)
}

/// Redeems neg_risk positions via the NegRiskAdapter contract.
///
/// For neg_risk markets (like 15-minute binary options), tokens are wrapped by
/// the NegRiskAdapter. Standard `CTF.redeemPositions()` won't work because the
/// tokens have a non-zero parentCollectionId. The NegRiskAdapter handles this
/// internally via `redeemPositions(bytes32 conditionId, uint256[] amounts)`.
///
/// # Arguments
/// * `wallet` - Wallet to sign and send from
/// * `rpc_url` - Polygon RPC URL
/// * `conditions` - List of (conditionId_hex, amounts_per_outcome_slot)
///
/// # Returns
/// Transaction hashes for each redemption.
pub async fn redeem_neg_risk_positions(
    wallet: &Wallet,
    rpc_url: &str,
    conditions: &[(&str, Vec<u128>)],
) -> Result<Vec<String>, TxError> {
    let http = Client::new();
    let adapter_addr = parse_address(NEG_RISK_ADAPTER)?;

    let mut nonce = polygon_tx::get_nonce(&http, rpc_url, wallet.address()).await?;
    let gas_price = polygon_tx::get_gas_price(&http, rpc_url).await?;
    let gas_price = gas_price + gas_price / 5; // 20% buffer

    info!(
        nonce,
        gas_price_gwei = gas_price / 1_000_000_000,
        count = conditions.len(),
        "Starting neg_risk redemption transactions via NegRiskAdapter"
    );

    let mut tx_hashes = Vec::new();

    for (condition_id_hex, amounts) in conditions {
        let stripped = condition_id_hex.strip_prefix("0x").unwrap_or(condition_id_hex);
        let cid_bytes = hex::decode(stripped)
            .map_err(|e| TxError::Rlp(format!("Invalid conditionId '{}': {}", condition_id_hex, e)))?;
        if cid_bytes.len() != 32 {
            return Err(TxError::Rlp(format!(
                "conditionId wrong length: {} bytes",
                cid_bytes.len()
            )));
        }
        let mut cid = [0u8; 32];
        cid.copy_from_slice(&cid_bytes);

        let calldata = build_neg_risk_redeem_positions(&cid, amounts);

        let tx = LegacyTx {
            nonce,
            gas_price,
            gas_limit: NEG_RISK_REDEEM_GAS_LIMIT,
            to: adapter_addr,
            value: [0u8; 32],
            data: calldata,
        };

        let signed = polygon_tx::sign_legacy_tx(&tx, POLYGON_CHAIN_ID, wallet.expose_private_key())?;
        let hash = polygon_tx::broadcast_tx(&http, rpc_url, &signed).await?;
        info!(
            tx_hash = %hash,
            condition_id = %condition_id_hex,
            amounts = ?amounts,
            "Neg risk redemption tx sent to NegRiskAdapter"
        );
        tx_hashes.push(hash);
        nonce += 1;
    }

    // Wait for receipts
    info!("Waiting for {} neg_risk redemption transactions to confirm...", tx_hashes.len());
    let mut all_success = true;

    for hash in &tx_hashes {
        match polygon_tx::wait_for_receipt(&http, rpc_url, hash, 60).await {
            Ok(true) => info!(tx_hash = %hash, "Neg risk redemption confirmed"),
            Ok(false) => {
                warn!(tx_hash = %hash, "Neg risk redemption reverted!");
                all_success = false;
            }
            Err(e) => {
                warn!(tx_hash = %hash, error = %e, "Failed to get neg risk receipt");
                all_success = false;
            }
        }
    }

    if all_success {
        info!("All {} neg_risk redemptions confirmed", tx_hashes.len());
    } else {
        warn!("Some neg_risk redemptions failed — check hashes on Polygonscan");
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

    #[test]
    fn redeem_positions_calldata_format() {
        let cid = [0x42; 32];
        let data = build_redeem_positions(&cid, &[1, 2]);

        // 4 (selector) + 32 (collateral) + 32 (parent) + 32 (cid) + 32 (offset) + 32 (len) + 2*32 (elements)
        assert_eq!(data.len(), 228);
        // conditionId at offset 68..100
        assert_eq!(&data[68..100], &[0x42; 32]);
        // array length at offset 132..164: should be 2
        assert_eq!(data[163], 2);
        // first element = 1 (indexSet)
        assert_eq!(data[195], 1);
        // second element = 2 (indexSet)
        assert_eq!(data[227], 2);
    }

    #[test]
    fn neg_risk_redeem_calldata_format() {
        let cid = [0xAB; 32];
        let amounts = vec![100_000_000u128, 0u128]; // 100 tokens for outcome 0, 0 for outcome 1

        let data = build_neg_risk_redeem_positions(&cid, &amounts);

        // 4 (selector) + 32 (cid) + 32 (offset) + 32 (len) + 2*32 (elements) = 164
        assert_eq!(data.len(), 164);

        // conditionId at offset 4..36
        assert_eq!(&data[4..36], &[0xAB; 32]);

        // offset at 36..68: should be 0x40 = 64
        assert_eq!(data[67], 0x40);

        // array length at 68..100: should be 2
        assert_eq!(data[99], 2);

        // first amount (100_000_000 = 0x05F5E100) in big-endian u128 at offset 100..132
        let amount_bytes = &data[100..132];
        // last 4 bytes should contain the value
        assert_eq!(amount_bytes[28], 0x05);
        assert_eq!(amount_bytes[29], 0xF5);
        assert_eq!(amount_bytes[30], 0xE1);
        assert_eq!(amount_bytes[31], 0x00);
        // rest should be zero
        assert_eq!(&amount_bytes[0..28], &[0u8; 28]);

        // second amount (0) at offset 132..164
        assert_eq!(&data[132..164], &[0u8; 32]);
    }

    #[test]
    fn neg_risk_redeem_selector_differs_from_ctf() {
        use sha3::{Digest, Keccak256};

        let ctf_selector = {
            let mut h = Keccak256::new();
            h.update(b"redeemPositions(address,bytes32,bytes32,uint256[])");
            let hash = h.finalize();
            [hash[0], hash[1], hash[2], hash[3]]
        };

        let neg_risk_selector = {
            let mut h = Keccak256::new();
            h.update(b"redeemPositions(bytes32,uint256[])");
            let hash = h.finalize();
            [hash[0], hash[1], hash[2], hash[3]]
        };

        // Different function signatures must produce different selectors
        assert_ne!(ctf_selector, neg_risk_selector);

        // Verify our calldata uses the correct selectors
        let cid = [0x01; 32];
        let ctf_data = build_redeem_positions(&cid, &[1]);
        let neg_data = build_neg_risk_redeem_positions(&cid, &[100]);

        assert_eq!(&ctf_data[0..4], &ctf_selector);
        assert_eq!(&neg_data[0..4], &neg_risk_selector);
    }
}
