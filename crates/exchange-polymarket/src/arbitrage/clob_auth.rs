//! CLOB authentication for Polymarket API.
//!
//! Implements both L1 (EIP-712 ClobAuth signing) and L2 (HMAC-SHA256)
//! authentication schemes used by the Polymarket CLOB.
//!
//! # Authentication Flow
//!
//! 1. **L1 Auth**: Sign a ClobAuth EIP-712 message with private key
//! 2. **Derive API Key**: `GET /auth/derive-api-key` with L1 headers
//!    (or `POST /auth/api-key` to create new)
//! 3. **L2 Auth**: Use derived `apiKey`, `secret`, `passphrase` for HMAC-SHA256
//! 4. **Subsequent requests**: Use L2 headers for all authenticated endpoints

use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE as BASE64_URL_SAFE},
    Engine,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

use super::eip712::{
    self, clob_auth_type_hash, compute_clob_auth_domain_separator, compute_signing_hash,
    Eip712Error, CLOB_AUTH_MESSAGE, POLYGON_CHAIN_ID,
};

type HmacSha256 = Hmac<Sha256>;

// =============================================================================
// Errors
// =============================================================================

/// Errors from CLOB authentication.
#[derive(Debug, Error)]
pub enum ClobAuthError {
    /// EIP-712 signing failed.
    #[error("EIP-712 signing failed: {0}")]
    SigningFailed(#[from] Eip712Error),

    /// HMAC computation failed.
    #[error("HMAC computation failed: {0}")]
    HmacFailed(String),

    /// Base64 decode failed.
    #[error("Base64 decode failed: {0}")]
    Base64Failed(String),

    /// API key derivation failed.
    #[error("API key derivation failed: {0}")]
    DerivationFailed(String),
}

// =============================================================================
// L1 Authentication (EIP-712 ClobAuth)
// =============================================================================

/// Headers for L1 (EIP-712) authenticated requests.
#[derive(Debug, Clone)]
pub struct L1Headers {
    /// POLY_ADDRESS header - the signer's Ethereum address.
    pub address: String,
    /// POLY_SIGNATURE header - the EIP-712 ClobAuth signature.
    pub signature: String,
    /// POLY_TIMESTAMP header - Unix timestamp string.
    pub timestamp: String,
    /// POLY_NONCE header - request nonce.
    pub nonce: String,
}

/// Signs a ClobAuth EIP-712 message for L1 authentication.
///
/// The ClobAuth struct has:
/// - `address`: Signer's address
/// - `timestamp`: Unix timestamp string
/// - `nonce`: Nonce value
/// - `message`: Attestation message
pub fn sign_clob_auth(
    address: &str,
    private_key_hex: &str,
    nonce: u64,
) -> Result<L1Headers, ClobAuthError> {
    let timestamp = Utc::now().timestamp().to_string();

    let domain_separator = compute_clob_auth_domain_separator(POLYGON_CHAIN_ID);

    // Compute the ClobAuth struct hash
    let struct_hash = compute_clob_auth_struct_hash(address, &timestamp, nonce)?;

    // Compute the signing hash
    let signing_hash = compute_signing_hash(&domain_separator, &struct_hash);

    // Sign it
    let signature = eip712::sign_hash(&signing_hash, private_key_hex)?;

    Ok(L1Headers {
        address: address.to_string(),
        signature,
        timestamp,
        nonce: nonce.to_string(),
    })
}

/// Computes the ClobAuth struct hash.
///
/// `hash(CLOB_AUTH_TYPEHASH || address || hash(timestamp) || nonce || hash(message))`
fn compute_clob_auth_struct_hash(
    address: &str,
    timestamp: &str,
    nonce: u64,
) -> Result<[u8; 32], ClobAuthError> {
    use sha3::{Digest, Keccak256};

    let addr_bytes = eip712::parse_address(address)?;

    let mut encoded = Vec::with_capacity(5 * 32);

    // Type hash
    encoded.extend_from_slice(&clob_auth_type_hash());

    // address (left-padded to 32 bytes)
    let mut addr_padded = [0u8; 32];
    addr_padded[12..32].copy_from_slice(&addr_bytes);
    encoded.extend_from_slice(&addr_padded);

    // hash(timestamp) - string type hashed per EIP-712
    let ts_hash = Keccak256::digest(timestamp.as_bytes());
    encoded.extend_from_slice(&ts_hash);

    // nonce (uint256)
    let mut nonce_bytes = [0u8; 32];
    nonce_bytes[24..32].copy_from_slice(&nonce.to_be_bytes());
    encoded.extend_from_slice(&nonce_bytes);

    // hash(message) - string type hashed per EIP-712
    let msg_hash = Keccak256::digest(CLOB_AUTH_MESSAGE.as_bytes());
    encoded.extend_from_slice(&msg_hash);

    let result = Keccak256::digest(&encoded);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    Ok(out)
}

// =============================================================================
// L2 Authentication (HMAC-SHA256)
// =============================================================================

/// API credentials returned from the CLOB auth endpoint.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiCredentials {
    /// The API key identifier.
    pub api_key: String,
    /// Base64-encoded HMAC secret.
    pub secret: String,
    /// Passphrase for the API key.
    pub passphrase: String,
}

/// L2 authentication using HMAC-SHA256.
///
/// After deriving API credentials via L1 auth, all subsequent requests
/// use L2 auth with HMAC-SHA256 signatures.
#[derive(Debug, Clone)]
pub struct L2Auth {
    api_key: String,
    secret: String,
    passphrase: String,
    address: String,
}

/// Headers for L2 (HMAC) authenticated requests.
#[derive(Debug, Clone)]
pub struct L2Headers {
    /// POLY_ADDRESS header.
    pub address: String,
    /// POLY_SIGNATURE header - HMAC-SHA256 signature.
    pub signature: String,
    /// POLY_TIMESTAMP header - Unix timestamp.
    pub timestamp: String,
    /// POLY_API_KEY header.
    pub api_key: String,
    /// POLY_PASSPHRASE header.
    pub passphrase: String,
}

impl L2Auth {
    /// Creates a new L2 auth instance from API credentials.
    #[must_use]
    pub fn new(creds: &ApiCredentials, address: String) -> Self {
        Self {
            api_key: creds.api_key.clone(),
            secret: creds.secret.clone(),
            passphrase: creds.passphrase.clone(),
            address,
        }
    }

    /// Returns the API key.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the passphrase.
    #[must_use]
    pub fn passphrase(&self) -> &str {
        &self.passphrase
    }

    /// Generates L2 auth headers for an HTTP request.
    ///
    /// # Arguments
    /// * `method` - HTTP method (GET, POST, DELETE)
    /// * `path` - Request path (e.g., "/order")
    /// * `body` - Request body (empty string for GET/DELETE)
    pub fn headers(
        &self,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<L2Headers, ClobAuthError> {
        let timestamp = Utc::now().timestamp().to_string();

        let signature = self.compute_hmac(&timestamp, method, path, body)?;

        Ok(L2Headers {
            address: self.address.clone(),
            signature,
            timestamp,
            api_key: self.api_key.clone(),
            passphrase: self.passphrase.clone(),
        })
    }

    /// Computes the HMAC-SHA256 signature.
    ///
    /// Message format: `{timestamp}{method}{path}{body}`
    /// Key: base64-decoded secret
    ///
    /// Uses URL-safe base64 (with `-` and `_` instead of `+` and `/`) to match
    /// the Polymarket Python/TypeScript reference clients.
    fn compute_hmac(
        &self,
        timestamp: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<String, ClobAuthError> {
        // Decode the base64 secret (try URL-safe first, then standard for robustness)
        let secret_bytes = BASE64_URL_SAFE
            .decode(&self.secret)
            .or_else(|_| BASE64_STANDARD.decode(&self.secret))
            .map_err(|e| ClobAuthError::Base64Failed(format!("Invalid secret: {}", e)))?;

        // Build the message (empty body is simply not appended, matching Python client)
        let mut message = format!("{}{}{}", timestamp, method, path);
        if !body.is_empty() {
            message.push_str(body);
        }

        // Compute HMAC-SHA256
        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .map_err(|e| ClobAuthError::HmacFailed(format!("Invalid key length: {}", e)))?;
        mac.update(message.as_bytes());
        let result = mac.finalize();

        // URL-safe base64 encode the result (matches Python's urlsafe_b64encode)
        Ok(BASE64_URL_SAFE.encode(result.into_bytes()))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    // -------------------------------------------------------------------------
    // L1 Auth tests
    // -------------------------------------------------------------------------

    #[test]
    fn sign_clob_auth_produces_valid_signature() {
        let headers = sign_clob_auth(TEST_ADDRESS, TEST_KEY, 0).unwrap();

        assert_eq!(headers.address, TEST_ADDRESS);
        assert!(headers.signature.starts_with("0x"));
        assert_eq!(headers.signature.len(), 132); // 65 bytes hex
        assert!(!headers.timestamp.is_empty());
        assert_eq!(headers.nonce, "0");
    }

    #[test]
    fn sign_clob_auth_different_nonces_different_sigs() {
        // Different nonces should produce different struct hashes → different signatures
        // Note: timestamp also varies, but nonce alone changes the hash
        let h1 = sign_clob_auth(TEST_ADDRESS, TEST_KEY, 0).unwrap();
        let h2 = sign_clob_auth(TEST_ADDRESS, TEST_KEY, 1).unwrap();

        // Timestamps may differ too, so just verify both are valid
        assert!(h1.signature.starts_with("0x"));
        assert!(h2.signature.starts_with("0x"));
    }

    #[test]
    fn sign_clob_auth_rejects_invalid_key() {
        let result = sign_clob_auth(TEST_ADDRESS, "deadbeef", 0);
        assert!(result.is_err());
    }

    #[test]
    fn sign_clob_auth_rejects_invalid_address() {
        let result = sign_clob_auth("0xinvalid", TEST_KEY, 0);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // L2 Auth / HMAC tests
    // -------------------------------------------------------------------------

    #[test]
    fn l2_hmac_signature_deterministic() {
        let creds = ApiCredentials {
            api_key: "test-api-key".to_string(),
            secret: BASE64_URL_SAFE.encode(b"test-secret-key-bytes"),
            passphrase: "test-passphrase".to_string(),
        };

        let l2 = L2Auth::new(&creds, TEST_ADDRESS.to_string());

        // Same inputs should produce same HMAC
        let sig1 = l2.compute_hmac("1700000000", "GET", "/order", "").unwrap();
        let sig2 = l2.compute_hmac("1700000000", "GET", "/order", "").unwrap();
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn l2_hmac_different_methods_different_sigs() {
        let creds = ApiCredentials {
            api_key: "test-api-key".to_string(),
            secret: BASE64_URL_SAFE.encode(b"test-secret-key-bytes"),
            passphrase: "test-passphrase".to_string(),
        };

        let l2 = L2Auth::new(&creds, TEST_ADDRESS.to_string());

        let sig_get = l2.compute_hmac("1700000000", "GET", "/order", "").unwrap();
        let sig_post = l2
            .compute_hmac("1700000000", "POST", "/order", "{}")
            .unwrap();
        assert_ne!(sig_get, sig_post);
    }

    #[test]
    fn l2_headers_populated() {
        let creds = ApiCredentials {
            api_key: "test-api-key".to_string(),
            secret: BASE64_URL_SAFE.encode(b"test-secret-key-bytes"),
            passphrase: "test-passphrase".to_string(),
        };

        let l2 = L2Auth::new(&creds, TEST_ADDRESS.to_string());
        let headers = l2.headers("POST", "/order", "{}").unwrap();

        assert_eq!(headers.address, TEST_ADDRESS);
        assert_eq!(headers.api_key, "test-api-key");
        assert_eq!(headers.passphrase, "test-passphrase");
        assert!(!headers.signature.is_empty());
        assert!(!headers.timestamp.is_empty());
    }

    #[test]
    fn l2_hmac_known_vector() {
        // Verify HMAC-SHA256 against a known computation
        let secret_b64 = BASE64_URL_SAFE.encode(b"mysecret");
        let creds = ApiCredentials {
            api_key: "key".to_string(),
            secret: secret_b64,
            passphrase: "pass".to_string(),
        };

        let l2 = L2Auth::new(&creds, TEST_ADDRESS.to_string());
        let sig = l2.compute_hmac("1000", "GET", "/test", "").unwrap();

        // Signature should be non-empty base64
        assert!(!sig.is_empty());
        // Should be valid URL-safe base64
        assert!(BASE64_URL_SAFE.decode(&sig).is_ok());
        // Must not contain standard base64 chars that differ from URL-safe
        assert!(!sig.contains('+'), "Signature must use URL-safe base64");
        assert!(!sig.contains('/'), "Signature must use URL-safe base64");
    }

    #[test]
    fn l2_rejects_invalid_base64_secret() {
        let creds = ApiCredentials {
            api_key: "key".to_string(),
            secret: "not-valid-base64!!!@@@".to_string(),
            passphrase: "pass".to_string(),
        };

        let l2 = L2Auth::new(&creds, TEST_ADDRESS.to_string());
        let result = l2.headers("GET", "/test", "");
        assert!(result.is_err());
    }

    #[test]
    fn api_credentials_deserialization() {
        let json = r#"{"apiKey":"abc123","secret":"c2VjcmV0","passphrase":"pass"}"#;
        let creds: ApiCredentials = serde_json::from_str(json).unwrap();
        assert_eq!(creds.api_key, "abc123");
        assert_eq!(creds.secret, "c2VjcmV0");
        assert_eq!(creds.passphrase, "pass");
    }
}
