use crate::signing::{sign_order_request, signature_to_hex};
use crate::wallet::create_wallet_from_private_key;
use algo_trade_data::database::OhlcvRecord;
use anyhow::{Context as _, Result};
use chrono::{DateTime, Duration, Utc};
use ethers::signers::LocalWallet;
use governor::{clock::DefaultClock, state::InMemoryState, Quota, RateLimiter};
use reqwest::Client;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Maximum number of candles per API request (Hyperliquid limit)
const MAX_CANDLES_PER_REQUEST: usize = 5000;

/// Rate limit: 1200 requests per minute = 20 per second
const RATE_LIMIT_QPS: NonZeroU32 = match NonZeroU32::new(20) {
    Some(n) => n,
    None => unreachable!(),
};

pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<governor::state::direct::NotKeyed, InMemoryState, DefaultClock>>,

    // Wallet for signing (optional, for authenticated requests)
    wallet: Option<LocalWallet>,
    account_address: Option<String>,
    nonce_counter: Arc<AtomicU64>,
}

impl HyperliquidClient {
    /// Creates a new Hyperliquid HTTP client
    ///
    /// # Panics
    /// Panics if current timestamp is negative (never happens in practice)
    #[must_use]
    pub fn new(base_url: String) -> Self {
        let quota = Quota::per_second(RATE_LIMIT_QPS);
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        let timestamp = Utc::now().timestamp_millis();
        let nonce = u64::try_from(timestamp).expect("Timestamp must be positive");

        Self {
            http_client: Client::new(),
            base_url,
            rate_limiter,
            wallet: None,
            account_address: None,
            nonce_counter: Arc::new(AtomicU64::new(nonce)),
        }
    }

    /// Creates authenticated client with wallet
    ///
    /// # Errors
    /// Returns error if private key format is invalid
    pub fn with_wallet(
        base_url: String,
        api_wallet_private_key: &str,
        account_address: String,
        nonce_counter: Arc<AtomicU64>,
    ) -> Result<Self> {
        let wallet = create_wallet_from_private_key(api_wallet_private_key)?;

        let quota = Quota::per_second(RATE_LIMIT_QPS);
        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Ok(Self {
            http_client: Client::new(),
            base_url,
            rate_limiter,
            wallet: Some(wallet),
            account_address: Some(account_address),
            nonce_counter,
        })
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

    /// Sends authenticated POST request with EIP-712 signed payload
    ///
    /// # Errors
    /// Returns error if wallet not configured, signing fails, or HTTP request fails
    pub async fn post_signed(
        &self,
        endpoint: &str,
        order_payload: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let wallet = self
            .wallet
            .as_ref()
            .context("Client not authenticated - use with_wallet()")?;

        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        let signature = sign_order_request(wallet, &order_payload, nonce).await?;
        let sig_hex = signature_to_hex(&signature);

        let signed_request = serde_json::json!({
            "action": order_payload,
            "nonce": nonce,
            "signature": sig_hex,
            "vaultAddress": self.account_address.as_ref(),
        });

        self.post(endpoint, signed_request).await
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
        let total_candles = Self::calculate_total_candles(start, end, interval_millis)?;

        tracing::info!(
            "Fetching {total_candles} candles for {symbol} (interval: {interval}, {start} to {end})"
        );

        let mut all_records = HashMap::new(); // Deduplicate by timestamp

        if total_candles <= MAX_CANDLES_PER_REQUEST {
            self.fetch_single_request(symbol, interval, start, end, &mut all_records)
                .await?;
        } else {
            self.fetch_paginated_requests(
                symbol,
                interval,
                start,
                end,
                total_candles,
                interval_millis,
                &mut all_records,
            )
            .await?;
        }

        let records = Self::finalize_records(all_records, total_candles, symbol);
        Ok(records)
    }

    /// Calculates total number of candles in time range
    ///
    /// # Errors
    /// Returns error if time range is negative or calculation overflows
    fn calculate_total_candles(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        interval_millis: i64,
    ) -> Result<usize> {
        let duration_millis = end.timestamp_millis() - start.timestamp_millis();
        if duration_millis < 0 {
            anyhow::bail!("End time must be after start time");
        }

        let candles = duration_millis / interval_millis;
        usize::try_from(candles).context("Candle count exceeds platform limits")
    }

    /// Fetches candles with a single API request
    async fn fetch_single_request(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        all_records: &mut HashMap<DateTime<Utc>, OhlcvRecord>,
    ) -> Result<()> {
        let records = self
            .fetch_candles_chunk(symbol, interval, start, end)
            .await?;
        for record in records {
            all_records.insert(record.timestamp, record);
        }
        Ok(())
    }

    /// Fetches candles with multiple paginated API requests
    #[allow(clippy::too_many_arguments)]
    async fn fetch_paginated_requests(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        total_candles: usize,
        interval_millis: i64,
        all_records: &mut HashMap<DateTime<Utc>, OhlcvRecord>,
    ) -> Result<()> {
        let num_requests = total_candles.div_ceil(MAX_CANDLES_PER_REQUEST);
        tracing::info!(
            "Requires {num_requests} paginated requests (Hyperliquid limit: 5000 candles/request)"
        );

        let mut current_end = end;
        for i in 0..num_requests {
            let chunk_duration = Self::calculate_chunk_duration(interval_millis)?;
            let chunk_start = (current_end - chunk_duration).max(start);

            tracing::debug!(
                "Request {}/{num_requests}: {chunk_start} to {current_end}",
                i + 1
            );

            let records = self
                .fetch_candles_chunk(symbol, interval, chunk_start, current_end)
                .await?;
            for record in records {
                all_records.insert(record.timestamp, record);
            }

            current_end = chunk_start;
            if current_end <= start {
                break;
            }
        }
        Ok(())
    }

    /// Calculates duration for a single chunk request
    ///
    /// # Errors
    /// Returns error if calculation would overflow
    fn calculate_chunk_duration(interval_millis: i64) -> Result<Duration> {
        let chunk_millis = interval_millis
            .checked_mul(
                i64::try_from(MAX_CANDLES_PER_REQUEST)
                    .context("MAX_CANDLES_PER_REQUEST too large")?,
            )
            .context("Chunk duration calculation overflow")?;

        Ok(Duration::milliseconds(chunk_millis))
    }

    /// Finalizes records: sorts, checks for gaps, returns vector
    fn finalize_records(
        all_records: HashMap<DateTime<Utc>, OhlcvRecord>,
        total_candles: usize,
        symbol: &str,
    ) -> Vec<OhlcvRecord> {
        let mut records: Vec<OhlcvRecord> = all_records.into_values().collect();
        records.sort_by_key(|r| r.timestamp);

        tracing::info!("Fetched {} unique candles for {symbol}", records.len());

        // Warn if significantly fewer candles than expected (possible data gaps)
        if records.len() < total_candles * 9 / 10 {
            tracing::warn!(
                "Expected ~{total_candles} candles but got {}. There may be data gaps.",
                records.len()
            );
        }

        records
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
                "Unsupported interval: '{interval}'. Valid: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 8h, 12h, 1d, 3d, 1w, 1M"
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
        let candles = response
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid response is not an array"))?;

        let mut records = Vec::new();
        for candle in candles {
            let timestamp_millis = candle["t"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing timestamp in candle data"))?;
            let timestamp = DateTime::from_timestamp_millis(timestamp_millis)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp: {timestamp_millis}"))?;

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

    /// Fetches list of all available symbols from Hyperliquid
    ///
    /// Calls the `/info` endpoint with `{"type": "meta"}` to get exchange metadata
    ///
    /// # Errors
    /// Returns error if API request fails or response format is unexpected
    pub async fn fetch_available_symbols(&self) -> Result<Vec<String>> {
        let request_body = serde_json::json!({"type": "meta"});
        let response = self.post("/info", request_body).await?;

        let universe = response
            .get("universe")
            .and_then(|u| u.as_array())
            .context("Missing or invalid 'universe' field in meta response")?;

        let mut symbols = Vec::new();
        for item in universe {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                symbols.push(name.to_string());
            }
        }

        if symbols.is_empty() {
            anyhow::bail!("No symbols found in Hyperliquid meta response");
        }

        Ok(symbols)
    }
}
