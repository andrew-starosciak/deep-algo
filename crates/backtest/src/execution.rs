use algo_trade_core::events::{FillEvent, OrderDirection, OrderEvent, OrderType};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct SimulatedExecutionHandler {
    commission_rate: Decimal,
    slippage_bps: Decimal,
}

impl SimulatedExecutionHandler {
    /// Creates a new `SimulatedExecutionHandler`.
    ///
    /// # Panics
    ///
    /// Panics if `commission_rate` or `slippage_bps` cannot be converted to `Decimal`.
    /// This should not happen with normal f64 values.
    #[must_use]
    pub fn new(commission_rate: f64, slippage_bps: f64) -> Self {
        Self {
            commission_rate: Decimal::from_str(&commission_rate.to_string()).unwrap(),
            slippage_bps: Decimal::from_str(&slippage_bps.to_string()).unwrap(),
        }
    }

    fn apply_slippage(&self, price: Decimal, direction: &OrderDirection) -> Decimal {
        let slippage = price * self.slippage_bps / Decimal::from(10000);
        match direction {
            OrderDirection::Buy => price + slippage,
            OrderDirection::Sell => price - slippage,
        }
    }
}

#[async_trait]
impl ExecutionHandler for SimulatedExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // For market orders, use provided price (assumed to be current market price)
        // For limit orders, assume immediate fill at limit price (simplified)
        let fill_price = match order.order_type {
            OrderType::Market => {
                let base_price = order.price.unwrap_or(Decimal::ZERO);
                self.apply_slippage(base_price, &order.direction)
            }
            OrderType::Limit => order.price.unwrap_or(Decimal::ZERO),
        };

        let commission = fill_price * order.quantity * self.commission_rate;

        let fill = FillEvent {
            order_id: uuid::Uuid::new_v4().to_string(),
            symbol: order.symbol,
            direction: order.direction,
            quantity: order.quantity,
            price: fill_price,
            commission,
            timestamp: order.timestamp,
        };

        Ok(fill)
    }
}
