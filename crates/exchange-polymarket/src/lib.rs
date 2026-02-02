//! Polymarket CLOB integration for statistical trading engine.
//!
//! This crate provides:
//! - REST client with rate limiting for Polymarket CLOB API
//! - Gamma API client for 15-minute market discovery
//! - Models for markets, tokens, and prices
//! - Odds polling collector for BTC-related binary markets
//! - Arbitrage execution layer for paired YES/NO trading
//!
//! # Example
//!
//! ```no_run
//! use algo_trade_polymarket::{PolymarketClient, GammaClient, OddsCollector, OddsCollectorConfig};
//! use algo_trade_polymarket::Coin;
//! use tokio::sync::mpsc;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Use Gamma API for 15-minute market discovery
//!     let gamma = GammaClient::new();
//!     let markets = gamma.get_all_current_15min_markets().await;
//!     println!("Discovered {} 15-min markets", markets.len());
//!
//!     // Or use the collector for continuous polling
//!     let client = PolymarketClient::new();
//!     let config = OddsCollectorConfig::default()
//!         .with_15min_coins(vec![Coin::Btc, Coin::Eth]);
//!     let (tx, mut rx) = mpsc::channel(100);
//!
//!     let mut collector = OddsCollector::new(client, config, tx);
//!     let count = collector.discover_markets().await?;
//!     println!("Discovered {} markets", count);
//!
//!     Ok(())
//! }
//! ```

pub mod arbitrage;
pub mod client;
pub mod gamma;
pub mod models;
pub mod odds_collector;
pub mod websocket;

// Re-export main types
pub use client::PolymarketClient;
pub use gamma::GammaClient;
pub use models::{Coin, GammaEvent, GammaMarket, Market, MarketFilter, Price, Token};
pub use odds_collector::{
    deduplicate_markets, filter_btc_markets, OddsCollector, OddsCollectorConfig,
    OddsCollectorEvent, OddsCollectorStats,
};
pub use websocket::{BookEvent, PolymarketWebSocket, WebSocketConfig, WebSocketError};
