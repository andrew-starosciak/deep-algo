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
        let mut ws = HyperliquidWebSocket::new(ws_url);
        ws.connect().await?;

        // Subscribe to candles for the symbol
        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "candle",
                "coin": symbol,
                "interval": interval
            }
        });
        ws.subscribe(subscription).await?;

        Ok(Self { ws, symbol, interval })
    }

    /// Fetches historical candles to warm up strategy state before live trading
    ///
    /// # Arguments
    /// * `api_url` - Hyperliquid REST API base URL
    /// * `lookback_periods` - Number of candles to fetch (e.g., 100 for 100 periods)
    ///
    /// # Errors
    /// Returns error if API request fails or data parsing fails
    pub async fn warmup(&self, api_url: String, lookback_periods: usize) -> Result<Vec<MarketEvent>> {
        let client = HyperliquidClient::new(api_url);
        let end = Utc::now();
        let interval_minutes = self.parse_interval_minutes()?;
        let total_minutes = lookback_periods * interval_minutes;
        let minutes_i64 = i64::try_from(total_minutes)
            .context("Lookback period too large")?;
        let start = end - Duration::minutes(minutes_i64);

        let records = client.fetch_candles(&self.symbol, &self.interval, start, end).await?;

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
}

#[async_trait]
impl DataProvider for LiveDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        while let Some(msg) = self.ws.next_message().await? {
            if let Some(data) = msg.get("data") {
                if let Some(candle) = data.as_object() {
                    let symbol = self.symbol.clone();
                    let open = Decimal::from_str(candle["o"].as_str().unwrap_or("0"))?;
                    let high = Decimal::from_str(candle["h"].as_str().unwrap_or("0"))?;
                    let low = Decimal::from_str(candle["l"].as_str().unwrap_or("0"))?;
                    let close = Decimal::from_str(candle["c"].as_str().unwrap_or("0"))?;
                    let volume = Decimal::from_str(candle["v"].as_str().unwrap_or("0"))?;
                    let timestamp = candle["t"].as_i64().unwrap_or(0);
                    let timestamp = chrono::DateTime::from_timestamp(timestamp / 1000, 0)
                        .unwrap_or_else(Utc::now);

                    return Ok(Some(MarketEvent::Bar {
                        symbol,
                        open,
                        high,
                        low,
                        close,
                        volume,
                        timestamp,
                    }));
                }
            }
        }
        Ok(None)
    }
}
