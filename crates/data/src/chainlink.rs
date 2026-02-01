//! Chainlink price feed client for reading BTC/USD prices on Polygon.
//!
//! This module provides a client to read the Chainlink BTC/USD price feed
//! used by Polymarket for settling 15-minute binary options.
//!
//! # Settlement Logic
//! - "Up" wins if end_price >= start_price
//! - "Down" wins if end_price < start_price

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Chainlink BTC/USD price feed on Polygon Mainnet.
/// Contract: 0xc907E116054Ad103354f2D350FD2514433D57F6f
/// Decimals: 8 (divide answer by 10^8 to get USD price)
const CHAINLINK_BTC_USD_POLYGON: &str = "0xc907E116054Ad103354f2D350FD2514433D57F6f";

/// Default Polygon RPC endpoint (public, rate-limited).
/// For production, use Alchemy, Infura, or your own node.
const DEFAULT_POLYGON_RPC: &str = "https://polygon-rpc.com";

/// Function selector for latestRoundData() = keccak256("latestRoundData()")[:4]
/// Returns: (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
const LATEST_ROUND_DATA_SELECTOR: &str = "0xfeaf968c";

/// Chainlink price data from a single round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainlinkPriceData {
    /// Round ID from Chainlink.
    pub round_id: u128,
    /// Price in USD (already converted from 8 decimals).
    pub price: Decimal,
    /// Timestamp when the round started.
    pub started_at: DateTime<Utc>,
    /// Timestamp when the round was last updated.
    pub updated_at: DateTime<Utc>,
    /// The round in which the answer was computed.
    pub answered_in_round: u128,
}

impl ChainlinkPriceData {
    /// Returns true if this price data is stale (older than max_age_secs).
    pub fn is_stale(&self, max_age_secs: i64) -> bool {
        let age = Utc::now() - self.updated_at;
        age.num_seconds() > max_age_secs
    }
}

/// Client for reading Chainlink price feeds via JSON-RPC.
#[derive(Debug, Clone)]
pub struct ChainlinkPriceFeed {
    /// HTTP client for RPC calls.
    client: reqwest::Client,
    /// Polygon RPC endpoint URL.
    rpc_url: String,
    /// Contract address for the price feed.
    contract_address: String,
    /// Number of decimals (8 for BTC/USD).
    decimals: u8,
}

impl Default for ChainlinkPriceFeed {
    fn default() -> Self {
        Self::new_btc_usd(DEFAULT_POLYGON_RPC.to_string())
    }
}

impl ChainlinkPriceFeed {
    /// Creates a new Chainlink price feed client for BTC/USD on Polygon.
    pub fn new_btc_usd(rpc_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address: CHAINLINK_BTC_USD_POLYGON.to_string(),
            decimals: 8,
        }
    }

    /// Creates a custom Chainlink price feed client.
    pub fn new(rpc_url: String, contract_address: String, decimals: u8) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address,
            decimals,
        }
    }

    /// Gets the latest price from the Chainlink price feed.
    ///
    /// Makes an eth_call to the contract's latestRoundData() function.
    ///
    /// # Errors
    /// Returns an error if the RPC call fails or response parsing fails.
    pub async fn get_latest_price(&self) -> Result<ChainlinkPriceData> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{
                "to": self.contract_address,
                "data": LATEST_ROUND_DATA_SELECTOR
            }, "latest"],
            "id": 1
        });

        let response: RpcResponse = self
            .client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!("RPC error: {} - {}", error.code, error.message));
        }

        let result = response
            .result
            .ok_or_else(|| anyhow!("No result in RPC response"))?;

        self.parse_latest_round_data(&result)
    }

    /// Parses the hex-encoded response from latestRoundData().
    ///
    /// Layout (each field is 32 bytes / 64 hex chars):
    /// - bytes 0-32: roundId (uint80, right-padded)
    /// - bytes 32-64: answer (int256)
    /// - bytes 64-96: startedAt (uint256)
    /// - bytes 96-128: updatedAt (uint256)
    /// - bytes 128-160: answeredInRound (uint80, right-padded)
    fn parse_latest_round_data(&self, hex_data: &str) -> Result<ChainlinkPriceData> {
        // Remove 0x prefix
        let data = hex_data.strip_prefix("0x").unwrap_or(hex_data);

        if data.len() < 320 {
            return Err(anyhow!(
                "Invalid response length: {} (expected 320)",
                data.len()
            ));
        }

        // Parse each 32-byte (64 hex char) field
        let round_id = u128::from_str_radix(&data[0..64], 16)?;
        let answer_raw = i128::from_str_radix(&data[64..128], 16)?;
        // Handle edge case where field might be all zeros
        let started_at_raw =
            u64::from_str_radix(data[128..192].trim_start_matches('0'), 16).unwrap_or(0);
        let updated_at_raw =
            u64::from_str_radix(data[192..256].trim_start_matches('0'), 16).unwrap_or(0);
        let answered_in_round = u128::from_str_radix(&data[256..320], 16)?;

        // Convert answer from 8 decimals to Decimal
        let divisor = Decimal::from(10u64.pow(self.decimals as u32));
        let price = Decimal::from_str(&answer_raw.to_string())? / divisor;

        // Convert timestamps
        let started_at =
            DateTime::from_timestamp(started_at_raw as i64, 0).unwrap_or_else(Utc::now);
        let updated_at =
            DateTime::from_timestamp(updated_at_raw as i64, 0).unwrap_or_else(Utc::now);

        Ok(ChainlinkPriceData {
            round_id,
            price,
            started_at,
            updated_at,
            answered_in_round,
        })
    }

    /// Gets the current BTC price as a simple Decimal.
    ///
    /// Convenience method that just returns the price.
    pub async fn get_btc_price(&self) -> Result<Decimal> {
        let data = self.get_latest_price().await?;
        Ok(data.price)
    }
}

/// JSON-RPC response structure.
#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<String>,
    error: Option<RpcError>,
}

/// JSON-RPC error structure.
#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// Tracks window prices for settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowPrices {
    /// Price at window start.
    pub start_price: Decimal,
    /// Timestamp of start price.
    pub start_time: DateTime<Utc>,
    /// Price at window end (None if window hasn't ended).
    pub end_price: Option<Decimal>,
    /// Timestamp of end price.
    pub end_time: Option<DateTime<Utc>>,
}

impl WindowPrices {
    /// Creates a new window with the start price.
    pub fn new(start_price: Decimal, start_time: DateTime<Utc>) -> Self {
        Self {
            start_price,
            start_time,
            end_price: None,
            end_time: None,
        }
    }

    /// Sets the end price.
    pub fn with_end_price(mut self, end_price: Decimal, end_time: DateTime<Utc>) -> Self {
        self.end_price = Some(end_price);
        self.end_time = Some(end_time);
        self
    }

    /// Returns true if the window is complete (has end price).
    pub fn is_complete(&self) -> bool {
        self.end_price.is_some()
    }

    /// Determines the settlement outcome.
    ///
    /// Returns true if "Up" wins (end >= start), false if "Down" wins.
    /// Returns None if the window is not complete.
    pub fn settlement_outcome(&self) -> Option<bool> {
        self.end_price.map(|end| end >= self.start_price)
    }

    /// Returns true if "Up" won.
    pub fn up_won(&self) -> Option<bool> {
        self.settlement_outcome()
    }

    /// Returns true if "Down" won.
    pub fn down_won(&self) -> Option<bool> {
        self.settlement_outcome().map(|up| !up)
    }
}

/// Settlement result for a paper trade.
#[derive(Debug, Clone)]
pub struct SettlementResult {
    /// Whether the trade was a winner.
    pub won: bool,
    /// The direction that won ("up" or "down").
    pub winning_direction: String,
    /// Start price at window open.
    pub start_price: Decimal,
    /// End price at window close.
    pub end_price: Decimal,
    /// Price change (end - start).
    pub price_change: Decimal,
    /// Timestamp of settlement.
    pub settled_at: DateTime<Utc>,
}

impl SettlementResult {
    /// Creates a settlement result from window prices and trade direction.
    ///
    /// # Arguments
    /// * `window` - The window prices with both start and end
    /// * `trade_direction` - "yes" for Up, "no" for Down
    pub fn from_window(window: &WindowPrices, trade_direction: &str) -> Option<Self> {
        let end_price = window.end_price?;
        let end_time = window.end_time?;

        let up_won = end_price >= window.start_price;
        let winning_direction = if up_won { "up" } else { "down" };

        // "yes" means betting on Up, "no" means betting on Down
        let won = match trade_direction {
            "yes" => up_won,
            "no" => !up_won,
            _ => return None,
        };

        Some(Self {
            won,
            winning_direction: winning_direction.to_string(),
            start_price: window.start_price,
            end_price,
            price_change: end_price - window.start_price,
            settled_at: end_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_window_prices_new() {
        let now = Utc::now();
        let window = WindowPrices::new(dec!(100000), now);

        assert_eq!(window.start_price, dec!(100000));
        assert_eq!(window.start_time, now);
        assert!(window.end_price.is_none());
        assert!(!window.is_complete());
    }

    #[test]
    fn test_window_prices_with_end() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(100500), end);

        assert!(window.is_complete());
        assert_eq!(window.end_price, Some(dec!(100500)));
    }

    #[test]
    fn test_settlement_outcome_up_wins() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(100500), end);

        assert_eq!(window.settlement_outcome(), Some(true)); // Up wins
        assert_eq!(window.up_won(), Some(true));
        assert_eq!(window.down_won(), Some(false));
    }

    #[test]
    fn test_settlement_outcome_down_wins() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(99500), end);

        assert_eq!(window.settlement_outcome(), Some(false)); // Down wins
        assert_eq!(window.up_won(), Some(false));
        assert_eq!(window.down_won(), Some(true));
    }

    #[test]
    fn test_settlement_outcome_equal_up_wins() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        // Per Polymarket rules: Up wins if end >= start (includes equal)
        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(100000), end);

        assert_eq!(window.settlement_outcome(), Some(true)); // Up wins on tie
    }

    #[test]
    fn test_settlement_result_yes_bet_up_wins() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(100500), end);

        let result = SettlementResult::from_window(&window, "yes").unwrap();

        assert!(result.won);
        assert_eq!(result.winning_direction, "up");
        assert_eq!(result.price_change, dec!(500));
    }

    #[test]
    fn test_settlement_result_no_bet_down_wins() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(99500), end);

        let result = SettlementResult::from_window(&window, "no").unwrap();

        assert!(result.won);
        assert_eq!(result.winning_direction, "down");
        assert_eq!(result.price_change, dec!(-500));
    }

    #[test]
    fn test_settlement_result_yes_bet_loses() {
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(15);

        let window = WindowPrices::new(dec!(100000), start).with_end_price(dec!(99500), end);

        let result = SettlementResult::from_window(&window, "yes").unwrap();

        assert!(!result.won); // Yes bet loses when Down wins
    }

    #[test]
    fn test_chainlink_price_data_is_stale() {
        let old_time = Utc::now() - chrono::Duration::minutes(5);
        let data = ChainlinkPriceData {
            round_id: 1,
            price: dec!(100000),
            started_at: old_time,
            updated_at: old_time,
            answered_in_round: 1,
        };

        assert!(data.is_stale(60)); // Stale if older than 60 seconds
        assert!(!data.is_stale(600)); // Not stale if max age is 10 minutes
    }

    #[test]
    fn test_parse_hex_to_int() {
        // Test that we can parse hex correctly
        let hex = "00000000000000000000000000000000000000000000000000000002540be400";
        let value = u64::from_str_radix(hex.trim_start_matches('0'), 16).unwrap_or(0);
        assert_eq!(value, 10_000_000_000u64); // 10 billion
    }
}
