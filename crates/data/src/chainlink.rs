//! Chainlink price feed client for reading oracle prices on Polygon.
//!
//! Supports BTC/USD, ETH/USD, SOL/USD, and XRP/USD feeds used by
//! Polymarket for settling 15-minute binary options.
//!
//! # Settlement Logic
//! - "Up" wins if end_price >= start_price
//! - "Down" wins if end_price < start_price

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{debug, warn};

// ============================================================================
// Chainlink price feed contract addresses on Polygon Mainnet
// All feeds use 8 decimals (divide answer by 10^8 to get USD price)
// Source: https://docs.chain.link/data-feeds/price-feeds/addresses
// ============================================================================

const CHAINLINK_BTC_USD_POLYGON: &str = "0xc907E116054Ad103354f2D350FD2514433D57F6f";
const CHAINLINK_ETH_USD_POLYGON: &str = "0xF9680D99D6C9589e2a93a78A04A279e509205945";
const CHAINLINK_SOL_USD_POLYGON: &str = "0x10C8264C0935b3B9870013e057f330Ff3e9C56dC";
const CHAINLINK_XRP_USD_POLYGON: &str = "0x785ba89291f676b5386652eB12b30cF361020694";

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

    /// Creates a new Chainlink price feed client for ETH/USD on Polygon.
    pub fn new_eth_usd(rpc_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address: CHAINLINK_ETH_USD_POLYGON.to_string(),
            decimals: 8,
        }
    }

    /// Creates a new Chainlink price feed client for SOL/USD on Polygon.
    pub fn new_sol_usd(rpc_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address: CHAINLINK_SOL_USD_POLYGON.to_string(),
            decimals: 8,
        }
    }

    /// Creates a new Chainlink price feed client for XRP/USD on Polygon.
    pub fn new_xrp_usd(rpc_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address: CHAINLINK_XRP_USD_POLYGON.to_string(),
            decimals: 8,
        }
    }

    /// Creates a Chainlink price feed for a coin by name.
    ///
    /// Supported coins: BTC, ETH, SOL, XRP (case-insensitive).
    pub fn for_coin(coin: &str, rpc_url: String) -> Option<Self> {
        let contract = match coin.to_uppercase().as_str() {
            "BTC" => CHAINLINK_BTC_USD_POLYGON,
            "ETH" => CHAINLINK_ETH_USD_POLYGON,
            "SOL" => CHAINLINK_SOL_USD_POLYGON,
            "XRP" => CHAINLINK_XRP_USD_POLYGON,
            _ => return None,
        };
        Some(Self {
            client: reqwest::Client::new(),
            rpc_url,
            contract_address: contract.to_string(),
            decimals: 8,
        })
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

// ============================================================================
// Chainlink Window Tracker
// ============================================================================

/// Record of Chainlink prices for a single coin in a single window.
#[derive(Debug, Clone)]
pub struct WindowPriceRecord {
    /// Price recorded near window open.
    pub start_price: Decimal,
    /// Most recent price (becomes end_price when window closes).
    pub latest_price: Decimal,
    /// Whether this window has been closed (new window started).
    pub closed: bool,
    /// Number of polls recorded for this window.
    pub poll_count: u32,
    /// When this window was first polled.
    pub first_polled_at: DateTime<Utc>,
    /// When this window was last polled.
    pub last_polled_at: DateTime<Utc>,
}

/// Tracks Chainlink oracle prices at 15-minute window boundaries.
///
/// Polls all configured coin feeds periodically and records start/end prices
/// for each window. Used as the source-of-truth settlement fallback
/// (replacing Binance klines) since Polymarket uses Chainlink for resolution.
///
/// # Usage
/// ```ignore
/// let mut tracker = ChainlinkWindowTracker::new("https://polygon-rpc.com");
/// // Poll every ~10 seconds in a background task
/// tracker.poll().await;
/// // At settlement time:
/// if let Some(outcome) = tracker.get_outcome("BTC", window_start_ts) {
///     // outcome is "UP" or "DOWN"
/// }
/// ```
pub struct ChainlinkWindowTracker {
    /// (coin_name, feed) pairs for all supported coins.
    feeds: Vec<(String, ChainlinkPriceFeed)>,
    /// (coin, window_start_ts) -> price record.
    windows: HashMap<(String, i64), WindowPriceRecord>,
    /// Window duration in seconds (900 for 15-min).
    window_secs: i64,
    /// Last window start timestamp per coin (to detect transitions).
    last_window: HashMap<String, i64>,
}

impl ChainlinkWindowTracker {
    /// Creates a new tracker for all 4 supported coins.
    pub fn new(rpc_url: &str) -> Self {
        let coins = ["BTC", "ETH", "SOL", "XRP"];
        let feeds = coins
            .iter()
            .filter_map(|coin| {
                ChainlinkPriceFeed::for_coin(coin, rpc_url.to_string())
                    .map(|feed| (coin.to_string(), feed))
            })
            .collect();

        Self {
            feeds,
            windows: HashMap::new(),
            window_secs: 900,
            last_window: HashMap::new(),
        }
    }

    /// Calculates the window start timestamp for a given time.
    fn window_start_for(&self, now: DateTime<Utc>) -> i64 {
        let ts = now.timestamp();
        (ts / self.window_secs) * self.window_secs
    }

    /// Polls all coin feeds and records prices.
    ///
    /// Call this every ~10 seconds from a background task. It:
    /// 1. Records the first price after a window opens as the start price
    /// 2. Continuously updates the latest price
    /// 3. Closes the previous window when a new one starts
    /// 4. Cleans up windows older than 2 hours
    pub async fn poll(&mut self) {
        let now = Utc::now();
        let current_window = self.window_start_for(now);

        for (coin, feed) in &self.feeds {
            let price = match feed.get_latest_price().await {
                Ok(data) => data.price,
                Err(e) => {
                    debug!(coin = %coin, error = %e, "Chainlink poll failed");
                    continue;
                }
            };

            // Detect window transition: close the previous window
            if let Some(&prev_window) = self.last_window.get(coin.as_str()) {
                if prev_window != current_window {
                    let prev_key = (coin.clone(), prev_window);
                    if let Some(record) = self.windows.get_mut(&prev_key) {
                        record.closed = true;
                    }
                }
            }
            self.last_window.insert(coin.clone(), current_window);

            // Update current window
            let key = (coin.clone(), current_window);
            self.windows
                .entry(key)
                .and_modify(|r| {
                    r.latest_price = price;
                    r.poll_count += 1;
                    r.last_polled_at = now;
                })
                .or_insert(WindowPriceRecord {
                    start_price: price,
                    latest_price: price,
                    closed: false,
                    poll_count: 1,
                    first_polled_at: now,
                    last_polled_at: now,
                });
        }

        // Cleanup old windows (older than 2 hours)
        let cutoff = current_window - 7200;
        self.windows.retain(|(_, ws), _| *ws >= cutoff);
    }

    /// Gets the settlement outcome for a coin in a specific window.
    ///
    /// Returns `Some("UP")` if end_price >= start_price, `Some("DOWN")` otherwise.
    /// Returns `None` if the window hasn't been recorded or hasn't closed yet.
    pub fn get_outcome(&self, coin: &str, window_start_ts: i64) -> Option<String> {
        let key = (coin.to_uppercase(), window_start_ts);
        let record = self.windows.get(&key)?;

        if !record.closed {
            return None; // Window still open
        }

        // Polymarket rule: UP wins if end >= start (includes ties)
        if record.latest_price >= record.start_price {
            Some("UP".to_string())
        } else {
            Some("DOWN".to_string())
        }
    }

    /// Returns the number of tracked windows (for diagnostics).
    pub fn tracked_window_count(&self) -> usize {
        self.windows.len()
    }

    /// Returns the number of closed (settleable) windows.
    pub fn closed_window_count(&self) -> usize {
        self.windows.values().filter(|r| r.closed).count()
    }

    /// Returns all tracked window records for database persistence.
    /// Returns (coin, window_start_ts, record) triples.
    pub fn get_all_windows(&self) -> Vec<(String, i64, WindowPriceRecord)> {
        self.windows
            .iter()
            .map(|((coin, ws), record)| (coin.clone(), *ws, record.clone()))
            .collect()
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

    #[test]
    fn test_for_coin_all_supported() {
        let rpc = "https://polygon-rpc.com".to_string();
        assert!(ChainlinkPriceFeed::for_coin("BTC", rpc.clone()).is_some());
        assert!(ChainlinkPriceFeed::for_coin("ETH", rpc.clone()).is_some());
        assert!(ChainlinkPriceFeed::for_coin("SOL", rpc.clone()).is_some());
        assert!(ChainlinkPriceFeed::for_coin("XRP", rpc.clone()).is_some());
        assert!(ChainlinkPriceFeed::for_coin("btc", rpc.clone()).is_some()); // case-insensitive
        assert!(ChainlinkPriceFeed::for_coin("DOGE", rpc).is_none());
    }

    #[test]
    fn test_window_tracker_get_outcome_unclosed() {
        let tracker = ChainlinkWindowTracker::new("https://polygon-rpc.com");
        // No data recorded yet
        assert!(tracker.get_outcome("BTC", 1700000000).is_none());
    }

    #[test]
    fn test_window_tracker_outcome_logic() {
        let mut tracker = ChainlinkWindowTracker::new("https://polygon-rpc.com");
        let now = Utc::now();

        // Manually insert a closed window record
        let key = ("BTC".to_string(), 1700000000i64);
        tracker.windows.insert(
            key.clone(),
            WindowPriceRecord {
                start_price: dec!(100000),
                latest_price: dec!(100500), // went up
                closed: true,
                poll_count: 1,
                first_polled_at: now,
                last_polled_at: now,
            },
        );
        assert_eq!(tracker.get_outcome("BTC", 1700000000), Some("UP".to_string()));

        // Down case
        let key2 = ("ETH".to_string(), 1700000000i64);
        tracker.windows.insert(
            key2,
            WindowPriceRecord {
                start_price: dec!(3500),
                latest_price: dec!(3490), // went down
                closed: true,
                poll_count: 1,
                first_polled_at: now,
                last_polled_at: now,
            },
        );
        assert_eq!(tracker.get_outcome("ETH", 1700000000), Some("DOWN".to_string()));

        // Tie goes to UP (Polymarket rule)
        let key3 = ("SOL".to_string(), 1700000000i64);
        tracker.windows.insert(
            key3,
            WindowPriceRecord {
                start_price: dec!(200),
                latest_price: dec!(200), // tie
                closed: true,
                poll_count: 1,
                first_polled_at: now,
                last_polled_at: now,
            },
        );
        assert_eq!(tracker.get_outcome("SOL", 1700000000), Some("UP".to_string()));
    }

    #[test]
    fn test_window_tracker_unclosed_returns_none() {
        let mut tracker = ChainlinkWindowTracker::new("https://polygon-rpc.com");
        let now = Utc::now();
        tracker.windows.insert(
            ("BTC".to_string(), 1700000000i64),
            WindowPriceRecord {
                start_price: dec!(100000),
                latest_price: dec!(100500),
                closed: false, // not closed yet
                poll_count: 1,
                first_polled_at: now,
                last_polled_at: now,
            },
        );
        assert!(tracker.get_outcome("BTC", 1700000000).is_none());
    }
}
