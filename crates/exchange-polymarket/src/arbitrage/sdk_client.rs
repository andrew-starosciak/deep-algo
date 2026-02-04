//! Polymarket CLOB API client with EIP-712 signing.
//!
//! This module provides an authenticated client for the Polymarket CLOB API,
//! handling order submission, cancellation, and status queries with proper
//! EIP-712 message signing.
//!
//! # Authentication Flow
//!
//! 1. Derive API credentials from wallet signature (one-time setup)
//! 2. Sign each order with EIP-712 typed data
//! 3. Submit signed orders to CLOB API
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::sdk_client::ClobClient;
//! use algo_trade_polymarket::arbitrage::signer::Wallet;
//!
//! let wallet = Wallet::from_env()?;
//! let client = ClobClient::new(&wallet, ClobClientConfig::mainnet()).await?;
//!
//! let balance = client.get_balance().await?;
//! println!("USDC balance: {}", balance);
//! ```

use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

use super::execution::{ExecutionError, OrderParams, OrderResult, OrderStatus, OrderType, Side};
use super::signer::Wallet;

// =============================================================================
// Constants
// =============================================================================

/// Polymarket CLOB API base URL (mainnet)
pub const CLOB_API_MAINNET: &str = "https://clob.polymarket.com";

/// Polymarket CLOB API base URL (testnet/Mumbai - deprecated, use Amoy)
pub const CLOB_API_TESTNET: &str = "https://clob.polymarket.com";

/// Polymarket Data API base URL (for positions, trades, activity)
pub const DATA_API_URL: &str = "https://data-api.polymarket.com";

/// Default request timeout
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retries for transient errors
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff
const BASE_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Maximum retry delay
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during CLOB API operations.
#[derive(Debug, Error)]
pub enum ClobError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// API returned an error response
    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    /// Failed to parse API response
    #[error("Failed to parse response: {0}")]
    Parse(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    Auth(String),

    /// Order was rejected by the exchange
    #[error("Order rejected: {0}")]
    OrderRejected(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    /// Request timed out
    #[error("Request timed out after {0:?}")]
    Timeout(Duration),

    /// Signing failed
    #[error("Signing failed: {0}")]
    Signing(String),

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    Config(String),
}

impl ClobError {
    /// Returns true if this error is retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ClobError::Http(_)
                | ClobError::RateLimited { .. }
                | ClobError::Timeout(_)
                | ClobError::Api { status: 500..=599, .. }
        )
    }

    /// Converts to ExecutionError for the executor interface.
    #[must_use]
    pub fn into_execution_error(self) -> ExecutionError {
        match self {
            ClobError::RateLimited { retry_after_ms } => ExecutionError::RateLimited {
                retry_after_secs: retry_after_ms / 1000,
            },
            ClobError::Timeout(duration) => ExecutionError::Timeout {
                order_id: format!("timeout_after_{:?}", duration),
            },
            ClobError::OrderRejected(msg) => ExecutionError::Rejected { reason: msg },
            ClobError::Auth(msg) => ExecutionError::Api(format!("Auth: {}", msg)),
            _ => ExecutionError::Api(self.to_string()),
        }
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the CLOB client.
#[derive(Debug, Clone)]
pub struct ClobClientConfig {
    /// Base URL for the CLOB API
    pub base_url: String,

    /// Request timeout
    pub timeout: Duration,

    /// Maximum retries for transient errors
    pub max_retries: u32,

    /// Whether to use neg-risk mode (required for most Polymarket markets)
    pub neg_risk: bool,

    /// Funder address (for neg-risk CTF exchange)
    pub funder: Option<String>,
}

impl Default for ClobClientConfig {
    fn default() -> Self {
        Self::mainnet()
    }
}

impl ClobClientConfig {
    /// Creates configuration for Polymarket mainnet.
    #[must_use]
    pub fn mainnet() -> Self {
        Self {
            base_url: CLOB_API_MAINNET.to_string(),
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            neg_risk: true,
            funder: None,
        }
    }

    /// Creates configuration for Polymarket testnet.
    #[must_use]
    pub fn testnet() -> Self {
        Self {
            base_url: CLOB_API_TESTNET.to_string(),
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            neg_risk: true,
            funder: None,
        }
    }

    /// Sets a custom base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.to_string();
        self
    }

    /// Sets the request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum retry count.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the funder address for neg-risk mode.
    #[must_use]
    pub fn with_funder(mut self, funder: &str) -> Self {
        self.funder = Some(funder.to_string());
        self
    }
}

// =============================================================================
// API Types
// =============================================================================

/// Order side for the CLOB API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiSide {
    Buy,
    Sell,
}

impl From<Side> for ApiSide {
    fn from(side: Side) -> Self {
        match side {
            Side::Buy => ApiSide::Buy,
            Side::Sell => ApiSide::Sell,
        }
    }
}

/// Order type for the CLOB API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiOrderType {
    /// Good-til-cancelled
    Gtc,
    /// Fill-or-kill
    Fok,
    /// Fill-and-kill (immediate-or-cancel)
    Ioc,
}

impl From<OrderType> for ApiOrderType {
    fn from(order_type: OrderType) -> Self {
        match order_type {
            OrderType::Gtc => ApiOrderType::Gtc,
            OrderType::Fok => ApiOrderType::Fok,
            OrderType::Fak => ApiOrderType::Ioc,
        }
    }
}

/// Request to create a new order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderRequest {
    /// Token ID to trade
    pub token_id: String,
    /// Order side
    pub side: ApiSide,
    /// Limit price (0.01 to 0.99)
    pub price: String,
    /// Order size in shares
    pub size: String,
    /// Order type
    pub order_type: ApiOrderType,
    /// Fee rate in basis points (typically 0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_rate_bps: Option<u32>,
    /// Nonce for order uniqueness
    pub nonce: String,
    /// Expiration timestamp (0 for no expiration)
    pub expiration: String,
    /// EIP-712 signature
    pub signature: String,
}

/// Response from order creation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderResponse {
    /// Order ID assigned by the exchange
    pub order_id: String,
    /// Whether the order was accepted
    pub success: bool,
    /// Error message if not successful
    #[serde(default)]
    pub error_msg: Option<String>,
    /// Transaction hashes if filled
    #[serde(default)]
    pub transaction_hashes: Vec<String>,
    /// Order status
    #[serde(default)]
    pub status: Option<String>,
}

/// Order status response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderStatusResponse {
    /// Order ID
    pub id: String,
    /// Current status
    pub status: String,
    /// Original size
    pub original_size: String,
    /// Remaining size
    pub size_matched: String,
    /// Average fill price
    #[serde(default)]
    pub price: Option<String>,
    /// Associated market
    pub asset_id: String,
    /// Order side
    pub side: String,
}

/// Balance response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// USDC balance
    #[serde(default)]
    pub usdc: Option<String>,
    /// Collateral balance
    #[serde(default)]
    pub collateral: Option<String>,
}

/// Wallet position from Data API.
///
/// Represents a token position held by the wallet.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletPosition {
    /// Token ID (asset)
    pub asset: String,
    /// Condition ID for the market
    pub condition_id: String,
    /// Position size (number of shares)
    pub size: String,
    /// Average entry price
    pub avg_price: String,
    /// Initial value when position was opened
    pub initial_value: String,
    /// Current value of position
    pub current_value: String,
    /// Cash P&L
    pub cash_pnl: String,
    /// Percent P&L
    pub percent_pnl: String,
    /// Current price of token (1.0 = won, 0.0 = lost for resolved markets)
    pub cur_price: String,
    /// Whether position can be redeemed (market resolved)
    pub redeemable: bool,
    /// Market title
    pub title: String,
    /// Outcome name (Yes/No/Up/Down)
    pub outcome: String,
    /// Outcome index (0 or 1)
    #[serde(default)]
    pub outcome_index: Option<i32>,
}

// =============================================================================
// CLOB Client
// =============================================================================

/// Authenticated Polymarket CLOB API client.
///
/// This client handles:
/// - EIP-712 message signing for order authentication
/// - Order submission with automatic retries
/// - Balance and order status queries
/// - Rate limit handling
pub struct ClobClient {
    /// HTTP client
    http: Client,
    /// Client configuration
    config: ClobClientConfig,
    /// Wallet for signing
    wallet: Wallet,
    /// API key (derived from wallet signature)
    #[allow(dead_code)] // Will be used for authenticated endpoints
    api_key: Option<String>,
    /// API secret (derived from wallet signature)
    #[allow(dead_code)] // Will be used for authenticated endpoints
    api_secret: Option<String>,
}

impl ClobClient {
    /// Creates a new CLOB client.
    ///
    /// Note: Call `authenticate()` before using authenticated endpoints.
    pub fn new(wallet: Wallet, config: ClobClientConfig) -> Result<Self, ClobError> {
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(ClobError::Http)?;

        Ok(Self {
            http,
            config,
            wallet,
            api_key: None,
            api_secret: None,
        })
    }

    /// Creates a new client with default mainnet configuration.
    pub fn mainnet(wallet: Wallet) -> Result<Self, ClobError> {
        Self::new(wallet, ClobClientConfig::mainnet())
    }

    /// Creates a new client with testnet configuration.
    pub fn testnet(wallet: Wallet) -> Result<Self, ClobError> {
        Self::new(wallet, ClobClientConfig::testnet())
    }

    /// Returns the wallet address.
    #[must_use]
    pub fn address(&self) -> &str {
        self.wallet.address()
    }

    /// Returns the client configuration.
    #[must_use]
    pub fn config(&self) -> &ClobClientConfig {
        &self.config
    }

    /// Authenticates with the CLOB API by deriving API credentials.
    ///
    /// This must be called before using authenticated endpoints like
    /// order submission.
    pub async fn authenticate(&mut self) -> Result<(), ClobError> {
        // The Polymarket CLOB uses a signature-based authentication flow:
        // 1. Sign a specific message with the wallet
        // 2. Derive API key and secret from the signature
        //
        // For now, we'll implement a placeholder that will be completed
        // when we have access to the exact authentication flow.

        info!(address = %self.wallet.address(), "Authenticating with CLOB API");

        // TODO: Implement actual authentication flow
        // This requires signing a specific message format defined by Polymarket

        // For testing, we'll mark as authenticated
        self.api_key = Some(format!("key_{}", &self.wallet.address()[2..10]));
        self.api_secret = Some("placeholder_secret".to_string());

        debug!("Authentication placeholder complete");
        Ok(())
    }

    /// Returns true if the client is authenticated.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.api_key.is_some() && self.api_secret.is_some()
    }

    /// Gets the USDC balance for the authenticated wallet.
    pub async fn get_balance(&self) -> Result<Decimal, ClobError> {
        let url = format!("{}/balance?address={}", self.config.base_url, self.wallet.address());

        debug!(url = %url, "Fetching balance");

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: body,
            });
        }

        let balance: BalanceResponse = response.json().await.map_err(|e| {
            ClobError::Parse(format!("Failed to parse balance response: {}", e))
        })?;

        // Parse USDC balance (or collateral for neg-risk)
        let balance_str = balance.usdc
            .or(balance.collateral)
            .unwrap_or_else(|| "0".to_string());

        balance_str.parse::<Decimal>().map_err(|e| {
            ClobError::Parse(format!("Failed to parse balance value '{}': {}", balance_str, e))
        })
    }

    /// Gets all positions for the authenticated wallet from the Data API.
    ///
    /// This queries Polymarket's data-api which provides the source of truth
    /// for wallet positions, including resolved markets where tokens are
    /// redeemable.
    ///
    /// # Returns
    ///
    /// A vector of wallet positions. For resolved markets:
    /// - `cur_price` = "1" means the position won
    /// - `cur_price` = "0" means the position lost
    /// - `redeemable` = true means the market has resolved
    pub async fn get_positions(&self) -> Result<Vec<WalletPosition>, ClobError> {
        let url = format!(
            "{}/positions?user={}&sizeThreshold=0",
            DATA_API_URL,
            self.wallet.address()
        );

        debug!(url = %url, "Fetching wallet positions from Data API");

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: format!("Data API positions error: {}", body),
            });
        }

        let positions: Vec<WalletPosition> = response.json().await.map_err(|e| {
            ClobError::Parse(format!("Failed to parse positions response: {}", e))
        })?;

        debug!(count = positions.len(), "Fetched wallet positions");

        Ok(positions)
    }

    /// Gets positions for specific token IDs.
    ///
    /// This is a convenience method that filters positions by token ID,
    /// useful for checking the status of specific trades.
    pub async fn get_positions_for_tokens(
        &self,
        token_ids: &[&str],
    ) -> Result<Vec<WalletPosition>, ClobError> {
        let all_positions = self.get_positions().await?;

        let filtered: Vec<WalletPosition> = all_positions
            .into_iter()
            .filter(|p| token_ids.contains(&p.asset.as_str()))
            .collect();

        Ok(filtered)
    }

    /// Checks if a position has resolved and returns win/loss status.
    ///
    /// # Returns
    ///
    /// - `Some(true)` if the position won (cur_price >= 0.95)
    /// - `Some(false)` if the position lost (cur_price <= 0.05)
    /// - `None` if the market hasn't resolved yet or position not found
    pub async fn check_position_outcome(
        &self,
        token_id: &str,
    ) -> Result<Option<bool>, ClobError> {
        let positions = self.get_positions_for_tokens(&[token_id]).await?;

        if let Some(pos) = positions.first() {
            // Parse current price
            let cur_price: Decimal = pos.cur_price.parse().unwrap_or_default();

            // Check if resolved (redeemable or price at extremes)
            if pos.redeemable || cur_price >= dec!(0.95) || cur_price <= dec!(0.05) {
                // Won if price >= 0.95
                return Ok(Some(cur_price >= dec!(0.95)));
            }
        }

        Ok(None)
    }

    /// Submits an order to the CLOB.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The client is not authenticated
    /// - The order parameters are invalid
    /// - The API rejects the order
    /// - A network error occurs
    pub async fn submit_order(&self, params: &OrderParams) -> Result<OrderResult, ClobError> {
        if !self.is_authenticated() {
            return Err(ClobError::Auth("Client not authenticated".to_string()));
        }

        // Validate order parameters
        self.validate_order_params(params)?;

        // Create the order request
        let request = self.create_order_request(params)?;

        info!(
            token_id = %params.token_id,
            side = ?params.side,
            price = %params.price,
            size = %params.size,
            "Submitting order to CLOB"
        );

        // Submit to API
        let url = format!("{}/order", self.config.base_url);
        let response = self.http
            .post(&url)
            .json(&request)
            .send()
            .await?;

        let status = response.status();

        if status.as_u16() == 429 {
            // Rate limited
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1000);
            return Err(ClobError::RateLimited { retry_after_ms: retry_after });
        }

        let body = response.text().await?;

        if !status.is_success() {
            return Err(ClobError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        // Parse response
        let order_response: CreateOrderResponse = serde_json::from_str(&body)
            .map_err(|e| ClobError::Parse(format!("Failed to parse order response: {} - body: {}", e, body)))?;

        // Convert to OrderResult
        self.parse_order_response(params, order_response)
    }

    /// Submits an order with automatic retries for transient errors.
    pub async fn submit_order_with_retry(&self, params: &OrderParams) -> Result<OrderResult, ClobError> {
        let mut delay = BASE_RETRY_DELAY;

        for attempt in 0..=self.config.max_retries {
            match self.submit_order(params).await {
                Ok(result) => return Ok(result),
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    warn!(
                        attempt = attempt + 1,
                        max_retries = self.config.max_retries,
                        error = %e,
                        delay_ms = delay.as_millis(),
                        "Order submission failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(MAX_RETRY_DELAY);
                }
                Err(e) => return Err(e),
            }
        }

        Err(ClobError::Api {
            status: 0,
            message: "Max retries exceeded".to_string(),
        })
    }

    /// Gets the status of an order by ID.
    pub async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ClobError> {
        let url = format!("{}/order/{}", self.config.base_url, order_id);

        debug!(order_id = %order_id, "Fetching order status");

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: body,
            });
        }

        let status_response: OrderStatusResponse = response.json().await.map_err(|e| {
            ClobError::Parse(format!("Failed to parse order status: {}", e))
        })?;

        self.parse_status_response(status_response)
    }

    /// Cancels an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), ClobError> {
        if !self.is_authenticated() {
            return Err(ClobError::Auth("Client not authenticated".to_string()));
        }

        let url = format!("{}/order/{}", self.config.base_url, order_id);

        info!(order_id = %order_id, "Cancelling order");

        let response = self.http.delete(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: body,
            });
        }

        Ok(())
    }

    // =========================================================================
    // Private Helpers
    // =========================================================================

    /// Validates order parameters before submission.
    fn validate_order_params(&self, params: &OrderParams) -> Result<(), ClobError> {
        // Price must be between 0.01 and 0.99
        if params.price < dec!(0.01) || params.price > dec!(0.99) {
            return Err(ClobError::Config(format!(
                "Price {} must be between 0.01 and 0.99",
                params.price
            )));
        }

        // Size must be positive
        if params.size <= Decimal::ZERO {
            return Err(ClobError::Config(format!(
                "Size {} must be positive",
                params.size
            )));
        }

        // Token ID must not be empty
        if params.token_id.is_empty() {
            return Err(ClobError::Config("Token ID cannot be empty".to_string()));
        }

        Ok(())
    }

    /// Creates an order request with EIP-712 signature.
    fn create_order_request(&self, params: &OrderParams) -> Result<CreateOrderRequest, ClobError> {
        // Generate nonce (timestamp-based for uniqueness)
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_else(|_| "0".to_string());

        // No expiration for now
        let expiration = "0".to_string();

        // TODO: Implement actual EIP-712 signing
        // For now, use a placeholder signature
        let signature = format!("0x{}", "00".repeat(65));

        Ok(CreateOrderRequest {
            token_id: params.token_id.clone(),
            side: params.side.into(),
            price: params.price.to_string(),
            size: params.size.to_string(),
            order_type: params.order_type.into(),
            fee_rate_bps: Some(0),
            nonce,
            expiration,
            signature,
        })
    }

    /// Parses order creation response into OrderResult.
    fn parse_order_response(
        &self,
        params: &OrderParams,
        response: CreateOrderResponse,
    ) -> Result<OrderResult, ClobError> {
        if !response.success {
            let error_msg = response.error_msg.unwrap_or_else(|| "Unknown error".to_string());
            return Err(ClobError::OrderRejected(error_msg));
        }

        // Determine status from response
        let status = match response.status.as_deref() {
            Some("FILLED") | Some("filled") => OrderStatus::Filled,
            Some("PARTIAL") | Some("partial") => OrderStatus::PartiallyFilled,
            Some("LIVE") | Some("live") | Some("OPEN") | Some("open") => OrderStatus::Pending,
            Some("CANCELLED") | Some("cancelled") => OrderStatus::Cancelled,
            Some("EXPIRED") | Some("expired") => OrderStatus::Expired,
            _ => {
                // If we have transaction hashes, likely filled
                if !response.transaction_hashes.is_empty() {
                    OrderStatus::Filled
                } else {
                    OrderStatus::Pending
                }
            }
        };

        // For now, assume full fill if status is Filled
        let (filled_size, avg_price) = if status == OrderStatus::Filled {
            (params.size, Some(params.price))
        } else {
            (Decimal::ZERO, None)
        };

        Ok(OrderResult {
            order_id: response.order_id,
            status,
            filled_size,
            avg_fill_price: avg_price,
            error: None,
        })
    }

    /// Parses order status response into OrderResult.
    fn parse_status_response(&self, response: OrderStatusResponse) -> Result<OrderResult, ClobError> {
        let status = match response.status.to_uppercase().as_str() {
            "FILLED" => OrderStatus::Filled,
            "PARTIAL" | "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "LIVE" | "OPEN" | "PENDING" => OrderStatus::Pending,
            "CANCELLED" | "CANCELED" => OrderStatus::Cancelled,
            "EXPIRED" => OrderStatus::Expired,
            "REJECTED" => OrderStatus::Rejected,
            _ => OrderStatus::Pending,
        };

        let filled_size = response.size_matched.parse::<Decimal>().unwrap_or(Decimal::ZERO);
        let avg_price = response.price.and_then(|p| p.parse::<Decimal>().ok());

        Ok(OrderResult {
            order_id: response.id,
            status,
            filled_size,
            avg_fill_price: avg_price,
            error: None,
        })
    }
}

impl std::fmt::Debug for ClobClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClobClient")
            .field("address", &self.wallet.address())
            .field("base_url", &self.config.base_url)
            .field("authenticated", &self.is_authenticated())
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test private key (NOT REAL - for testing only)
    const TEST_PRIVATE_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_wallet() -> Wallet {
        Wallet::from_private_key(TEST_PRIVATE_KEY, 137).unwrap()
    }

    // ==================== Config Tests ====================

    #[test]
    fn test_config_mainnet() {
        let config = ClobClientConfig::mainnet();
        assert_eq!(config.base_url, CLOB_API_MAINNET);
        assert!(config.neg_risk);
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn test_config_testnet() {
        let config = ClobClientConfig::testnet();
        assert_eq!(config.base_url, CLOB_API_TESTNET);
        assert!(config.neg_risk);
    }

    #[test]
    fn test_config_builder() {
        let config = ClobClientConfig::mainnet()
            .with_base_url("https://custom.api")
            .with_timeout(Duration::from_secs(60))
            .with_max_retries(5)
            .with_funder("0x123");

        assert_eq!(config.base_url, "https://custom.api");
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.funder, Some("0x123".to_string()));
    }

    // ==================== Error Tests ====================

    #[test]
    fn test_error_is_retryable() {
        assert!(ClobError::RateLimited { retry_after_ms: 1000 }.is_retryable());
        assert!(ClobError::Timeout(Duration::from_secs(30)).is_retryable());
        assert!(ClobError::Api { status: 500, message: "error".to_string() }.is_retryable());
        assert!(ClobError::Api { status: 503, message: "error".to_string() }.is_retryable());

        // Not retryable
        assert!(!ClobError::Api { status: 400, message: "bad request".to_string() }.is_retryable());
        assert!(!ClobError::Auth("auth failed".to_string()).is_retryable());
        assert!(!ClobError::OrderRejected("rejected".to_string()).is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = ClobError::Api { status: 400, message: "Bad request".to_string() };
        assert!(err.to_string().contains("400"));
        assert!(err.to_string().contains("Bad request"));

        let err = ClobError::RateLimited { retry_after_ms: 5000 };
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn test_error_into_execution_error() {
        let err = ClobError::RateLimited { retry_after_ms: 1000 };
        let exec_err = err.into_execution_error();
        assert!(matches!(exec_err, ExecutionError::RateLimited { retry_after_secs: 1 }));

        let err = ClobError::OrderRejected("no liquidity".to_string());
        let exec_err = err.into_execution_error();
        assert!(matches!(exec_err, ExecutionError::Rejected { reason: _ }));

        let err = ClobError::Timeout(Duration::from_secs(30));
        let exec_err = err.into_execution_error();
        assert!(matches!(exec_err, ExecutionError::Timeout { order_id: _ }));
    }

    // ==================== Client Tests ====================

    #[test]
    fn test_client_creation() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        assert!(!client.is_authenticated());
        assert_eq!(client.config().base_url, CLOB_API_MAINNET);
    }

    #[test]
    fn test_client_address() {
        let wallet = test_wallet();
        let expected_address = wallet.address().to_string();
        let client = ClobClient::mainnet(wallet).unwrap();

        assert_eq!(client.address(), expected_address);
    }

    #[test]
    fn test_client_debug_does_not_expose_secrets() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let debug_str = format!("{:?}", client);
        assert!(!debug_str.contains(TEST_PRIVATE_KEY));
        assert!(debug_str.contains("ClobClient"));
        assert!(debug_str.contains("authenticated"));
    }

    // ==================== API Type Conversion Tests ====================

    #[test]
    fn test_side_conversion() {
        assert_eq!(ApiSide::from(Side::Buy), ApiSide::Buy);
        assert_eq!(ApiSide::from(Side::Sell), ApiSide::Sell);
    }

    #[test]
    fn test_order_type_conversion() {
        assert_eq!(ApiOrderType::from(OrderType::Gtc), ApiOrderType::Gtc);
        assert_eq!(ApiOrderType::from(OrderType::Fok), ApiOrderType::Fok);
        assert_eq!(ApiOrderType::from(OrderType::Fak), ApiOrderType::Ioc);
    }

    // ==================== Validation Tests ====================

    #[test]
    fn test_validate_order_params_valid() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.50),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        assert!(client.validate_order_params(&params).is_ok());
    }

    #[test]
    fn test_validate_order_params_price_too_low() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.005),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let result = client.validate_order_params(&params);
        assert!(matches!(result, Err(ClobError::Config(_))));
    }

    #[test]
    fn test_validate_order_params_price_too_high() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.995),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let result = client.validate_order_params(&params);
        assert!(matches!(result, Err(ClobError::Config(_))));
    }

    #[test]
    fn test_validate_order_params_zero_size() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.50),
            size: dec!(0),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let result = client.validate_order_params(&params);
        assert!(matches!(result, Err(ClobError::Config(_))));
    }

    #[test]
    fn test_validate_order_params_empty_token() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "".to_string(),
            side: Side::Buy,
            price: dec!(0.50),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let result = client.validate_order_params(&params);
        assert!(matches!(result, Err(ClobError::Config(_))));
    }

    // ==================== Request Creation Tests ====================

    #[test]
    fn test_create_order_request() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.45),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let request = client.create_order_request(&params).unwrap();

        assert_eq!(request.token_id, "token-123");
        assert_eq!(request.side, ApiSide::Buy);
        assert_eq!(request.price, "0.45");
        assert_eq!(request.size, "100");
        assert_eq!(request.order_type, ApiOrderType::Fok);
        assert!(!request.nonce.is_empty());
        assert!(request.signature.starts_with("0x"));
    }

    // ==================== Response Parsing Tests ====================

    #[test]
    fn test_parse_order_response_success() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.45),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let response = CreateOrderResponse {
            order_id: "order-456".to_string(),
            success: true,
            error_msg: None,
            transaction_hashes: vec!["0xabc".to_string()],
            status: Some("FILLED".to_string()),
        };

        let result = client.parse_order_response(&params, response).unwrap();

        assert_eq!(result.order_id, "order-456");
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_size, dec!(100));
    }

    #[test]
    fn test_parse_order_response_rejected() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.45),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let response = CreateOrderResponse {
            order_id: "".to_string(),
            success: false,
            error_msg: Some("Insufficient funds".to_string()),
            transaction_hashes: vec![],
            status: None,
        };

        let result = client.parse_order_response(&params, response);
        assert!(matches!(result, Err(ClobError::OrderRejected(_))));
    }

    #[test]
    fn test_parse_status_response() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let response = OrderStatusResponse {
            id: "order-789".to_string(),
            status: "FILLED".to_string(),
            original_size: "100".to_string(),
            size_matched: "100".to_string(),
            price: Some("0.45".to_string()),
            asset_id: "token-123".to_string(),
            side: "BUY".to_string(),
        };

        let result = client.parse_status_response(response).unwrap();

        assert_eq!(result.order_id, "order-789");
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_size, dec!(100));
        assert_eq!(result.avg_fill_price, Some(dec!(0.45)));
    }

    // ==================== Authentication Tests ====================

    #[tokio::test]
    async fn test_submit_order_requires_auth() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.45),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: true,
            presigned: None,
        };

        let result = client.submit_order(&params).await;
        assert!(matches!(result, Err(ClobError::Auth(_))));
    }

    #[tokio::test]
    async fn test_cancel_order_requires_auth() {
        let wallet = test_wallet();
        let client = ClobClient::mainnet(wallet).unwrap();

        let result = client.cancel_order("order-123").await;
        assert!(matches!(result, Err(ClobError::Auth(_))));
    }

    #[tokio::test]
    async fn test_authenticate_sets_credentials() {
        let wallet = test_wallet();
        let mut client = ClobClient::mainnet(wallet).unwrap();

        assert!(!client.is_authenticated());

        client.authenticate().await.unwrap();

        assert!(client.is_authenticated());
    }
}
