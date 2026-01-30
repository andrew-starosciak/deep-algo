//! Polymarket CLOB integration for statistical trading engine.
//!
//! This crate provides:
//! - REST client with rate limiting for Polymarket CLOB API
//! - Models for markets, tokens, and prices
//! - Odds polling collector for BTC-related binary markets
//!
//! # Example
//!
//! ```no_run
//! use algo_trade_polymarket::{PolymarketClient, OddsCollector, OddsCollectorConfig};
//! use tokio::sync::mpsc;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = PolymarketClient::new();
//!     let config = OddsCollectorConfig::default();
//!     let (tx, mut rx) = mpsc::channel(100);
//!
//!     let mut collector = OddsCollector::new(client, config, tx);
//!
//!     // Discover BTC markets
//!     let count = collector.discover_markets().await?;
//!     println!("Discovered {} BTC markets", count);
//!
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod models;
pub mod odds_collector;

// Re-export main types
pub use client::PolymarketClient;
pub use models::{Market, MarketFilter, Price, Token};
pub use odds_collector::{
    deduplicate_markets, filter_btc_markets, OddsCollector, OddsCollectorConfig,
    OddsCollectorEvent, OddsCollectorStats,
};
