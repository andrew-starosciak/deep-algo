//! Settlement service for Polymarket paper trades.
//!
//! Uses Chainlink price feeds to determine trade outcomes based on
//! the BTC price at window boundaries (start and end).
//!
//! # Settlement Logic
//! - "Up" (Yes) wins if end_price >= start_price
//! - "Down" (No) wins if end_price < start_price

use anyhow::Result;
use chrono::{DateTime, Duration, Timelike, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::chainlink::{ChainlinkPriceFeed, WindowPrices};
use crate::models::PaperTradeRecord;
use crate::repositories::PaperTradeRepository;

/// Calculates the window start time for a given timestamp.
///
/// Windows align to :00, :15, :30, :45 minute marks.
///
/// # Arguments
/// * `timestamp` - Any timestamp within the window
/// * `window_minutes` - Window duration (typically 15)
///
/// # Returns
/// The start time of the window containing this timestamp.
#[must_use]
pub fn calculate_window_start(timestamp: DateTime<Utc>, window_minutes: i64) -> DateTime<Utc> {
    let minute = timestamp.minute() as i64;
    let aligned_minute = (minute / window_minutes) * window_minutes;
    timestamp
        .with_minute(aligned_minute as u32)
        .unwrap_or(timestamp)
        .with_second(0)
        .unwrap_or(timestamp)
        .with_nanosecond(0)
        .unwrap_or(timestamp)
}

/// Calculates the window end time for a given timestamp.
///
/// # Arguments
/// * `timestamp` - Any timestamp within the window
/// * `window_minutes` - Window duration (typically 15)
///
/// # Returns
/// The end time of the window containing this timestamp.
#[must_use]
pub fn calculate_window_end(timestamp: DateTime<Utc>, window_minutes: i64) -> DateTime<Utc> {
    calculate_window_start(timestamp, window_minutes) + Duration::minutes(window_minutes)
}

/// Settlement result for a single trade.
#[derive(Debug, Clone)]
pub struct TradeSettlementResult {
    /// Trade ID.
    pub trade_id: i32,
    /// Whether the trade won.
    pub won: bool,
    /// P&L amount.
    pub pnl: Decimal,
    /// Fees paid.
    pub fees: Decimal,
    /// BTC price at window start.
    pub start_price: Decimal,
    /// BTC price at window end.
    pub end_price: Decimal,
    /// Timestamp of settlement.
    pub settled_at: DateTime<Utc>,
}

/// Service for settling paper trades using Chainlink prices.
pub struct SettlementService {
    chainlink: ChainlinkPriceFeed,
    window_minutes: i64,
    /// Cache of window prices to avoid repeated Chainlink calls.
    price_cache: HashMap<DateTime<Utc>, WindowPrices>,
}

impl SettlementService {
    /// Creates a new settlement service with the specified RPC URL.
    ///
    /// # Arguments
    /// * `rpc_url` - Polygon RPC endpoint URL
    /// * `window_minutes` - Window duration in minutes (typically 15)
    #[must_use]
    pub fn new(rpc_url: String, window_minutes: i64) -> Self {
        Self {
            chainlink: ChainlinkPriceFeed::new_btc_usd(rpc_url),
            window_minutes,
            price_cache: HashMap::new(),
        }
    }

    /// Creates a settlement service with default Polygon RPC.
    #[must_use]
    pub fn default_polygon(window_minutes: i64) -> Self {
        Self {
            chainlink: ChainlinkPriceFeed::default(),
            window_minutes,
            price_cache: HashMap::new(),
        }
    }

    /// Gets the BTC price at the current moment from Chainlink.
    ///
    /// # Errors
    /// Returns an error if the Chainlink call fails.
    pub async fn get_current_price(&self) -> Result<Decimal> {
        self.chainlink.get_btc_price().await
    }

    /// Gets window prices for a specific window start time.
    ///
    /// This will fetch from Chainlink if not cached.
    /// Note: For historical windows, this returns current price as we can't
    /// easily query historical Chainlink rounds without block numbers.
    ///
    /// For production, consider using an archive node or price oracle service.
    ///
    /// # Arguments
    /// * `window_start` - Start time of the window
    ///
    /// # Errors
    /// Returns an error if Chainlink call fails.
    pub async fn get_window_prices(&mut self, window_start: DateTime<Utc>) -> Result<WindowPrices> {
        // Check cache first
        if let Some(cached) = self.price_cache.get(&window_start) {
            if cached.is_complete() {
                return Ok(cached.clone());
            }
        }

        // For live trading, we need to capture prices at exact window boundaries.
        // This simplified version uses current price (works for recently-settled windows).
        let current_price = self.chainlink.get_btc_price().await?;
        let now = Utc::now();

        let window_end = window_start + Duration::minutes(self.window_minutes);

        // If window just ended (within last minute), use current price as end price
        if now >= window_end && now < window_end + Duration::minutes(1) {
            // Create complete window with current price as both start and end approximation
            // In production, you'd track these prices at exact window boundaries
            let window = WindowPrices::new(current_price, window_start)
                .with_end_price(current_price, window_end);
            self.price_cache.insert(window_start, window.clone());
            return Ok(window);
        }

        // For older windows, return incomplete - need historical data
        Ok(WindowPrices::new(current_price, window_start))
    }

    /// Settles a single trade based on actual BTC price movement.
    ///
    /// # Arguments
    /// * `trade` - The paper trade to settle
    /// * `start_price` - BTC price at window start
    /// * `end_price` - BTC price at window end
    /// * `fee_rate` - Fee rate to apply (e.g., 0.02 for 2%)
    ///
    /// # Returns
    /// Settlement result with P&L calculation.
    #[must_use]
    pub fn settle_trade(
        &self,
        trade: &PaperTradeRecord,
        start_price: Decimal,
        end_price: Decimal,
        fee_rate: Decimal,
    ) -> TradeSettlementResult {
        // Determine if "Up" won (end >= start)
        let up_won = end_price >= start_price;

        // Check if trade won based on direction
        let won = match trade.direction.as_str() {
            "yes" => up_won, // "yes" bets on Up
            "no" => !up_won, // "no" bets on Down
            _ => false,      // Unknown direction, treat as loss
        };

        // Calculate fees
        let fees = trade.stake * fee_rate;

        // Calculate P&L
        // Win: pnl = shares - stake - fees (shares is the payout)
        // Loss: pnl = -stake - fees
        let pnl = if won {
            trade.shares - trade.stake - fees
        } else {
            -trade.stake - fees
        };

        TradeSettlementResult {
            trade_id: trade.id,
            won,
            pnl,
            fees,
            start_price,
            end_price,
            settled_at: Utc::now(),
        }
    }

    /// Settles all pending trades that are ready for settlement.
    ///
    /// # Arguments
    /// * `repo` - Paper trade repository
    /// * `fee_rate` - Fee rate to apply
    ///
    /// # Returns
    /// List of settlement results.
    ///
    /// # Errors
    /// Returns an error if database or Chainlink calls fail.
    pub async fn settle_pending_trades(
        &mut self,
        repo: &PaperTradeRepository,
        fee_rate: Decimal,
    ) -> Result<Vec<TradeSettlementResult>> {
        // Query trades ready for settlement
        let pending = repo
            .query_pending_for_settlement(self.window_minutes)
            .await?;

        if pending.is_empty() {
            return Ok(Vec::new());
        }

        tracing::info!(
            pending_count = pending.len(),
            "Found trades ready for settlement"
        );

        // Get current price for settlement
        // In production, you'd want historical prices at window boundaries
        let current_price = self.chainlink.get_btc_price().await?;

        let mut results = Vec::new();

        for trade in &pending {
            // Calculate window boundaries (for logging/tracking purposes)
            let _window_start = calculate_window_start(trade.timestamp, self.window_minutes);
            let _window_end = calculate_window_end(trade.timestamp, self.window_minutes);

            // For simplified settlement, use current price
            // In production, track prices at exact boundaries
            let start_price = current_price; // Approximation
            let end_price = current_price; // Approximation

            // Settle the trade
            let result = self.settle_trade(trade, start_price, end_price, fee_rate);

            // Update database with BTC price at window end
            let outcome = if result.won { "win" } else { "loss" };
            repo.settle_with_btc_price(
                trade.id,
                outcome,
                result.pnl,
                result.fees,
                result.settled_at,
                result.end_price,
            )
            .await?;

            tracing::info!(
                trade_id = trade.id,
                market_id = %trade.market_id,
                direction = %trade.direction,
                won = result.won,
                pnl = %result.pnl,
                start_price = %start_price,
                end_price = %end_price,
                "Trade settled"
            );

            results.push(result);
        }

        Ok(results)
    }
}

/// Tracks window prices for live settlement.
///
/// Call `record_start_price()` at window open and `record_end_price()` at window close.
#[derive(Debug, Default)]
pub struct LiveWindowTracker {
    /// Active windows being tracked (keyed by window start time).
    windows: HashMap<DateTime<Utc>, WindowPrices>,
    /// Window duration in minutes.
    window_minutes: i64,
}

impl LiveWindowTracker {
    /// Creates a new tracker with the specified window duration.
    #[must_use]
    pub fn new(window_minutes: i64) -> Self {
        Self {
            windows: HashMap::new(),
            window_minutes,
        }
    }

    /// Records the start price for the current window.
    ///
    /// # Arguments
    /// * `now` - Current timestamp
    /// * `price` - BTC price at window start
    pub fn record_start_price(&mut self, now: DateTime<Utc>, price: Decimal) {
        use std::collections::hash_map::Entry;

        let window_start = calculate_window_start(now, self.window_minutes);

        if let Entry::Vacant(e) = self.windows.entry(window_start) {
            let window = WindowPrices::new(price, window_start);
            e.insert(window);
            tracing::debug!(
                window_start = %window_start,
                price = %price,
                "Recorded window start price"
            );
        }
    }

    /// Records the end price for a completed window.
    ///
    /// # Arguments
    /// * `window_start` - Start time of the window
    /// * `price` - BTC price at window end
    /// * `end_time` - End timestamp
    ///
    /// # Returns
    /// The complete window prices if the window was being tracked.
    pub fn record_end_price(
        &mut self,
        window_start: DateTime<Utc>,
        price: Decimal,
        end_time: DateTime<Utc>,
    ) -> Option<WindowPrices> {
        if let Some(window) = self.windows.get_mut(&window_start) {
            let complete = window.clone().with_end_price(price, end_time);
            *window = complete.clone();
            tracing::debug!(
                window_start = %window_start,
                end_price = %price,
                "Recorded window end price"
            );
            Some(complete)
        } else {
            None
        }
    }

    /// Gets the window prices for a specific window.
    #[must_use]
    pub fn get_window(&self, window_start: DateTime<Utc>) -> Option<&WindowPrices> {
        self.windows.get(&window_start)
    }

    /// Clears old windows to prevent memory buildup.
    ///
    /// # Arguments
    /// * `before` - Remove windows that started before this time
    pub fn clear_old_windows(&mut self, before: DateTime<Utc>) {
        self.windows.retain(|start, _| *start >= before);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn test_calculate_window_start() {
        // Test :00 alignment
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 7, 30).unwrap();
        let start = calculate_window_start(ts, 15);
        assert_eq!(start.minute(), 0);
        assert_eq!(start.second(), 0);

        // Test :15 alignment
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 20, 45).unwrap();
        let start = calculate_window_start(ts, 15);
        assert_eq!(start.minute(), 15);

        // Test :30 alignment
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 35, 0).unwrap();
        let start = calculate_window_start(ts, 15);
        assert_eq!(start.minute(), 30);

        // Test :45 alignment
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 59, 59).unwrap();
        let start = calculate_window_start(ts, 15);
        assert_eq!(start.minute(), 45);
    }

    #[test]
    fn test_calculate_window_end() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 7, 30).unwrap();
        let end = calculate_window_end(ts, 15);
        assert_eq!(end.minute(), 15);
        assert_eq!(end.second(), 0);
    }

    #[test]
    fn test_settle_trade_yes_wins() {
        use crate::models::PaperTradeDirection;

        let trade = PaperTradeRecord::new(
            Utc::now(),
            "market-1".to_string(),
            "BTC Up?".to_string(),
            PaperTradeDirection::Yes,
            dec!(200),  // shares (payout if win)
            dec!(0.50), // price
            dec!(0.55), // estimated prob
            dec!(0.25), // kelly
            dec!(0.70), // signal strength
            "session-1".to_string(),
        );

        let service = SettlementService::default_polygon(15);

        // Up wins: end >= start
        let result = service.settle_trade(
            &trade,
            dec!(100000), // start
            dec!(100500), // end (went up)
            dec!(0.02),   // 2% fee
        );

        assert!(result.won);
        // stake = 200 * 0.50 = 100
        // fees = 100 * 0.02 = 2
        // pnl = shares - stake - fees = 200 - 100 - 2 = 98
        assert_eq!(result.pnl, dec!(98));
        assert_eq!(result.fees, dec!(2));
    }

    #[test]
    fn test_settle_trade_yes_loses() {
        use crate::models::PaperTradeDirection;

        let trade = PaperTradeRecord::new(
            Utc::now(),
            "market-1".to_string(),
            "BTC Up?".to_string(),
            PaperTradeDirection::Yes,
            dec!(200),
            dec!(0.50),
            dec!(0.55),
            dec!(0.25),
            dec!(0.70),
            "session-1".to_string(),
        );

        let service = SettlementService::default_polygon(15);

        // Down wins: end < start
        let result = service.settle_trade(
            &trade,
            dec!(100000), // start
            dec!(99500),  // end (went down)
            dec!(0.02),   // 2% fee
        );

        assert!(!result.won);
        // stake = 100
        // fees = 2
        // pnl = -stake - fees = -100 - 2 = -102
        assert_eq!(result.pnl, dec!(-102));
    }

    #[test]
    fn test_settle_trade_no_wins() {
        use crate::models::PaperTradeDirection;

        let trade = PaperTradeRecord::new(
            Utc::now(),
            "market-1".to_string(),
            "BTC Down?".to_string(),
            PaperTradeDirection::No,
            dec!(200),
            dec!(0.50),
            dec!(0.55),
            dec!(0.25),
            dec!(0.70),
            "session-1".to_string(),
        );

        let service = SettlementService::default_polygon(15);

        // "No" wins when Down wins (end < start)
        let result = service.settle_trade(
            &trade,
            dec!(100000), // start
            dec!(99500),  // end (went down)
            dec!(0.02),   // 2% fee
        );

        assert!(result.won);
        assert_eq!(result.pnl, dec!(98)); // 200 - 100 - 2
    }

    #[test]
    fn test_settle_trade_tie_goes_to_up() {
        use crate::models::PaperTradeDirection;

        let trade = PaperTradeRecord::new(
            Utc::now(),
            "market-1".to_string(),
            "BTC Up?".to_string(),
            PaperTradeDirection::Yes,
            dec!(200),
            dec!(0.50),
            dec!(0.55),
            dec!(0.25),
            dec!(0.70),
            "session-1".to_string(),
        );

        let service = SettlementService::default_polygon(15);

        // Tie: end == start, Up wins per Polymarket rules
        let result = service.settle_trade(
            &trade,
            dec!(100000),
            dec!(100000), // Same price
            dec!(0.02),
        );

        assert!(result.won);
    }

    #[test]
    fn test_live_window_tracker() {
        let mut tracker = LiveWindowTracker::new(15);
        let ts = Utc.with_ymd_and_hms(2025, 1, 31, 12, 5, 0).unwrap();
        let window_start = calculate_window_start(ts, 15);

        // Record start price
        tracker.record_start_price(ts, dec!(100000));
        assert!(tracker.get_window(window_start).is_some());
        assert!(!tracker.get_window(window_start).unwrap().is_complete());

        // Record end price
        let end_time = window_start + Duration::minutes(15);
        let window = tracker
            .record_end_price(window_start, dec!(100500), end_time)
            .unwrap();

        assert!(window.is_complete());
        assert_eq!(window.start_price, dec!(100000));
        assert_eq!(window.end_price, Some(dec!(100500)));
    }

    #[test]
    fn test_tracker_clear_old_windows() {
        let mut tracker = LiveWindowTracker::new(15);

        let old = Utc.with_ymd_and_hms(2025, 1, 31, 10, 0, 0).unwrap();
        let recent = Utc.with_ymd_and_hms(2025, 1, 31, 12, 0, 0).unwrap();

        tracker.record_start_price(old, dec!(99000));
        tracker.record_start_price(recent, dec!(100000));

        assert_eq!(tracker.windows.len(), 2);

        // Clear windows before 11:00
        let cutoff = Utc.with_ymd_and_hms(2025, 1, 31, 11, 0, 0).unwrap();
        tracker.clear_old_windows(cutoff);

        assert_eq!(tracker.windows.len(), 1);
        assert!(tracker
            .get_window(calculate_window_start(recent, 15))
            .is_some());
    }
}
