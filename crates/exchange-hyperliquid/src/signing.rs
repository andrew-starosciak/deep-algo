use anyhow::Result;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Signature;
use serde_json::json;

/// Sign Hyperliquid order request with EIP-712
///
/// # Errors
/// Returns error if signing fails
pub async fn sign_order_request(
    wallet: &LocalWallet,
    order_payload: &serde_json::Value,
    nonce: u64,
) -> Result<Signature> {
    // Create message hash from order payload + nonce
    let message = json!({
        "action": order_payload,
        "nonce": nonce,
    });

    let message_str = serde_json::to_string(&message)?;
    let message_bytes = message_str.as_bytes();

    // Sign the message
    let signature = wallet.sign_message(message_bytes).await?;

    Ok(signature)
}

/// Convert signature to hex string for Hyperliquid API
#[must_use]
pub fn signature_to_hex(signature: &Signature) -> String {
    format!("0x{}", hex::encode(signature.to_vec()))
}
