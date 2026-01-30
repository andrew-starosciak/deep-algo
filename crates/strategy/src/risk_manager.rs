use algo_trade_core::events::{
    OrderDirection, OrderEvent, OrderType, SignalDirection, SignalEvent,
};
use algo_trade_core::position_sizing::calculate_position_size;
use algo_trade_core::traits::RiskManager;
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct SimpleRiskManager {
    risk_per_trade_pct: Decimal, // Percentage of equity to risk per trade (e.g., 0.05 = 5%)
    max_position_pct: Decimal,   // Maximum position size as % of equity (e.g., 0.20 = 20%)
    leverage: u8,                // Leverage multiplier (1-50 for Hyperliquid)
}

impl SimpleRiskManager {
    /// Creates a new `SimpleRiskManager` with leverage-aware position sizing.
    ///
    /// # Parameters
    /// - `risk_per_trade_pct`: Percentage of account equity to allocate per trade (0.0-1.0)
    /// - `max_position_pct`: Maximum position size as percentage of equity (0.0-1.0)
    /// - `leverage`: Leverage multiplier (1-50 for Hyperliquid)
    ///
    /// # Examples
    /// ```
    /// use algo_trade_strategy::SimpleRiskManager;
    /// // Risk 5% of equity per trade, max 20% position, 5x leverage
    /// let risk_manager = SimpleRiskManager::new(0.05, 0.20, 5);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the f64 values cannot be converted to `Decimal`.
    #[must_use]
    pub fn new(risk_per_trade_pct: f64, max_position_pct: f64, leverage: u8) -> Self {
        Self {
            risk_per_trade_pct: Decimal::from_str(&risk_per_trade_pct.to_string()).unwrap(),
            max_position_pct: Decimal::from_str(&max_position_pct.to_string()).unwrap(),
            leverage,
        }
    }
}

#[async_trait]
impl RiskManager for SimpleRiskManager {
    async fn evaluate_signal(
        &self,
        signal: &SignalEvent,
        account_equity: Decimal,
        current_position: Option<Decimal>,
    ) -> Result<Vec<OrderEvent>> {
        // Handle Exit signals
        if signal.direction == SignalDirection::Exit {
            return current_position.map_or_else(
                || Ok(vec![]), // No position to close
                |pos| {
                    if pos == Decimal::ZERO {
                        return Ok(vec![]); // Already flat
                    }

                    let direction = if pos > Decimal::ZERO {
                        OrderDirection::Sell // Close long
                    } else {
                        OrderDirection::Buy // Close short
                    };

                    let close_order = OrderEvent {
                        symbol: signal.symbol.clone(),
                        order_type: OrderType::Market,
                        direction,
                        quantity: pos.abs(),
                        price: Some(signal.price),
                        timestamp: signal.timestamp,
                    };
                    Ok(vec![close_order])
                },
            );
        }

        // Convert signal direction to order direction
        let entry_direction = match signal.direction {
            SignalDirection::Long => OrderDirection::Buy,
            SignalDirection::Short => OrderDirection::Sell,
            SignalDirection::Exit => unreachable!(), // Handled above
        };

        // Calculate position size with leverage using core module
        let target_quantity = calculate_position_size(
            account_equity,
            self.leverage,
            self.risk_per_trade_pct.try_into().unwrap_or(0.05),
            self.max_position_pct.try_into().unwrap_or(0.20),
            signal.price,
        )?;

        // Round to 8 decimal places (standard for crypto)
        let rounded_qty =
            (target_quantity * Decimal::from(100_000_000)).round() / Decimal::from(100_000_000);

        // Step 5: Check if we need to flip positions
        let needs_flip = match (&signal.direction, current_position) {
            (SignalDirection::Long, Some(pos)) if pos < Decimal::ZERO => true, // Short → Long
            (SignalDirection::Short, Some(pos)) if pos > Decimal::ZERO => true, // Long → Short
            _ => false,
        };

        if needs_flip {
            let pos = current_position.unwrap();

            // Order 1: Close existing position
            let close_direction = if pos > Decimal::ZERO {
                OrderDirection::Sell
            } else {
                OrderDirection::Buy
            };

            let close_order = OrderEvent {
                symbol: signal.symbol.clone(),
                order_type: OrderType::Market,
                direction: close_direction,
                quantity: pos.abs(),
                price: Some(signal.price),
                timestamp: signal.timestamp,
            };

            // Order 2: Open new position
            let entry_order = OrderEvent {
                symbol: signal.symbol.clone(),
                order_type: OrderType::Market,
                direction: entry_direction,
                quantity: rounded_qty,
                price: Some(signal.price),
                timestamp: signal.timestamp,
            };

            Ok(vec![close_order, entry_order])
        } else {
            // Normal entry or adding to position
            let entry_order = OrderEvent {
                symbol: signal.symbol.clone(),
                order_type: OrderType::Market,
                direction: entry_direction,
                quantity: rounded_qty,
                price: Some(signal.price),
                timestamp: signal.timestamp,
            };

            Ok(vec![entry_order])
        }
    }
}
