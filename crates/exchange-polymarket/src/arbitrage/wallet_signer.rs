//! Real EIP-712 wallet signer implementing the `OrderSigner` trait.
//!
//! This bridges the [`super::signer::Wallet`] with the [`super::presigned_orders::OrderSigner`]
//! trait, enabling pre-signed order pools to use real EIP-712 signatures.

use rust_decimal::Decimal;
use std::sync::Arc;

use super::eip712::{self, Eip712Config, Eip712Error, SIDE_BUY, SIDE_SELL};
use super::execution::Side;
use super::presigned_orders::{OrderSigner, PreSignedError};
use super::signer::Wallet;

/// A real wallet-backed order signer using EIP-712 typed data signing.
///
/// Wraps a [`Wallet`] and signs orders using the Polymarket CTF Exchange
/// EIP-712 domain. All signing happens locally via k256.
pub struct WalletSigner {
    wallet: Arc<Wallet>,
    config: Eip712Config,
}

impl WalletSigner {
    /// Creates a new wallet signer.
    ///
    /// # Arguments
    /// * `wallet` - Shared wallet containing the private key
    /// * `config` - EIP-712 configuration (chain ID, neg_risk flag)
    #[must_use]
    pub fn new(wallet: Arc<Wallet>, config: Eip712Config) -> Self {
        Self { wallet, config }
    }

    /// Creates a signer for Polygon mainnet standard markets.
    #[must_use]
    pub fn mainnet(wallet: Arc<Wallet>) -> Self {
        Self::new(wallet, Eip712Config::default())
    }

    /// Creates a signer for neg-risk markets.
    #[must_use]
    pub fn neg_risk(wallet: Arc<Wallet>) -> Self {
        Self::new(
            wallet,
            Eip712Config {
                neg_risk: true,
                ..Default::default()
            },
        )
    }
}

impl std::fmt::Debug for WalletSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletSigner")
            .field("address", &self.wallet.address())
            .field("config", &self.config)
            .finish()
    }
}

fn side_to_eip712(side: Side) -> u8 {
    match side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    }
}

#[async_trait::async_trait]
impl OrderSigner for WalletSigner {
    async fn sign_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
        nonce: &str,
        expiration: u64,
    ) -> Result<String, PreSignedError> {
        let nonce_u64: u64 = nonce
            .parse()
            .map_err(|e| PreSignedError::SigningFailed(format!("Invalid nonce: {}", e)))?;

        let eip712_side = side_to_eip712(side);

        let order = eip712::build_order(&eip712::BuildOrderParams {
            maker_address: self.wallet.address(),
            token_id,
            side: eip712_side,
            price,
            size,
            expiration_secs: expiration,
            nonce: nonce_u64,
            fee_rate_bps: 0, // CLOB handles fees
        })
        .map_err(|e: Eip712Error| PreSignedError::SigningFailed(e.to_string()))?;

        let signature = eip712::sign_order(&order, &self.config, self.wallet.expose_private_key())
            .map_err(|e: Eip712Error| PreSignedError::SigningFailed(e.to_string()))?;

        Ok(signature)
    }

    fn address(&self) -> &str {
        self.wallet.address()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn test_wallet() -> Arc<Wallet> {
        let wallet = Wallet::from_private_key(
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            137,
        )
        .unwrap();
        Arc::new(wallet)
    }

    #[test]
    fn wallet_signer_address_matches_wallet() {
        let wallet = test_wallet();
        let signer = WalletSigner::mainnet(wallet.clone());
        assert_eq!(signer.address(), wallet.address());
    }

    #[tokio::test]
    async fn wallet_signer_produces_real_signature() {
        let wallet = test_wallet();
        let signer = WalletSigner::mainnet(wallet);

        let sig = signer
            .sign_order("12345", Side::Buy, dec!(0.50), dec!(100), "0", 1700000000)
            .await
            .unwrap();

        assert!(sig.starts_with("0x"));
        assert_eq!(sig.len(), 132); // 0x + 130 hex chars
    }

    #[tokio::test]
    async fn wallet_signer_sell_side() {
        let wallet = test_wallet();
        let signer = WalletSigner::mainnet(wallet);

        let sig = signer
            .sign_order("12345", Side::Sell, dec!(0.60), dec!(50), "0", 1700000000)
            .await
            .unwrap();

        assert!(sig.starts_with("0x"));
        assert_eq!(sig.len(), 132);
    }

    #[tokio::test]
    async fn wallet_signer_neg_risk() {
        let wallet = test_wallet();
        let signer_std = WalletSigner::mainnet(wallet.clone());
        let signer_neg = WalletSigner::neg_risk(wallet);

        let sig_std = signer_std
            .sign_order("12345", Side::Buy, dec!(0.50), dec!(100), "0", 1700000000)
            .await
            .unwrap();
        let sig_neg = signer_neg
            .sign_order("12345", Side::Buy, dec!(0.50), dec!(100), "0", 1700000000)
            .await
            .unwrap();

        // Different exchange addresses → different domain separators → different signatures
        assert_ne!(sig_std, sig_neg);
    }

    #[tokio::test]
    async fn wallet_signer_invalid_nonce() {
        let wallet = test_wallet();
        let signer = WalletSigner::mainnet(wallet);

        let result = signer
            .sign_order("12345", Side::Buy, dec!(0.50), dec!(100), "not_a_number", 0)
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn wallet_signer_debug_no_key() {
        let wallet = test_wallet();
        let signer = WalletSigner::mainnet(wallet);
        let debug = format!("{:?}", signer);
        assert!(debug.contains("WalletSigner"));
        assert!(debug.contains("0x"));
        assert!(!debug.contains("ac0974bec39a17e"));
    }
}
