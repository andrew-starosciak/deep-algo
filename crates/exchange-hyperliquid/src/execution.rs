use algo_trade_core::events::{FillEvent, OrderDirection, OrderEvent};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use crate::client::HyperliquidClient;

pub struct LiveExecutionHandler {
    client: HyperliquidClient,
}

impl LiveExecutionHandler {
    /// Creates a new live execution handler
    #[must_use]
    pub const fn new(client: HyperliquidClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ExecutionHandler for LiveExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // Build Hyperliquid order request
        let order_request = json!({
            "type": "order",
            "orders": [{
                "a": 0, // Asset index (0 = first coin, will be mapped from symbol)
                "b": matches!(order.direction, OrderDirection::Buy),
                "p": order.price.unwrap_or(Decimal::ZERO).to_string(),
                "s": order.quantity.to_string(),
                "r": false, // reduceOnly
                "t": {
                    "limit": {
                        "tif": "Gtc" // Good-till-cancel
                    }
                },
            }],
            "grouping": "na",
        });

        let response = self.client.post_signed("/exchange", order_request).await?;

        // Parse Hyperliquid response
        let status = response.get("status")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing status in order response"))?;

        if status != "ok" {
            let error = response.get("response")
                .and_then(|r| r.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("Order failed: {error}");
        }

        let statuses = response.get("response")
            .and_then(|r| r.get("data"))
            .and_then(|d| d.get("statuses"))
            .and_then(|s| s.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?;

        let first_status = statuses.first()
            .ok_or_else(|| anyhow::anyhow!("No order status in response"))?;

        let filled = first_status.get("filled")
            .and_then(|f| f.as_object())
            .ok_or_else(|| anyhow::anyhow!("Order not filled"))?;

        let avg_price = filled.get("avgPx")
            .and_then(|p| p.as_str())
            .and_then(|p| Decimal::from_str_exact(p).ok())
            .unwrap_or_else(|| order.price.unwrap_or(Decimal::ZERO));

        let fill = FillEvent {
            order_id: filled.get("oid")
                .and_then(serde_json::Value::as_u64)
                .map(|o| o.to_string())
                .unwrap_or_default(),
            symbol: order.symbol,
            direction: order.direction,
            quantity: order.quantity,
            price: avg_price,
            commission: Decimal::ZERO, // Hyperliquid uses maker/taker fees, extract if needed
            timestamp: Utc::now(),
        };

        Ok(fill)
    }
}
