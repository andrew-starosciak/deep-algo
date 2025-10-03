use algo_trade_data::database::OhlcvRecord;
use anyhow::{Context as _, Result};
use chrono::{DateTime, Duration, Utc};
use governor::{Quota, RateLimiter, state::InMemoryState, clock::DefaultClock};
use reqwest::Client;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::Arc;

pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<governor::state::direct::NotKeyed, InMemoryState, DefaultClock>>,
}

impl HyperliquidClient {
    /// Creates a new Hyperliquid HTTP client
    ///
    /// # Panics
    /// Panics if rate limiter quota cannot be created
    #[must_use]
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

    /// Sends a GET request to the specified endpoint
    ///
    /// # Errors
    /// Returns error if HTTP request fails or response cannot be parsed as JSON
    pub async fn get(&self, endpoint: &str) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.get(&url).send().await?;
        let json = response.json().await?;
        Ok(json)
    }

    /// Sends a POST request to the specified endpoint with JSON body
    ///
    /// # Errors
    /// Returns error if HTTP request fails or response cannot be parsed as JSON
    pub async fn post(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        self.rate_limiter.until_ready().await;
        let url = format!("{}{}", self.base_url, endpoint);
        let response = self.http_client.post(&url).json(&body).send().await?;
        let json = response.json().await?;
        Ok(json)
    }

    /// Fetches OHLCV candles from Hyperliquid API with automatic pagination
    ///
    /// Handles Hyperliquid's 5000 candle limit by splitting large requests
    /// into multiple API calls and deduplicating results.
    ///
    /// # Arguments
    /// * `symbol` - Trading symbol (e.g., "BTC" for perpetuals)
    /// * `interval` - Candle interval (e.g., "1h", "1d")
    /// * `start` - Start time (inclusive)
    /// * `end` - End time (inclusive)
    ///
    /// # Errors
    /// Returns error if API request fails or response parsing fails
    pub async fn fetch_candles(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let interval_millis = Self::interval_to_millis(interval)?;
        let total_candles = ((end.timestamp_millis() - start.timestamp_millis()) / interval_millis) as usize;

        tracing::info!(
            "Fetching {} candles for {} (interval: {}, {} to {})",
            total_candles, symbol, interval, start, end
        );

        const MAX_CANDLES_PER_REQUEST: usize = 5000;
        let mut all_records = HashMap::new();  // Deduplicate by timestamp

        if total_candles <= MAX_CANDLES_PER_REQUEST {
            // Single request
            let records = self.fetch_candles_chunk(symbol, interval, start, end).await?;
            for record in records {
                all_records.insert(record.timestamp, record);
            }
        } else {
            // Multiple requests (pagination backward from end)
            let num_requests = total_candles.div_ceil(MAX_CANDLES_PER_REQUEST);
            tracing::info!("Requires {} paginated requests (Hyperliquid limit: 5000 candles/request)", num_requests);

            let mut current_end = end;
            for i in 0..num_requests {
                let chunk_duration = Duration::milliseconds(interval_millis * MAX_CANDLES_PER_REQUEST as i64);
                let chunk_start = current_end - chunk_duration;
                let chunk_start = chunk_start.max(start);  // Don't go before requested start

                tracing::debug!("Request {}/{}: {} to {}", i + 1, num_requests, chunk_start, current_end);

                let records = self.fetch_candles_chunk(symbol, interval, chunk_start, current_end).await?;
                for record in records {
                    all_records.insert(record.timestamp, record);
                }

                current_end = chunk_start;
                if current_end <= start {
                    break;
                }
            }
        }

        // Convert to sorted vector
        let mut records: Vec<OhlcvRecord> = all_records.into_values().collect();
        records.sort_by_key(|r| r.timestamp);

        tracing::info!("Fetched {} unique candles for {}", records.len(), symbol);

        // Warn if significantly fewer candles than expected (possible data gaps)
        if records.len() < total_candles * 9 / 10 {
            tracing::warn!(
                "Expected ~{} candles but got {}. There may be data gaps.",
                total_candles, records.len()
            );
        }

        Ok(records)
    }

    /// Converts interval string to milliseconds
    ///
    /// # Errors
    /// Returns error if interval is not supported
    fn interval_to_millis(interval: &str) -> Result<i64> {
        Ok(match interval {
            "1m" => 60 * 1000,
            "3m" => 3 * 60 * 1000,
            "5m" => 5 * 60 * 1000,
            "15m" => 15 * 60 * 1000,
            "30m" => 30 * 60 * 1000,
            "1h" => 60 * 60 * 1000,
            "2h" => 2 * 60 * 60 * 1000,
            "4h" => 4 * 60 * 60 * 1000,
            "8h" => 8 * 60 * 60 * 1000,
            "12h" => 12 * 60 * 60 * 1000,
            "1d" => 24 * 60 * 60 * 1000,
            "3d" => 3 * 24 * 60 * 60 * 1000,
            "1w" => 7 * 24 * 60 * 60 * 1000,
            "1M" => 30 * 24 * 60 * 60 * 1000,  // Approximate
            _ => anyhow::bail!(
                "Unsupported interval: '{}'. Valid: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 8h, 12h, 1d, 3d, 1w, 1M",
                interval
            ),
        })
    }

    /// Fetches single chunk of candles (up to 5000)
    async fn fetch_candles_chunk(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<OhlcvRecord>> {
        let request_body = serde_json::json!({
            "type": "candleSnapshot",
            "req": {
                "coin": symbol,
                "interval": interval,
                "startTime": start.timestamp_millis(),
                "endTime": end.timestamp_millis(),
            }
        });

        let response = self.post("/info", request_body).await?;

        // Parse response array
        let candles = response.as_array()
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid response is not an array"))?;

        let mut records = Vec::new();
        for candle in candles {
            let timestamp_millis = candle["t"].as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing timestamp in candle data"))?;
            let timestamp = DateTime::from_timestamp_millis(timestamp_millis)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp: {}", timestamp_millis))?;

            let record = OhlcvRecord {
                timestamp,
                symbol: candle["s"].as_str().unwrap_or(symbol).to_string(),
                exchange: "hyperliquid".to_string(),
                open: Decimal::from_str(candle["o"].as_str().unwrap_or("0"))
                    .context("Failed to parse open price")?,
                high: Decimal::from_str(candle["h"].as_str().unwrap_or("0"))
                    .context("Failed to parse high price")?,
                low: Decimal::from_str(candle["l"].as_str().unwrap_or("0"))
                    .context("Failed to parse low price")?,
                close: Decimal::from_str(candle["c"].as_str().unwrap_or("0"))
                    .context("Failed to parse close price")?,
                volume: Decimal::from_str(candle["v"].as_str().unwrap_or("0"))
                    .context("Failed to parse volume")?,
            };
            records.push(record);
        }

        Ok(records)
    }
}
