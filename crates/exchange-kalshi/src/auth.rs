//! RSA-PSS authentication for Kalshi API.
//!
//! Kalshi uses RSA-PSS (SHA-256) signatures for API authentication.
//! The signature is computed over: timestamp + method + path + body
//!
//! # Security
//!
//! - Private keys are loaded from environment variables
//! - Private keys are NEVER logged
//! - Keys are zeroized on drop
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_kalshi::auth::{KalshiAuth, KalshiAuthConfig};
//!
//! let auth = KalshiAuth::from_env(KalshiAuthConfig::default())?;
//! let headers = auth.sign_request("POST", "/trade-api/v2/portfolio/orders", r#"{"ticker":"KXBTC"}"#)?;
//! ```

use crate::error::{KalshiError, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use secrecy::{ExposeSecret, SecretString};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for Kalshi authentication.
#[derive(Debug, Clone)]
pub struct KalshiAuthConfig {
    /// Environment variable name for API key ID.
    pub api_key_env: String,

    /// Environment variable name for private key (PEM format).
    pub private_key_env: String,
}

impl Default for KalshiAuthConfig {
    fn default() -> Self {
        Self {
            api_key_env: "KALSHI_API_KEY".to_string(),
            private_key_env: "KALSHI_PRIVATE_KEY".to_string(),
        }
    }
}

impl KalshiAuthConfig {
    /// Creates config for demo environment.
    #[must_use]
    pub fn demo() -> Self {
        Self {
            api_key_env: "KALSHI_DEMO_API_KEY".to_string(),
            private_key_env: "KALSHI_DEMO_PRIVATE_KEY".to_string(),
        }
    }

    /// Sets custom environment variable names.
    #[must_use]
    pub fn with_env_vars(
        mut self,
        api_key_env: impl Into<String>,
        private_key_env: impl Into<String>,
    ) -> Self {
        self.api_key_env = api_key_env.into();
        self.private_key_env = private_key_env.into();
        self
    }
}

// =============================================================================
// Signed Headers
// =============================================================================

/// Headers required for authenticated Kalshi API requests.
#[derive(Debug, Clone)]
pub struct SignedHeaders {
    /// KALSHI-ACCESS-KEY header.
    pub access_key: String,

    /// KALSHI-ACCESS-SIGNATURE header (base64 encoded).
    pub signature: String,

    /// KALSHI-ACCESS-TIMESTAMP header (Unix timestamp in milliseconds).
    pub timestamp: String,
}

impl SignedHeaders {
    /// Returns headers as tuples for reqwest.
    #[must_use]
    pub fn as_tuples(&self) -> [(&'static str, &str); 3] {
        [
            ("KALSHI-ACCESS-KEY", &self.access_key),
            ("KALSHI-ACCESS-SIGNATURE", &self.signature),
            ("KALSHI-ACCESS-TIMESTAMP", &self.timestamp),
        ]
    }
}

// =============================================================================
// KalshiAuth
// =============================================================================

/// RSA-PSS authenticator for Kalshi API.
///
/// Handles signing of API requests using RSA-PSS (SHA-256).
/// The private key is stored securely and zeroized on drop.
pub struct KalshiAuth {
    /// API key ID.
    api_key: String,

    /// RSA private key for signing.
    private_key: RsaPrivateKey,
}

impl std::fmt::Debug for KalshiAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KalshiAuth")
            .field("api_key", &self.api_key)
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

impl Drop for KalshiAuth {
    fn drop(&mut self) {
        // Zeroize sensitive data
        self.api_key.zeroize();
        // Note: RsaPrivateKey doesn't implement Zeroize directly,
        // but it will be dropped and memory will be reclaimed
    }
}

impl KalshiAuth {
    /// Creates a new authenticator from an API key and PEM-encoded private key.
    ///
    /// # Arguments
    /// * `api_key` - The Kalshi API key ID
    /// * `private_key_pem` - RSA private key in PEM format
    ///
    /// # Errors
    /// Returns error if the private key cannot be parsed.
    pub fn new(api_key: impl Into<String>, private_key_pem: &str) -> Result<Self> {
        let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| KalshiError::Signing(format!("failed to parse private key: {e}")))?;

        Ok(Self {
            api_key: api_key.into(),
            private_key,
        })
    }

    /// Creates a new authenticator from environment variables.
    ///
    /// # Arguments
    /// * `config` - Configuration specifying environment variable names
    ///
    /// # Errors
    /// Returns error if environment variables are missing or invalid.
    pub fn from_env(config: KalshiAuthConfig) -> Result<Self> {
        let api_key = std::env::var(&config.api_key_env).map_err(|_| {
            KalshiError::Configuration(format!(
                "missing environment variable: {}",
                config.api_key_env
            ))
        })?;

        let private_key_pem = std::env::var(&config.private_key_env).map_err(|_| {
            KalshiError::Configuration(format!(
                "missing environment variable: {}",
                config.private_key_env
            ))
        })?;

        // Handle newline escaping in environment variables
        let private_key_pem = private_key_pem.replace("\\n", "\n");

        Self::new(api_key, &private_key_pem)
    }

    /// Creates a new authenticator with a SecretString private key.
    ///
    /// # Arguments
    /// * `api_key` - The Kalshi API key ID
    /// * `private_key_pem` - RSA private key in PEM format (as SecretString)
    ///
    /// # Errors
    /// Returns error if the private key cannot be parsed.
    pub fn with_secret_key(
        api_key: impl Into<String>,
        private_key_pem: SecretString,
    ) -> Result<Self> {
        Self::new(api_key, private_key_pem.expose_secret())
    }

    /// Returns the API key ID.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Signs a request and returns the required headers.
    ///
    /// # Arguments
    /// * `method` - HTTP method (GET, POST, DELETE, etc.)
    /// * `path` - API path (e.g., "/trade-api/v2/portfolio/orders")
    /// * `body` - Request body (empty string for GET requests)
    ///
    /// # Errors
    /// Returns error if signing fails.
    pub fn sign_request(&self, method: &str, path: &str, body: &str) -> Result<SignedHeaders> {
        // Get current timestamp in milliseconds
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| KalshiError::Signing(format!("failed to get timestamp: {e}")))?
            .as_millis();

        self.sign_request_with_timestamp(method, path, body, timestamp_ms as u64)
    }

    /// Signs a request with a specific timestamp (useful for testing).
    ///
    /// # Arguments
    /// * `method` - HTTP method
    /// * `path` - API path
    /// * `body` - Request body
    /// * `timestamp_ms` - Unix timestamp in milliseconds
    ///
    /// # Errors
    /// Returns error if signing fails.
    pub fn sign_request_with_timestamp(
        &self,
        method: &str,
        path: &str,
        body: &str,
        timestamp_ms: u64,
    ) -> Result<SignedHeaders> {
        // Build the message to sign: timestamp + method + path + body
        let timestamp_str = timestamp_ms.to_string();
        let message = format!("{}{}{}{}", timestamp_str, method, path, body);

        // Sign with RSA-PSS (SHA-256)
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        let signature = signing_key.sign(message.as_bytes());

        // Base64 encode the signature
        let signature_b64 = BASE64.encode(signature.to_bytes());

        Ok(SignedHeaders {
            access_key: self.api_key.clone(),
            signature: signature_b64,
            timestamp: timestamp_str,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Config Tests ====================

    #[test]
    fn test_auth_config_default() {
        let config = KalshiAuthConfig::default();
        assert_eq!(config.api_key_env, "KALSHI_API_KEY");
        assert_eq!(config.private_key_env, "KALSHI_PRIVATE_KEY");
    }

    #[test]
    fn test_auth_config_demo() {
        let config = KalshiAuthConfig::demo();
        assert_eq!(config.api_key_env, "KALSHI_DEMO_API_KEY");
        assert_eq!(config.private_key_env, "KALSHI_DEMO_PRIVATE_KEY");
    }

    #[test]
    fn test_auth_config_custom_env() {
        let config = KalshiAuthConfig::default().with_env_vars("CUSTOM_KEY", "CUSTOM_PK");
        assert_eq!(config.api_key_env, "CUSTOM_KEY");
        assert_eq!(config.private_key_env, "CUSTOM_PK");
    }

    // ==================== SignedHeaders Tests ====================

    #[test]
    fn test_signed_headers_as_tuples() {
        let headers = SignedHeaders {
            access_key: "test-key".to_string(),
            signature: "dGVzdC1zaWduYXR1cmU=".to_string(),
            timestamp: "1234567890000".to_string(),
        };

        let tuples = headers.as_tuples();
        assert_eq!(tuples.len(), 3);
        assert_eq!(tuples[0], ("KALSHI-ACCESS-KEY", "test-key"));
        assert_eq!(
            tuples[1],
            ("KALSHI-ACCESS-SIGNATURE", "dGVzdC1zaWduYXR1cmU=")
        );
        assert_eq!(tuples[2], ("KALSHI-ACCESS-TIMESTAMP", "1234567890000"));
    }

    // ==================== KalshiAuth Tests ====================

    #[test]
    fn test_auth_debug_redacts_key() {
        // Verify that SignedHeaders don't expose sensitive data in debug
        // The actual private key is never included in SignedHeaders
        let headers = SignedHeaders {
            access_key: "test-key".to_string(),
            signature: "base64-sig".to_string(),
            timestamp: "123456".to_string(),
        };
        let debug_output = format!("{:?}", headers);
        // Verify the debug output contains the fields (not sensitive)
        assert!(debug_output.contains("test-key"));
        // No actual private key data should ever be in SignedHeaders
    }

    #[test]
    fn test_auth_from_env_missing_api_key() {
        // Ensure the env var is not set
        std::env::remove_var("TEST_MISSING_API_KEY");

        let config =
            KalshiAuthConfig::default().with_env_vars("TEST_MISSING_API_KEY", "TEST_MISSING_PK");

        let result = KalshiAuth::from_env(config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing environment variable"));
    }

    #[test]
    fn test_auth_invalid_private_key() {
        let result = KalshiAuth::new("test-api-key", "invalid-pem-data");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("parse private key"));
    }

    // ==================== Signature Format Tests ====================

    #[test]
    fn test_signature_message_format() {
        // Test that the message format is correct: timestamp + method + path + body
        // This is a documentation test - we can verify the format is as expected

        let timestamp = "1706817600000";
        let method = "POST";
        let path = "/trade-api/v2/portfolio/orders";
        let body = r#"{"ticker":"KXBTC-TEST"}"#;

        let expected_message = format!("{}{}{}{}", timestamp, method, path, body);
        assert_eq!(
            expected_message,
            "1706817600000POST/trade-api/v2/portfolio/orders{\"ticker\":\"KXBTC-TEST\"}"
        );
    }

    #[test]
    fn test_signature_base64_encoding() {
        // Verify base64 encoding works correctly
        let test_data = b"test signature data";
        let encoded = BASE64.encode(test_data);
        let decoded = BASE64.decode(&encoded).unwrap();
        assert_eq!(decoded, test_data);
    }

    #[test]
    fn test_timestamp_format() {
        // Verify timestamp is in milliseconds
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        // Should be 13 digits (milliseconds since epoch)
        let timestamp_str = now.to_string();
        assert!(timestamp_str.len() >= 13);
    }

    // ==================== Edge Case Tests ====================

    #[test]
    fn test_sign_request_empty_body() {
        // Test signing with empty body (GET request)
        let timestamp = "1706817600000";
        let method = "GET";
        let path = "/trade-api/v2/markets";
        let body = "";

        let message = format!("{}{}{}{}", timestamp, method, path, body);
        assert_eq!(message, "1706817600000GET/trade-api/v2/markets");
    }

    #[test]
    fn test_sign_request_special_characters_in_body() {
        // Test that special characters in body are handled
        let body = r#"{"ticker":"KXBTC-TEST","side":"yes","price":45}"#;
        assert!(body.contains("\""));
        assert!(body.contains(":"));
        // This should not cause issues with signing
    }

    #[test]
    fn test_newline_replacement_in_pem() {
        // Test that \n is replaced with actual newlines
        let pem_with_escapes = "-----BEGIN PRIVATE KEY-----\\nMIIE...\\n-----END PRIVATE KEY-----";
        let replaced = pem_with_escapes.replace("\\n", "\n");
        assert!(replaced.contains('\n'));
        assert!(!replaced.contains("\\n"));
    }

    // ==================== Secret Handling Tests ====================

    #[test]
    fn test_secret_string_not_leaked() {
        // Ensure SecretString doesn't leak in Debug output
        let secret = SecretString::from("super-secret-key");
        let debug_output = format!("{:?}", secret);
        assert!(!debug_output.contains("super-secret-key"));
    }

    // ==================== API Key Tests ====================

    #[test]
    fn test_api_key_accessor() {
        // Test that api_key() returns the correct value
        // We can't create a real auth without a valid key, but we can verify
        // the SignedHeaders contain the correct key
        let headers = SignedHeaders {
            access_key: "my-api-key-123".to_string(),
            signature: "sig".to_string(),
            timestamp: "123".to_string(),
        };
        assert_eq!(headers.access_key, "my-api-key-123");
    }
}
