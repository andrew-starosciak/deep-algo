//! Secure wallet module for Polymarket order signing.
//!
//! This module provides secure private key management for signing orders
//! on Polymarket's CLOB. It implements security best practices:
//!
//! - Private keys stored using `SecretString` (zeroized on drop)
//! - Private keys never exposed in Debug output
//! - Private keys never logged
//! - Environment variable-based configuration
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::signer::{Wallet, WalletConfig};
//!
//! // Load wallet from environment
//! std::env::set_var("POLYMARKET_PRIVATE_KEY", "0x...");
//! let wallet = Wallet::from_env(WalletConfig::mainnet())?;
//!
//! println!("Wallet address: {}", wallet.address());
//! println!("Chain ID: {}", wallet.chain_id());
//! ```
//!
//! # Security Considerations
//!
//! - Never log or print the private key
//! - Use environment variables, never hardcode keys
//! - The wallet's Debug impl only shows address and chain_id
//! - Private key is zeroized from memory when wallet is dropped

use secrecy::{ExposeSecret, SecretString};
use std::env;
use thiserror::Error;

// =============================================================================
// Constants
// =============================================================================

/// Default environment variable name for the private key.
pub const DEFAULT_PRIVATE_KEY_ENV: &str = "POLYMARKET_PRIVATE_KEY";

/// Polygon mainnet chain ID.
pub const POLYGON_MAINNET_CHAIN_ID: u64 = 137;

/// Polygon Amoy testnet chain ID.
pub const POLYGON_AMOY_CHAIN_ID: u64 = 80002;

/// Expected length of a hex-encoded private key (without 0x prefix).
const PRIVATE_KEY_HEX_LEN: usize = 64;

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur when working with the wallet.
#[derive(Debug, Error)]
pub enum WalletError {
    /// Environment variable not set or empty.
    #[error("Missing environment variable: {0}")]
    MissingEnvVar(String),

    /// Private key has invalid format.
    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    /// Signing operation failed.
    #[error("Signing failed: {0}")]
    SigningFailed(String),
}

// =============================================================================
// WalletConfig
// =============================================================================

/// Configuration for wallet initialization.
///
/// Specifies which environment variable contains the private key
/// and which blockchain network to use.
#[derive(Debug, Clone)]
pub struct WalletConfig {
    /// Environment variable name containing the private key.
    private_key_env: String,
    /// Chain ID for the target network.
    chain_id: u64,
}

impl WalletConfig {
    /// Creates a new wallet configuration.
    ///
    /// # Arguments
    /// * `private_key_env` - Name of the environment variable containing the private key
    /// * `chain_id` - Target blockchain chain ID
    #[must_use]
    pub fn new(private_key_env: impl Into<String>, chain_id: u64) -> Self {
        Self {
            private_key_env: private_key_env.into(),
            chain_id,
        }
    }

    /// Creates configuration for Polygon mainnet (chain ID 137).
    ///
    /// Uses the default environment variable `POLYMARKET_PRIVATE_KEY`.
    #[must_use]
    pub fn mainnet() -> Self {
        Self {
            private_key_env: DEFAULT_PRIVATE_KEY_ENV.to_string(),
            chain_id: POLYGON_MAINNET_CHAIN_ID,
        }
    }

    /// Creates configuration for Polygon Amoy testnet (chain ID 80002).
    ///
    /// Uses the default environment variable `POLYMARKET_PRIVATE_KEY`.
    #[must_use]
    pub fn testnet() -> Self {
        Self {
            private_key_env: DEFAULT_PRIVATE_KEY_ENV.to_string(),
            chain_id: POLYGON_AMOY_CHAIN_ID,
        }
    }

    /// Sets a custom environment variable name for the private key.
    ///
    /// # Arguments
    /// * `env_var` - Name of the environment variable
    #[must_use]
    pub fn with_env_var(mut self, env_var: impl Into<String>) -> Self {
        self.private_key_env = env_var.into();
        self
    }

    /// Sets a custom chain ID.
    ///
    /// # Arguments
    /// * `chain_id` - Target blockchain chain ID
    #[must_use]
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Returns the environment variable name.
    #[must_use]
    pub fn env_var(&self) -> &str {
        &self.private_key_env
    }

    /// Returns the chain ID.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self::mainnet()
    }
}

// =============================================================================
// Wallet
// =============================================================================

/// Secure wallet for Polymarket order signing.
///
/// Holds the private key securely using `SecretString` which:
/// - Prevents accidental logging via Debug
/// - Zeroizes memory on drop
/// - Requires explicit access via `expose_secret()`
///
/// # Security
///
/// - Private key is NEVER exposed in Debug output
/// - Private key is NEVER logged
/// - Private key is zeroized when wallet is dropped
/// - Only the address and chain_id are visible in Debug output
pub struct Wallet {
    /// The private key stored securely.
    /// SECURITY: Never log or expose this value.
    #[allow(dead_code)] // Will be used for EIP-712 signing in Phase 3
    private_key: SecretString,
    /// Derived Ethereum address (checksummed).
    address: String,
    /// Target chain ID.
    chain_id: u64,
}

impl Wallet {
    /// Creates a wallet from an environment variable.
    ///
    /// Reads the private key from the environment variable specified in the config,
    /// validates its format, and derives the Ethereum address.
    ///
    /// # Arguments
    /// * `config` - Wallet configuration specifying env var and chain ID
    ///
    /// # Errors
    /// - `WalletError::MissingEnvVar` - Environment variable not set or empty
    /// - `WalletError::InvalidPrivateKey` - Key is not valid 64-character hex
    ///
    /// # Example
    /// ```ignore
    /// std::env::set_var("POLYMARKET_PRIVATE_KEY", "0xabcd...");
    /// let wallet = Wallet::from_env(WalletConfig::mainnet())?;
    /// ```
    pub fn from_env(config: WalletConfig) -> Result<Self, WalletError> {
        let env_var = &config.private_key_env;

        // Read from environment
        let key_raw = env::var(env_var).map_err(|_| WalletError::MissingEnvVar(env_var.clone()))?;

        if key_raw.is_empty() {
            return Err(WalletError::MissingEnvVar(env_var.clone()));
        }

        // Normalize: remove 0x prefix if present
        let key_hex = key_raw.strip_prefix("0x").unwrap_or(&key_raw);

        // Validate format
        Self::validate_private_key(key_hex)?;

        // Derive address from private key
        let address = Self::derive_address(key_hex)?;

        // Store securely
        let private_key = SecretString::from(key_hex.to_string());

        Ok(Self {
            private_key,
            address,
            chain_id: config.chain_id,
        })
    }

    /// Creates a wallet from a raw private key string (for testing only).
    ///
    /// # Arguments
    /// * `private_key` - Hex-encoded private key (with or without 0x prefix)
    /// * `chain_id` - Target chain ID
    ///
    /// # Errors
    /// - `WalletError::InvalidPrivateKey` - Key is not valid 64-character hex
    ///
    /// # Security
    /// This method should only be used in tests. In production, use `from_env`.
    #[cfg(test)]
    pub fn from_private_key(private_key: &str, chain_id: u64) -> Result<Self, WalletError> {
        let key_hex = private_key.strip_prefix("0x").unwrap_or(private_key);
        Self::validate_private_key(key_hex)?;
        let address = Self::derive_address(key_hex)?;
        let secret_key = SecretString::from(key_hex.to_string());

        Ok(Self {
            private_key: secret_key,
            address,
            chain_id,
        })
    }

    /// Validates that a private key is properly formatted.
    ///
    /// Requirements:
    /// - Exactly 64 hex characters (256 bits)
    /// - All characters are valid hex (0-9, a-f, A-F)
    fn validate_private_key(key_hex: &str) -> Result<(), WalletError> {
        // Check length
        if key_hex.len() != PRIVATE_KEY_HEX_LEN {
            return Err(WalletError::InvalidPrivateKey(format!(
                "Expected {} hex characters, got {}",
                PRIVATE_KEY_HEX_LEN,
                key_hex.len()
            )));
        }

        // Check all characters are valid hex
        if !key_hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(WalletError::InvalidPrivateKey(
                "Key contains non-hexadecimal characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Derives the Ethereum address from a private key.
    ///
    /// Uses secp256k1 to derive the public key and keccak256 to hash it,
    /// producing a standard Ethereum address with EIP-55 checksum.
    fn derive_address(key_hex: &str) -> Result<String, WalletError> {
        use k256::ecdsa::SigningKey;
        use sha3::{Digest, Keccak256};

        // 1. Parse hex to 32-byte private key
        let key_bytes = hex::decode(key_hex).map_err(|e| {
            WalletError::InvalidPrivateKey(format!("Invalid hex encoding: {}", e))
        })?;

        if key_bytes.len() != 32 {
            return Err(WalletError::InvalidPrivateKey(format!(
                "Key must be 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        // 2. Create signing key from bytes
        let signing_key = SigningKey::from_slice(&key_bytes).map_err(|e| {
            WalletError::InvalidPrivateKey(format!("Invalid secp256k1 key: {}", e))
        })?;

        // 3. Get uncompressed public key (65 bytes: 0x04 || x || y)
        let verifying_key = signing_key.verifying_key();
        let public_key_bytes = verifying_key.to_encoded_point(false);
        let public_key_bytes = public_key_bytes.as_bytes();

        // 4. Keccak256 hash of public key (skip the 0x04 prefix, hash 64 bytes)
        let mut hasher = Keccak256::new();
        hasher.update(&public_key_bytes[1..]); // Skip 0x04 prefix
        let hash = hasher.finalize();

        // 5. Take last 20 bytes as address
        let address_bytes: [u8; 20] = hash[12..32]
            .try_into()
            .expect("hash slice should be 20 bytes");

        // 6. Apply EIP-55 checksum
        let address = eip55_checksum(&address_bytes);

        Ok(address)
    }

    /// Returns the wallet's Ethereum address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Returns the target chain ID.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Provides access to the private key for signing operations.
    ///
    /// # Security
    /// This method should only be called by signing code.
    /// The returned reference must NEVER be logged or stored.
    #[must_use]
    #[allow(dead_code)] // Will be used for EIP-712 signing in Phase 3
    pub(crate) fn expose_private_key(&self) -> &str {
        self.private_key.expose_secret()
    }
}

/// Applies EIP-55 mixed-case checksum to an Ethereum address.
///
/// This function takes 20 raw address bytes and returns a checksummed
/// address string with "0x" prefix.
fn eip55_checksum(address_bytes: &[u8; 20]) -> String {
    use sha3::{Digest, Keccak256};

    // Convert to lowercase hex (without 0x prefix)
    let hex_address: String = address_bytes.iter().map(|b| format!("{:02x}", b)).collect();

    // Hash the lowercase address
    let mut hasher = Keccak256::new();
    hasher.update(hex_address.as_bytes());
    let hash = hasher.finalize();

    // Apply checksum: uppercase if corresponding hash nibble >= 8
    let mut checksummed = String::with_capacity(42);
    checksummed.push_str("0x");

    for (i, c) in hex_address.chars().enumerate() {
        let hash_byte = hash[i / 2];
        let hash_nibble = if i % 2 == 0 {
            hash_byte >> 4
        } else {
            hash_byte & 0x0f
        };

        if hash_nibble >= 8 {
            checksummed.push(c.to_ascii_uppercase());
        } else {
            checksummed.push(c);
        }
    }

    checksummed
}

// Custom Debug implementation that NEVER exposes the private key
impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("address", &self.address)
            .field("chain_id", &self.chain_id)
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test private key - NOT A REAL KEY, just valid hex format
    // SECURITY: This is a dummy key for testing only
    const TEST_PRIVATE_KEY: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TEST_PRIVATE_KEY_WITH_PREFIX: &str =
        "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    // Another test key with different bytes
    const TEST_PRIVATE_KEY_2: &str =
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    // ==================== WalletConfig Tests ====================

    #[test]
    fn test_wallet_config_mainnet() {
        let config = WalletConfig::mainnet();

        assert_eq!(config.env_var(), DEFAULT_PRIVATE_KEY_ENV);
        assert_eq!(config.chain_id(), POLYGON_MAINNET_CHAIN_ID);
    }

    #[test]
    fn test_wallet_config_testnet() {
        let config = WalletConfig::testnet();

        assert_eq!(config.env_var(), DEFAULT_PRIVATE_KEY_ENV);
        assert_eq!(config.chain_id(), POLYGON_AMOY_CHAIN_ID);
    }

    #[test]
    fn test_wallet_config_custom() {
        let config = WalletConfig::new("CUSTOM_KEY", 1337);

        assert_eq!(config.env_var(), "CUSTOM_KEY");
        assert_eq!(config.chain_id(), 1337);
    }

    #[test]
    fn test_wallet_config_with_env_var() {
        let config = WalletConfig::mainnet().with_env_var("MY_PRIVATE_KEY");

        assert_eq!(config.env_var(), "MY_PRIVATE_KEY");
        assert_eq!(config.chain_id(), POLYGON_MAINNET_CHAIN_ID);
    }

    #[test]
    fn test_wallet_config_with_chain_id() {
        let config = WalletConfig::mainnet().with_chain_id(42);

        assert_eq!(config.env_var(), DEFAULT_PRIVATE_KEY_ENV);
        assert_eq!(config.chain_id(), 42);
    }

    #[test]
    fn test_wallet_config_default() {
        let config = WalletConfig::default();

        assert_eq!(config.chain_id(), POLYGON_MAINNET_CHAIN_ID);
    }

    // ==================== Wallet Creation Tests ====================

    #[test]
    fn test_wallet_from_private_key() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();

        assert!(!wallet.address().is_empty());
        assert!(wallet.address().starts_with("0x"));
        assert_eq!(wallet.chain_id(), POLYGON_MAINNET_CHAIN_ID);
    }

    #[test]
    fn test_wallet_from_private_key_with_prefix() {
        let wallet =
            Wallet::from_private_key(TEST_PRIVATE_KEY_WITH_PREFIX, POLYGON_MAINNET_CHAIN_ID)
                .unwrap();

        assert!(!wallet.address().is_empty());
        assert!(wallet.address().starts_with("0x"));
    }

    #[test]
    fn test_wallet_different_keys_different_addresses() {
        let wallet1 = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();
        let wallet2 =
            Wallet::from_private_key(TEST_PRIVATE_KEY_2, POLYGON_MAINNET_CHAIN_ID).unwrap();

        // Different private keys should produce different addresses
        assert_ne!(wallet1.address(), wallet2.address());
    }

    #[test]
    fn test_wallet_same_key_same_address() {
        let wallet1 = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();
        let wallet2 = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();

        // Same private key should produce same address
        assert_eq!(wallet1.address(), wallet2.address());
    }

    // ==================== Validation Tests ====================

    #[test]
    fn test_validate_private_key_valid() {
        assert!(Wallet::validate_private_key(TEST_PRIVATE_KEY).is_ok());
    }

    #[test]
    fn test_validate_private_key_too_short() {
        let result = Wallet::validate_private_key("0123456789abcdef");

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, WalletError::InvalidPrivateKey(_)));
        assert!(err.to_string().contains("64"));
    }

    #[test]
    fn test_validate_private_key_too_long() {
        let long_key = format!("{}ff", TEST_PRIVATE_KEY);
        let result = Wallet::validate_private_key(&long_key);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WalletError::InvalidPrivateKey(_)
        ));
    }

    #[test]
    fn test_validate_private_key_invalid_chars() {
        // 'g' is not a valid hex character
        let invalid_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg";
        let result = Wallet::validate_private_key(invalid_key);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, WalletError::InvalidPrivateKey(_)));
        assert!(err.to_string().contains("non-hexadecimal"));
    }

    #[test]
    fn test_validate_private_key_empty() {
        let result = Wallet::validate_private_key("");

        assert!(result.is_err());
    }

    // ==================== Environment Variable Tests ====================

    #[test]
    fn test_wallet_from_env_missing_var() {
        // Use a unique env var name that definitely doesn't exist
        let config = WalletConfig::mainnet().with_env_var("NONEXISTENT_WALLET_KEY_12345");
        let result = Wallet::from_env(config);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, WalletError::MissingEnvVar(_)));
        assert!(err.to_string().contains("NONEXISTENT_WALLET_KEY_12345"));
    }

    #[test]
    fn test_wallet_from_env_success() {
        // Set a test environment variable
        let env_var = "TEST_WALLET_KEY_SUCCESS";
        std::env::set_var(env_var, TEST_PRIVATE_KEY);

        let config = WalletConfig::mainnet().with_env_var(env_var);
        let result = Wallet::from_env(config);

        // Clean up
        std::env::remove_var(env_var);

        assert!(result.is_ok());
        let wallet = result.unwrap();
        assert!(!wallet.address().is_empty());
    }

    #[test]
    fn test_wallet_from_env_with_0x_prefix() {
        let env_var = "TEST_WALLET_KEY_PREFIX";
        std::env::set_var(env_var, TEST_PRIVATE_KEY_WITH_PREFIX);

        let config = WalletConfig::mainnet().with_env_var(env_var);
        let result = Wallet::from_env(config);

        std::env::remove_var(env_var);

        assert!(result.is_ok());
    }

    #[test]
    fn test_wallet_from_env_empty_value() {
        let env_var = "TEST_WALLET_KEY_EMPTY";
        std::env::set_var(env_var, "");

        let config = WalletConfig::mainnet().with_env_var(env_var);
        let result = Wallet::from_env(config);

        std::env::remove_var(env_var);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WalletError::MissingEnvVar(_)));
    }

    #[test]
    fn test_wallet_from_env_invalid_key() {
        let env_var = "TEST_WALLET_KEY_INVALID";
        std::env::set_var(env_var, "not-a-valid-key");

        let config = WalletConfig::mainnet().with_env_var(env_var);
        let result = Wallet::from_env(config);

        std::env::remove_var(env_var);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WalletError::InvalidPrivateKey(_)
        ));
    }

    // ==================== Security Tests ====================

    #[test]
    fn test_debug_does_not_expose_private_key() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();

        let debug_output = format!("{:?}", wallet);

        // Private key should NOT appear in debug output
        assert!(!debug_output.contains(TEST_PRIVATE_KEY));
        assert!(debug_output.contains("REDACTED"));
        // But address and chain_id should be visible
        assert!(debug_output.contains("address"));
        assert!(debug_output.contains("chain_id"));
    }

    #[test]
    fn test_wallet_address_format() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();

        let address = wallet.address();

        // Address should be 42 characters (0x + 40 hex chars)
        assert_eq!(address.len(), 42);
        assert!(address.starts_with("0x"));
        // All characters after 0x should be hex
        assert!(address[2..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_address_derivation_known_vector() {
        // Known test vector from Foundry's default account 0
        // This verifies our secp256k1 + keccak256 derivation is correct
        let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let expected_address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

        let wallet = Wallet::from_private_key(private_key, POLYGON_MAINNET_CHAIN_ID).unwrap();

        assert_eq!(
            wallet.address(),
            expected_address,
            "Address derivation should match known test vector"
        );
    }

    #[test]
    fn test_eip55_checksum_applied() {
        // The address should have mixed case due to EIP-55 checksum
        // Known vector: this key produces an address with mixed case
        let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = Wallet::from_private_key(private_key, POLYGON_MAINNET_CHAIN_ID).unwrap();
        let address = wallet.address();

        // Should have both uppercase and lowercase letters (EIP-55 checksum)
        let has_upper = address[2..].chars().any(|c| c.is_ascii_uppercase());
        let has_lower = address[2..].chars().any(|c| c.is_ascii_lowercase());
        assert!(has_upper && has_lower, "EIP-55 checksum should produce mixed case");
    }

    #[test]
    fn test_private_key_access_internal() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();

        // The internal method should return the key for signing
        let key = wallet.expose_private_key();
        assert_eq!(key, TEST_PRIVATE_KEY);
    }

    // ==================== WalletError Tests ====================

    #[test]
    fn test_wallet_error_display_missing_env_var() {
        let err = WalletError::MissingEnvVar("MY_KEY".to_string());
        assert!(err.to_string().contains("MY_KEY"));
        assert!(err.to_string().contains("Missing"));
    }

    #[test]
    fn test_wallet_error_display_invalid_key() {
        let err = WalletError::InvalidPrivateKey("bad format".to_string());
        assert!(err.to_string().contains("Invalid private key"));
        assert!(err.to_string().contains("bad format"));
    }

    #[test]
    fn test_wallet_error_display_signing_failed() {
        let err = WalletError::SigningFailed("timeout".to_string());
        assert!(err.to_string().contains("Signing failed"));
        assert!(err.to_string().contains("timeout"));
    }

    // ==================== Chain ID Tests ====================

    #[test]
    fn test_wallet_chain_id_mainnet() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_MAINNET_CHAIN_ID).unwrap();
        assert_eq!(wallet.chain_id(), 137);
    }

    #[test]
    fn test_wallet_chain_id_testnet() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, POLYGON_AMOY_CHAIN_ID).unwrap();
        assert_eq!(wallet.chain_id(), 80002);
    }

    #[test]
    fn test_wallet_chain_id_custom() {
        let wallet = Wallet::from_private_key(TEST_PRIVATE_KEY, 1337).unwrap();
        assert_eq!(wallet.chain_id(), 1337);
    }

    // ==================== Constants Tests ====================

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_PRIVATE_KEY_ENV, "POLYMARKET_PRIVATE_KEY");
        assert_eq!(POLYGON_MAINNET_CHAIN_ID, 137);
        assert_eq!(POLYGON_AMOY_CHAIN_ID, 80002);
    }
}
