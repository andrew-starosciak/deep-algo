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
use wreq_util::Emulation;

use super::clob_auth::{self, L2Auth};
use super::eip712::{self, Eip712Config, SIDE_BUY, SIDE_SELL};
use super::execution::{ExecutionError, OrderParams, OrderResult, OrderStatus, OrderType, Side};
use super::signer::Wallet;
use super::types::L2OrderBook;

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
                | ClobError::Api {
                    status: 500..=599,
                    ..
                }
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

    /// Whether to use neg-risk mode (check market's neg_risk field via CLOB API)
    pub neg_risk: bool,

    /// Taker fee rate in basis points (from market's taker_base_fee field).
    pub taker_fee_bps: u16,

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
            neg_risk: false,
            taker_fee_bps: 0,
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
            neg_risk: false,
            taker_fee_bps: 0,
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

    /// Sets the taker fee rate in basis points.
    ///
    /// Fee-enabled markets (15-min crypto, 5-min crypto, NCAAB, Serie A)
    /// require `1000` bps. Non-fee markets use `0`.
    #[must_use]
    pub fn with_taker_fee_bps(mut self, bps: u16) -> Self {
        self.taker_fee_bps = bps;
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

/// Signed order payload matching the Polymarket CLOB API format.
///
/// Field names use camelCase to match the API. Numeric amounts are strings.
/// Side is "BUY"/"SELL". signatureType is an integer (0 = EOA).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrderPayload {
    /// Random salt for uniqueness.
    pub salt: u64,
    /// Maker (funder) address.
    pub maker: String,
    /// Signer address.
    pub signer: String,
    /// Taker address (zero address for public orders).
    pub taker: String,
    /// Conditional token ID as decimal string.
    pub token_id: String,
    /// Maximum amount maker spends (raw USDC units, 6 decimals).
    pub maker_amount: String,
    /// Minimum amount taker pays (raw token units, 6 decimals).
    pub taker_amount: String,
    /// Expiration timestamp ("0" for no expiration).
    pub expiration: String,
    /// Nonce for on-chain cancellation ("0" typically).
    pub nonce: String,
    /// Fee rate in basis points ("0" typically).
    pub fee_rate_bps: String,
    /// Order side: "BUY" or "SELL".
    pub side: String,
    /// Signature type: 0 = EOA, 1 = POLY_PROXY.
    pub signature_type: u8,
    /// EIP-712 hex signature.
    pub signature: String,
}

/// Top-level POST /order request payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostOrderPayload {
    /// The signed order object.
    pub order: SignedOrderPayload,
    /// API key of the order owner.
    pub owner: String,
    /// Order type: "FOK", "GTC", or "GTD".
    pub order_type: String,
}

/// Response from order creation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderResponse {
    /// Order ID assigned by the exchange.
    /// API returns "orderID" (not "orderId"), so we need an alias.
    #[serde(alias = "orderID")]
    pub order_id: String,
    /// Whether the order was accepted
    pub success: bool,
    /// Error message if not successful
    #[serde(default)]
    pub error_msg: Option<String>,
    /// Transaction hashes if filled.
    /// API returns "transactionsHashes" (with extra 's'), so we need an alias.
    #[serde(default, alias = "transactionsHashes")]
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

/// Balance/allowance response from `/balance-allowance`.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    /// Available balance.
    #[serde(default)]
    pub balance: Option<String>,
    /// Allowance for the exchange contract.
    #[serde(default)]
    pub allowance: Option<String>,
}

/// Order book level from the CLOB `/book` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ClobBookLevel {
    /// Price as a string for precision.
    pub price: String,
    /// Size at this price level.
    pub size: String,
}

/// Order book response from `GET /book?token_id=...`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClobBookResponse {
    /// Bid levels (buy orders).
    #[serde(default)]
    pub bids: Vec<ClobBookLevel>,
    /// Ask levels (sell orders).
    #[serde(default)]
    pub asks: Vec<ClobBookLevel>,
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
    pub size: f64,
    /// Average entry price
    pub avg_price: f64,
    /// Initial value when position was opened
    pub initial_value: f64,
    /// Current value of position
    pub current_value: f64,
    /// Cash P&L
    pub cash_pnl: f64,
    /// Percent P&L
    pub percent_pnl: f64,
    /// Current price of token (1.0 = won, 0.0 = lost for resolved markets)
    pub cur_price: f64,
    /// Whether position can be redeemed (market resolved)
    pub redeemable: bool,
    /// Market title
    pub title: String,
    /// Outcome name (Yes/No/Up/Down)
    pub outcome: String,
    /// Outcome index (0 or 1)
    #[serde(default)]
    pub outcome_index: Option<i32>,
    /// Whether position is negative risk
    #[serde(default)]
    pub negative_risk: bool,
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
    /// HTTP client (reqwest — GET endpoints)
    http: Client,
    /// HTTP client with Chrome TLS emulation (wreq — POST endpoints, Cloudflare bypass)
    wreq_http: wreq::Client,
    /// Client configuration
    config: ClobClientConfig,
    /// Wallet for signing
    wallet: Wallet,
    /// API key (derived from wallet signature)
    api_key: Option<String>,
    /// API secret (derived from wallet signature)
    api_secret: Option<String>,
    /// L2 auth for HMAC-signed requests.
    l2_auth: Option<L2Auth>,
    /// API passphrase.
    api_passphrase: Option<String>,
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

        // Build wreq client with Chrome TLS emulation for POST endpoints.
        // This bypasses Cloudflare's TLS fingerprinting that blocks standard reqwest.
        let mut wreq_builder = wreq::Client::builder()
            .emulation(Emulation::Chrome136)
            .cookie_store(true)
            .timeout(config.timeout);

        if let Ok(proxy_url) = std::env::var("POLYMARKET_PROXY") {
            info!(proxy = %proxy_url, "Configuring SOCKS5 proxy for CLOB POST requests");
            wreq_builder = wreq_builder.proxy(
                wreq::Proxy::all(&proxy_url)
                    .map_err(|e| ClobError::Config(format!("Invalid POLYMARKET_PROXY: {e}")))?,
            );
        }

        let wreq_http = wreq_builder
            .build()
            .map_err(|e| ClobError::Config(format!("Failed to build wreq client: {e}")))?;

        Ok(Self {
            http,
            wreq_http,
            config,
            wallet,
            api_key: None,
            api_secret: None,
            l2_auth: None,
            api_passphrase: None,
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
    /// Uses EIP-712 ClobAuth signing to derive or create API credentials.
    /// This must be called before using authenticated endpoints like
    /// order submission.
    pub async fn authenticate(&mut self) -> Result<(), ClobError> {
        info!(address = %self.wallet.address(), "Authenticating with CLOB API");

        // Step 1: Sign ClobAuth EIP-712 message
        let l1_headers = clob_auth::sign_clob_auth(
            self.wallet.address(),
            self.wallet.expose_private_key(),
            0, // nonce
        )
        .map_err(|e| ClobError::Auth(format!("L1 signing failed: {}", e)))?;

        // Step 2: Try to derive existing API key first
        let derive_url = format!("{}/auth/derive-api-key", self.config.base_url);
        let derive_response = self
            .http
            .get(&derive_url)
            .header("POLY_ADDRESS", &l1_headers.address)
            .header("POLY_SIGNATURE", &l1_headers.signature)
            .header("POLY_TIMESTAMP", &l1_headers.timestamp)
            .header("POLY_NONCE", &l1_headers.nonce)
            .send()
            .await?;

        let creds = if derive_response.status().is_success() {
            let body = derive_response.text().await?;
            serde_json::from_str::<clob_auth::ApiCredentials>(&body).map_err(|e| {
                ClobError::Parse(format!(
                    "Failed to parse API credentials: {} - body: {}",
                    e, body
                ))
            })?
        } else {
            // Step 3: Create new API key if derivation fails
            debug!(status = %derive_response.status(), "Derive failed, creating new API key");

            // Re-sign (timestamp may have expired)
            let l1_headers = clob_auth::sign_clob_auth(
                self.wallet.address(),
                self.wallet.expose_private_key(),
                0,
            )
            .map_err(|e| ClobError::Auth(format!("L1 signing failed: {}", e)))?;

            let create_url = format!("{}/auth/api-key", self.config.base_url);
            let create_response = self
                .wreq_http
                .post(&create_url)
                .header("POLY_ADDRESS", &l1_headers.address)
                .header("POLY_SIGNATURE", &l1_headers.signature)
                .header("POLY_TIMESTAMP", &l1_headers.timestamp)
                .header("POLY_NONCE", &l1_headers.nonce)
                .send()
                .await
                .map_err(|e| ClobError::Auth(format!("wreq POST /auth/api-key failed: {e}")))?;

            let status = create_response.status();
            let body = create_response.text().await.map_err(|e| {
                ClobError::Auth(format!("wreq response body read failed: {e}"))
            })?;

            if !status.is_success() {
                return Err(ClobError::Auth(format!(
                    "API key creation failed ({}): {}",
                    status, body
                )));
            }

            serde_json::from_str::<clob_auth::ApiCredentials>(&body).map_err(|e| {
                ClobError::Parse(format!(
                    "Failed to parse API credentials: {} - body: {}",
                    e, body
                ))
            })?
        };

        // Step 4: Store credentials and create L2 auth
        let l2_auth = L2Auth::new(&creds, self.wallet.address().to_string());

        info!(api_key = %creds.api_key, "CLOB authentication successful");

        self.api_key = Some(creds.api_key);
        self.api_secret = Some(creds.secret);
        self.api_passphrase = Some(creds.passphrase);
        self.l2_auth = Some(l2_auth);

        Ok(())
    }

    /// Returns true if the client is authenticated.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.l2_auth.is_some()
    }

    /// Gets the USDC balance for the authenticated wallet.
    pub async fn get_balance(&self) -> Result<Decimal, ClobError> {
        // HMAC signs over the base path only; query params are appended to the URL separately
        let hmac_path = "/balance-allowance";

        // Try both signature types: 1 (Poly proxy) then 0 (EOA direct)
        // Polymarket creates proxy wallets when depositing through the web UI
        for sig_type in [1u8, 0] {
            let url = format!(
                "{}/balance-allowance?asset_type=COLLATERAL&signature_type={}",
                self.config.base_url, sig_type
            );

            debug!(url = %url, signature_type = sig_type, "Fetching balance");

            let mut request = self.http.get(&url);

            if let Some(ref l2_auth) = self.l2_auth {
                let headers = l2_auth
                    .headers("GET", hmac_path, "")
                    .map_err(|e| {
                        ClobError::Auth(format!("L2 header generation failed: {}", e))
                    })?;
                request = request
                    .header("POLY_ADDRESS", &headers.address)
                    .header("POLY_SIGNATURE", &headers.signature)
                    .header("POLY_TIMESTAMP", &headers.timestamp)
                    .header("POLY_API_KEY", &headers.api_key)
                    .header("POLY_PASSPHRASE", &headers.passphrase);
            }

            let response = request.send().await?;

            if !response.status().is_success() {
                continue;
            }

            let body_text = response.text().await.unwrap_or_default();
            debug!(raw_body = %body_text, signature_type = sig_type, "Balance response");

            let balance: BalanceResponse =
                serde_json::from_str(&body_text).map_err(|e| {
                    ClobError::Parse(format!(
                        "Failed to parse balance response: {} - body: {}",
                        e, body_text
                    ))
                })?;

            let balance_str = balance.balance.unwrap_or_else(|| "0".to_string());
            let raw_balance = balance_str.parse::<Decimal>().map_err(|e| {
                ClobError::Parse(format!(
                    "Failed to parse balance value '{}': {}",
                    balance_str, e
                ))
            })?;

            // API returns raw USDC units (6 decimals): 45999455 = $45.999455
            let usdc_scale = Decimal::from(1_000_000u64);
            let balance_val = raw_balance / usdc_scale;

            if balance_val > Decimal::ZERO {
                info!(balance = %balance_val, signature_type = sig_type, "Found CLOB balance");
                return Ok(balance_val);
            }
        }

        Ok(Decimal::ZERO)
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

        let positions: Vec<WalletPosition> = response
            .json()
            .await
            .map_err(|e| ClobError::Parse(format!("Failed to parse positions response: {}", e)))?;

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
    pub async fn check_position_outcome(&self, token_id: &str) -> Result<Option<bool>, ClobError> {
        let positions = self.get_positions_for_tokens(&[token_id]).await?;

        if let Some(pos) = positions.first() {
            // Check if resolved (redeemable or price at extremes)
            if pos.redeemable || pos.cur_price >= 0.95 || pos.cur_price <= 0.05 {
                // Won if price >= 0.95
                return Ok(Some(pos.cur_price >= 0.95));
            }
        }

        Ok(None)
    }

    /// Finds and redeems all resolved positions on-chain.
    ///
    /// Queries the Data API for positions with `redeemable = true`,
    /// groups them by condition ID, and calls `CTF.redeemPositions()`
    /// for each one.
    ///
    /// # Arguments
    /// * `rpc_url` - Polygon RPC URL for on-chain transactions
    ///
    /// # Returns
    /// Number of conditions redeemed (0 if none found).
    pub async fn redeem_resolved_positions(
        &self,
        rpc_url: &str,
    ) -> Result<u64, ClobError> {
        use std::collections::HashMap;
        use super::approvals;

        let positions = self.get_positions().await?;

        let redeemable_count = positions.iter().filter(|p| p.redeemable && p.size > 0.0).count();
        let total_count = positions.len();
        info!(
            total = total_count,
            redeemable = redeemable_count,
            "Fetched positions for redemption check"
        );
        for pos in positions.iter().filter(|p| p.redeemable && p.size > 0.0) {
            info!(
                title = %pos.title,
                outcome = %pos.outcome,
                size = pos.size,
                condition_id = %pos.condition_id,
                neg_risk = pos.negative_risk,
                outcome_index = ?pos.outcome_index,
                "Found redeemable position"
            );
        }

        // Separate redeemable positions into standard CTF vs neg_risk paths.
        // Standard markets: redeem via CTF.redeemPositions(collateral, parent, cid, indexSets)
        // Neg risk markets: redeem via NegRiskAdapter.redeemPositions(cid, amounts)
        let mut standard: HashMap<String, Vec<u32>> = HashMap::new();
        let mut neg_risk: HashMap<String, Vec<(u32, f64)>> = HashMap::new();

        for pos in &positions {
            if !pos.redeemable || pos.size <= 0.0 {
                continue;
            }

            if pos.negative_risk {
                // Neg risk: collect (outcome_index, size) per condition
                let outcome_idx = match pos.outcome_index {
                    Some(idx) if idx >= 0 => idx as u32,
                    Some(idx) => {
                        warn!(
                            condition_id = %pos.condition_id,
                            outcome_index = idx,
                            "Neg risk position has invalid negative outcome_index, skipping"
                        );
                        continue;
                    }
                    None => {
                        warn!(
                            condition_id = %pos.condition_id,
                            title = %pos.title,
                            "Neg risk position missing outcome_index, skipping"
                        );
                        continue;
                    }
                };
                neg_risk
                    .entry(pos.condition_id.clone())
                    .or_default()
                    .push((outcome_idx, pos.size));
            } else {
                // Standard CTF: collect index sets per condition
                let idx_set = match pos.outcome_index {
                    Some(0) => 1u32,
                    Some(1) => 2u32,
                    _ => {
                        let entry = standard.entry(pos.condition_id.clone()).or_default();
                        if !entry.contains(&1) { entry.push(1); }
                        if !entry.contains(&2) { entry.push(2); }
                        continue;
                    }
                };
                let entry = standard.entry(pos.condition_id.clone()).or_default();
                if !entry.contains(&idx_set) {
                    entry.push(idx_set);
                }
            }
        }

        if standard.is_empty() && neg_risk.is_empty() {
            return Ok(0);
        }

        let mut total_redeemed = 0u64;

        // Redeem standard positions via CTF
        if !standard.is_empty() {
            info!(
                count = standard.len(),
                "Redeeming {} standard conditions via CTF",
                standard.len()
            );
            let conditions: Vec<(String, Vec<u32>)> = standard.into_iter().collect();
            let conditions_ref: Vec<(&str, Vec<u32>)> = conditions
                .iter()
                .map(|(cid, idx)| (cid.as_str(), idx.clone()))
                .collect();

            let tx_hashes = approvals::redeem_positions(&self.wallet, rpc_url, &conditions_ref)
                .await
                .map_err(|e| ClobError::Api {
                    status: 0,
                    message: format!("Standard redemption failed: {}", e),
                })?;

            info!(tx_count = tx_hashes.len(), "Standard redemption txs confirmed");
            total_redeemed += conditions_ref.len() as u64;
        }

        // Redeem neg_risk positions via NegRiskAdapter
        if !neg_risk.is_empty() {
            info!(
                count = neg_risk.len(),
                "Redeeming {} neg_risk conditions via NegRiskAdapter",
                neg_risk.len()
            );

            // Convert (outcome_index, size) pairs to amounts arrays.
            // Each element amounts[i] = raw token amount for outcome slot i.
            let scale = 10u128.pow(approvals::CTF_DECIMALS);
            let conditions: Vec<(String, Vec<u128>)> = neg_risk
                .into_iter()
                .map(|(cid, slots)| {
                    let max_idx = slots.iter().map(|(i, _)| *i).max().unwrap_or(0) as usize;
                    let mut amounts = vec![0u128; max_idx + 1];
                    for (idx, size) in slots {
                        amounts[idx as usize] = (size * scale as f64).round() as u128;
                    }
                    (cid, amounts)
                })
                .collect();
            let conditions_ref: Vec<(&str, Vec<u128>)> = conditions
                .iter()
                .map(|(cid, amounts)| (cid.as_str(), amounts.clone()))
                .collect();

            let tx_hashes = approvals::redeem_neg_risk_positions(
                &self.wallet, rpc_url, &conditions_ref,
            )
            .await
            .map_err(|e| ClobError::Api {
                status: 0,
                message: format!("Neg risk redemption failed: {}", e),
            })?;

            info!(tx_count = tx_hashes.len(), "Neg risk redemption txs confirmed");
            total_redeemed += conditions_ref.len() as u64;
        }

        Ok(total_redeemed)
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
            maker_amount = %request.order.maker_amount,
            taker_amount = %request.order.taker_amount,
            "Submitting order to CLOB"
        );

        // Submit to API with L2 auth headers
        let url = format!("{}/order", self.config.base_url);
        let body_json = serde_json::to_string(&request)
            .map_err(|e| ClobError::Parse(format!("Failed to serialize order: {}", e)))?;

        debug!(body = %body_json, "Order request body");

        let l2_auth = self
            .l2_auth
            .as_ref()
            .ok_or_else(|| ClobError::Auth("L2 auth not initialized".to_string()))?;
        let l2_headers = l2_auth
            .headers("POST", "/order", &body_json)
            .map_err(|e| ClobError::Auth(format!("L2 header generation failed: {}", e)))?;

        // Time the actual HTTP round-trip (network latency only)
        let http_start = std::time::Instant::now();
        let response = self
            .wreq_http
            .post(&url)
            .header("POLY_ADDRESS", &l2_headers.address)
            .header("POLY_SIGNATURE", &l2_headers.signature)
            .header("POLY_TIMESTAMP", &l2_headers.timestamp)
            .header("POLY_API_KEY", &l2_headers.api_key)
            .header("POLY_PASSPHRASE", &l2_headers.passphrase)
            .header("Content-Type", "application/json")
            .body(body_json)
            .send()
            .await
            .map_err(|e| ClobError::Api {
                status: 0,
                message: format!("wreq POST /order failed: {e}"),
            })?;
        let http_latency_ms = http_start.elapsed().as_millis() as u64;

        let status = response.status();

        if status.as_u16() == 429 {
            // Rate limited
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1000);
            return Err(ClobError::RateLimited {
                retry_after_ms: retry_after,
            });
        }

        let body = response.text().await.map_err(|e| ClobError::Api {
            status: status.as_u16(),
            message: format!("wreq response body read failed: {e}"),
        })?;

        if !status.is_success() {
            // FOK orders that can't be fully filled return HTTP 400 with a specific message.
            // This is normal liquidity behavior, not an error - return a Rejected OrderResult
            // instead of an error so the circuit breaker is not triggered.
            if status.as_u16() == 400 && body.contains("couldn't be fully filled") {
                info!(body = %body, latency_ms = http_latency_ms, "FOK order not filled (insufficient liquidity)");
                let mut result = OrderResult::rejected(
                    "fok-not-filled",
                    format!("FOK not filled: {}", body),
                );
                result.latency_ms = Some(http_latency_ms);
                return Ok(result);
            }

            warn!(
                http_status = status.as_u16(),
                body = %body,
                "Order API error response"
            );
            return Err(ClobError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        debug!(body = %body, latency_ms = http_latency_ms, "Order API response");

        // Parse response
        let order_response: CreateOrderResponse = serde_json::from_str(&body).map_err(|e| {
            ClobError::Parse(format!(
                "Failed to parse order response: {} - body: {}",
                e, body
            ))
        })?;

        // Convert to OrderResult and attach HTTP latency
        let mut result = self.parse_order_response(params, order_response)?;
        result.latency_ms = Some(http_latency_ms);
        Ok(result)
    }

    /// Submits an order with automatic retries for transient errors.
    pub async fn submit_order_with_retry(
        &self,
        params: &OrderParams,
    ) -> Result<OrderResult, ClobError> {
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

        let status_response: OrderStatusResponse = response
            .json()
            .await
            .map_err(|e| ClobError::Parse(format!("Failed to parse order status: {}", e)))?;

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

    /// Cancels all open orders for this wallet.
    pub async fn cancel_all_orders(&self) -> Result<u32, ClobError> {
        if !self.is_authenticated() {
            return Err(ClobError::Auth("Client not authenticated".to_string()));
        }

        let l2_auth = self
            .l2_auth
            .as_ref()
            .ok_or_else(|| ClobError::Auth("L2 auth not initialized".to_string()))?;

        let url = format!("{}/cancel-all", self.config.base_url);
        info!("Cancelling all open orders");

        let l2_headers = l2_auth
            .headers("DELETE", "/cancel-all", "")
            .map_err(|e| ClobError::Auth(format!("L2 header generation failed: {}", e)))?;

        let response = self
            .wreq_http
            .delete(&url)
            .header("POLY_ADDRESS", &l2_headers.address)
            .header("POLY_SIGNATURE", &l2_headers.signature)
            .header("POLY_TIMESTAMP", &l2_headers.timestamp)
            .header("POLY_API_KEY", &l2_headers.api_key)
            .header("POLY_PASSPHRASE", &l2_headers.passphrase)
            .send()
            .await
            .map_err(|e| ClobError::Api {
                status: 0,
                message: format!("wreq DELETE /cancel-all failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status, body = %body, "Cancel-all request failed");
            return Ok(0);
        }

        let body = response.text().await.unwrap_or_default();
        info!(response = %body, "Cancel-all response");
        Ok(1)
    }

    /// Fetches the L2 order book for a token from the CLOB REST API.
    ///
    /// Public endpoint — no authentication required.
    pub async fn get_clob_book(&self, token_id: &str) -> Result<L2OrderBook, ClobError> {
        let url = format!("{}/book?token_id={}", self.config.base_url, token_id);

        debug!(token_id = %token_id, "Fetching CLOB order book");

        let response = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: body,
            });
        }

        let book_response: ClobBookResponse = response
            .json()
            .await
            .map_err(|e| ClobError::Parse(format!("Failed to parse order book: {}", e)))?;

        let mut book = L2OrderBook::new(token_id.to_string());
        let bids: Vec<(Decimal, Decimal)> = book_response
            .bids
            .iter()
            .filter_map(|level| {
                let price = level.price.parse::<Decimal>().ok()?;
                let size = level.size.parse::<Decimal>().ok()?;
                Some((price, size))
            })
            .collect();
        let asks: Vec<(Decimal, Decimal)> = book_response
            .asks
            .iter()
            .filter_map(|level| {
                let price = level.price.parse::<Decimal>().ok()?;
                let size = level.size.parse::<Decimal>().ok()?;
                Some((price, size))
            })
            .collect();
        book.apply_snapshot(bids, asks);

        Ok(book)
    }

    /// Queries the taker fee rate for a token from the CLOB API.
    ///
    /// Returns the fee rate in basis points (e.g. 1000 = 10%).
    /// Fee-enabled markets (15-min crypto, 5-min crypto) return 1000;
    /// non-fee markets return 0.
    ///
    /// Public endpoint — no authentication required.
    pub async fn get_fee_rate_bps(&self, token_id: &str) -> Result<u16, ClobError> {
        let url = format!("{}/fee-rate?token_id={}", self.config.base_url, token_id);

        let response = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClobError::Api {
                status,
                message: body,
            });
        }

        #[derive(Deserialize)]
        struct FeeRateResponse {
            #[serde(default)]
            fee_rate_bps: Option<String>,
            #[serde(default, rename = "feeRateBps")]
            fee_rate_bps_alt: Option<String>,
        }

        let body = response.text().await.unwrap_or_default();
        let parsed: FeeRateResponse = serde_json::from_str(&body)
            .map_err(|e| ClobError::Parse(format!("Failed to parse fee-rate response: {} - body: {}", e, body)))?;

        let bps_str = parsed
            .fee_rate_bps
            .or(parsed.fee_rate_bps_alt)
            .unwrap_or_else(|| "0".to_string());

        bps_str.parse::<u16>().map_err(|e| {
            ClobError::Parse(format!("Invalid fee_rate_bps '{}': {}", bps_str, e))
        })
    }

    /// Updates the taker fee rate used for order signing.
    pub fn set_taker_fee_bps(&mut self, bps: u16) {
        self.config.taker_fee_bps = bps;
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

    /// Creates an order request with real EIP-712 signature.
    fn create_order_request(&self, params: &OrderParams) -> Result<PostOrderPayload, ClobError> {
        // Nonce: "0" for on-chain cancellation (salt provides uniqueness)
        let nonce: u64 = 0;

        // Expiration: CLOB requires 0 for FOK, FAK, and GTC orders.
        // Only GTD (Good-til-Date) orders use a non-zero expiration.
        let expiration_secs: u64 = 0;

        // Convert side to EIP-712 format
        let eip712_side = match params.side {
            Side::Buy => SIDE_BUY,
            Side::Sell => SIDE_SELL,
        };

        // Build EIP-712 order
        let eip712_config = Eip712Config {
            chain_id: self.wallet.chain_id(),
            neg_risk: self.config.neg_risk,
        };

        // FOK: CLOB enforces max 2dp for BUY maker_amount.
        // FAK/GTC: CLOB expects exact product (up to 4dp).
        let maker_amount_dp = match params.order_type {
            OrderType::Fok => 2,
            OrderType::Fak | OrderType::Gtc => 4,
        };

        let order = eip712::build_order(&eip712::BuildOrderParams {
            maker_address: self.wallet.address(),
            token_id: &params.token_id,
            side: eip712_side,
            price: params.price,
            size: params.size,
            expiration_secs,
            nonce,
            fee_rate_bps: self.config.taker_fee_bps,
            maker_amount_dp,
        })
        .map_err(|e| ClobError::Signing(format!("Failed to build order: {}", e)))?;

        // Sign the order
        let signature =
            eip712::sign_order(&order, &eip712_config, self.wallet.expose_private_key())
                .map_err(|e| ClobError::Signing(format!("Failed to sign order: {}", e)))?;

        // Build the Polymarket API payload
        let maker_addr = format!("0x{}", hex::encode(order.maker));
        let signer_addr = format!("0x{}", hex::encode(order.signer));
        let taker_addr = format!("0x{}", hex::encode(order.taker));

        let side_str = match eip712_side {
            SIDE_BUY => "BUY",
            _ => "SELL",
        };

        let order_type_str = match params.order_type {
            OrderType::Fok => "FOK",
            OrderType::Fak => "FAK", // Fill-And-Kill (Polymarket's term for IOC)
            OrderType::Gtc => "GTC",
        };

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| ClobError::Auth("Not authenticated - no API key".to_string()))?;

        Ok(PostOrderPayload {
            order: SignedOrderPayload {
                salt: order.salt,
                maker: maker_addr,
                signer: signer_addr,
                taker: taker_addr,
                token_id: order.token_id.clone(),
                maker_amount: order.maker_amount.to_string(),
                taker_amount: order.taker_amount.to_string(),
                expiration: order.expiration.to_string(),
                nonce: order.nonce.to_string(),
                fee_rate_bps: order.fee_rate_bps.to_string(),
                side: side_str.to_string(),
                signature_type: order.signature_type,
                signature,
            },
            owner: api_key.clone(),
            order_type: order_type_str.to_string(),
        })
    }

    /// Parses order creation response into OrderResult.
    fn parse_order_response(
        &self,
        params: &OrderParams,
        response: CreateOrderResponse,
    ) -> Result<OrderResult, ClobError> {
        if !response.success {
            let error_msg = response
                .error_msg
                .unwrap_or_else(|| "Unknown error".to_string());
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
            latency_ms: None,
        })
    }

    /// Parses order status response into OrderResult.
    fn parse_status_response(
        &self,
        response: OrderStatusResponse,
    ) -> Result<OrderResult, ClobError> {
        let status = match response.status.to_uppercase().as_str() {
            "FILLED" => OrderStatus::Filled,
            "PARTIAL" | "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "LIVE" | "OPEN" | "PENDING" => OrderStatus::Pending,
            "CANCELLED" | "CANCELED" => OrderStatus::Cancelled,
            "EXPIRED" => OrderStatus::Expired,
            "REJECTED" => OrderStatus::Rejected,
            _ => OrderStatus::Pending,
        };

        let filled_size = response
            .size_matched
            .parse::<Decimal>()
            .unwrap_or(Decimal::ZERO);
        let avg_price = response.price.and_then(|p| p.parse::<Decimal>().ok());

        Ok(OrderResult {
            order_id: response.id,
            status,
            filled_size,
            avg_fill_price: avg_price,
            error: None,
            latency_ms: None,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Test private key (NOT REAL - for testing only)
    const TEST_PRIVATE_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_wallet() -> Wallet {
        Wallet::from_private_key(TEST_PRIVATE_KEY, 137).unwrap()
    }

    /// Creates a test client with a fake API key set (for testing order creation).
    fn test_client_with_auth() -> ClobClient {
        let wallet = test_wallet();
        let mut client = ClobClient::mainnet(wallet).unwrap();
        client.api_key = Some("test-api-key-1234".to_string());
        client
    }

    // ==================== Config Tests ====================

    #[test]
    fn test_config_mainnet() {
        let config = ClobClientConfig::mainnet();
        assert_eq!(config.base_url, CLOB_API_MAINNET);
        assert!(!config.neg_risk);
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn test_config_testnet() {
        let config = ClobClientConfig::testnet();
        assert_eq!(config.base_url, CLOB_API_TESTNET);
        assert!(!config.neg_risk);
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
        assert!(ClobError::RateLimited {
            retry_after_ms: 1000
        }
        .is_retryable());
        assert!(ClobError::Timeout(Duration::from_secs(30)).is_retryable());
        assert!(ClobError::Api {
            status: 500,
            message: "error".to_string()
        }
        .is_retryable());
        assert!(ClobError::Api {
            status: 503,
            message: "error".to_string()
        }
        .is_retryable());

        // Not retryable
        assert!(!ClobError::Api {
            status: 400,
            message: "bad request".to_string()
        }
        .is_retryable());
        assert!(!ClobError::Auth("auth failed".to_string()).is_retryable());
        assert!(!ClobError::OrderRejected("rejected".to_string()).is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = ClobError::Api {
            status: 400,
            message: "Bad request".to_string(),
        };
        assert!(err.to_string().contains("400"));
        assert!(err.to_string().contains("Bad request"));

        let err = ClobError::RateLimited {
            retry_after_ms: 5000,
        };
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn test_error_into_execution_error() {
        let err = ClobError::RateLimited {
            retry_after_ms: 1000,
        };
        let exec_err = err.into_execution_error();
        assert!(matches!(
            exec_err,
            ExecutionError::RateLimited {
                retry_after_secs: 1
            }
        ));

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
            neg_risk: false,
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
            neg_risk: false,
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
            neg_risk: false,
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
            neg_risk: false,
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
            neg_risk: false,
            presigned: None,
        };

        let result = client.validate_order_params(&params);
        assert!(matches!(result, Err(ClobError::Config(_))));
    }

    // ==================== Request Creation Tests ====================

    #[test]
    fn test_create_order_request() {
        let client = test_client_with_auth();

        let params = OrderParams {
            token_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.45),
            size: dec!(100),
            order_type: OrderType::Fok,
            neg_risk: false,
            presigned: None,
        };

        let payload = client.create_order_request(&params).unwrap();

        // Top-level fields
        assert_eq!(payload.order_type, "FOK");
        assert!(!payload.owner.is_empty()); // API key

        // Order fields
        assert_eq!(payload.order.token_id, "token-123");
        assert_eq!(payload.order.side, "BUY");
        assert_ne!(payload.order.salt, 0);
        assert!(!payload.order.maker.is_empty());
        assert!(!payload.order.signer.is_empty());
        assert_eq!(payload.order.maker, payload.order.signer); // EOA
        assert_eq!(payload.order.nonce, "0");
        assert_eq!(payload.order.fee_rate_bps, "0");
        assert_eq!(payload.order.signature_type, 0); // EOA
        assert!(payload.order.signature.starts_with("0x"));

        // Amounts should be non-empty strings
        assert!(!payload.order.maker_amount.is_empty());
        assert!(!payload.order.taker_amount.is_empty());
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
            neg_risk: false,
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
            neg_risk: false,
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
            neg_risk: false,
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
        let mock_server = MockServer::start().await;

        // Mock the derive-api-key endpoint to return valid credentials
        Mock::given(method("GET"))
            .and(path("/auth/derive-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "apiKey": "test-api-key-123",
                "secret": base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE, b"test-secret-bytes"),
                "passphrase": "test-passphrase"
            })))
            .mount(&mock_server)
            .await;

        let wallet = test_wallet();
        let config = ClobClientConfig::mainnet().with_base_url(&mock_server.uri());
        let mut client = ClobClient::new(wallet, config).unwrap();

        assert!(!client.is_authenticated());

        client.authenticate().await.unwrap();

        assert!(client.is_authenticated());
    }
}
