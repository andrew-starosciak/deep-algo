//! Polymarket CLOB REST client with rate limiting.
//!
//! Provides typed access to Polymarket API endpoints with automatic
//! rate limiting using the governor crate.

use crate::models::{Market, MarketFilter, MarketsResponse, Price, RawMarket};
use anyhow::{anyhow, Result};
use governor::{Quota, RateLimiter};
use nonzero_ext::nonzero;
use reqwest::Client;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

/// Default Polymarket CLOB API base URL.
pub const POLYMARKET_CLOB_URL: &str = "https://clob.polymarket.com";

/// Polymarket CLOB REST client.
pub struct PolymarketClient {
    /// HTTP client
    http: Client,
    /// Base URL for API
    base_url: String,
    /// Rate limiter (requests per minute)
    rate_limiter: Arc<RateLimiter<governor::state::NotKeyed, governor::state::InMemoryState, governor::clock::DefaultClock>>,
}

impl PolymarketClient {
    /// Creates a new client with default settings.
    ///
    /// Rate limited to 60 requests per minute by default.
    pub fn new() -> Self {
        Self::with_rate_limit(nonzero!(60u32))
    }

    /// Creates a new client with custom rate limit.
    pub fn with_rate_limit(requests_per_minute: NonZeroU32) -> Self {
        let quota = Quota::per_minute(requests_per_minute);
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            http: Client::new(),
            base_url: POLYMARKET_CLOB_URL.to_string(),
            rate_limiter,
        }
    }

    /// Sets a custom base URL (useful for testing).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Returns the base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Waits for rate limit and makes a GET request.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        // Wait for rate limiter
        self.rate_limiter.until_ready().await;

        let url = format!("{}{}", self.base_url, path);
        tracing::debug!("GET {}", url);

        let response = self.http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("API error {}: {}", status, text));
        }

        let body = response.json::<T>().await?;
        Ok(body)
    }

    /// Gets a list of markets, optionally filtered.
    ///
    /// # Arguments
    /// * `filter` - Optional filter criteria
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    /// A tuple of (markets, next_cursor)
    pub async fn get_markets(
        &self,
        filter: Option<&MarketFilter>,
        cursor: Option<&str>,
    ) -> Result<(Vec<Market>, Option<String>)> {
        let mut path = "/markets".to_string();
        let mut params = Vec::new();

        if let Some(f) = filter {
            if f.active_only {
                params.push("active=true".to_string());
            }
            if let Some(ref query) = f.query {
                params.push(format!("tag_slug={}", urlencoding::encode(query)));
            }
        }

        if let Some(c) = cursor {
            params.push(format!("next_cursor={}", c));
        }

        if !params.is_empty() {
            path = format!("{}?{}", path, params.join("&"));
        }

        let response: MarketsResponse = self.get(&path).await?;

        let markets: Vec<Market> = response
            .data
            .unwrap_or_default()
            .into_iter()
            .map(Market::from)
            .collect();

        // Apply additional client-side filtering
        let markets = if let Some(f) = filter {
            markets
                .into_iter()
                .filter(|m| {
                    if let Some(min_liq) = f.min_liquidity {
                        m.has_sufficient_liquidity(min_liq)
                    } else {
                        true
                    }
                })
                .collect()
        } else {
            markets
        };

        Ok((markets, response.next_cursor))
    }

    /// Gets a specific market by condition ID.
    pub async fn get_market(&self, condition_id: &str) -> Result<Market> {
        let path = format!("/markets/{}", condition_id);
        let raw: RawMarket = self.get(&path).await?;
        Ok(Market::from(raw))
    }

    /// Gets current prices for a list of token IDs.
    pub async fn get_prices(&self, token_ids: &[String]) -> Result<HashMap<String, Price>> {
        if token_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = token_ids.join(",");
        let path = format!("/prices?token_ids={}", ids);

        // The API might return an array or object depending on endpoint version
        // We handle both cases
        let response: serde_json::Value = self.get(&path).await?;

        let mut prices = HashMap::new();

        if let Some(arr) = response.as_array() {
            for item in arr {
                if let Ok(price) = serde_json::from_value::<Price>(item.clone()) {
                    prices.insert(price.token_id.clone(), price);
                }
            }
        } else if let Some(obj) = response.as_object() {
            for (token_id, value) in obj {
                if let Ok(price) = serde_json::from_value::<PriceValue>(value.clone()) {
                    prices.insert(token_id.clone(), Price {
                        token_id: token_id.clone(),
                        bid: price.bid.and_then(|v| Decimal::try_from(v).ok()),
                        ask: price.ask.and_then(|v| Decimal::try_from(v).ok()),
                        last: price.last.and_then(|v| Decimal::try_from(v).ok()),
                        spread: price.spread.and_then(|v| Decimal::try_from(v).ok()),
                    });
                }
            }
        }

        Ok(prices)
    }

    /// Discovers BTC-related markets.
    ///
    /// Fetches markets and filters for those related to Bitcoin.
    pub async fn discover_btc_markets(&self) -> Result<Vec<Market>> {
        let filter = MarketFilter::btc_markets();
        let (markets, _) = self.get_markets(Some(&filter), None).await?;

        // Additional client-side filtering to ensure BTC relevance
        let btc_markets: Vec<Market> = markets
            .into_iter()
            .filter(|m| m.is_btc_related())
            .collect();

        Ok(btc_markets)
    }

    /// Discovers active, tradeable BTC markets with sufficient liquidity.
    pub async fn discover_tradeable_btc_markets(
        &self,
        min_liquidity: Decimal,
    ) -> Result<Vec<Market>> {
        let btc_markets = self.discover_btc_markets().await?;

        let tradeable: Vec<Market> = btc_markets
            .into_iter()
            .filter(|m| m.is_tradeable() && m.has_sufficient_liquidity(min_liquidity))
            .collect();

        Ok(tradeable)
    }
}

impl Default for PolymarketClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal price value for parsing API responses.
#[derive(Debug, serde::Deserialize)]
struct PriceValue {
    bid: Option<f64>,
    ask: Option<f64>,
    last: Option<f64>,
    spread: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_client_creation() {
        let client = PolymarketClient::new();
        assert_eq!(client.base_url(), POLYMARKET_CLOB_URL);
    }

    #[test]
    fn test_client_with_custom_rate_limit() {
        let client = PolymarketClient::with_rate_limit(nonzero!(120u32));
        assert_eq!(client.base_url(), POLYMARKET_CLOB_URL);
    }

    #[test]
    fn test_client_with_base_url() {
        let client = PolymarketClient::new()
            .with_base_url("http://localhost:8080");
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_get_markets_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "condition_id": "0x123",
                        "question": "Will Bitcoin hit $100k?",
                        "description": "BTC price prediction",
                        "end_date_iso": null,
                        "tokens": [
                            {"token_id": "yes-1", "outcome": "Yes", "price": 0.65, "winner": null},
                            {"token_id": "no-1", "outcome": "No", "price": 0.35, "winner": null}
                        ],
                        "active": true,
                        "tags": ["crypto"],
                        "volume_num_24hr": 50000.0,
                        "liquidity_num": 100000.0
                    }
                ],
                "next_cursor": null
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let (markets, cursor) = client.get_markets(None, None).await.unwrap();

        assert_eq!(markets.len(), 1);
        assert!(cursor.is_none());
        assert_eq!(markets[0].condition_id, "0x123");
        assert!(markets[0].is_btc_related());
    }

    #[tokio::test]
    async fn test_get_markets_with_filter() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .and(query_param("active", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "next_cursor": null
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let filter = MarketFilter {
            active_only: true,
            ..Default::default()
        };
        let (markets, _) = client.get_markets(Some(&filter), None).await.unwrap();

        assert!(markets.is_empty());
    }

    #[tokio::test]
    async fn test_get_markets_with_pagination() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "next_cursor": "page2cursor"
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let (_, cursor) = client.get_markets(None, None).await.unwrap();

        assert_eq!(cursor, Some("page2cursor".to_string()));
    }

    #[tokio::test]
    async fn test_get_market_by_id() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets/0x123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "condition_id": "0x123",
                "question": "Will BTC hit $100k?",
                "description": null,
                "end_date_iso": null,
                "tokens": [
                    {"token_id": "yes-1", "outcome": "Yes", "price": 0.70, "winner": null}
                ],
                "active": true,
                "tags": null
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let market = client.get_market("0x123").await.unwrap();

        assert_eq!(market.condition_id, "0x123");
        assert_eq!(market.yes_price(), Some(dec!(0.70)));
    }

    #[tokio::test]
    async fn test_get_prices() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/prices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "yes-token": {"bid": 0.64, "ask": 0.66, "last": 0.65, "spread": 0.02}
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let prices = client.get_prices(&["yes-token".to_string()]).await.unwrap();

        assert!(prices.contains_key("yes-token"));
        let price = &prices["yes-token"];
        assert_eq!(price.bid, Some(dec!(0.64)));
        assert_eq!(price.ask, Some(dec!(0.66)));
    }

    #[tokio::test]
    async fn test_get_prices_empty() {
        let client = PolymarketClient::new();
        let prices = client.get_prices(&[]).await.unwrap();
        assert!(prices.is_empty());
    }

    #[tokio::test]
    async fn test_discover_btc_markets() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "condition_id": "btc-1",
                        "question": "Will Bitcoin hit $100k?",
                        "tokens": [
                            {"token_id": "y1", "outcome": "Yes", "price": 0.65, "winner": null}
                        ],
                        "active": true
                    },
                    {
                        "condition_id": "eth-1",
                        "question": "Will Ethereum hit $10k?",
                        "tokens": [],
                        "active": true
                    },
                    {
                        "condition_id": "btc-2",
                        "question": "Will BTC crash?",
                        "tokens": [],
                        "active": true
                    }
                ],
                "next_cursor": null
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let btc_markets = client.discover_btc_markets().await.unwrap();

        // Should only include BTC-related markets
        assert_eq!(btc_markets.len(), 2);
        assert!(btc_markets.iter().all(|m| m.is_btc_related()));
    }

    #[tokio::test]
    async fn test_discover_tradeable_btc_markets() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "condition_id": "btc-1",
                        "question": "Will Bitcoin hit $100k?",
                        "tokens": [
                            {"token_id": "y1", "outcome": "Yes", "price": 0.65, "winner": null},
                            {"token_id": "n1", "outcome": "No", "price": 0.35, "winner": null}
                        ],
                        "active": true,
                        "liquidity_num": 100000.0
                    },
                    {
                        "condition_id": "btc-2",
                        "question": "Will Bitcoin crash?",
                        "tokens": [
                            {"token_id": "y2", "outcome": "Yes", "price": 0.30, "winner": null},
                            {"token_id": "n2", "outcome": "No", "price": 0.70, "winner": null}
                        ],
                        "active": true,
                        "liquidity_num": 1000.0
                    }
                ],
                "next_cursor": null
            })))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let markets = client.discover_tradeable_btc_markets(dec!(50000)).await.unwrap();

        // Only btc-1 has sufficient liquidity
        assert_eq!(markets.len(), 1);
        assert_eq!(markets[0].condition_id, "btc-1");
    }

    #[tokio::test]
    async fn test_api_error_handling() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::new().with_base_url(mock_server.uri());
        let result = client.get_markets(None, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("500") || err.contains("Internal Server Error"));
    }

    #[tokio::test]
    async fn test_rate_limiting_behavior() {
        // This test verifies rate limiting doesn't break basic functionality
        // The actual rate limiting is handled by the governor crate
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/markets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "next_cursor": null
            })))
            .expect(3)  // Expect exactly 3 calls
            .mount(&mock_server)
            .await;

        let client = PolymarketClient::with_rate_limit(nonzero!(1000u32))
            .with_base_url(mock_server.uri());

        // Make 3 rapid requests - should all succeed with high rate limit
        for _ in 0..3 {
            let result = client.get_markets(None, None).await;
            assert!(result.is_ok());
        }
    }
}
