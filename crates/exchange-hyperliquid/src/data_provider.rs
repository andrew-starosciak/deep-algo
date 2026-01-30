use algo_trade_core::events::MarketEvent;
use algo_trade_core::traits::DataProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::{websocket::HyperliquidWebSocket, HyperliquidClient};

pub struct LiveDataProvider {
    ws: HyperliquidWebSocket,
    symbol: String,
    #[allow(dead_code)]
    interval: String,
}

impl LiveDataProvider {
    /// Creates a new live data provider connected to Hyperliquid WebSocket
    ///
    /// # Errors
    /// Returns error if WebSocket connection fails or subscription fails
    pub async fn new(ws_url: String, symbol: String, interval: String) -> Result<Self> {
        let mut ws = HyperliquidWebSocket::new(ws_url.clone());

        // Connect to WebSocket
        ws.connect().await.map_err(|e| {
            tracing::error!("WebSocket connection failed: {:?}", e);
            e
        })?;

        // Subscribe to candles for the symbol
        let subscription = Self::create_subscription(&symbol, &interval);

        tracing::debug!("Subscribing to candles: {}", subscription);
        ws.subscribe(subscription).await.map_err(|e| {
            tracing::error!("WebSocket subscription failed: {:?}", e);
            e
        })?;

        tracing::info!(
            "Successfully subscribed to {} candles (interval: {})",
            symbol,
            interval
        );
        Ok(Self {
            ws,
            symbol,
            interval,
        })
    }

    /// Create subscription JSON for candle data
    fn create_subscription(symbol: &str, interval: &str) -> serde_json::Value {
        serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "candle",
                "coin": symbol,
                "interval": interval
            }
        })
    }

    /// Fetches historical candles to warm up strategy state before live trading
    ///
    /// # Arguments
    /// * `api_url` - Hyperliquid REST API base URL
    /// * `lookback_periods` - Number of candles to fetch (e.g., 100 for 100 periods)
    ///
    /// # Errors
    /// Returns error if API request fails or data parsing fails
    pub async fn warmup(
        &self,
        api_url: String,
        lookback_periods: usize,
    ) -> Result<Vec<MarketEvent>> {
        let client = HyperliquidClient::new(api_url);
        let end = Utc::now();
        let interval_minutes = self.parse_interval_minutes()?;
        let total_minutes = lookback_periods * interval_minutes;
        let minutes_i64 = i64::try_from(total_minutes).context("Lookback period too large")?;
        let start = end - Duration::minutes(minutes_i64);

        let records = client
            .fetch_candles(&self.symbol, &self.interval, start, end)
            .await?;

        let events = records
            .into_iter()
            .map(|r| MarketEvent::Bar {
                symbol: r.symbol,
                open: r.open,
                high: r.high,
                low: r.low,
                close: r.close,
                volume: r.volume,
                timestamp: r.timestamp,
            })
            .collect();

        Ok(events)
    }

    /// Parse interval string (e.g., "1m", "5m", "1h") to minutes
    fn parse_interval_minutes(&self) -> Result<usize> {
        let interval = &self.interval;
        if interval.ends_with('m') {
            let minutes = interval.trim_end_matches('m').parse::<usize>()?;
            Ok(minutes)
        } else if interval.ends_with('h') {
            let hours = interval.trim_end_matches('h').parse::<usize>()?;
            Ok(hours * 60)
        } else if interval.ends_with('d') {
            let days = interval.trim_end_matches('d').parse::<usize>()?;
            Ok(days * 1440)
        } else {
            anyhow::bail!("Invalid interval format: {interval}. Expected format: 1m, 5m, 1h, 1d")
        }
    }

    /// Extract candle object from data field (handles both object and array formats)
    fn extract_candle_from_data(
        data: &serde_json::Value,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        if let Some(obj) = data.as_object() {
            Some(obj)
        } else if let Some(arr) = data.as_array() {
            arr.first()?.as_object()
        } else {
            None
        }
    }

    /// Parse candle fields into `MarketEvent`
    fn parse_candle_to_event(
        &self,
        candle: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<MarketEvent> {
        let symbol = candle
            .get("s")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.symbol)
            .to_string();

        let open = candle
            .get("o")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'o' (open) field in candle"))?;
        let high = candle
            .get("h")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'h' (high) field in candle"))?;
        let low = candle
            .get("l")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'l' (low) field in candle"))?;
        let close = candle
            .get("c")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'c' (close) field in candle"))?;
        let volume = candle
            .get("v")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'v' (volume) field in candle"))?;
        let timestamp = candle
            .get("t")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("Missing 't' (timestamp) field in candle"))?;

        let open = Decimal::from_str(open)?;
        let high = Decimal::from_str(high)?;
        let low = Decimal::from_str(low)?;
        let close = Decimal::from_str(close)?;
        let volume = Decimal::from_str(volume)?;
        let timestamp = chrono::DateTime::from_timestamp_millis(timestamp)
            .ok_or_else(|| anyhow::anyhow!("Invalid timestamp: {timestamp}"))?;

        tracing::debug!(
            "Parsed candle: {} @ {} (OHLCV: {}/{}/{}/{}/{})",
            symbol,
            timestamp,
            open,
            high,
            low,
            close,
            volume
        );

        Ok(MarketEvent::Bar {
            symbol,
            open,
            high,
            low,
            close,
            volume,
            timestamp,
        })
    }
}

#[async_trait]
impl DataProvider for LiveDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        while let Some(msg) = self.ws.next_message().await? {
            tracing::trace!("Received WebSocket message: {}", msg);

            // Filter out subscription confirmation messages
            if let Some(channel) = msg.get("channel").and_then(serde_json::Value::as_str) {
                if channel == "subscriptionResponse" {
                    tracing::debug!("Subscription confirmation received");
                    continue;
                }
            }

            // Check if this is a candle data message
            if msg.get("channel").is_some() && msg.get("data").is_some() {
                if let Some(data) = msg.get("data") {
                    if let Some(candle) = Self::extract_candle_from_data(data) {
                        return Ok(Some(self.parse_candle_to_event(candle)?));
                    }
                    tracing::warn!("Could not extract candle from data: {data}");
                }
            } else if msg.get("method") == Some(&serde_json::json!("subscribe")) {
                tracing::debug!("Subscription confirmation received: {}", msg);
            } else {
                tracing::debug!("Received non-data message: {}", msg);
            }
        }
        Ok(None)
    }
}
