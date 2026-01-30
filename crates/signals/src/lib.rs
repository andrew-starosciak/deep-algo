pub mod common;
pub mod funding;
pub mod generator;
pub mod liquidations;
pub mod trades;

// Re-export signal generators for convenience
pub use generator::{
    CompositeSignal, FundingRateSignal, LiquidationCascadeSignal, OrderBookImbalanceSignal,
};
