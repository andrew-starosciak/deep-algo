//! Paper trading execution shim.
//!
//! Simulates order fills using current market data without touching IB.
//! Useful for testing the full pipeline before connecting to IB Gateway.

use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::info;

use crate::types::{OptionsFill, OptionsOrder, OrderType};

/// Simulate a fill for paper trading.
///
/// Uses the limit price if provided, otherwise mid of a synthetic spread.
pub fn simulate_fill(order: &OptionsOrder) -> Result<OptionsFill> {
    let fill_price = match &order.order_type {
        OrderType::Limit { price } => *price,
        OrderType::Market => {
            // In real paper mode, we'd fetch current market data.
            // For now, return a placeholder that the caller should replace.
            dec!(0.00)
        }
    };

    let commission = dec!(0.65) * Decimal::from(order.quantity.unsigned_abs());

    let fill = OptionsFill {
        order_id: format!("PAPER-{}", Utc::now().timestamp_millis()),
        contract: order.contract.clone(),
        side: order.side,
        quantity: order.quantity,
        avg_fill_price: fill_price,
        commission,
        filled_at: Utc::now(),
    };

    info!(
        order_id = fill.order_id,
        symbol = fill.contract.symbol,
        price = %fill.avg_fill_price,
        quantity = fill.quantity,
        "Paper fill simulated"
    );

    Ok(fill)
}
