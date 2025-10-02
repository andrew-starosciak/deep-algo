use anyhow::Result;
use governor::{Quota, RateLimiter, state::InMemoryState, clock::DefaultClock};
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::Arc;

pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<governor::state::direct::NotKeyed, InMemoryState, DefaultClock>>,
}

impl HyperliquidClient {
    pub fn new(base_url: String) -> Self {
        // 1200 requests per minute = 20 per second
        let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            http_client: Client::new(),
            base_url,
            rate_limiter,
        }
    }

    pub async fn get(&self, endpoint: &str) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.get(&url).send().await?;
        let json = response.json().await?;
        Ok(json)
    }

    pub async fn post(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.post(&url).json(&body).send().await?;
        let json = response.json().await?;
        Ok(json)
    }
}
