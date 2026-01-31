//! Binary outcome backtesting module for Polymarket-style prediction markets.
//!
//! This module provides types and utilities for simulating binary outcome bets,
//! calculating fees, tracking settlement results, and computing comprehensive metrics.

pub mod engine;
pub mod fees;
pub mod metrics;
pub mod outcome;
pub mod pit;

pub use engine::{BacktestResults, BinaryBacktestConfig, BinaryBacktestEngine};
pub use fees::{FeeModel, FeeTier, FlatFees, PolymarketFees, ZeroFees};
pub use metrics::{calculate_break_even, BinaryMetrics};
pub use outcome::{BetDirection, BinaryBet, BinaryOutcome, SettlementResult};
pub use pit::{PointInTimeProvider, DEFAULT_MAX_LOOKBACK_SECONDS};
