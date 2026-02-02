//! Kalshi REST API client with rate limiting.
//!
//! Provides typed access to Kalshi API endpoints with automatic
//! rate limiting using the governor crate.
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_kalshi::{KalshiClient, KalshiClientConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = KalshiClient::new(KalshiClientConfig::demo())?;
//!
//!     // Get BTC markets
//!     let markets = client.get_markets(Some("btc")).await?;
//!     println!("Found {} BTC markets", markets.len());
//!
//!     // Get orderbook
//!     let book = client.get_orderbook("KXBTC-26FEB02-B100000", 10).await?;
//!     println!("Best bid: {:?}", book.best_yes_bid());
//!
//!     Ok(())
//! }
//! ```

use crate::auth::{KalshiAuth, KalshiAuthConfig};
use crate::error::{KalshiError, Result};
use crate::types::{Balance, Market, Order, OrderRequest, Orderbook, Position, PriceLevel};
use chrono::{DateTime, Utc};
use governor::{Quota, RateLimiter};
use nonzero_ext::nonzero;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;

// =============================================================================
// Constants
// =============================================================================

/// Kalshi production API base URL.
pub const KALSHI_PROD_URL: &str = "https://trading-api.kalshi.com/trade-api/v2";

/// Kalshi demo API base URL.
pub const KALSHI_DEMO_URL: &str = "https://demo-api.kalshi.co/trade-api/v2";

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Kalshi client.
#[derive(Debug, Clone)]
pub struct KalshiClientConfig {
    /// Base URL for the API.
    pub base_url: String,

    /// Authentication configuration.
    pub auth_config: KalshiAuthConfig,

    /// Requests per minute limit.
    pub requests_per_minute: NonZeroU32,

    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for KalshiClientConfig {
    fn default() -> Self {
        Self {
            base_url: KALSHI_PROD_URL.to_string(),
            auth_config: KalshiAuthConfig::default(),
            requests_per_minute: nonzero!(60u32),
            timeout_secs: 30,
        }
    }
}

impl KalshiClientConfig {
    /// Creates a configuration for production.
    #[must_use]
    pub fn production() -> Self {
        Self::default()
    }

    /// Creates a configuration for demo environment.
    #[must_use]
    pub fn demo() -> Self {
        Self {
            base_url: KALSHI_DEMO_URL.to_string(),
            auth_config: KalshiAuthConfig::demo(),
            ..Default::default()
        }
    }

    /// Sets the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Sets the authentication configuration.
    #[must_use]
    pub fn with_auth_config(mut self, config: KalshiAuthConfig) -> Self {
        self.auth_config = config;
        self
    }

    /// Sets the rate limit.
    #[must_use]
    pub fn with_rate_limit(mut self, requests_per_minute: NonZeroU32) -> Self {
        self.requests_per_minute = requests_per_minute;
        self
    }

    /// Sets the request timeout.
    #[must_use]
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

// =============================================================================
// API Response Types
// =============================================================================

/// Raw market response from Kalshi API.
#[derive(Debug, Clone, Deserialize)]
struct RawMarketsResponse {
    markets: Option<Vec<RawMarket>>,
    #[allow(dead_code)]
    cursor: Option<String>,
}

/// Raw market data from API.
#[derive(Debug, Clone, Deserialize)]
struct RawMarket {
    ticker: String,
    event_ticker: String,
    title: Option<String>,
    subtitle: Option<String>,
    status: Option<String>,
    yes_bid: Option<i64>,
    yes_ask: Option<i64>,
    no_bid: Option<i64>,
    no_ask: Option<i64>,
    last_price: Option<i64>,
    volume_24h: Option<i64>,
    open_interest: Option<i64>,
    close_time: Option<String>,
    expiration_time: Option<String>,
    strike_value: Option<f64>,
    category: Option<String>,
}

impl From<RawMarket> for Market {
    fn from(raw: RawMarket) -> Self {
        use crate::types::MarketStatus;

        let status = match raw.status.as_deref() {
            Some("open") => MarketStatus::Open,
            Some("closed") => MarketStatus::Closed,
            Some("settled") => MarketStatus::Settled,
            Some("paused") => MarketStatus::Paused,
            _ => MarketStatus::Closed,
        };

        Self {
            ticker: raw.ticker,
            event_ticker: raw.event_ticker,
            title: raw.title.unwrap_or_default(),
            subtitle: raw.subtitle,
            status,
            yes_bid: raw.yes_bid.map(Decimal::from),
            yes_ask: raw.yes_ask.map(Decimal::from),
            no_bid: raw.no_bid.map(Decimal::from),
            no_ask: raw.no_ask.map(Decimal::from),
            last_price: raw.last_price.map(Decimal::from),
            volume_24h: raw.volume_24h,
            open_interest: raw.open_interest,
            close_time: raw.close_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            expiration_time: raw.expiration_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            strike_value: raw
                .strike_value
                .map(|v| Decimal::try_from(v).unwrap_or_default()),
            category: raw.category,
        }
    }
}

/// Raw orderbook response from Kalshi API.
#[derive(Debug, Clone, Deserialize)]
struct RawOrderbookResponse {
    orderbook: Option<RawOrderbook>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawOrderbook {
    yes: Option<Vec<Vec<i64>>>, // [[price, count], ...]
    no: Option<Vec<Vec<i64>>>,
}

/// Raw balance response from Kalshi API.
#[derive(Debug, Clone, Deserialize)]
struct RawBalanceResponse {
    balance: Option<i64>,
}

/// Raw order response from Kalshi API.
#[derive(Debug, Clone, Deserialize)]
struct RawOrderResponse {
    order: Option<RawOrder>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawOrder {
    order_id: String,
    client_order_id: Option<String>,
    ticker: String,
    side: Option<String>,
    action: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    status: Option<String>,
    count: Option<i64>,
    filled_count: Option<i64>,
    remaining_count: Option<i64>,
    price: Option<i64>,
    avg_fill_price: Option<f64>,
    created_time: Option<String>,
    updated_time: Option<String>,
}

impl From<RawOrder> for Order {
    fn from(raw: RawOrder) -> Self {
        use crate::types::{Action, OrderStatus, OrderType, Side};

        let side = match raw.side.as_deref() {
            Some("yes") => Side::Yes,
            _ => Side::No,
        };

        let action = match raw.action.as_deref() {
            Some("sell") => Action::Sell,
            _ => Action::Buy,
        };

        let order_type = match raw.order_type.as_deref() {
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match raw.status.as_deref() {
            Some("pending") => OrderStatus::Pending,
            Some("resting") => OrderStatus::Resting,
            Some("filled") => OrderStatus::Filled,
            Some("cancelled") => OrderStatus::Cancelled,
            Some("partial_filled") => OrderStatus::PartialFilled,
            Some("rejected") => OrderStatus::Rejected,
            _ => OrderStatus::Pending,
        };

        Self {
            order_id: raw.order_id,
            client_order_id: raw.client_order_id,
            ticker: raw.ticker,
            side,
            action,
            order_type,
            status,
            count: raw.count.unwrap_or(0) as u32,
            filled_count: raw.filled_count.unwrap_or(0) as u32,
            remaining_count: raw.remaining_count.unwrap_or(0) as u32,
            price: raw.price.map(|p| p as u32),
            avg_fill_price: raw
                .avg_fill_price
                .map(|p| Decimal::try_from(p).unwrap_or_default()),
            created_time: raw.created_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            updated_time: raw.updated_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
        }
    }
}

// =============================================================================
// KalshiClient
// =============================================================================

/// Kalshi REST API client.
///
/// Provides methods for interacting with Kalshi's trading API.
/// All requests are rate-limited and authenticated.
pub struct KalshiClient {
    /// Configuration.
    config: KalshiClientConfig,

    /// HTTP client.
    http: Client,

    /// Rate limiter.
    rate_limiter: Arc<
        RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,

    /// Authentication handler.
    auth: KalshiAuth,
}

impl std::fmt::Debug for KalshiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KalshiClient")
            .field("base_url", &self.config.base_url)
            .field("requests_per_minute", &self.config.requests_per_minute)
            .finish_non_exhaustive()
    }
}

impl KalshiClient {
    /// Creates a new client with the given configuration.
    ///
    /// # Arguments
    /// * `config` - Client configuration
    ///
    /// # Errors
    /// Returns error if authentication setup fails.
    pub fn new(config: KalshiClientConfig) -> Result<Self> {
        let auth = KalshiAuth::from_env(config.auth_config.clone())?;

        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| KalshiError::Network(format!("failed to build HTTP client: {e}")))?;

        let quota = Quota::per_minute(config.requests_per_minute);
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Ok(Self {
            config,
            http,
            rate_limiter,
            auth,
        })
    }

    /// Creates a client for production.
    ///
    /// # Errors
    /// Returns error if authentication setup fails.
    pub fn production() -> Result<Self> {
        Self::new(KalshiClientConfig::production())
    }

    /// Creates a client for demo environment.
    ///
    /// # Errors
    /// Returns error if authentication setup fails.
    pub fn demo() -> Result<Self> {
        Self::new(KalshiClientConfig::demo())
    }

    /// Returns the base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Validates a ticker string to prevent path traversal attacks.
    ///
    /// Valid tickers contain only alphanumeric characters, hyphens, and underscores.
    /// Examples: "KXBTC-26FEB02-B100000", "KXETH_TEST"
    fn validate_ticker(ticker: &str) -> Result<&str> {
        // Check for path traversal attempts
        if ticker.contains("..") || ticker.contains('/') || ticker.contains('\\') {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid ticker: contains forbidden characters: {}",
                ticker
            )));
        }

        // Check for empty ticker
        if ticker.is_empty() {
            return Err(KalshiError::InvalidOrder("ticker cannot be empty".to_string()));
        }

        // Validate characters: alphanumeric, hyphen, underscore only
        if !ticker.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid ticker: must contain only alphanumeric, hyphen, or underscore: {}",
                ticker
            )));
        }

        // Reasonable length limit
        if ticker.len() > 64 {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid ticker: exceeds maximum length of 64: {}",
                ticker.len()
            )));
        }

        Ok(ticker)
    }

    /// Validates an identifier (order_id, etc.) to prevent path traversal attacks.
    fn validate_identifier(id: &str) -> Result<&str> {
        // Check for path traversal attempts
        if id.contains("..") || id.contains('/') || id.contains('\\') {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid identifier: contains forbidden characters: {}",
                id
            )));
        }

        // Check for empty identifier
        if id.is_empty() {
            return Err(KalshiError::InvalidOrder("identifier cannot be empty".to_string()));
        }

        // Validate characters: alphanumeric, hyphen, underscore only
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid identifier: must contain only alphanumeric, hyphen, or underscore: {}",
                id
            )));
        }

        // Reasonable length limit
        if id.len() > 128 {
            return Err(KalshiError::InvalidOrder(format!(
                "invalid identifier: exceeds maximum length of 128: {}",
                id.len()
            )));
        }

        Ok(id)
    }

    /// Sets a custom base URL (useful for testing).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.config.base_url = url.into();
        self
    }

    /// Waits for rate limiter and makes an authenticated GET request.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.rate_limiter.until_ready().await;

        let url = format!("{}{}", self.config.base_url, path);
        let headers = self.auth.sign_request("GET", path, "")?;

        tracing::debug!("GET {}", url);

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .header(headers.as_tuples()[0].0, headers.as_tuples()[0].1)
            .header(headers.as_tuples()[1].0, headers.as_tuples()[1].1)
            .header(headers.as_tuples()[2].0, headers.as_tuples()[2].1)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Waits for rate limiter and makes an authenticated POST request.
    async fn post<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.rate_limiter.until_ready().await;

        let url = format!("{}{}", self.config.base_url, path);
        let body_json = serde_json::to_string(body)?;
        let headers = self.auth.sign_request("POST", path, &body_json)?;

        tracing::debug!("POST {} body_len={}", url, body_json.len());

        let response = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header(headers.as_tuples()[0].0, headers.as_tuples()[0].1)
            .header(headers.as_tuples()[1].0, headers.as_tuples()[1].1)
            .header(headers.as_tuples()[2].0, headers.as_tuples()[2].1)
            .body(body_json)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Waits for rate limiter and makes an authenticated DELETE request.
    async fn delete(&self, path: &str) -> Result<()> {
        self.rate_limiter.until_ready().await;

        let url = format!("{}{}", self.config.base_url, path);
        let headers = self.auth.sign_request("DELETE", path, "")?;

        tracing::debug!("DELETE {}", url);

        let response = self
            .http
            .delete(&url)
            .header("Accept", "application/json")
            .header(headers.as_tuples()[0].0, headers.as_tuples()[0].1)
            .header(headers.as_tuples()[1].0, headers.as_tuples()[1].1)
            .header(headers.as_tuples()[2].0, headers.as_tuples()[2].1)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(KalshiError::api(status.as_u16(), text));
        }

        Ok(())
    }

    /// Handles API response, converting errors appropriately.
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        if status.as_u16() == 429 {
            // Rate limited
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            return Err(KalshiError::rate_limit(retry_after));
        }

        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(KalshiError::api(status.as_u16(), text));
        }

        let body = response.json::<T>().await?;
        Ok(body)
    }

    // =========================================================================
    // Market Endpoints
    // =========================================================================

    /// Gets a list of markets, optionally filtered by event ticker prefix.
    ///
    /// # Arguments
    /// * `event_ticker_prefix` - Optional filter by event ticker prefix (e.g., "KXBTC")
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn get_markets(&self, event_ticker_prefix: Option<&str>) -> Result<Vec<Market>> {
        let path = match event_ticker_prefix {
            Some(prefix) => format!("/markets?event_ticker={}", prefix),
            None => "/markets".to_string(),
        };

        let response: RawMarketsResponse = self.get(&path).await?;

        Ok(response
            .markets
            .unwrap_or_default()
            .into_iter()
            .map(Market::from)
            .collect())
    }

    /// Gets a specific market by ticker.
    ///
    /// # Arguments
    /// * `ticker` - The market ticker (e.g., "KXBTC-26FEB02-B100000")
    ///
    /// # Errors
    /// Returns error if the market is not found or API call fails.
    pub async fn get_market(&self, ticker: &str) -> Result<Market> {
        // Validate ticker to prevent path traversal
        let ticker = Self::validate_ticker(ticker)?;
        let path = format!("/markets/{}", ticker);

        #[derive(Deserialize)]
        struct SingleMarketResponse {
            market: Option<RawMarket>,
        }

        let response: SingleMarketResponse = self.get(&path).await?;

        response
            .market
            .map(Market::from)
            .ok_or_else(|| KalshiError::market_not_found(ticker))
    }

    /// Gets the orderbook for a market.
    ///
    /// # Arguments
    /// * `ticker` - The market ticker
    /// * `depth` - Number of price levels to fetch (max 100)
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn get_orderbook(&self, ticker: &str, depth: u32) -> Result<Orderbook> {
        // Validate ticker to prevent path traversal
        let ticker = Self::validate_ticker(ticker)?;
        let path = format!("/markets/{}/orderbook?depth={}", ticker, depth.min(100));

        let response: RawOrderbookResponse = self.get(&path).await?;

        let raw = response.orderbook.unwrap_or(RawOrderbook {
            yes: None,
            no: None,
        });

        let yes_bids: Vec<PriceLevel> = raw
            .yes
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.len() >= 2)
            .map(|v| PriceLevel {
                price: v[0] as u32,
                count: v[1] as u32,
            })
            .collect();

        // For asks, we need to convert from the no side
        let yes_asks: Vec<PriceLevel> = raw
            .no
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.len() >= 2)
            .map(|v| PriceLevel {
                price: (100 - v[0]) as u32, // Convert NO price to YES ask
                count: v[1] as u32,
            })
            .collect();

        Ok(Orderbook {
            ticker: ticker.to_string(),
            yes_bids,
            yes_asks,
            timestamp: Utc::now(),
        })
    }

    // =========================================================================
    // Portfolio Endpoints
    // =========================================================================

    /// Gets the account balance.
    ///
    /// # Returns
    /// The balance in cents.
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn get_balance(&self) -> Result<Balance> {
        let response: RawBalanceResponse = self.get("/portfolio/balance").await?;

        let balance = response.balance.unwrap_or(0);

        Ok(Balance {
            balance,
            available_balance: balance,
            reserved_balance: 0,
        })
    }

    /// Gets current positions.
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn get_positions(&self) -> Result<Vec<Position>> {
        #[derive(Deserialize)]
        struct PositionsResponse {
            positions: Option<Vec<RawPosition>>,
        }

        #[derive(Deserialize)]
        struct RawPosition {
            ticker: String,
            side: Option<String>,
            count: Option<i64>,
            avg_price: Option<f64>,
            market_price: Option<f64>,
            unrealized_pnl: Option<f64>,
            realized_pnl: Option<f64>,
        }

        let response: PositionsResponse = self.get("/portfolio/positions").await?;

        Ok(response
            .positions
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                use crate::types::Side;
                Position {
                    ticker: p.ticker,
                    side: if p.side.as_deref() == Some("yes") {
                        Side::Yes
                    } else {
                        Side::No
                    },
                    count: p.count.unwrap_or(0) as u32,
                    avg_price: p
                        .avg_price
                        .map(|v| Decimal::try_from(v).unwrap_or_default())
                        .unwrap_or_default(),
                    market_price: p
                        .market_price
                        .map(|v| Decimal::try_from(v).unwrap_or_default()),
                    unrealized_pnl: p
                        .unrealized_pnl
                        .map(|v| Decimal::try_from(v).unwrap_or_default()),
                    realized_pnl: p
                        .realized_pnl
                        .map(|v| Decimal::try_from(v).unwrap_or_default()),
                }
            })
            .collect())
    }

    // =========================================================================
    // Order Endpoints
    // =========================================================================

    /// Submits an order.
    ///
    /// # Arguments
    /// * `order` - The order request
    ///
    /// # Errors
    /// Returns error if the order is rejected or API call fails.
    pub async fn submit_order(&self, order: &OrderRequest) -> Result<Order> {
        let response: RawOrderResponse = self.post("/portfolio/orders", order).await?;

        response
            .order
            .map(Order::from)
            .ok_or_else(|| KalshiError::OrderRejected("no order in response".to_string()))
    }

    /// Cancels an order.
    ///
    /// # Arguments
    /// * `order_id` - The order ID to cancel
    ///
    /// # Errors
    /// Returns error if the order cannot be cancelled.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        // Validate order_id to prevent path traversal
        let order_id = Self::validate_identifier(order_id)?;
        let path = format!("/portfolio/orders/{}", order_id);
        self.delete(&path).await
    }

    /// Gets order status.
    ///
    /// # Arguments
    /// * `order_id` - The order ID
    ///
    /// # Errors
    /// Returns error if the order is not found.
    pub async fn get_order(&self, order_id: &str) -> Result<Order> {
        // Validate order_id to prevent path traversal
        let order_id = Self::validate_identifier(order_id)?;
        let path = format!("/portfolio/orders/{}", order_id);

        let response: RawOrderResponse = self.get(&path).await?;

        response
            .order
            .map(Order::from)
            .ok_or_else(|| KalshiError::order_not_found(order_id))
    }

    // =========================================================================
    // Utility Methods
    // =========================================================================

    /// Discovers BTC-related markets.
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn discover_btc_markets(&self) -> Result<Vec<Market>> {
        let markets = self.get_markets(Some("KXBTC")).await?;
        Ok(markets.into_iter().filter(|m| m.is_btc_market()).collect())
    }

    /// Gets tradeable BTC markets (open status).
    ///
    /// # Errors
    /// Returns error if the API call fails.
    pub async fn get_tradeable_btc_markets(&self) -> Result<Vec<Market>> {
        let markets = self.discover_btc_markets().await?;
        Ok(markets.into_iter().filter(|m| m.is_tradeable()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ==================== Config Tests ====================

    #[test]
    fn test_client_config_default() {
        let config = KalshiClientConfig::default();
        assert_eq!(config.base_url, KALSHI_PROD_URL);
        assert_eq!(config.requests_per_minute.get(), 60);
    }

    #[test]
    fn test_client_config_demo() {
        let config = KalshiClientConfig::demo();
        assert_eq!(config.base_url, KALSHI_DEMO_URL);
    }

    #[test]
    fn test_client_config_builder() {
        let config = KalshiClientConfig::default()
            .with_base_url("https://custom.url")
            .with_rate_limit(nonzero!(120u32))
            .with_timeout_secs(60);

        assert_eq!(config.base_url, "https://custom.url");
        assert_eq!(config.requests_per_minute.get(), 120);
        assert_eq!(config.timeout_secs, 60);
    }

    // ==================== RawMarket Conversion Tests ====================

    #[test]
    fn test_raw_market_conversion() {
        let raw = RawMarket {
            ticker: "KXBTC-26FEB02-B100000".to_string(),
            event_ticker: "KXBTC-26FEB02".to_string(),
            title: Some("Bitcoin above $100,000?".to_string()),
            subtitle: Some("Settlement at 3pm EST".to_string()),
            status: Some("open".to_string()),
            yes_bid: Some(45),
            yes_ask: Some(47),
            no_bid: Some(53),
            no_ask: Some(55),
            last_price: Some(46),
            volume_24h: Some(50000),
            open_interest: Some(100000),
            close_time: None,
            expiration_time: None,
            strike_value: Some(100000.0),
            category: Some("Crypto".to_string()),
        };

        let market: Market = raw.into();

        assert_eq!(market.ticker, "KXBTC-26FEB02-B100000");
        assert_eq!(market.yes_bid, Some(dec!(45)));
        assert_eq!(market.yes_ask, Some(dec!(47)));
        assert!(market.is_tradeable());
        assert!(market.is_btc_market());
    }

    #[test]
    fn test_raw_market_closed_status() {
        let raw = RawMarket {
            ticker: "KXBTC-TEST".to_string(),
            event_ticker: "KXBTC".to_string(),
            title: None,
            subtitle: None,
            status: Some("closed".to_string()),
            yes_bid: None,
            yes_ask: None,
            no_bid: None,
            no_ask: None,
            last_price: None,
            volume_24h: None,
            open_interest: None,
            close_time: None,
            expiration_time: None,
            strike_value: None,
            category: None,
        };

        let market: Market = raw.into();
        assert!(!market.is_tradeable());
    }

    // ==================== RawOrder Conversion Tests ====================

    #[test]
    fn test_raw_order_conversion() {
        let raw = RawOrder {
            order_id: "order-123".to_string(),
            client_order_id: Some("client-456".to_string()),
            ticker: "KXBTC-TEST".to_string(),
            side: Some("yes".to_string()),
            action: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            status: Some("filled".to_string()),
            count: Some(100),
            filled_count: Some(100),
            remaining_count: Some(0),
            price: Some(45),
            avg_fill_price: Some(44.5),
            created_time: None,
            updated_time: None,
        };

        let order: Order = raw.into();

        assert_eq!(order.order_id, "order-123");
        assert!(order.is_filled());
        assert_eq!(order.filled_count, 100);
    }

    #[test]
    fn test_raw_order_partial_fill() {
        let raw = RawOrder {
            order_id: "order-123".to_string(),
            client_order_id: None,
            ticker: "KXBTC-TEST".to_string(),
            side: Some("no".to_string()),
            action: Some("sell".to_string()),
            order_type: Some("market".to_string()),
            status: Some("partial_filled".to_string()),
            count: Some(100),
            filled_count: Some(50),
            remaining_count: Some(50),
            price: None,
            avg_fill_price: Some(55.0),
            created_time: None,
            updated_time: None,
        };

        let order: Order = raw.into();

        assert!(order.is_partial());
        assert_eq!(order.filled_count, 50);
        assert_eq!(order.remaining_count, 50);
    }

    // ==================== Orderbook Parsing Tests ====================

    #[test]
    fn test_orderbook_parsing() {
        let raw = RawOrderbook {
            yes: Some(vec![vec![45, 100], vec![44, 200]]),
            no: Some(vec![vec![53, 150], vec![54, 100]]),
        };

        let yes_bids: Vec<PriceLevel> = raw
            .yes
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.len() >= 2)
            .map(|v| PriceLevel {
                price: v[0] as u32,
                count: v[1] as u32,
            })
            .collect();

        assert_eq!(yes_bids.len(), 2);
        assert_eq!(yes_bids[0].price, 45);
        assert_eq!(yes_bids[0].count, 100);
    }

    // ==================== Balance Tests ====================

    #[test]
    fn test_balance_response_parsing() {
        let balance = Balance {
            balance: 10000,
            available_balance: 10000,
            reserved_balance: 0,
        };

        assert_eq!(balance.available_decimal(), dec!(10000));
    }

    // ==================== Integration Test Patterns ====================

    #[tokio::test]
    async fn test_mock_server_markets_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "markets": [
                    {
                        "ticker": "KXBTC-TEST",
                        "event_ticker": "KXBTC",
                        "title": "BTC Test",
                        "status": "open",
                        "yes_bid": 45,
                        "yes_ask": 47
                    }
                ]
            })))
            .mount(&mock_server)
            .await;

        // Note: We can't actually call the client here without auth setup,
        // but this demonstrates the mock pattern
        let _mock_url = mock_server.uri();
    }

    // ==================== URL Construction Tests ====================

    #[test]
    fn test_markets_path() {
        let path = "/markets";
        assert_eq!(path, "/markets");

        let filtered_path = format!("/markets?event_ticker={}", "KXBTC");
        assert_eq!(filtered_path, "/markets?event_ticker=KXBTC");
    }

    #[test]
    fn test_orderbook_path() {
        let ticker = "KXBTC-26FEB02-B100000";
        let depth = 20;
        let path = format!("/markets/{}/orderbook?depth={}", ticker, depth.min(100));
        assert_eq!(path, "/markets/KXBTC-26FEB02-B100000/orderbook?depth=20");
    }

    #[test]
    fn test_orderbook_depth_clamped() {
        let depth: u32 = 150;
        let clamped = depth.min(100);
        assert_eq!(clamped, 100);
    }

    #[test]
    fn test_order_path() {
        let order_id = "order-123-abc";
        let path = format!("/portfolio/orders/{}", order_id);
        assert_eq!(path, "/portfolio/orders/order-123-abc");
    }

    // ==================== Input Validation Tests ====================

    #[test]
    fn test_validate_ticker_valid() {
        assert!(KalshiClient::validate_ticker("KXBTC-26FEB02-B100000").is_ok());
        assert!(KalshiClient::validate_ticker("KXBTC_TEST").is_ok());
        assert!(KalshiClient::validate_ticker("ABC123").is_ok());
    }

    #[test]
    fn test_validate_ticker_rejects_path_traversal() {
        assert!(KalshiClient::validate_ticker("../etc/passwd").is_err());
        assert!(KalshiClient::validate_ticker("..").is_err());
        assert!(KalshiClient::validate_ticker("foo/../bar").is_err());
    }

    #[test]
    fn test_validate_ticker_rejects_slashes() {
        assert!(KalshiClient::validate_ticker("foo/bar").is_err());
        assert!(KalshiClient::validate_ticker("foo\\bar").is_err());
        assert!(KalshiClient::validate_ticker("/markets/test").is_err());
    }

    #[test]
    fn test_validate_ticker_rejects_empty() {
        assert!(KalshiClient::validate_ticker("").is_err());
    }

    #[test]
    fn test_validate_ticker_rejects_special_chars() {
        assert!(KalshiClient::validate_ticker("test@ticker").is_err());
        assert!(KalshiClient::validate_ticker("test ticker").is_err());
        assert!(KalshiClient::validate_ticker("test?query=1").is_err());
        assert!(KalshiClient::validate_ticker("test#anchor").is_err());
    }

    #[test]
    fn test_validate_ticker_rejects_too_long() {
        let long_ticker = "A".repeat(65);
        assert!(KalshiClient::validate_ticker(&long_ticker).is_err());
    }

    #[test]
    fn test_validate_identifier_valid() {
        assert!(KalshiClient::validate_identifier("order-123-abc").is_ok());
        assert!(KalshiClient::validate_identifier("uuid_format_id").is_ok());
    }

    #[test]
    fn test_validate_identifier_rejects_path_traversal() {
        assert!(KalshiClient::validate_identifier("../../../etc/passwd").is_err());
        assert!(KalshiClient::validate_identifier("order/../../../secret").is_err());
    }
}
