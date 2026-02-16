//! Interactive Brokers exchange integration for options trading.
//!
//! Provides IB Gateway/TWS connectivity, options chain queries,
//! order execution, and account management. Used by `options-manager`
//! for deterministic position management.

pub mod account;
pub mod client;
pub mod execution;
pub mod market_data;
pub mod options_chain;
pub mod paper;
pub mod types;
