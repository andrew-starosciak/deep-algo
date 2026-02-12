//! Minimal Polygon (EVM) transaction construction and broadcasting.
//!
//! Supports legacy (pre-EIP-1559) transactions with EIP-155 replay protection.
//! Uses existing `k256` and `sha3` crates — no additional dependencies.

use k256::ecdsa::{RecoveryId, SigningKey};
use reqwest::Client;
use sha3::{Digest, Keccak256};
use thiserror::Error;
use tracing::{debug, info};

// =============================================================================
// Errors
// =============================================================================

/// Errors from transaction construction and broadcasting.
#[derive(Debug, Error)]
pub enum TxError {
    /// RLP encoding error.
    #[error("RLP encoding error: {0}")]
    Rlp(String),

    /// Transaction signing failed.
    #[error("Signing failed: {0}")]
    Signing(String),

    /// RPC request failed.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Transaction was rejected.
    #[error("Transaction rejected: {0}")]
    Rejected(String),

    /// Timeout waiting for receipt.
    #[error("Timeout waiting for transaction receipt")]
    Timeout,

    /// Invalid private key.
    #[error("Invalid private key: {0}")]
    InvalidKey(String),
}

// =============================================================================
// RLP Encoding (minimal, internal)
// =============================================================================

/// RLP-encodes a byte slice.
fn rlp_encode_bytes(data: &[u8]) -> Vec<u8> {
    if data.len() == 1 && data[0] < 0x80 {
        // Single byte < 0x80: encoded as itself
        vec![data[0]]
    } else if data.len() <= 55 {
        let mut out = Vec::with_capacity(1 + data.len());
        out.push(0x80 + data.len() as u8);
        out.extend_from_slice(data);
        out
    } else {
        let len_bytes = to_minimal_be_bytes_usize(data.len());
        let mut out = Vec::with_capacity(1 + len_bytes.len() + data.len());
        out.push(0xb7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(data);
        out
    }
}

/// RLP-encodes a u64 value.
fn rlp_encode_u64(val: u64) -> Vec<u8> {
    if val == 0 {
        rlp_encode_bytes(&[])
    } else {
        let bytes = to_minimal_be_bytes_u64(val);
        rlp_encode_bytes(&bytes)
    }
}

/// RLP-encodes a big-endian U256 value (as raw bytes, stripped of leading zeros).
fn rlp_encode_uint_bytes(val: &[u8]) -> Vec<u8> {
    let stripped = strip_leading_zeros(val);
    if stripped.is_empty() {
        rlp_encode_bytes(&[])
    } else {
        rlp_encode_bytes(stripped)
    }
}

/// RLP-encodes a list of already-encoded items.
fn rlp_encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flat_map(|i| i.iter().copied()).collect();
    let payload_len = payload.len();

    if payload_len <= 55 {
        let mut out = Vec::with_capacity(1 + payload_len);
        out.push(0xc0 + payload_len as u8);
        out.extend_from_slice(&payload);
        out
    } else {
        let len_bytes = to_minimal_be_bytes_usize(payload_len);
        let mut out = Vec::with_capacity(1 + len_bytes.len() + payload_len);
        out.push(0xf7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(&payload);
        out
    }
}

/// Converts u64 to minimal big-endian bytes (no leading zeros).
fn to_minimal_be_bytes_u64(val: u64) -> Vec<u8> {
    let bytes = val.to_be_bytes();
    let stripped = strip_leading_zeros(&bytes);
    if stripped.is_empty() {
        vec![0]
    } else {
        stripped.to_vec()
    }
}

/// Converts usize to minimal big-endian bytes.
fn to_minimal_be_bytes_usize(val: usize) -> Vec<u8> {
    to_minimal_be_bytes_u64(val as u64)
}

/// Strips leading zero bytes.
fn strip_leading_zeros(data: &[u8]) -> &[u8] {
    let start = data.iter().position(|&b| b != 0).unwrap_or(data.len());
    &data[start..]
}

// =============================================================================
// Transaction Types
// =============================================================================

/// A legacy (type 0) Ethereum transaction.
pub struct LegacyTx {
    /// Transaction nonce.
    pub nonce: u64,
    /// Gas price in wei.
    pub gas_price: u64,
    /// Gas limit.
    pub gas_limit: u64,
    /// Recipient address (20 bytes).
    pub to: [u8; 20],
    /// Value in wei (U256 big-endian, typically 0 for approvals).
    pub value: [u8; 32],
    /// Calldata.
    pub data: Vec<u8>,
}

// =============================================================================
// Transaction Signing (EIP-155)
// =============================================================================

/// Signs a legacy transaction with EIP-155 replay protection.
///
/// Returns the RLP-encoded signed transaction ready for broadcasting.
pub fn sign_legacy_tx(
    tx: &LegacyTx,
    chain_id: u64,
    private_key_hex: &str,
) -> Result<Vec<u8>, TxError> {
    // Build unsigned RLP for signing (includes chain_id, 0, 0 per EIP-155)
    let unsigned_rlp = rlp_encode_list(&[
        rlp_encode_u64(tx.nonce),
        rlp_encode_u64(tx.gas_price),
        rlp_encode_u64(tx.gas_limit),
        rlp_encode_bytes(&tx.to),
        rlp_encode_uint_bytes(&tx.value),
        rlp_encode_bytes(&tx.data),
        rlp_encode_u64(chain_id),
        rlp_encode_bytes(&[]),
        rlp_encode_bytes(&[]),
    ]);

    // Hash for signing
    let hash = Keccak256::digest(&unsigned_rlp);

    // Sign with k256
    let key_bytes =
        hex::decode(private_key_hex.strip_prefix("0x").unwrap_or(private_key_hex))
            .map_err(|e| TxError::InvalidKey(e.to_string()))?;

    let signing_key =
        SigningKey::from_bytes(key_bytes.as_slice().into())
            .map_err(|e| TxError::InvalidKey(e.to_string()))?;

    let (signature, recovery_id) =
        signing_key
            .sign_prehash_recoverable(&hash)
            .map_err(|e| TxError::Signing(e.to_string()))?;

    let r_bytes = signature.r().to_bytes();
    let s_bytes = signature.s().to_bytes();

    // EIP-155: v = chain_id * 2 + 35 + recovery_id
    let v = chain_id * 2 + 35 + recovery_id.to_byte() as u64;

    // Build signed RLP
    let signed_rlp = rlp_encode_list(&[
        rlp_encode_u64(tx.nonce),
        rlp_encode_u64(tx.gas_price),
        rlp_encode_u64(tx.gas_limit),
        rlp_encode_bytes(&tx.to),
        rlp_encode_uint_bytes(&tx.value),
        rlp_encode_bytes(&tx.data),
        rlp_encode_u64(v),
        rlp_encode_uint_bytes(r_bytes.as_slice()),
        rlp_encode_uint_bytes(s_bytes.as_slice()),
    ]);

    Ok(signed_rlp)
}

// =============================================================================
// RPC Helpers
// =============================================================================

/// Gets the transaction count (nonce) for an address.
pub async fn get_nonce(http: &Client, rpc_url: &str, address: &str) -> Result<u64, TxError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_getTransactionCount",
        "params": [address, "latest"],
        "id": 1
    });

    let resp: serde_json::Value = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    parse_hex_u64(&resp)
}

/// Gets the current gas price.
pub async fn get_gas_price(http: &Client, rpc_url: &str) -> Result<u64, TxError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_gasPrice",
        "params": [],
        "id": 1
    });

    let resp: serde_json::Value = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    parse_hex_u64(&resp)
}

/// Broadcasts a signed transaction.
///
/// Returns the transaction hash.
pub async fn broadcast_tx(
    http: &Client,
    rpc_url: &str,
    signed_tx: &[u8],
) -> Result<String, TxError> {
    let tx_hex = format!("0x{}", hex::encode(signed_tx));

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_sendRawTransaction",
        "params": [tx_hex],
        "id": 1
    });

    let resp: serde_json::Value = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if let Some(error) = resp.get("error") {
        let msg = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(TxError::Rejected(msg.to_string()));
    }

    resp.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| TxError::Rpc("No tx hash in response".to_string()))
}

/// Waits for a transaction receipt, polling every 2 seconds.
///
/// Returns `true` if the transaction succeeded (status = 1).
pub async fn wait_for_receipt(
    http: &Client,
    rpc_url: &str,
    tx_hash: &str,
    timeout_secs: u64,
) -> Result<bool, TxError> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        if start.elapsed() > timeout {
            return Err(TxError::Timeout);
        }

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getTransactionReceipt",
            "params": [tx_hash],
            "id": 1
        });

        let resp: serde_json::Value = http
            .post(rpc_url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        if let Some(result) = resp.get("result") {
            if !result.is_null() {
                let status = result
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("0x0");
                let success = status == "0x1";

                if success {
                    info!(tx_hash, "Transaction confirmed");
                } else {
                    debug!(tx_hash, "Transaction reverted");
                }

                return Ok(success);
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// Parses a hex string result from a JSON-RPC response to u64.
fn parse_hex_u64(resp: &serde_json::Value) -> Result<u64, TxError> {
    if let Some(error) = resp.get("error") {
        let msg = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        return Err(TxError::Rpc(msg.to_string()));
    }

    let hex_str = resp
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or_else(|| TxError::Rpc("No result in response".to_string()))?;

    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    u64::from_str_radix(stripped, 16)
        .map_err(|e| TxError::Rpc(format!("Failed to parse hex '{}': {}", hex_str, e)))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlp_encode_empty_bytes() {
        assert_eq!(rlp_encode_bytes(&[]), vec![0x80]);
    }

    #[test]
    fn rlp_encode_single_byte() {
        assert_eq!(rlp_encode_bytes(&[0x42]), vec![0x42]);
        assert_eq!(rlp_encode_bytes(&[0x80]), vec![0x81, 0x80]);
    }

    #[test]
    fn rlp_encode_short_string() {
        let data = b"hello";
        let encoded = rlp_encode_bytes(data);
        assert_eq!(encoded[0], 0x80 + 5);
        assert_eq!(&encoded[1..], data);
    }

    #[test]
    fn rlp_encode_zero() {
        // 0 encodes as empty bytes
        assert_eq!(rlp_encode_u64(0), vec![0x80]);
    }

    #[test]
    fn rlp_encode_small_int() {
        assert_eq!(rlp_encode_u64(1), vec![0x01]);
        assert_eq!(rlp_encode_u64(127), vec![0x7f]);
        assert_eq!(rlp_encode_u64(128), vec![0x81, 0x80]);
    }

    #[test]
    fn rlp_encode_u64_large() {
        // 1000 = 0x03E8
        let encoded = rlp_encode_u64(1000);
        assert_eq!(encoded, vec![0x82, 0x03, 0xe8]);
    }

    #[test]
    fn rlp_encode_empty_list() {
        assert_eq!(rlp_encode_list(&[]), vec![0xc0]);
    }

    #[test]
    fn rlp_encode_simple_list() {
        let items = vec![rlp_encode_u64(1), rlp_encode_u64(2)];
        let encoded = rlp_encode_list(&items);
        assert_eq!(encoded, vec![0xc2, 0x01, 0x02]);
    }

    #[test]
    fn sign_tx_produces_valid_output() {
        // Use hardhat account 0 for testing
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

        let tx = LegacyTx {
            nonce: 0,
            gas_price: 30_000_000_000, // 30 gwei
            gas_limit: 100_000,
            to: [0u8; 20],
            value: [0u8; 32],
            data: vec![],
        };

        let signed = sign_legacy_tx(&tx, 137, key).unwrap();

        // Should be a valid RLP-encoded transaction (starts with 0xf8 or higher)
        assert!(!signed.is_empty());
        assert!(signed[0] >= 0xc0, "Should be an RLP list");
    }

    #[test]
    fn sign_tx_different_nonces_different_output() {
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

        let tx1 = LegacyTx {
            nonce: 0,
            gas_price: 30_000_000_000,
            gas_limit: 100_000,
            to: [0u8; 20],
            value: [0u8; 32],
            data: vec![],
        };

        let tx2 = LegacyTx {
            nonce: 1,
            gas_price: 30_000_000_000,
            gas_limit: 100_000,
            to: [0u8; 20],
            value: [0u8; 32],
            data: vec![],
        };

        let signed1 = sign_legacy_tx(&tx1, 137, key).unwrap();
        let signed2 = sign_legacy_tx(&tx2, 137, key).unwrap();

        assert_ne!(signed1, signed2);
    }

    #[test]
    fn rlp_encode_uint_bytes_strips_zeros() {
        let val = [0u8; 32]; // zero
        assert_eq!(rlp_encode_uint_bytes(&val), vec![0x80]);

        let mut val = [0u8; 32];
        val[31] = 1; // 1
        assert_eq!(rlp_encode_uint_bytes(&val), vec![0x01]);
    }
}
