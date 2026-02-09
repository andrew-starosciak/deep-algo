//! Gamma API client for 15-minute market discovery.
//!
//! Uses the Gamma API to discover 15-minute Up/Down binary markets
//! that are not available through the CLOB API.

use crate::models::{Coin, GammaEvent, Market};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use governor::{Quota, RateLimiter};
use nonzero_ext::nonzero;
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::Arc;

/// Gamma API base URL.
pub const GAMMA_API_URL: &str = "https://gamma-api.polymarket.com";

/// Gamma API client for 15-minute market discovery.
pub struct GammaClient {
    /// HTTP client
    http: Client,
    /// Base URL for API
    base_url: String,
    /// Rate limiter (requests per minute)
    rate_limiter: Arc<
        RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
}

impl GammaClient {
    /// Creates a new client with default settings.
    ///
    /// Rate limited to 30 requests per minute by default.
    pub fn new() -> Self {
        Self::with_rate_limit(nonzero!(30u32))
    }

    /// Creates a new client with custom rate limit.
    pub fn with_rate_limit(requests_per_minute: NonZeroU32) -> Self {
        let quota = Quota::per_minute(requests_per_minute);
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            http: Client::new(),
            base_url: GAMMA_API_URL.to_string(),
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
        self.rate_limiter.until_ready().await;

        let url = format!("{}{}", self.base_url, path);
        tracing::debug!("GET {}", url);

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gamma API error {}: {}", status, text));
        }

        let body = response.json::<T>().await?;
        Ok(body)
    }

    /// Calculates the 15-minute window timestamp for a given time.
    ///
    /// Returns the start of the current 15-minute window as Unix timestamp.
    /// Formula: `timestamp = (unix_time / 900) * 900`
    #[must_use]
    pub fn calculate_window_timestamp(time: DateTime<Utc>) -> i64 {
        let unix_time = time.timestamp();
        (unix_time / 900) * 900
    }

    /// Generates the event slug for a coin and timestamp.
    ///
    /// Format: `{coin}-updown-15m-{timestamp}`
    #[must_use]
    pub fn generate_event_slug(coin: Coin, window_timestamp: i64) -> String {
        format!("{}-updown-15m-{}", coin.slug_prefix(), window_timestamp)
    }

    /// Gets the 15-minute market event for a specific coin and time.
    ///
    /// # Arguments
    /// * `coin` - The coin (btc, eth, sol, xrp)
    /// * `time` - The time to get the market for (will be aligned to 15-min window)
    ///
    /// # Returns
    /// The event containing the market(s), or an error if not found.
    pub async fn get_15min_event(&self, coin: Coin, time: DateTime<Utc>) -> Result<GammaEvent> {
        let window_timestamp = Self::calculate_window_timestamp(time);
        let slug = Self::generate_event_slug(coin, window_timestamp);
        let path = format!("/events?slug={}", slug);

        tracing::debug!(
            coin = coin.slug_prefix(),
            window_timestamp = window_timestamp,
            slug = %slug,
            "Fetching 15-min market event"
        );

        // API returns an array of events
        let events: Vec<GammaEvent> = self.get(&path).await?;

        events
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No event found for slug: {}", slug))
    }

    /// Gets the current 15-minute market for a coin.
    ///
    /// Convenience method that uses the current time.
    pub async fn get_current_15min_market(&self, coin: Coin) -> Result<Market> {
        let now = Utc::now();
        let event = self.get_15min_event(coin, now).await?;

        event
            .markets
            .into_iter()
            .next()
            .and_then(|m| m.to_market())
            .ok_or_else(|| anyhow!("No valid market in event"))
    }

    /// Gets current 15-minute markets for all supported coins.
    ///
    /// Returns a vector of markets, one for each coin that has an active market.
    /// Markets that fail to fetch are logged and skipped.
    pub async fn get_all_current_15min_markets(&self) -> Vec<Market> {
        let mut markets = Vec::new();

        for coin in Coin::all() {
            match self.get_current_15min_market(*coin).await {
                Ok(market) => {
                    tracing::debug!(
                        coin = coin.slug_prefix(),
                        condition_id = %market.condition_id,
                        up_price = ?market.up_price(),
                        down_price = ?market.down_price(),
                        "Discovered 15-min market"
                    );
                    markets.push(market);
                }
                Err(e) => {
                    tracing::warn!(
                        coin = coin.slug_prefix(),
                        error = %e,
                        "Failed to fetch 15-min market"
                    );
                }
            }
        }

        markets
    }

    /// Gets 15-minute markets for specific coins only.
    pub async fn get_15min_markets_for_coins(&self, coins: &[Coin]) -> Vec<Market> {
        // Fetch all coins concurrently to avoid sequential HTTP latency
        let futures: Vec<_> = coins
            .iter()
            .map(|coin| async move {
                match self.get_current_15min_market(*coin).await {
                    Ok(market) => Some(market),
                    Err(e) => {
                        tracing::warn!(
                            coin = coin.slug_prefix(),
                            error = %e,
                            "Failed to fetch 15-min market"
                        );
                        None
                    }
                }
            })
            .collect();

        futures_util::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Gets the outcome of a resolved 15-minute market.
    ///
    /// Returns the winning direction ("UP" or "DOWN") for the specified coin and window.
    /// Returns None if the market hasn't resolved yet.
    ///
    /// # Arguments
    /// * `coin` - The coin (btc, eth, sol, xrp)
    /// * `window_end` - The end time of the 15-minute window
    pub async fn get_market_outcome(
        &self,
        coin: Coin,
        window_end: DateTime<Utc>,
    ) -> Result<Option<String>> {
        // Calculate the window start timestamp (window_end - 15 minutes, rounded to window boundary)
        let window_timestamp =
            Self::calculate_window_timestamp(window_end - chrono::Duration::minutes(15));
        let slug = Self::generate_event_slug(coin, window_timestamp);
        let path = format!("/events?slug={}", slug);

        tracing::debug!(
            coin = coin.slug_prefix(),
            window_timestamp = window_timestamp,
            slug = %slug,
            "Fetching resolved market outcome"
        );

        let events: Vec<GammaEvent> = match self.get(&path).await {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(error = %e, "Failed to fetch event for outcome");
                return Ok(None);
            }
        };

        let event = match events.into_iter().next() {
            Some(e) => e,
            None => {
                tracing::debug!(slug = %slug, "No event found for outcome");
                return Ok(None);
            }
        };

        // Look for a market with resolved tokens or resolved outcome prices
        for gamma_market in &event.markets {
            if let Some(market) = gamma_market.to_market() {
                // Check if tokens have winner field set (works for live/recently resolved)
                for token in &market.tokens {
                    if let Some(is_winner) = token.winner {
                        if is_winner {
                            let outcome = token.outcome.to_uppercase();
                            tracing::info!(
                                coin = coin.slug_prefix(),
                                outcome = %outcome,
                                token_id = %token.token_id,
                                "Got market outcome from Gamma API (token winner)"
                            );
                            return Ok(Some(outcome));
                        }
                    }
                }
            }

            // Fallback: check outcomePrices for resolved markets where tokens is null.
            // Resolved markets have outcomePrices like ["1","0"] (Up won) or ["0","1"] (Down won).
            if let Some((up_price, down_price)) = gamma_market.parse_outcome_prices() {
                if up_price == Decimal::ONE && down_price == Decimal::ZERO {
                    tracing::info!(
                        coin = coin.slug_prefix(),
                        outcome = "UP",
                        "Got market outcome from Gamma API (outcomePrices)"
                    );
                    return Ok(Some("UP".to_string()));
                } else if up_price == Decimal::ZERO && down_price == Decimal::ONE {
                    tracing::info!(
                        coin = coin.slug_prefix(),
                        outcome = "DOWN",
                        "Got market outcome from Gamma API (outcomePrices)"
                    );
                    return Ok(Some("DOWN".to_string()));
                }
                // Prices are neither 0/1 nor 1/0 — market not yet resolved
            }
        }

        // Market exists but not yet resolved
        tracing::debug!(slug = %slug, "Market not yet resolved");
        Ok(None)
    }

    /// Gets outcomes for multiple coins at once.
    ///
    /// Returns a map of coin -> outcome ("UP" or "DOWN").
    /// Coins without resolved outcomes are not included.
    pub async fn get_market_outcomes(
        &self,
        coins: &[Coin],
        window_end: DateTime<Utc>,
    ) -> std::collections::HashMap<Coin, String> {
        let mut outcomes = std::collections::HashMap::new();

        for coin in coins {
            if let Ok(Some(outcome)) = self.get_market_outcome(*coin, window_end).await {
                outcomes.insert(*coin, outcome);
            }
        }

        outcomes
    }
}

impl Default for GammaClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_calculate_window_timestamp_aligned() {
        // 2026-01-31 12:00:00 UTC - already aligned
        let time = Utc.with_ymd_and_hms(2026, 1, 31, 12, 0, 0).unwrap();
        let ts = GammaClient::calculate_window_timestamp(time);
        assert_eq!(ts % 900, 0);
    }

    #[test]
    fn test_calculate_window_timestamp_mid_window() {
        // 2026-01-31 12:07:30 UTC - mid-window
        let time = Utc.with_ymd_and_hms(2026, 1, 31, 12, 7, 30).unwrap();
        let ts = GammaClient::calculate_window_timestamp(time);
        assert_eq!(ts % 900, 0);

        // Should round down to :00
        let expected = Utc.with_ymd_and_hms(2026, 1, 31, 12, 0, 0).unwrap();
        assert_eq!(ts, expected.timestamp());
    }

    #[test]
    fn test_calculate_window_timestamp_next_window() {
        // 2026-01-31 12:15:00 UTC - next window
        let time = Utc.with_ymd_and_hms(2026, 1, 31, 12, 15, 0).unwrap();
        let ts = GammaClient::calculate_window_timestamp(time);
        assert_eq!(ts % 900, 0);
        assert_eq!(ts, time.timestamp());
    }

    #[test]
    fn test_generate_event_slug() {
        let ts = 1769860800;

        assert_eq!(
            GammaClient::generate_event_slug(Coin::Btc, ts),
            "btc-updown-15m-1769860800"
        );
        assert_eq!(
            GammaClient::generate_event_slug(Coin::Eth, ts),
            "eth-updown-15m-1769860800"
        );
        assert_eq!(
            GammaClient::generate_event_slug(Coin::Sol, ts),
            "sol-updown-15m-1769860800"
        );
        assert_eq!(
            GammaClient::generate_event_slug(Coin::Xrp, ts),
            "xrp-updown-15m-1769860800"
        );
    }

    #[test]
    fn test_client_creation() {
        let client = GammaClient::new();
        assert_eq!(client.base_url(), GAMMA_API_URL);
    }

    #[test]
    fn test_client_with_base_url() {
        let client = GammaClient::new().with_base_url("http://localhost:8080");
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_get_15min_event_success() {
        let mock_server = MockServer::start().await;

        // Calculate the expected timestamp for 2026-01-31 12:05:00
        let time = Utc.with_ymd_and_hms(2026, 1, 31, 12, 5, 0).unwrap();
        let expected_ts = GammaClient::calculate_window_timestamp(time);
        let expected_slug = format!("btc-updown-15m-{}", expected_ts);

        Mock::given(method("GET"))
            .and(path("/events"))
            .and(query_param("slug", &expected_slug))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": expected_slug,
                    "title": "BTC 15-min Up/Down",
                    "markets": [{
                        "conditionId": "0xabc123",
                        "outcomePrices": "[\"0.53\", \"0.47\"]",
                        "clobTokenIds": "[\"token1\", \"token2\"]",
                        "liquidity": "10000.00",
                        "endDate": "2026-01-31T12:15:00Z",
                        "question": "Will BTC go up?",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let event = client.get_15min_event(Coin::Btc, time).await.unwrap();

        assert_eq!(event.slug, expected_slug);
        assert_eq!(event.markets.len(), 1);
        assert_eq!(event.markets[0].condition_id, "0xabc123");
    }

    #[tokio::test]
    async fn test_get_15min_event_not_found() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let result = client.get_15min_event(Coin::Btc, Utc::now()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No event found"));
    }

    #[tokio::test]
    async fn test_get_current_15min_market() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": "btc-updown-15m-test",
                    "title": "BTC 15-min",
                    "markets": [{
                        "conditionId": "0xmarket1",
                        "outcomePrices": "[\"0.60\", \"0.40\"]",
                        "clobTokenIds": "[\"up-id\", \"down-id\"]",
                        "liquidity": "15000.00",
                        "question": "BTC Up or Down",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let market = client.get_current_15min_market(Coin::Btc).await.unwrap();

        assert_eq!(market.condition_id, "0xmarket1");
        assert!(market.is_15min_market());
        assert_eq!(market.up_price(), Some(dec!(0.60)));
        assert_eq!(market.down_price(), Some(dec!(0.40)));
        assert_eq!(market.liquidity, Some(dec!(15000.00)));
    }

    #[tokio::test]
    async fn test_get_all_current_15min_markets() {
        let mock_server = MockServer::start().await;

        // This will match all requests to /events
        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": "test-market",
                    "markets": [{
                        "conditionId": "0xtest",
                        "outcomePrices": "[\"0.50\", \"0.50\"]",
                        "clobTokenIds": "[\"up\", \"down\"]",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let markets = client.get_all_current_15min_markets().await;

        // Should get 4 markets (one per coin)
        assert_eq!(markets.len(), 4);
        for market in &markets {
            assert!(market.is_15min_market());
        }
    }

    #[tokio::test]
    async fn test_get_15min_markets_for_coins() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "slug": "test",
                    "markets": [{
                        "conditionId": "0xtest",
                        "outcomePrices": "[\"0.55\", \"0.45\"]",
                        "clobTokenIds": "[\"up\", \"down\"]",
                        "active": true
                    }]
                }
            ])))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let markets = client
            .get_15min_markets_for_coins(&[Coin::Btc, Coin::Eth])
            .await;

        // Should get 2 markets (BTC and ETH only)
        assert_eq!(markets.len(), 2);
    }

    #[tokio::test]
    async fn test_api_error_handling() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/events"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let client = GammaClient::new().with_base_url(mock_server.uri());

        let result = client.get_15min_event(Coin::Btc, Utc::now()).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("500") || err.contains("Internal Server Error"));
    }
}
