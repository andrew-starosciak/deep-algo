//! Arbitrage data types for Polymarket binary markets.
//!
//! This module provides core data structures for arbitrage detection and execution:
//! - [`L2OrderBook`]: Level 2 order book with incremental update support
//! - [`FillSimulation`]: Result of simulating a fill through the order book
//! - [`ArbitrageOpportunity`]: Detected arbitrage opportunity with profit analysis
//! - [`ArbitragePosition`]: Tracked position with YES/NO leg balancing

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BTreeMap;

/// L2 order book with incremental update support.
///
/// Maintains sorted price levels for bids (descending) and asks (ascending).
/// Supports both full snapshots and incremental delta updates.
#[derive(Debug, Clone)]
pub struct L2OrderBook {
    /// Token ID this order book represents
    pub token_id: String,
    /// Bid levels: price -> size (sorted descending by price)
    pub bids: BTreeMap<Reverse<Decimal>, Decimal>,
    /// Ask levels: price -> size (sorted ascending by price)
    pub asks: BTreeMap<Decimal, Decimal>,
    /// Timestamp of last update in milliseconds
    pub last_update_ms: Option<i64>,
}

impl L2OrderBook {
    /// Creates a new empty order book for the given token.
    #[must_use]
    pub fn new(token_id: String) -> Self {
        Self {
            token_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_update_ms: None,
        }
    }

    /// Returns the best (highest) bid price.
    #[must_use]
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.keys().next().map(|r| r.0)
    }

    /// Returns the best (lowest) ask price.
    #[must_use]
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.keys().next().copied()
    }

    /// Returns the bid-ask spread, if both sides have liquidity.
    #[must_use]
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Returns the mid price, if both sides have liquidity.
    #[must_use]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }

    /// Applies a full snapshot, replacing all existing levels.
    pub fn apply_snapshot(&mut self, bids: Vec<(Decimal, Decimal)>, asks: Vec<(Decimal, Decimal)>) {
        self.bids.clear();
        self.asks.clear();
        for (price, size) in bids {
            if size > Decimal::ZERO {
                self.bids.insert(Reverse(price), size);
            }
        }
        for (price, size) in asks {
            if size > Decimal::ZERO {
                self.asks.insert(price, size);
            }
        }
    }

    /// Applies a delta update to a single price level.
    ///
    /// If size is zero or negative, the level is removed.
    pub fn apply_delta(&mut self, side: Side, price: Decimal, size: Decimal) {
        match side {
            Side::Buy => {
                if size <= Decimal::ZERO {
                    self.bids.remove(&Reverse(price));
                } else {
                    self.bids.insert(Reverse(price), size);
                }
            }
            Side::Sell => {
                if size <= Decimal::ZERO {
                    self.asks.remove(&price);
                } else {
                    self.asks.insert(price, size);
                }
            }
        }
    }

    /// Returns total bid depth (sum of all bid sizes).
    #[must_use]
    pub fn total_bid_depth(&self) -> Decimal {
        self.bids.values().copied().sum()
    }

    /// Returns total ask depth (sum of all ask sizes).
    #[must_use]
    pub fn total_ask_depth(&self) -> Decimal {
        self.asks.values().copied().sum()
    }

    /// Returns the number of bid levels.
    #[must_use]
    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }

    /// Returns the number of ask levels.
    #[must_use]
    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }

    /// Checks if the order book has any liquidity.
    #[must_use]
    pub fn has_liquidity(&self) -> bool {
        !self.bids.is_empty() || !self.asks.is_empty()
    }
}

impl Default for L2OrderBook {
    fn default() -> Self {
        Self::new(String::new())
    }
}

/// Result of walking the order book for a given size.
///
/// Contains fill statistics including VWAP, worst price, and whether
/// sufficient depth exists to fill the entire order.
#[derive(Debug, Clone)]
pub struct FillSimulation {
    /// Amount that could be filled
    pub filled: Decimal,
    /// Total cost of the fill (sum of price * size at each level)
    pub total_cost: Decimal,
    /// Volume-weighted average price
    pub vwap: Decimal,
    /// Worst (least favorable) price encountered
    pub worst_price: Decimal,
    /// Best (most favorable) price encountered
    pub best_price: Decimal,
    /// Whether there was sufficient depth to fill the entire target size
    pub sufficient_depth: bool,
}

/// Detected arbitrage opportunity with full profit analysis.
///
/// Represents a situation where buying both YES and NO tokens
/// costs less than $1.00, guaranteeing profit regardless of outcome.
#[derive(Debug, Clone, Serialize)]
pub struct ArbitrageOpportunity {
    /// Market condition ID
    pub market_id: String,
    /// YES token ID
    pub yes_token_id: String,
    /// NO token ID
    pub no_token_id: String,

    // Prices
    /// Worst fill price for YES side
    pub yes_worst_fill: Decimal,
    /// Worst fill price for NO side
    pub no_worst_fill: Decimal,
    /// Combined pair cost (yes_worst_fill + no_worst_fill)
    pub pair_cost: Decimal,

    // Profit analysis
    /// Gross profit per pair before fees ($1.00 - pair_cost)
    pub gross_profit_per_pair: Decimal,
    /// Expected fee based on E[Fee] = 0.01 * (2 - pair_cost)
    pub expected_fee: Decimal,
    /// Gas cost for both transactions
    pub gas_cost: Decimal,
    /// Net profit per pair after fees and gas
    pub net_profit_per_pair: Decimal,
    /// Return on investment as percentage
    pub roi: Decimal,

    // Sizing
    /// Recommended position size
    pub recommended_size: Decimal,
    /// Total investment required (size * pair_cost)
    pub total_investment: Decimal,
    /// Guaranteed payout (one side always pays $1)
    pub guaranteed_payout: Decimal,

    // Risk metrics
    /// Available depth on YES side
    pub yes_depth: Decimal,
    /// Available depth on NO side
    pub no_depth: Decimal,
    /// Risk score (0.0 = low risk, 1.0 = high risk)
    pub risk_score: f64,

    /// Timestamp when opportunity was detected
    pub detected_at: DateTime<Utc>,
}

/// Paired arbitrage position tracking.
///
/// Tracks the YES and NO legs of an arbitrage position,
/// including imbalance and guaranteed profit calculations.
#[derive(Debug, Clone)]
pub struct ArbitragePosition {
    /// Unique position identifier
    pub id: uuid::Uuid,
    /// Market condition ID
    pub market_id: String,

    // YES leg
    /// Number of YES shares held
    pub yes_shares: Decimal,
    /// Total cost of YES shares
    pub yes_cost: Decimal,
    /// Average price paid for YES shares
    pub yes_avg_price: Decimal,

    // NO leg
    /// Number of NO shares held
    pub no_shares: Decimal,
    /// Total cost of NO shares
    pub no_cost: Decimal,
    /// Average price paid for NO shares
    pub no_avg_price: Decimal,

    // Combined metrics
    /// Cost per pair of YES+NO
    pub pair_cost: Decimal,
    /// Guaranteed payout at settlement
    pub guaranteed_payout: Decimal,
    /// Imbalance between YES and NO shares
    pub imbalance: Decimal,

    /// When the position was opened
    pub opened_at: DateTime<Utc>,
    /// Current position status
    pub status: PositionStatus,
}

impl ArbitragePosition {
    /// Calculates the current pair cost based on actual fills.
    #[must_use]
    pub fn calculate_pair_cost(&self) -> Decimal {
        let min_qty = self.yes_shares.min(self.no_shares);
        if min_qty == Decimal::ZERO {
            return Decimal::MAX;
        }
        (self.yes_cost + self.no_cost) / min_qty
    }

    /// Calculates the guaranteed profit at settlement.
    #[must_use]
    pub fn guaranteed_profit(&self) -> Decimal {
        self.calculate_guaranteed_payout() - (self.yes_cost + self.no_cost)
    }

    /// Calculates the guaranteed payout (minimum of YES and NO shares).
    #[must_use]
    pub fn calculate_guaranteed_payout(&self) -> Decimal {
        self.yes_shares.min(self.no_shares)
    }

    /// Calculates the current imbalance (YES - NO).
    #[must_use]
    pub fn calculate_imbalance(&self) -> Decimal {
        self.yes_shares - self.no_shares
    }

    /// Calculates the imbalance ratio (abs(imbalance) / max shares).
    #[must_use]
    pub fn imbalance_ratio(&self) -> Decimal {
        let max = self.yes_shares.max(self.no_shares);
        if max == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.calculate_imbalance().abs() / max
    }

    /// Returns true if the position is balanced (imbalance within tolerance).
    #[must_use]
    pub fn is_balanced(&self, tolerance: Decimal) -> bool {
        self.calculate_imbalance().abs() <= tolerance
    }
}

/// Position lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionStatus {
    /// Accumulating shares, not yet balanced
    Building,
    /// Balanced and within threshold
    Complete,
    /// Market closed, awaiting payout
    Settling,
    /// Final P&L realized
    Settled,
}

/// Order side (buy or sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Buying (taking from asks)
    Buy,
    /// Selling (taking from bids)
    Sell,
}

/// Order type for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Fill-or-Kill: Must fill entirely or cancel (required for arbitrage)
    FOK,
    /// Fill-and-Kill: Fill what's available, cancel rest (for unwinding)
    FAK,
    /// Good-til-Cancelled: Rests on book until filled or cancelled
    GTC,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_orderbook() -> L2OrderBook {
        let mut book = L2OrderBook::new("test-token".to_string());
        book.apply_snapshot(
            vec![(dec!(0.48), dec!(100)), (dec!(0.47), dec!(200))],
            vec![(dec!(0.50), dec!(150)), (dec!(0.51), dec!(250))],
        );
        book
    }

    #[test]
    fn test_orderbook_new() {
        let book = L2OrderBook::new("token-123".to_string());
        assert_eq!(book.token_id, "token-123");
        assert!(book.bids.is_empty());
        assert!(book.asks.is_empty());
        assert!(book.last_update_ms.is_none());
    }

    #[test]
    fn test_orderbook_best_bid() {
        let book = create_test_orderbook();
        assert_eq!(book.best_bid(), Some(dec!(0.48)));
    }

    #[test]
    fn test_orderbook_best_ask() {
        let book = create_test_orderbook();
        assert_eq!(book.best_ask(), Some(dec!(0.50)));
    }

    #[test]
    fn test_orderbook_spread() {
        let book = create_test_orderbook();
        assert_eq!(book.spread(), Some(dec!(0.02)));
    }

    #[test]
    fn test_orderbook_mid_price() {
        let book = create_test_orderbook();
        assert_eq!(book.mid_price(), Some(dec!(0.49)));
    }

    #[test]
    fn test_orderbook_apply_delta_add() {
        let mut book = create_test_orderbook();
        book.apply_delta(Side::Buy, dec!(0.49), dec!(50));
        assert_eq!(book.best_bid(), Some(dec!(0.49)));
    }

    #[test]
    fn test_orderbook_apply_delta_remove() {
        let mut book = create_test_orderbook();
        book.apply_delta(Side::Buy, dec!(0.48), Decimal::ZERO);
        assert_eq!(book.best_bid(), Some(dec!(0.47)));
    }

    #[test]
    fn test_orderbook_total_depth() {
        let book = create_test_orderbook();
        assert_eq!(book.total_bid_depth(), dec!(300));
        assert_eq!(book.total_ask_depth(), dec!(400));
    }

    #[test]
    fn test_orderbook_levels() {
        let book = create_test_orderbook();
        assert_eq!(book.bid_levels(), 2);
        assert_eq!(book.ask_levels(), 2);
    }

    #[test]
    fn test_orderbook_has_liquidity() {
        let book = create_test_orderbook();
        assert!(book.has_liquidity());

        let empty_book = L2OrderBook::new("empty".to_string());
        assert!(!empty_book.has_liquidity());
    }

    #[test]
    fn test_position_calculations() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test-market".to_string(),
            yes_shares: dec!(100),
            yes_cost: dec!(48),
            yes_avg_price: dec!(0.48),
            no_shares: dec!(100),
            no_cost: dec!(48),
            no_avg_price: dec!(0.48),
            pair_cost: dec!(0.96),
            guaranteed_payout: dec!(100),
            imbalance: Decimal::ZERO,
            opened_at: Utc::now(),
            status: PositionStatus::Complete,
        };

        assert_eq!(position.calculate_pair_cost(), dec!(0.96));
        assert_eq!(position.calculate_guaranteed_payout(), dec!(100));
        assert_eq!(position.guaranteed_profit(), dec!(4)); // 100 - 96
        assert_eq!(position.calculate_imbalance(), Decimal::ZERO);
        assert!(position.is_balanced(dec!(1)));
    }

    #[test]
    fn test_position_imbalance() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test-market".to_string(),
            yes_shares: dec!(110),
            yes_cost: dec!(52.8),
            yes_avg_price: dec!(0.48),
            no_shares: dec!(100),
            no_cost: dec!(48),
            no_avg_price: dec!(0.48),
            pair_cost: dec!(0.96),
            guaranteed_payout: dec!(100),
            imbalance: dec!(10),
            opened_at: Utc::now(),
            status: PositionStatus::Building,
        };

        assert_eq!(position.calculate_imbalance(), dec!(10));
        assert!(!position.is_balanced(dec!(5)));
        assert!(position.is_balanced(dec!(10)));
    }

    // ============================================
    // Edge Case Tests
    // ============================================

    #[test]
    fn test_orderbook_spread_empty_returns_none() {
        let empty_book = L2OrderBook::new("empty".to_string());
        assert!(empty_book.spread().is_none());
    }

    #[test]
    fn test_orderbook_mid_price_empty_returns_none() {
        let empty_book = L2OrderBook::new("empty".to_string());
        assert!(empty_book.mid_price().is_none());
    }

    #[test]
    fn test_orderbook_spread_one_sided_returns_none() {
        let mut book = L2OrderBook::new("one-sided".to_string());
        book.apply_snapshot(vec![(dec!(0.48), dec!(100))], vec![]);
        assert!(book.spread().is_none());

        let mut book2 = L2OrderBook::new("one-sided".to_string());
        book2.apply_snapshot(vec![], vec![(dec!(0.52), dec!(100))]);
        assert!(book2.spread().is_none());
    }

    #[test]
    fn test_orderbook_mid_price_one_sided_returns_none() {
        let mut book = L2OrderBook::new("one-sided".to_string());
        book.apply_snapshot(vec![(dec!(0.48), dec!(100))], vec![]);
        assert!(book.mid_price().is_none());
    }

    #[test]
    fn test_orderbook_apply_snapshot_filters_zero_size() {
        let mut book = L2OrderBook::new("test".to_string());
        book.apply_snapshot(
            vec![(dec!(0.48), Decimal::ZERO), (dec!(0.47), dec!(100))],
            vec![(dec!(0.52), dec!(100)), (dec!(0.53), Decimal::ZERO)],
        );
        assert_eq!(book.bid_levels(), 1);
        assert_eq!(book.ask_levels(), 1);
    }

    #[test]
    fn test_orderbook_apply_delta_negative_size_removes() {
        let mut book = create_test_orderbook();
        book.apply_delta(Side::Buy, dec!(0.48), dec!(-1));
        assert_eq!(book.best_bid(), Some(dec!(0.47)));

        book.apply_delta(Side::Sell, dec!(0.50), dec!(-1));
        assert_eq!(book.best_ask(), Some(dec!(0.51)));
    }

    #[test]
    fn test_position_pair_cost_zero_shares_returns_max() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test".to_string(),
            yes_shares: Decimal::ZERO,
            yes_cost: Decimal::ZERO,
            yes_avg_price: Decimal::ZERO,
            no_shares: Decimal::ZERO,
            no_cost: Decimal::ZERO,
            no_avg_price: Decimal::ZERO,
            pair_cost: Decimal::ZERO,
            guaranteed_payout: Decimal::ZERO,
            imbalance: Decimal::ZERO,
            opened_at: Utc::now(),
            status: PositionStatus::Building,
        };
        assert_eq!(position.calculate_pair_cost(), Decimal::MAX);
    }

    #[test]
    fn test_position_imbalance_ratio_zero_shares_returns_zero() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test".to_string(),
            yes_shares: Decimal::ZERO,
            yes_cost: Decimal::ZERO,
            yes_avg_price: Decimal::ZERO,
            no_shares: Decimal::ZERO,
            no_cost: Decimal::ZERO,
            no_avg_price: Decimal::ZERO,
            pair_cost: Decimal::ZERO,
            guaranteed_payout: Decimal::ZERO,
            imbalance: Decimal::ZERO,
            opened_at: Utc::now(),
            status: PositionStatus::Building,
        };
        assert_eq!(position.imbalance_ratio(), Decimal::ZERO);
    }

    #[test]
    fn test_position_imbalance_ratio_calculation() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test".to_string(),
            yes_shares: dec!(100),
            yes_cost: dec!(48),
            yes_avg_price: dec!(0.48),
            no_shares: dec!(80),
            no_cost: dec!(38.4),
            no_avg_price: dec!(0.48),
            pair_cost: dec!(0.96),
            guaranteed_payout: dec!(80),
            imbalance: dec!(20),
            opened_at: Utc::now(),
            status: PositionStatus::Building,
        };
        // imbalance_ratio = |100 - 80| / max(100, 80) = 20 / 100 = 0.2
        assert_eq!(position.imbalance_ratio(), dec!(0.2));
    }

    #[test]
    fn test_position_guaranteed_payout_uses_min_shares() {
        let position = ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: "test".to_string(),
            yes_shares: dec!(100),
            yes_cost: dec!(48),
            yes_avg_price: dec!(0.48),
            no_shares: dec!(75),
            no_cost: dec!(36),
            no_avg_price: dec!(0.48),
            pair_cost: dec!(0.96),
            guaranteed_payout: dec!(75),
            imbalance: dec!(25),
            opened_at: Utc::now(),
            status: PositionStatus::Building,
        };
        // Guaranteed payout is min(YES, NO) = min(100, 75) = 75
        assert_eq!(position.calculate_guaranteed_payout(), dec!(75));
    }

    #[test]
    fn test_orderbook_default() {
        let book = L2OrderBook::default();
        assert!(book.token_id.is_empty());
        assert!(!book.has_liquidity());
    }
}
