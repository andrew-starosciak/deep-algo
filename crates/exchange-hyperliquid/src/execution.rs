use algo_trade_core::events::{FillEvent, OrderDirection, OrderEvent, OrderType};
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
    pub fn new(client: HyperliquidClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ExecutionHandler for LiveExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        let order_payload = json!({
            "type": match order.order_type {
                OrderType::Market => "market",
                OrderType::Limit => "limit",
            },
            "coin": order.symbol,
            "is_buy": matches!(order.direction, OrderDirection::Buy),
            "sz": order.quantity.to_string(),
            "limit_px": order.price.map(|p| p.to_string()),
        });

        let response = self.client.post("/exchange", order_payload).await?;

        // Parse response and create FillEvent
        // Note: Actual implementation needs to parse Hyperliquid response format
        let fill = FillEvent {
            order_id: response["orderId"].as_str().unwrap_or("").to_string(),
            symbol: order.symbol,
            direction: order.direction,
            quantity: order.quantity,
            price: order.price.unwrap_or(Decimal::ZERO),
            commission: Decimal::ZERO, // Extract from response
            timestamp: Utc::now(),
        };

        Ok(fill)
    }
}
