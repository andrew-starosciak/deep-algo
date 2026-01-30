//! Signal generators for statistical trading.
//!
//! This module contains implementations of various signal generators
//! that produce trading signals based on market data analysis.

mod composite;
mod funding_rate;
mod liquidation_cascade;
mod orderbook_imbalance;

pub use composite::CompositeSignal;
pub use funding_rate::FundingRateSignal;
pub use liquidation_cascade::LiquidationCascadeSignal;
pub use orderbook_imbalance::OrderBookImbalanceSignal;
