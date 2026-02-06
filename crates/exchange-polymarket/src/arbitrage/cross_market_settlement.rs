//! Cross-market opportunity settlement handler.
//!
//! Polls for pending cross-market opportunities that have passed their window end,
//! checks the actual outcomes from Polymarket, and updates the database with
//! realized P&L and correlation accuracy.
//!
//! # Settlement Modes
//!
//! - **Paper mode**: Uses Chainlink/Binance price data as fallback (may not match Polymarket exactly)
//! - **Live mode**: Queries wallet positions from Polymarket Data API (source of truth)

use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use algo_trade_data::models::CrossMarketOpportunityRecord;
use algo_trade_data::repositories::CrossMarketRepository;

use super::correlation_tracker::CorrelationTracker;
use super::cross_market_types::CoinPair;
use super::sdk_client::ClobClient;
use crate::models::Coin;

/// Settlement mode determines how outcomes are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettlementMode {
    /// Paper mode: Use Chainlink/Binance fallback for settlement.
    /// May not match Polymarket resolution exactly.
    #[default]
    Paper,
    /// Live mode: Query wallet positions from Polymarket Data API.
    /// This is the source of truth for actual P&L.
    Live,
}

/// Configuration for the settlement handler.
#[derive(Debug, Clone)]
pub struct CrossMarketSettlementConfig {
    /// Settlement mode (Paper or Live).
    pub mode: SettlementMode,
    /// How long to wait after window_end before checking settlement (ms).
    /// Markets take ~1-2 minutes to resolve after window close.
    pub settlement_delay_ms: i64,
    /// How many pending opportunities to process per batch.
    pub batch_size: i64,
    /// Poll interval for checking pending settlements (ms).
    pub poll_interval_ms: u64,
    /// Fee rate on winning payouts (e.g., 0.02 for 2%).
    pub fee_rate: Decimal,
    /// Maximum age before marking as expired (ms).
    pub max_pending_age_ms: i64,
}

impl Default for CrossMarketSettlementConfig {
    fn default() -> Self {
        Self {
            mode: SettlementMode::Paper,
            settlement_delay_ms: 120_000, // 2 minutes
            batch_size: 50,
            poll_interval_ms: 30_000,           // 30 seconds
            fee_rate: dec!(0.02),               // 2% fee
            max_pending_age_ms: 60 * 60 * 1000, // 1 hour
        }
    }
}

impl CrossMarketSettlementConfig {
    /// Creates a config for live mode settlement.
    #[must_use]
    pub fn live() -> Self {
        Self {
            mode: SettlementMode::Live,
            ..Default::default()
        }
    }

    /// Creates a config for paper mode settlement.
    #[must_use]
    pub fn paper() -> Self {
        Self {
            mode: SettlementMode::Paper,
            ..Default::default()
        }
    }
}

/// Statistics for settlement processing.
#[derive(Debug, Clone, Default)]
pub struct SettlementStats {
    /// Total opportunities processed.
    pub total_processed: u64,
    /// Successful settlements.
    pub settled: u64,
    /// Expired (no outcome data).
    pub expired: u64,
    /// Errors during settlement.
    pub errors: u64,
    /// Total wins (WIN or DOUBLE_WIN).
    pub wins: u64,
    /// Total losses.
    pub losses: u64,
    /// Double wins (both legs won).
    pub double_wins: u64,
    /// Total realized P&L.
    pub total_pnl: Decimal,
    /// Times correlation held (coins moved together).
    pub correlation_correct: u64,
}

impl SettlementStats {
    /// Returns the win rate.
    #[must_use]
    pub fn win_rate(&self) -> f64 {
        if self.settled > 0 {
            self.wins as f64 / self.settled as f64
        } else {
            0.0
        }
    }

    /// Returns the correlation accuracy.
    #[must_use]
    pub fn correlation_accuracy(&self) -> f64 {
        if self.settled > 0 {
            self.correlation_correct as f64 / self.settled as f64
        } else {
            0.0
        }
    }
}

// These response types are for future use when we query market/token status
#[allow(dead_code)]
/// Response from the Polymarket prices endpoint.
#[derive(Debug, Deserialize)]
struct TokenPricesResponse {
    #[serde(flatten)]
    prices: HashMap<String, String>,
}

#[allow(dead_code)]
/// Response from the Polymarket market endpoint.
#[derive(Debug, Deserialize)]
struct MarketResponse {
    #[serde(rename = "conditionId")]
    condition_id: String,
    tokens: Vec<TokenInfo>,
    #[serde(default)]
    closed: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenInfo {
    token_id: String,
    outcome: String,
    #[serde(default)]
    winner: Option<bool>,
}

/// Cross-market settlement handler.
///
/// Continuously polls for pending opportunities and settles them
/// when outcomes are available.
///
/// # Settlement Flow
///
/// - **Live mode**: Uses wallet positions from Polymarket Data API (source of truth)
/// - **Paper mode**: Falls back to CLOB prices, then Chainlink/Binance
pub struct CrossMarketSettlementHandler {
    /// Database repository.
    repo: CrossMarketRepository,
    /// HTTP client for Polymarket API.
    http: Client,
    /// SDK client for wallet-based settlement (live mode only).
    wallet_client: Option<Arc<ClobClient>>,
    /// Configuration.
    config: CrossMarketSettlementConfig,
    /// Stop signal.
    stop_flag: Arc<AtomicBool>,
    /// Statistics.
    stats: Arc<RwLock<SettlementStats>>,
    /// Dynamic correlation tracker (updated on each settlement).
    correlation_tracker: Option<Arc<CorrelationTracker>>,
}

impl CrossMarketSettlementHandler {
    /// Creates a new settlement handler for paper mode.
    #[must_use]
    pub fn new(repo: CrossMarketRepository, config: CrossMarketSettlementConfig) -> Self {
        Self {
            repo,
            http: Client::new(),
            wallet_client: None,
            config,
            stop_flag: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(SettlementStats::default())),
            correlation_tracker: None,
        }
    }

    /// Creates a handler for live mode with wallet-based settlement.
    ///
    /// In live mode, settlement queries the wallet's actual positions
    /// from Polymarket's Data API, providing accurate P&L.
    #[must_use]
    pub fn new_live(
        repo: CrossMarketRepository,
        wallet_client: Arc<ClobClient>,
        config: CrossMarketSettlementConfig,
    ) -> Self {
        let mut config = config;
        config.mode = SettlementMode::Live;

        Self {
            repo,
            http: Client::new(),
            wallet_client: Some(wallet_client),
            config,
            stop_flag: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(SettlementStats::default())),
            correlation_tracker: None,
        }
    }

    /// Creates a handler with default config (paper mode).
    #[must_use]
    pub fn with_defaults(repo: CrossMarketRepository) -> Self {
        Self::new(repo, CrossMarketSettlementConfig::default())
    }

    /// Sets the correlation tracker for dynamic correlation updates.
    #[must_use]
    pub fn with_correlation_tracker(mut self, tracker: Arc<CorrelationTracker>) -> Self {
        self.correlation_tracker = Some(tracker);
        self
    }

    /// Returns a handle to stop the handler.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop_flag.clone()
    }

    /// Returns a handle to the stats.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<SettlementStats>> {
        self.stats.clone()
    }

    /// Runs the settlement handler continuously.
    ///
    /// # Errors
    /// Returns an error if the handler encounters a fatal error.
    pub async fn run(&self) -> Result<()> {
        info!("Starting cross-market settlement handler");
        info!(
            "Config: mode={:?}, delay={}ms, batch={}, poll={}ms, fee={:.1}%",
            self.config.mode,
            self.config.settlement_delay_ms,
            self.config.batch_size,
            self.config.poll_interval_ms,
            self.config.fee_rate * dec!(100)
        );

        if self.config.mode == SettlementMode::Live {
            if self.wallet_client.is_some() {
                info!("Live mode: Using wallet positions for settlement (source of truth)");
            } else {
                warn!(
                    "Live mode enabled but no wallet client provided, falling back to paper mode"
                );
            }
        }

        loop {
            if self.stop_flag.load(Ordering::SeqCst) {
                info!("Settlement handler stopping");
                break;
            }

            // Process pending settlements
            match self.process_pending_batch().await {
                Ok(count) => {
                    if count > 0 {
                        info!("Processed {} settlements", count);
                    }
                }
                Err(e) => {
                    error!("Error processing settlements: {}", e);
                    let mut stats = self.stats.write().await;
                    stats.errors += 1;
                }
            }

            // Cleanup stale entries
            if let Err(e) = self.cleanup_stale().await {
                warn!("Error cleaning up stale entries: {}", e);
            }

            // Wait before next poll
            tokio::time::sleep(tokio::time::Duration::from_millis(
                self.config.poll_interval_ms,
            ))
            .await;
        }

        let stats = self.stats.read().await;
        info!(
            "Settlement handler stopped. Stats: processed={}, settled={}, wins={}, losses={}, P&L=${}",
            stats.total_processed,
            stats.settled,
            stats.wins,
            stats.losses,
            stats.total_pnl
        );

        Ok(())
    }

    /// Processes a batch of pending settlements.
    async fn process_pending_batch(&self) -> Result<u64> {
        // Get pending opportunities that have passed window_end + delay
        let pending = self
            .repo
            .get_pending_settlement(self.config.batch_size)
            .await?;

        let mut processed = 0u64;

        for opp in pending {
            // Check if enough time has passed since window_end
            let window_end = opp.window_end.unwrap_or(opp.timestamp);
            let settlement_time =
                window_end + Duration::milliseconds(self.config.settlement_delay_ms);

            if Utc::now() < settlement_time {
                continue;
            }

            // Try to settle this opportunity
            match self.settle_opportunity(&opp).await {
                Ok(()) => {
                    processed += 1;
                    let mut stats = self.stats.write().await;
                    stats.total_processed += 1;
                }
                Err(e) => {
                    warn!(
                        id = opp.id,
                        coin1 = opp.coin1,
                        coin2 = opp.coin2,
                        error = %e,
                        "Failed to settle opportunity"
                    );
                    let mut stats = self.stats.write().await;
                    stats.errors += 1;
                }
            }
        }

        Ok(processed)
    }

    /// Settles a single opportunity.
    ///
    /// Settlement flow depends on mode:
    ///
    /// **Live mode** (source of truth):
    /// 1. Query wallet positions from Polymarket Data API
    /// 2. Token price >= 0.95 means won, <= 0.05 means lost
    ///
    /// **Paper mode** (fallback):
    /// 1. Polymarket CLOB token prices (for recently closed markets)
    /// 2. Chainlink/Binance price data (for resolved markets)
    async fn settle_opportunity(&self, opp: &CrossMarketOpportunityRecord) -> Result<()> {
        debug!(
            id = opp.id,
            coin1 = opp.coin1,
            coin2 = opp.coin2,
            leg1_token = %opp.leg1_token_id,
            leg2_token = %opp.leg2_token_id,
            mode = ?self.config.mode,
            "Checking settlement for opportunity"
        );

        // In live mode, try wallet-based settlement first (source of truth)
        if self.config.mode == SettlementMode::Live {
            if let Some(ref client) = self.wallet_client {
                match self.try_settle_via_wallet(client, opp).await {
                    Ok((leg1_won, leg2_won)) => {
                        return self
                            .finalize_settlement(opp, leg1_won, leg2_won, "wallet_positions")
                            .await;
                    }
                    Err(e) => {
                        debug!(
                            id = opp.id,
                            error = %e,
                            "Wallet settlement failed, trying fallback methods"
                        );
                    }
                }
            }
        }

        // Try CLOB prices (works for recently closed markets)
        let clob_result = self.try_settle_via_clob(opp).await;

        match clob_result {
            Ok((leg1_won, leg2_won, method)) => {
                return self
                    .finalize_settlement(opp, leg1_won, leg2_won, &method)
                    .await;
            }
            Err(e) => {
                debug!(
                    id = opp.id,
                    error = %e,
                    "CLOB settlement failed, trying Chainlink/Binance fallback"
                );
            }
        }

        // Fallback to Chainlink/Binance (for resolved markets where orderbook is gone)
        let window_end = opp.window_end.unwrap_or(opp.timestamp);
        let window_start = window_end - Duration::milliseconds(15 * 60 * 1000);

        let coin1_outcome = self
            .get_coin_outcome(&opp.coin1, window_start, window_end)
            .await?;
        let coin2_outcome = self
            .get_coin_outcome(&opp.coin2, window_start, window_end)
            .await?;

        match (coin1_outcome, coin2_outcome) {
            (Some(c1), Some(c2)) => {
                let leg1_won = (opp.leg1_direction == "UP" && c1 == "UP")
                    || (opp.leg1_direction == "DOWN" && c1 == "DOWN");
                let leg2_won = (opp.leg2_direction == "UP" && c2 == "UP")
                    || (opp.leg2_direction == "DOWN" && c2 == "DOWN");

                warn!(
                    id = opp.id,
                    coin1_outcome = %c1,
                    coin2_outcome = %c2,
                    "Using Chainlink/Binance fallback - may not match Polymarket exactly"
                );

                self.finalize_settlement(opp, leg1_won, leg2_won, "chainlink_fallback")
                    .await
            }
            _ => Err(anyhow!("Outcomes not available from any source")),
        }
    }

    /// Settles using wallet positions from Polymarket Data API.
    ///
    /// This is the source of truth for live trading:
    /// - cur_price >= 0.95 means the position won
    /// - cur_price <= 0.05 means the position lost
    /// - redeemable = true means the market has resolved
    async fn try_settle_via_wallet(
        &self,
        client: &ClobClient,
        opp: &CrossMarketOpportunityRecord,
    ) -> Result<(bool, bool)> {
        let token_ids = [opp.leg1_token_id.as_str(), opp.leg2_token_id.as_str()];

        let positions = client
            .get_positions_for_tokens(&token_ids)
            .await
            .map_err(|e| anyhow!("Failed to fetch wallet positions: {}", e))?;

        // Find our positions
        let leg1_pos = positions.iter().find(|p| p.asset == opp.leg1_token_id);
        let leg2_pos = positions.iter().find(|p| p.asset == opp.leg2_token_id);

        // Check if both positions are resolved
        let leg1_result = leg1_pos.and_then(|p| {
            let price: f64 = p.cur_price.parse().ok()?;
            if p.redeemable || price >= 0.95 || price <= 0.05 {
                Some(price >= 0.95)
            } else {
                None
            }
        });

        let leg2_result = leg2_pos.and_then(|p| {
            let price: f64 = p.cur_price.parse().ok()?;
            if p.redeemable || price >= 0.95 || price <= 0.05 {
                Some(price >= 0.95)
            } else {
                None
            }
        });

        match (leg1_result, leg2_result) {
            (Some(leg1_won), Some(leg2_won)) => {
                info!(
                    id = opp.id,
                    leg1_won = leg1_won,
                    leg2_won = leg2_won,
                    "Settled via wallet positions (source of truth)"
                );
                Ok((leg1_won, leg2_won))
            }
            (Some(_), None) => {
                debug!(id = opp.id, "Leg 2 position not found or not resolved");
                Err(anyhow!("Leg 2 not resolved in wallet"))
            }
            (None, Some(_)) => {
                debug!(id = opp.id, "Leg 1 position not found or not resolved");
                Err(anyhow!("Leg 1 not resolved in wallet"))
            }
            (None, None) => {
                debug!(id = opp.id, "Neither position found or resolved");
                Err(anyhow!("Positions not found or not resolved in wallet"))
            }
        }
    }

    /// Try to settle using CLOB token prices.
    async fn try_settle_via_clob(
        &self,
        opp: &CrossMarketOpportunityRecord,
    ) -> Result<(bool, bool, String)> {
        let url = format!(
            "https://clob.polymarket.com/prices?token_ids={},{}",
            opp.leg1_token_id, opp.leg2_token_id
        );

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("CLOB API error: {}", response.status()));
        }

        let prices: std::collections::HashMap<String, serde_json::Value> = response.json().await?;

        let leg1_price = prices
            .get(&opp.leg1_token_id)
            .and_then(|v| v.get("price").or(v.get("mid")))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| prices.get(&opp.leg1_token_id).and_then(|v| v.as_f64()));

        let leg2_price = prices
            .get(&opp.leg2_token_id)
            .and_then(|v| v.get("price").or(v.get("mid")))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| prices.get(&opp.leg2_token_id).and_then(|v| v.as_f64()));

        match (leg1_price, leg2_price) {
            (Some(p1), Some(p2)) => {
                let l1_won = p1 >= 0.95;
                let l2_won = p2 >= 0.95;
                debug!(
                    id = opp.id,
                    leg1_price = p1,
                    leg2_price = p2,
                    leg1_won = l1_won,
                    leg2_won = l2_won,
                    "CLOB token prices fetched"
                );
                Ok((l1_won, l2_won, "clob_prices".to_string()))
            }
            _ => Err(anyhow!("Token prices not available in CLOB response")),
        }
    }

    /// Finalize settlement with determined outcomes.
    async fn finalize_settlement(
        &self,
        opp: &CrossMarketOpportunityRecord,
        leg1_won: bool,
        leg2_won: bool,
        method: &str,
    ) -> Result<()> {
        // Derive coin outcomes from leg results
        let c1 = if leg1_won {
            opp.leg1_direction.clone()
        } else if opp.leg1_direction == "UP" {
            "DOWN".to_string()
        } else {
            "UP".to_string()
        };
        let c2 = if leg2_won {
            opp.leg2_direction.clone()
        } else if opp.leg2_direction == "UP" {
            "DOWN".to_string()
        } else {
            "UP".to_string()
        };

        // Determine trade result
        let trade_result = match (leg1_won, leg2_won) {
            (true, true) => "DOUBLE_WIN",
            (true, false) | (false, true) => "WIN",
            (false, false) => "LOSE",
        };

        // Calculate P&L
        let payout = match trade_result {
            "DOUBLE_WIN" => Decimal::TWO,
            "WIN" => Decimal::ONE,
            _ => Decimal::ZERO,
        };
        let fees = payout * self.config.fee_rate;
        let actual_pnl = payout - fees - opp.total_cost;

        // Check if correlation held (both coins moved in same direction)
        let correlation_correct = c1 == c2;

        // Update database
        self.repo
            .settle_opportunity(
                opp.id,
                &c1,
                &c2,
                trade_result,
                actual_pnl,
                correlation_correct,
            )
            .await?;

        // Feed correlation observation to dynamic tracker
        if let Some(ref tracker) = self.correlation_tracker {
            if let (Some(coin1), Some(coin2)) =
                (Coin::from_slug(&opp.coin1), Coin::from_slug(&opp.coin2))
            {
                let pair = CoinPair::new(coin1, coin2);
                tracker.record_observation(pair, correlation_correct);
                debug!(
                    pair = format!("{}/{}", opp.coin1, opp.coin2),
                    correct = correlation_correct,
                    "Recorded correlation observation"
                );
            }
        }

        // Update stats
        let mut stats = self.stats.write().await;
        stats.settled += 1;
        stats.total_pnl += actual_pnl;

        match trade_result {
            "DOUBLE_WIN" => {
                stats.wins += 1;
                stats.double_wins += 1;
            }
            "WIN" => {
                stats.wins += 1;
            }
            _ => {
                stats.losses += 1;
            }
        }

        if correlation_correct {
            stats.correlation_correct += 1;
        }

        info!(
            id = opp.id,
            pair = format!("{}/{}", opp.coin1, opp.coin2),
            result = trade_result,
            pnl = %actual_pnl,
            correlation = correlation_correct,
            method = method,
            "Settled opportunity"
        );

        Ok(())
    }

    /// Gets the outcome for a coin (UP or DOWN) based on price data.
    ///
    /// Uses the 15-minute kline that covers the window - compares close vs open.
    /// If close > open, the coin went UP. If close <= open, the coin went DOWN.
    ///
    /// Tries multiple APIs in order: Binance.US, Binance global, CoinGecko.
    ///
    /// Returns None if the kline hasn't closed yet (window still in progress).
    async fn get_coin_outcome(
        &self,
        coin: &str,
        window_start: chrono::DateTime<Utc>,
        window_end: chrono::DateTime<Utc>,
    ) -> Result<Option<String>> {
        // Try Binance.US first (works in US), then Binance global, then CoinGecko
        if let Ok(Some(outcome)) = self
            .get_coin_outcome_binance(coin, window_start, window_end, true)
            .await
        {
            return Ok(Some(outcome));
        }
        if let Ok(Some(outcome)) = self
            .get_coin_outcome_binance(coin, window_start, window_end, false)
            .await
        {
            return Ok(Some(outcome));
        }
        // Fall back to CoinGecko
        self.get_coin_outcome_coingecko(coin, window_start, window_end)
            .await
    }

    /// Gets outcome from Binance klines API.
    async fn get_coin_outcome_binance(
        &self,
        coin: &str,
        window_start: chrono::DateTime<Utc>,
        window_end: chrono::DateTime<Utc>,
        use_us: bool,
    ) -> Result<Option<String>> {
        // Convert coin to Binance symbol
        let symbol = match coin.to_uppercase().as_str() {
            "BTC" => {
                if use_us {
                    "BTCUSD"
                } else {
                    "BTCUSDT"
                }
            }
            "ETH" => {
                if use_us {
                    "ETHUSD"
                } else {
                    "ETHUSDT"
                }
            }
            "SOL" => {
                if use_us {
                    "SOLUSD"
                } else {
                    "SOLUSDT"
                }
            }
            "XRP" => {
                if use_us {
                    "XRPUSD"
                } else {
                    "XRPUSDT"
                }
            }
            other => return Err(anyhow!("Unknown coin: {}", other)),
        };

        let start_ms = window_start.timestamp_millis();
        let end_ms = window_end.timestamp_millis();

        let base_url = if use_us {
            "https://api.binance.us"
        } else {
            "https://api.binance.com"
        };

        let url = format!(
            "{}/api/v3/klines?symbol={}&interval=15m&startTime={}&endTime={}&limit=1",
            base_url, symbol, start_ms, end_ms
        );

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Binance API error {}: {}", status, text));
        }

        // Binance kline format: [open_time, open, high, low, close, volume, close_time, ...]
        let klines: Vec<Vec<serde_json::Value>> = response.json().await?;

        if klines.is_empty() {
            debug!(symbol = symbol, "No kline data yet for window");
            return Ok(None);
        }

        let kline = &klines[0];

        // Check if kline is closed (close_time is in the past)
        let close_time_ms = kline
            .get(6)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("Invalid kline close_time"))?;

        let now_ms = Utc::now().timestamp_millis();
        if close_time_ms > now_ms {
            debug!(
                symbol = symbol,
                close_time = close_time_ms,
                "Kline not yet closed"
            );
            return Ok(None);
        }

        // Parse open and close prices
        let open_str = kline
            .get(1)
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Invalid kline open price"))?;
        let close_str = kline
            .get(4)
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Invalid kline close price"))?;

        let open: f64 = open_str.parse()?;
        let close: f64 = close_str.parse()?;

        // Determine outcome: UP if close > open, DOWN otherwise
        let outcome = if close > open {
            "UP".to_string()
        } else {
            "DOWN".to_string()
        };

        debug!(
            symbol = symbol,
            open = open,
            close = close,
            outcome = %outcome,
            source = if use_us { "Binance.US" } else { "Binance" },
            "Determined coin outcome from kline"
        );

        Ok(Some(outcome))
    }

    /// Gets outcome from CoinGecko API (fallback).
    async fn get_coin_outcome_coingecko(
        &self,
        coin: &str,
        window_start: chrono::DateTime<Utc>,
        window_end: chrono::DateTime<Utc>,
    ) -> Result<Option<String>> {
        // Convert coin to CoinGecko ID
        let coin_id = match coin.to_uppercase().as_str() {
            "BTC" => "bitcoin",
            "ETH" => "ethereum",
            "SOL" => "solana",
            "XRP" => "ripple",
            other => return Err(anyhow!("Unknown coin: {}", other)),
        };

        // CoinGecko market_chart/range endpoint
        let from_ts = window_start.timestamp();
        let to_ts = window_end.timestamp();

        let url = format!(
            "https://api.coingecko.com/api/v3/coins/{}/market_chart/range?vs_currency=usd&from={}&to={}",
            coin_id, from_ts, to_ts
        );

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("CoinGecko API error {}: {}", status, text));
        }

        #[derive(Deserialize)]
        struct CoinGeckoResponse {
            prices: Vec<Vec<f64>>, // [[timestamp_ms, price], ...]
        }

        let data: CoinGeckoResponse = response.json().await?;

        if data.prices.len() < 2 {
            debug!(coin = coin, "Insufficient CoinGecko price data");
            return Ok(None);
        }

        // First price is near window_start, last is near window_end
        let open = data.prices.first().and_then(|p| p.get(1).copied());
        let close = data.prices.last().and_then(|p| p.get(1).copied());

        match (open, close) {
            (Some(o), Some(c)) => {
                let outcome = if c > o { "UP" } else { "DOWN" };
                debug!(
                    coin = coin,
                    open = o,
                    close = c,
                    outcome = outcome,
                    "Determined coin outcome from CoinGecko"
                );
                Ok(Some(outcome.to_string()))
            }
            _ => {
                debug!(coin = coin, "Invalid CoinGecko price data");
                Ok(None)
            }
        }
    }

    /// Cleans up stale pending entries.
    async fn cleanup_stale(&self) -> Result<()> {
        let cutoff = Utc::now() - Duration::milliseconds(self.config.max_pending_age_ms);

        // Get very old pending entries and mark them as expired
        let stale = self.repo.get_pending_settlement(100).await?;

        for opp in stale {
            let window_end = opp.window_end.unwrap_or(opp.timestamp);
            if window_end < cutoff {
                warn!(
                    id = opp.id,
                    window_end = %window_end,
                    "Marking stale opportunity as expired"
                );
                self.repo.mark_expired(opp.id).await?;

                let mut stats = self.stats.write().await;
                stats.expired += 1;
                stats.total_processed += 1;
            }
        }

        Ok(())
    }

    /// Processes a single opportunity immediately (for testing/manual settlement).
    ///
    /// # Errors
    /// Returns an error if settlement fails.
    pub async fn settle_by_id(&self, id: i32) -> Result<()> {
        let pending = self.repo.get_pending_settlement(1000).await?;
        let opp = pending
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| anyhow!("Opportunity {} not found or not pending", id))?;

        self.settle_opportunity(&opp).await
    }

    /// Manually settles an opportunity with given outcomes (for testing/backfill).
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub async fn settle_manually(
        &self,
        id: i32,
        coin1_outcome: &str,
        coin2_outcome: &str,
    ) -> Result<()> {
        // Get the opportunity
        let pending = self.repo.get_pending_settlement(1000).await?;
        let opp = pending
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| anyhow!("Opportunity {} not found or not pending", id))?;

        // Determine results
        let leg1_won = opp.leg1_direction == coin1_outcome;
        let leg2_won = opp.leg2_direction == coin2_outcome;

        let trade_result = match (leg1_won, leg2_won) {
            (true, true) => "DOUBLE_WIN",
            (true, false) | (false, true) => "WIN",
            (false, false) => "LOSE",
        };

        let payout = match trade_result {
            "DOUBLE_WIN" => Decimal::TWO,
            "WIN" => Decimal::ONE,
            _ => Decimal::ZERO,
        };
        let fees = payout * self.config.fee_rate;
        let actual_pnl = payout - fees - opp.total_cost;
        let correlation_correct = coin1_outcome == coin2_outcome;

        self.repo
            .settle_opportunity(
                id,
                coin1_outcome,
                coin2_outcome,
                trade_result,
                actual_pnl,
                correlation_correct,
            )
            .await?;

        info!(
            id = id,
            result = trade_result,
            pnl = %actual_pnl,
            "Manually settled opportunity"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn settlement_config_defaults() {
        let config = CrossMarketSettlementConfig::default();
        assert_eq!(config.settlement_delay_ms, 120_000);
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.fee_rate, dec!(0.02));
    }

    #[test]
    fn settlement_stats_win_rate() {
        let mut stats = SettlementStats::default();
        stats.settled = 100;
        stats.wins = 96;
        stats.losses = 4;

        assert!((stats.win_rate() - 0.96).abs() < 0.001);
    }

    #[test]
    fn settlement_stats_correlation_accuracy() {
        let mut stats = SettlementStats::default();
        stats.settled = 100;
        stats.correlation_correct = 85;

        assert!((stats.correlation_accuracy() - 0.85).abs() < 0.001);
    }

    #[test]
    fn settlement_stats_zero_settled() {
        let stats = SettlementStats::default();
        assert_eq!(stats.win_rate(), 0.0);
        assert_eq!(stats.correlation_accuracy(), 0.0);
    }
}
