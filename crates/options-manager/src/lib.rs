//! Deterministic options position management.
//!
//! Runs as a long-lived service that:
//! - Polls DB for approved trade recommendations and executes via IB
//! - Monitors open positions with current prices/greeks
//! - Enforces hard stops, profit targets, time stops
//! - Tracks cross-platform exposure (HyperLiquid + Polymarket)
//! - Enforces allocation caps
//!
//! No LLM in the execution path — all rules are deterministic.

pub mod allocation;
pub mod correlation;
pub mod executor;
pub mod monitor;
pub mod service;
pub mod stops;
pub mod targets;
pub mod types;
