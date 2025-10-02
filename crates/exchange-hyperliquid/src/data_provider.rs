use algo_trade_core::events::MarketEvent;
use algo_trade_core::traits::DataProvider;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::websocket::HyperliquidWebSocket;

pub struct LiveDataProvider {
    ws: HyperliquidWebSocket,
}

impl LiveDataProvider {
    /// Creates a new live data provider connected to Hyperliquid WebSocket
    ///
    /// # Errors
    /// Returns error if WebSocket connection fails or subscription fails
    pub async fn new(ws_url: String, symbols: Vec<String>) -> Result<Self> {
        let mut ws = HyperliquidWebSocket::new(ws_url);
        ws.connect().await?;

        // Subscribe to trades for all symbols
        for symbol in symbols {
            let subscription = serde_json::json!({
                "method": "subscribe",
                "subscription": {
                    "type": "trades",
                    "coin": symbol
                }
            });
            ws.subscribe(subscription).await?;
        }

        Ok(Self { ws })
    }
}

#[async_trait]
impl DataProvider for LiveDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        while let Some(msg) = self.ws.next_message().await? {
            if let Some(data) = msg.get("data") {
                if let Some(trades) = data.as_array() {
                    if let Some(trade) = trades.first() {
                        let symbol = trade["coin"].as_str().unwrap_or("").to_string();
                        let price = Decimal::from_str(trade["px"].as_str().unwrap_or("0"))?;
                        let size = Decimal::from_str(trade["sz"].as_str().unwrap_or("0"))?;
                        let timestamp = Utc::now();

                        return Ok(Some(MarketEvent::Trade {
                            symbol,
                            price,
                            size,
                            timestamp,
                        }));
                    }
                }
            }
        }
        Ok(None)
    }
}
