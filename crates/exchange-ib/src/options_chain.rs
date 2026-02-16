//! Options chain queries — strikes, expiries, greeks, OI.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use tracing::debug;

use crate::client::IBClient;
use crate::types::{OptionGreeks, OptionQuote, OptionRight, OptionsContract};

/// Options chain for a single underlying.
#[derive(Debug, Clone)]
pub struct OptionsChain {
    pub underlying: String,
    pub underlying_price: Decimal,
    pub expirations: Vec<NaiveDate>,
    pub strikes: Vec<Decimal>,
    pub quotes: Vec<OptionQuote>,
}

/// Filter for querying a subset of the chain.
#[derive(Debug, Clone, Default)]
pub struct ChainFilter {
    /// Filter to specific expiration(s).
    pub expiry: Option<NaiveDate>,
    /// Filter to specific right (call/put).
    pub right: Option<OptionRight>,
    /// Minimum strike price.
    pub min_strike: Option<Decimal>,
    /// Maximum strike price.
    pub max_strike: Option<Decimal>,
    /// Minimum open interest.
    pub min_open_interest: Option<u64>,
}

impl IBClient {
    /// Fetch full options chain for an underlying symbol.
    pub async fn options_chain(
        &self,
        symbol: &str,
        filter: &ChainFilter,
    ) -> Result<OptionsChain> {
        debug!(symbol, "Fetching options chain");

        // Build the underlying stock contract
        let _stock = ibapi::contracts::Contract::stock(symbol);

        // TODO: Use ibapi to:
        // 1. Request security definition option parameters (expirations + strikes)
        // 2. For each matching expiry/strike, request market data (bid/ask/greeks)
        // 3. Assemble into OptionsChain

        Ok(OptionsChain {
            underlying: symbol.to_uppercase(),
            underlying_price: Decimal::ZERO,
            expirations: vec![],
            strikes: vec![],
            quotes: vec![],
        })
    }

    /// Fetch a single option quote.
    pub async fn option_quote(
        &self,
        contract: &OptionsContract,
    ) -> Result<OptionQuote> {
        debug!(
            symbol = contract.symbol,
            strike = %contract.strike,
            right = %contract.right,
            expiry = %contract.expiry,
            "Fetching option quote"
        );

        // TODO: Build ibapi Contract for this option and request market data snapshot

        Ok(OptionQuote {
            contract: contract.clone(),
            bid: Decimal::ZERO,
            ask: Decimal::ZERO,
            last: Decimal::ZERO,
            mid: Decimal::ZERO,
            volume: 0,
            open_interest: 0,
            iv: 0.0,
            greeks: OptionGreeks::default(),
        })
    }
}
