//! Error types for Kalshi exchange integration.
//!
//! Provides typed errors for authentication, API communication, validation,
//! and execution failures.

use thiserror::Error;

/// Errors that can occur when interacting with Kalshi.
#[derive(Debug, Error)]
pub enum KalshiError {
    /// Authentication failed.
    #[error("authentication error: {0}")]
    Authentication(String),

    /// RSA signing error.
    #[error("RSA signing error: {0}")]
    Signing(String),

    /// API request failed.
    #[error("API error: {status_code} - {message}")]
    Api {
        /// HTTP status code.
        status_code: u16,
        /// Error message from API.
        message: String,
    },

    /// Rate limit exceeded.
    #[error("rate limit exceeded, retry after {retry_after_secs}s")]
    RateLimit {
        /// Seconds to wait before retry.
        retry_after_secs: u64,
    },

    /// Network error.
    #[error("network error: {0}")]
    Network(String),

    /// Request timeout.
    #[error("request timeout: {0}")]
    Timeout(String),

    /// Invalid order parameters.
    #[error("invalid order: {0}")]
    InvalidOrder(String),

    /// Insufficient balance.
    #[error(
        "insufficient balance: required {required_cents} cents, available {available_cents} cents"
    )]
    InsufficientBalance {
        /// Required amount in cents.
        required_cents: i64,
        /// Available amount in cents.
        available_cents: i64,
    },

    /// Order rejected by exchange.
    #[error("order rejected: {0}")]
    OrderRejected(String),

    /// Order not found.
    #[error("order not found: {order_id}")]
    OrderNotFound {
        /// The order ID that was not found.
        order_id: String,
    },

    /// Market not found.
    #[error("market not found: {ticker}")]
    MarketNotFound {
        /// The market ticker that was not found.
        ticker: String,
    },

    /// Market is closed for trading.
    #[error("market closed: {ticker}")]
    MarketClosed {
        /// The closed market's ticker.
        ticker: String,
    },

    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Circuit breaker is open.
    #[error("circuit breaker open: {reason}")]
    CircuitBreakerOpen {
        /// Reason the circuit breaker tripped.
        reason: String,
    },

    /// Daily limit exceeded.
    #[error("daily limit exceeded: {limit_type} - used {used}, max {max}")]
    DailyLimitExceeded {
        /// Type of limit (volume, orders, etc.).
        limit_type: String,
        /// Amount already used.
        used: String,
        /// Maximum allowed.
        max: String,
    },
}

impl KalshiError {
    /// Creates an API error from status code and message.
    pub fn api(status_code: u16, message: impl Into<String>) -> Self {
        Self::Api {
            status_code,
            message: message.into(),
        }
    }

    /// Creates a rate limit error.
    pub fn rate_limit(retry_after_secs: u64) -> Self {
        Self::RateLimit { retry_after_secs }
    }

    /// Creates an insufficient balance error.
    pub fn insufficient_balance(required_cents: i64, available_cents: i64) -> Self {
        Self::InsufficientBalance {
            required_cents,
            available_cents,
        }
    }

    /// Creates an order not found error.
    pub fn order_not_found(order_id: impl Into<String>) -> Self {
        Self::OrderNotFound {
            order_id: order_id.into(),
        }
    }

    /// Creates a market not found error.
    pub fn market_not_found(ticker: impl Into<String>) -> Self {
        Self::MarketNotFound {
            ticker: ticker.into(),
        }
    }

    /// Creates a market closed error.
    pub fn market_closed(ticker: impl Into<String>) -> Self {
        Self::MarketClosed {
            ticker: ticker.into(),
        }
    }

    /// Creates a circuit breaker open error.
    pub fn circuit_breaker_open(reason: impl Into<String>) -> Self {
        Self::CircuitBreakerOpen {
            reason: reason.into(),
        }
    }

    /// Creates a daily limit exceeded error.
    pub fn daily_limit_exceeded(
        limit_type: impl Into<String>,
        used: impl Into<String>,
        max: impl Into<String>,
    ) -> Self {
        Self::DailyLimitExceeded {
            limit_type: limit_type.into(),
            used: used.into(),
            max: max.into(),
        }
    }

    /// Returns true if the error is retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout(_) | Self::RateLimit { .. }
        )
    }

    /// Returns true if the error indicates the request should be retried later.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network(_) | Self::Timeout(_) | Self::RateLimit { .. } => true,
            Self::Api { status_code, .. } => *status_code >= 500,
            _ => false,
        }
    }

    /// Returns the suggested retry delay in seconds, if applicable.
    #[must_use]
    pub fn retry_delay_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimit { retry_after_secs } => Some(*retry_after_secs),
            Self::Network(_) | Self::Timeout(_) => Some(1),
            Self::Api { status_code, .. } if *status_code >= 500 => Some(2),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for KalshiError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout(err.to_string())
        } else if err.is_connect() {
            Self::Network(format!("connection failed: {err}"))
        } else {
            Self::Network(err.to_string())
        }
    }
}

impl From<serde_json::Error> for KalshiError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

/// Result type alias for Kalshi operations.
pub type Result<T> = std::result::Result<T, KalshiError>;

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Error Construction Tests ====================

    #[test]
    fn test_api_error_construction() {
        let err = KalshiError::api(400, "bad request");
        assert!(matches!(
            err,
            KalshiError::Api {
                status_code: 400,
                ..
            }
        ));
        assert!(err.to_string().contains("400"));
        assert!(err.to_string().contains("bad request"));
    }

    #[test]
    fn test_rate_limit_error_construction() {
        let err = KalshiError::rate_limit(60);
        assert!(matches!(
            err,
            KalshiError::RateLimit {
                retry_after_secs: 60
            }
        ));
        assert!(err.to_string().contains("60"));
    }

    #[test]
    fn test_insufficient_balance_error() {
        let err = KalshiError::insufficient_balance(5000, 2500);
        assert!(err.to_string().contains("5000"));
        assert!(err.to_string().contains("2500"));
    }

    #[test]
    fn test_order_not_found_error() {
        let err = KalshiError::order_not_found("order-123");
        assert!(err.to_string().contains("order-123"));
    }

    #[test]
    fn test_market_not_found_error() {
        let err = KalshiError::market_not_found("KXBTC-26FEB02-B100000");
        assert!(err.to_string().contains("KXBTC-26FEB02-B100000"));
    }

    #[test]
    fn test_market_closed_error() {
        let err = KalshiError::market_closed("KXBTC-26FEB02-B100000");
        assert!(err.to_string().contains("KXBTC-26FEB02-B100000"));
    }

    #[test]
    fn test_circuit_breaker_open_error() {
        let err = KalshiError::circuit_breaker_open("too many failures");
        assert!(err.to_string().contains("too many failures"));
    }

    #[test]
    fn test_daily_limit_exceeded_error() {
        let err = KalshiError::daily_limit_exceeded("volume", "$5000", "$10000");
        assert!(err.to_string().contains("volume"));
        assert!(err.to_string().contains("$5000"));
        assert!(err.to_string().contains("$10000"));
    }

    // ==================== Retryable Tests ====================

    #[test]
    fn test_network_error_is_retryable() {
        let err = KalshiError::Network("connection refused".to_string());
        assert!(err.is_retryable());
        assert!(err.is_transient());
    }

    #[test]
    fn test_timeout_error_is_retryable() {
        let err = KalshiError::Timeout("request timed out".to_string());
        assert!(err.is_retryable());
        assert!(err.is_transient());
    }

    #[test]
    fn test_rate_limit_error_is_retryable() {
        let err = KalshiError::rate_limit(30);
        assert!(err.is_retryable());
        assert!(err.is_transient());
    }

    #[test]
    fn test_server_error_is_transient() {
        let err = KalshiError::api(500, "internal server error");
        assert!(!err.is_retryable()); // is_retryable only checks specific types
        assert!(err.is_transient());
    }

    #[test]
    fn test_client_error_is_not_transient() {
        let err = KalshiError::api(400, "bad request");
        assert!(!err.is_retryable());
        assert!(!err.is_transient());
    }

    #[test]
    fn test_auth_error_is_not_retryable() {
        let err = KalshiError::Authentication("invalid key".to_string());
        assert!(!err.is_retryable());
        assert!(!err.is_transient());
    }

    #[test]
    fn test_invalid_order_is_not_retryable() {
        let err = KalshiError::InvalidOrder("size too large".to_string());
        assert!(!err.is_retryable());
        assert!(!err.is_transient());
    }

    // ==================== Retry Delay Tests ====================

    #[test]
    fn test_rate_limit_retry_delay() {
        let err = KalshiError::rate_limit(60);
        assert_eq!(err.retry_delay_secs(), Some(60));
    }

    #[test]
    fn test_network_error_retry_delay() {
        let err = KalshiError::Network("connection failed".to_string());
        assert_eq!(err.retry_delay_secs(), Some(1));
    }

    #[test]
    fn test_timeout_retry_delay() {
        let err = KalshiError::Timeout("request timed out".to_string());
        assert_eq!(err.retry_delay_secs(), Some(1));
    }

    #[test]
    fn test_server_error_retry_delay() {
        let err = KalshiError::api(503, "service unavailable");
        assert_eq!(err.retry_delay_secs(), Some(2));
    }

    #[test]
    fn test_client_error_no_retry_delay() {
        let err = KalshiError::api(400, "bad request");
        assert_eq!(err.retry_delay_secs(), None);
    }

    #[test]
    fn test_auth_error_no_retry_delay() {
        let err = KalshiError::Authentication("invalid".to_string());
        assert_eq!(err.retry_delay_secs(), None);
    }

    // ==================== Error Display Tests ====================

    #[test]
    fn test_error_display_authentication() {
        let err = KalshiError::Authentication("invalid API key".to_string());
        let display = err.to_string();
        assert!(display.contains("authentication"));
        assert!(display.contains("invalid API key"));
    }

    #[test]
    fn test_error_display_signing() {
        let err = KalshiError::Signing("invalid private key".to_string());
        let display = err.to_string();
        assert!(display.contains("signing"));
    }

    #[test]
    fn test_error_display_order_rejected() {
        let err = KalshiError::OrderRejected("insufficient margin".to_string());
        let display = err.to_string();
        assert!(display.contains("rejected"));
        assert!(display.contains("insufficient margin"));
    }

    #[test]
    fn test_error_display_configuration() {
        let err = KalshiError::Configuration("missing API key".to_string());
        let display = err.to_string();
        assert!(display.contains("configuration"));
    }
}
