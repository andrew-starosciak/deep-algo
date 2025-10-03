use algo_trade_core::events::{OrderDirection, OrderEvent, OrderType, SignalDirection, SignalEvent};
use algo_trade_core::traits::RiskManager;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct SimpleRiskManager {
    _max_position_size: Decimal,
    fixed_quantity: Decimal,
}

impl SimpleRiskManager {
    /// Creates a new `SimpleRiskManager` with the specified parameters.
    ///
    /// # Panics
    ///
    /// Panics if the f64 values cannot be converted to `Decimal`.
    #[must_use]
    pub fn new(max_position_size: f64, fixed_quantity: f64) -> Self {
        Self {
            _max_position_size: Decimal::from_str(&max_position_size.to_string()).unwrap(),
            fixed_quantity: Decimal::from_str(&fixed_quantity.to_string()).unwrap(),
        }
    }
}

#[async_trait]
impl RiskManager for SimpleRiskManager {
    async fn evaluate_signal(&self, signal: &SignalEvent) -> Result<Option<OrderEvent>> {
        // Simple implementation: convert signal to fixed-size market order
        let direction = match signal.direction {
            SignalDirection::Long => OrderDirection::Buy,
            SignalDirection::Short => OrderDirection::Sell,
            SignalDirection::Exit => return Ok(None), // Handle exits separately
        };

        let order = OrderEvent {
            symbol: signal.symbol.clone(),
            order_type: OrderType::Market,
            direction,
            quantity: self.fixed_quantity,
            price: Some(signal.price),
            timestamp: Utc::now(),
        };

        Ok(Some(order))
    }
}
