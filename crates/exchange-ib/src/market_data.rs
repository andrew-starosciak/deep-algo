//! Market data — real-time quotes and historical bars for underlyings.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tracing::debug;

use crate::client::IBClient;

/// A historical price bar.
#[derive(Debug, Clone)]
pub struct PriceBar {
    pub timestamp: DateTime<Utc>,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: u64,
}

/// Stock quote snapshot.
#[derive(Debug, Clone)]
pub struct StockQuote {
    pub symbol: String,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last: Decimal,
    pub volume: u64,
    pub timestamp: DateTime<Utc>,
}

impl IBClient {
    /// Fetch historical daily bars for a stock.
    pub async fn historical_bars(
        &self,
        symbol: &str,
        duration_days: u32,
    ) -> Result<Vec<PriceBar>> {
        debug!(symbol, duration_days, "Fetching historical bars");

        let contract = ibapi::contracts::Contract::stock(symbol);

        // TODO: Use ibapi historical_data() to fetch bars
        // Convert ibapi bars to our PriceBar type

        Ok(vec![])
    }

    /// Fetch a snapshot quote for a stock.
    pub async fn stock_quote(&self, symbol: &str) -> Result<StockQuote> {
        debug!(symbol, "Fetching stock quote");

        // TODO: Use ibapi market data snapshot

        Ok(StockQuote {
            symbol: symbol.to_uppercase(),
            bid: Decimal::ZERO,
            ask: Decimal::ZERO,
            last: Decimal::ZERO,
            volume: 0,
            timestamp: Utc::now(),
        })
    }
}
