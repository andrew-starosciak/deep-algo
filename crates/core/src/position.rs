use crate::events::{FillEvent, OrderDirection};
use rust_decimal::Decimal;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Position {
    pub symbol: String,
    pub quantity: Decimal,
    pub avg_price: Decimal,
}

impl Position {
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    fn new(symbol: String, quantity: Decimal, avg_price: Decimal) -> Self {
        Self {
            symbol,
            quantity,
            avg_price,
        }
    }
}

pub struct PositionTracker {
    positions: HashMap<String, Position>,
}

impl PositionTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
        }
    }

    /// Processes a fill event and calculates realized `PnL` if position closed.
    ///
    /// Returns `Some(pnl)` when a position is fully or partially closed, `None` when opening.
    #[must_use]
    pub fn process_fill(&mut self, fill: &FillEvent) -> Option<Decimal> {
        let position = self.positions.get_mut(&fill.symbol);

        match (&fill.direction, position) {
            // Opening new long position
            (OrderDirection::Buy, None) => {
                self.positions.insert(
                    fill.symbol.clone(),
                    Position::new(fill.symbol.clone(), fill.quantity, fill.price),
                );
                None
            }

            // Adding to existing long position
            (OrderDirection::Buy, Some(pos)) if pos.quantity > Decimal::ZERO => {
                let total_cost = pos.avg_price * pos.quantity + fill.price * fill.quantity;
                pos.quantity += fill.quantity;
                pos.avg_price = total_cost / pos.quantity;
                None
            }

            // Closing long position (sell)
            (OrderDirection::Sell, Some(pos)) if pos.quantity > Decimal::ZERO => {
                let close_quantity = fill.quantity.min(pos.quantity);
                let pnl = (fill.price - pos.avg_price) * close_quantity - fill.commission;

                pos.quantity -= close_quantity;
                if pos.quantity == Decimal::ZERO {
                    self.positions.remove(&fill.symbol);
                }

                Some(pnl)
            }

            // Opening new short position
            (OrderDirection::Sell, None) => {
                self.positions.insert(
                    fill.symbol.clone(),
                    Position::new(fill.symbol.clone(), -fill.quantity, fill.price),
                );
                None
            }

            // Adding to existing short position
            (OrderDirection::Sell, Some(pos)) if pos.quantity < Decimal::ZERO => {
                let total_cost = pos.avg_price * pos.quantity.abs() + fill.price * fill.quantity;
                pos.quantity -= fill.quantity;
                pos.avg_price = total_cost / pos.quantity.abs();
                None
            }

            // Closing short position (buy)
            (OrderDirection::Buy, Some(pos)) if pos.quantity < Decimal::ZERO => {
                let close_quantity = fill.quantity.min(pos.quantity.abs());
                let pnl = (pos.avg_price - fill.price) * close_quantity - fill.commission;

                pos.quantity += close_quantity;
                if pos.quantity == Decimal::ZERO {
                    self.positions.remove(&fill.symbol);
                }

                Some(pnl)
            }

            // Mismatch cases (shouldn't happen with proper risk management)
            _ => None,
        }
    }

    #[must_use]
    pub fn get_position(&self, symbol: &str) -> Option<&Position> {
        self.positions.get(symbol)
    }

    #[must_use]
    pub const fn all_positions(&self) -> &HashMap<String, Position> {
        &self.positions
    }
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}
