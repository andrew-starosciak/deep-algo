//! Cross-exchange arbitrage opportunity detection.
//!
//! This module detects arbitrage opportunities between Kalshi and Polymarket
//! by analyzing order books from both exchanges and calculating net profit
//! after fees.

use algo_trade_kalshi::Orderbook as KalshiOrderbook;
use algo_trade_polymarket::arbitrage::types::L2OrderBook;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use crate::fees::FeeCalculator;
use crate::types::{MatchedMarket, Side};

// =============================================================================
// Detection Configuration
// =============================================================================

/// Configuration for arbitrage detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorConfig {
    /// Minimum net edge required to consider an opportunity (as decimal).
    pub min_net_edge: Decimal,

    /// Minimum gross edge required (as decimal).
    pub min_gross_edge: Decimal,

    /// Minimum size (in shares/contracts) for an opportunity.
    pub min_size: Decimal,

    /// Maximum size to consider for a single opportunity.
    pub max_size: Decimal,

    /// Maximum slippage tolerance (as decimal).
    pub max_slippage: Decimal,

    /// Whether to require both sides to have sufficient depth.
    pub require_full_depth: bool,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            min_net_edge: dec!(0.005),  // 0.5% minimum net profit
            min_gross_edge: dec!(0.01), // 1% minimum gross profit
            min_size: dec!(10),
            max_size: dec!(1000),
            max_slippage: dec!(0.02), // 2% maximum slippage
            require_full_depth: true,
        }
    }
}

impl DetectorConfig {
    /// Creates a conservative configuration for lower risk.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            min_net_edge: dec!(0.01), // 1% minimum
            min_gross_edge: dec!(0.015),
            min_size: dec!(10),
            max_size: dec!(100),
            max_slippage: dec!(0.01),
            require_full_depth: true,
        }
    }

    /// Creates an aggressive configuration for more opportunities.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            min_net_edge: dec!(0.003), // 0.3% minimum
            min_gross_edge: dec!(0.008),
            min_size: dec!(5),
            max_size: dec!(2000),
            max_slippage: dec!(0.03),
            require_full_depth: false,
        }
    }

    /// Sets the minimum net edge.
    #[must_use]
    pub fn with_min_net_edge(mut self, edge: Decimal) -> Self {
        self.min_net_edge = edge;
        self
    }

    /// Sets the minimum size.
    #[must_use]
    pub fn with_min_size(mut self, size: Decimal) -> Self {
        self.min_size = size;
        self
    }

    /// Sets the maximum size.
    #[must_use]
    pub fn with_max_size(mut self, size: Decimal) -> Self {
        self.max_size = size;
        self
    }
}

// =============================================================================
// Cross-Exchange Opportunity
// =============================================================================

/// A detected cross-exchange arbitrage opportunity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossExchangeOpportunity {
    /// The matched market this opportunity is for.
    pub matched_market: MatchedMarket,

    // Kalshi side
    /// Side to buy on Kalshi (YES or NO).
    pub kalshi_side: Side,
    /// Kalshi price in cents (1-99).
    pub kalshi_price: Decimal,
    /// Available size on Kalshi (contracts).
    pub kalshi_size: u32,

    // Polymarket side
    /// Side to buy on Polymarket (opposite of Kalshi).
    pub polymarket_side: Side,
    /// Polymarket price in dollars (0.01-0.99).
    pub polymarket_price: Decimal,
    /// Available size on Polymarket (shares).
    pub polymarket_size: Decimal,

    // Cost analysis
    /// Combined cost to buy both sides (should be < $1.00).
    pub combined_cost: Decimal,
    /// Gross edge before fees ($1.00 - combined_cost).
    pub gross_edge: Decimal,
    /// Gross edge as percentage.
    pub gross_edge_pct: Decimal,

    // Fee breakdown
    /// Kalshi trading fee.
    pub kalshi_fee: Decimal,
    /// Polymarket total fee.
    pub polymarket_fee: Decimal,

    // Net profit
    /// Net edge after all fees.
    pub net_edge: Decimal,
    /// Net edge as percentage.
    pub net_edge_pct: Decimal,

    // Sizing
    /// Maximum tradeable size (limited by smaller book).
    pub max_size: Decimal,
    /// Expected total profit (net_edge * max_size).
    pub expected_profit: Decimal,

    /// When the opportunity was detected.
    pub detected_at: DateTime<Utc>,
}

impl CrossExchangeOpportunity {
    /// Returns true if this opportunity is profitable after fees.
    #[must_use]
    pub fn is_profitable(&self) -> bool {
        self.net_edge > Decimal::ZERO
    }

    /// Returns the return on investment as a percentage.
    #[must_use]
    pub fn roi_pct(&self) -> Decimal {
        if self.combined_cost == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.net_edge / self.combined_cost * dec!(100)
    }

    /// Returns the time until market settlement.
    #[must_use]
    pub fn time_to_settlement(&self) -> chrono::Duration {
        self.matched_market.time_to_settlement()
    }

    /// Returns true if the opportunity is still valid (market tradeable).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.matched_market.is_tradeable() && self.is_profitable()
    }
}

// =============================================================================
// Cross-Exchange Detector
// =============================================================================

/// Detects arbitrage opportunities across Kalshi and Polymarket.
#[derive(Debug)]
pub struct CrossExchangeDetector {
    config: DetectorConfig,
    fee_calculator: FeeCalculator,
}

impl CrossExchangeDetector {
    /// Creates a new detector with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: DetectorConfig::default(),
            fee_calculator: FeeCalculator::new(),
        }
    }

    /// Creates a new detector with custom configuration.
    #[must_use]
    pub fn with_config(config: DetectorConfig) -> Self {
        Self {
            config,
            fee_calculator: FeeCalculator::new(),
        }
    }

    /// Creates a detector with custom configuration and fee calculator.
    #[must_use]
    pub fn with_config_and_fees(config: DetectorConfig, fee_calculator: FeeCalculator) -> Self {
        Self {
            config,
            fee_calculator,
        }
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &DetectorConfig {
        &self.config
    }

    /// Returns the fee calculator.
    #[must_use]
    pub fn fee_calculator(&self) -> &FeeCalculator {
        &self.fee_calculator
    }

    /// Detects arbitrage opportunities for a matched market.
    ///
    /// Checks all four possible combinations:
    /// 1. Kalshi YES + Polymarket NO
    /// 2. Kalshi NO + Polymarket YES
    /// 3. (For Up/Down markets) Kalshi YES + Polymarket Down
    /// 4. (For Up/Down markets) Kalshi NO + Polymarket Up
    ///
    /// # Arguments
    /// * `matched` - The matched market to analyze
    /// * `kalshi_book` - Kalshi orderbook
    /// * `poly_yes_book` - Polymarket YES/Up token orderbook
    /// * `poly_no_book` - Polymarket NO/Down token orderbook
    ///
    /// # Returns
    /// The best arbitrage opportunity if one exists above threshold.
    #[must_use]
    pub fn detect(
        &self,
        matched: &MatchedMarket,
        kalshi_book: &KalshiOrderbook,
        poly_yes_book: &L2OrderBook,
        poly_no_book: &L2OrderBook,
    ) -> Option<CrossExchangeOpportunity> {
        trace!(
            kalshi_ticker = %matched.kalshi_ticker,
            polymarket_id = %matched.polymarket_condition_id,
            "Checking for cross-exchange arbitrage"
        );

        // Check opportunity 1: Buy Kalshi YES + Polymarket NO
        let opp1 = self.check_opportunity(
            matched,
            Side::Yes,
            kalshi_book.best_yes_ask()?,
            kalshi_book.yes_ask_depth(),
            Side::No,
            poly_no_book.best_ask()?,
            poly_no_book.total_ask_depth(),
        );

        // Check opportunity 2: Buy Kalshi NO + Polymarket YES
        let opp2 = self.check_opportunity(
            matched,
            Side::No,
            kalshi_book.best_no_ask()?,
            kalshi_book.yes_bid_depth(), // NO depth = inverse of YES bid
            Side::Yes,
            poly_yes_book.best_ask()?,
            poly_yes_book.total_ask_depth(),
        );

        // Return the better opportunity (if any meet threshold)
        let best = match (opp1, opp2) {
            (Some(o1), Some(o2)) => {
                if o1.net_edge >= o2.net_edge {
                    Some(o1)
                } else {
                    Some(o2)
                }
            }
            (Some(o), None) | (None, Some(o)) => Some(o),
            (None, None) => None,
        };

        if let Some(ref opp) = best {
            info!(
                kalshi_ticker = %matched.kalshi_ticker,
                kalshi_side = %opp.kalshi_side,
                kalshi_price = %opp.kalshi_price,
                polymarket_side = %opp.polymarket_side,
                polymarket_price = %opp.polymarket_price,
                combined_cost = %opp.combined_cost,
                gross_edge_pct = %opp.gross_edge_pct,
                net_edge_pct = %opp.net_edge_pct,
                max_size = %opp.max_size,
                expected_profit = %opp.expected_profit,
                "Cross-exchange arbitrage opportunity detected"
            );
        }

        best
    }

    /// Checks a specific arbitrage opportunity.
    #[allow(clippy::too_many_arguments)]
    fn check_opportunity(
        &self,
        matched: &MatchedMarket,
        kalshi_side: Side,
        kalshi_price_cents: u32,
        kalshi_depth: u32,
        polymarket_side: Side,
        polymarket_price: Decimal,
        polymarket_depth: Decimal,
    ) -> Option<CrossExchangeOpportunity> {
        // Convert Kalshi cents to dollars
        let kalshi_price_dollars = Decimal::from(kalshi_price_cents) / dec!(100);

        // Calculate combined cost
        let combined_cost = kalshi_price_dollars + polymarket_price;

        // Check if there's an arbitrage (cost < $1)
        if combined_cost >= Decimal::ONE {
            trace!(
                kalshi_side = %kalshi_side,
                kalshi_price = %kalshi_price_dollars,
                polymarket_side = %polymarket_side,
                polymarket_price = %polymarket_price,
                combined_cost = %combined_cost,
                "No arbitrage - combined cost >= $1.00"
            );
            return None;
        }

        // Calculate gross edge
        let gross_edge = Decimal::ONE - combined_cost;
        let gross_edge_pct = gross_edge * dec!(100);

        if gross_edge < self.config.min_gross_edge {
            trace!(
                gross_edge_pct = %gross_edge_pct,
                min_gross_edge_pct = %(self.config.min_gross_edge * dec!(100)),
                "Gross edge below threshold"
            );
            return None;
        }

        // Calculate max size (limited by smaller depth)
        let kalshi_size_shares = Decimal::from(kalshi_depth);
        let max_size = kalshi_size_shares
            .min(polymarket_depth)
            .min(self.config.max_size);

        if max_size < self.config.min_size {
            trace!(
                max_size = %max_size,
                min_size = %self.config.min_size,
                "Size below minimum"
            );
            return None;
        }

        // Check depth requirements
        if self.config.require_full_depth
            && (kalshi_size_shares < self.config.min_size
                || polymarket_depth < self.config.min_size)
        {
            trace!(
                kalshi_depth = %kalshi_size_shares,
                polymarket_depth = %polymarket_depth,
                "Insufficient depth"
            );
            return None;
        }

        // Calculate fees
        let fees = self.fee_calculator.calculate_arbitrage_fees(
            Decimal::from(kalshi_price_cents),
            max_size,
            polymarket_price,
            max_size,
        );

        // Calculate net edge
        let net_edge = gross_edge - fees.total_fee / max_size;
        let net_edge_pct = net_edge * dec!(100);

        if net_edge < self.config.min_net_edge {
            debug!(
                gross_edge_pct = %gross_edge_pct,
                net_edge_pct = %net_edge_pct,
                total_fees = %fees.total_fee,
                "Net edge below threshold after fees"
            );
            return None;
        }

        // Calculate expected profit
        let expected_profit = net_edge * max_size;

        Some(CrossExchangeOpportunity {
            matched_market: matched.clone(),
            kalshi_side,
            kalshi_price: Decimal::from(kalshi_price_cents),
            kalshi_size: kalshi_depth,
            polymarket_side,
            polymarket_price,
            polymarket_size: polymarket_depth,
            combined_cost,
            gross_edge,
            gross_edge_pct,
            kalshi_fee: fees.kalshi_fee,
            polymarket_fee: fees.total_polymarket_fee(),
            net_edge,
            net_edge_pct,
            max_size,
            expected_profit,
            detected_at: Utc::now(),
        })
    }

    /// Calculates the theoretical maximum edge without fees.
    ///
    /// Useful for understanding the opportunity quality before fees.
    #[must_use]
    pub fn calculate_theoretical_edge(
        kalshi_price_cents: u32,
        polymarket_price: Decimal,
    ) -> Option<Decimal> {
        let kalshi_price_dollars = Decimal::from(kalshi_price_cents) / dec!(100);
        let combined = kalshi_price_dollars + polymarket_price;

        if combined < Decimal::ONE {
            Some(Decimal::ONE - combined)
        } else {
            None
        }
    }

    /// Checks if prices create an arbitrage opportunity (before fees).
    #[must_use]
    pub fn is_arbitrage_possible(kalshi_price_cents: u32, polymarket_price: Decimal) -> bool {
        let kalshi_price_dollars = Decimal::from(kalshi_price_cents) / dec!(100);
        kalshi_price_dollars + polymarket_price < Decimal::ONE
    }

    /// Estimates the break-even size for a given opportunity.
    ///
    /// Returns the minimum size needed to cover fixed costs.
    #[must_use]
    pub fn estimate_break_even_size(&self, gross_edge: Decimal, fixed_costs: Decimal) -> Decimal {
        if gross_edge <= Decimal::ZERO {
            return Decimal::MAX;
        }
        fixed_costs / gross_edge
    }
}

impl Default for CrossExchangeDetector {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Detection Result Summary
// =============================================================================

/// Summary of detection results for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectionSummary {
    /// Total markets scanned.
    pub markets_scanned: u32,
    /// Opportunities found meeting gross edge threshold.
    pub gross_opportunities: u32,
    /// Opportunities found meeting net edge threshold.
    pub net_opportunities: u32,
    /// Best opportunity found (if any).
    pub best_opportunity: Option<OpportunitySummary>,
    /// Total potential profit across all opportunities.
    pub total_potential_profit: Decimal,
}

/// Summary of a single opportunity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpportunitySummary {
    /// Kalshi ticker.
    pub kalshi_ticker: String,
    /// Net edge percentage.
    pub net_edge_pct: Decimal,
    /// Expected profit.
    pub expected_profit: Decimal,
    /// Maximum size.
    pub max_size: Decimal,
}

impl From<&CrossExchangeOpportunity> for OpportunitySummary {
    fn from(opp: &CrossExchangeOpportunity) -> Self {
        Self {
            kalshi_ticker: opp.matched_market.kalshi_ticker.clone(),
            net_edge_pct: opp.net_edge_pct,
            expected_profit: opp.expected_profit,
            max_size: opp.max_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_kalshi::PriceLevel;
    use std::cmp::Reverse;
    use std::collections::BTreeMap;

    // ==================== Helper Functions ====================

    fn create_matched_market() -> MatchedMarket {
        MatchedMarket::new(
            "KXBTC-TEST-B100000".to_string(),
            "0xtest123".to_string(),
            "yes-token".to_string(),
            "no-token".to_string(),
            "BTC".to_string(),
            dec!(100000),
            Utc::now() + chrono::Duration::hours(1),
            0.95,
        )
    }

    fn create_kalshi_orderbook(
        yes_bid_price: u32,
        yes_bid_depth: u32,
        yes_ask_price: u32,
        yes_ask_depth: u32,
    ) -> KalshiOrderbook {
        KalshiOrderbook {
            ticker: "KXBTC-TEST".to_string(),
            yes_bids: vec![PriceLevel {
                price: yes_bid_price,
                count: yes_bid_depth,
            }],
            yes_asks: vec![PriceLevel {
                price: yes_ask_price,
                count: yes_ask_depth,
            }],
            timestamp: Utc::now(),
        }
    }

    fn create_polymarket_orderbook(
        bid_price: Decimal,
        ask_price: Decimal,
        depth: Decimal,
    ) -> L2OrderBook {
        let mut book = L2OrderBook::new("test-token".to_string());
        let mut bids = BTreeMap::new();
        let mut asks = BTreeMap::new();

        bids.insert(Reverse(bid_price), depth);
        asks.insert(ask_price, depth);

        book.bids = bids;
        book.asks = asks;
        book
    }

    // ==================== DetectorConfig Tests ====================

    #[test]
    fn test_detector_config_default() {
        let config = DetectorConfig::default();

        assert_eq!(config.min_net_edge, dec!(0.005));
        assert_eq!(config.min_gross_edge, dec!(0.01));
        assert_eq!(config.min_size, dec!(10));
    }

    #[test]
    fn test_detector_config_conservative() {
        let config = DetectorConfig::conservative();

        assert_eq!(config.min_net_edge, dec!(0.01));
        assert_eq!(config.max_size, dec!(100));
    }

    #[test]
    fn test_detector_config_aggressive() {
        let config = DetectorConfig::aggressive();

        assert_eq!(config.min_net_edge, dec!(0.003));
        assert!(!config.require_full_depth);
    }

    #[test]
    fn test_detector_config_builder() {
        let config = DetectorConfig::default()
            .with_min_net_edge(dec!(0.02))
            .with_min_size(dec!(50))
            .with_max_size(dec!(500));

        assert_eq!(config.min_net_edge, dec!(0.02));
        assert_eq!(config.min_size, dec!(50));
        assert_eq!(config.max_size, dec!(500));
    }

    // ==================== Static Method Tests ====================

    #[test]
    fn test_is_arbitrage_possible_yes() {
        // Kalshi 46 cents + Poly 52 cents = 98 cents < $1
        assert!(CrossExchangeDetector::is_arbitrage_possible(46, dec!(0.52)));
    }

    #[test]
    fn test_is_arbitrage_possible_no() {
        // Kalshi 55 cents + Poly 50 cents = $1.05 >= $1
        assert!(!CrossExchangeDetector::is_arbitrage_possible(
            55,
            dec!(0.50)
        ));
    }

    #[test]
    fn test_is_arbitrage_possible_boundary() {
        // Exactly $1 = no arbitrage
        assert!(!CrossExchangeDetector::is_arbitrage_possible(
            50,
            dec!(0.50)
        ));
    }

    #[test]
    fn test_calculate_theoretical_edge() {
        // 46 + 52 = 98, edge = 2 cents
        let edge = CrossExchangeDetector::calculate_theoretical_edge(46, dec!(0.52));
        assert_eq!(edge, Some(dec!(0.02)));
    }

    #[test]
    fn test_calculate_theoretical_edge_no_arb() {
        let edge = CrossExchangeDetector::calculate_theoretical_edge(55, dec!(0.50));
        assert!(edge.is_none());
    }

    #[test]
    fn test_calculate_theoretical_edge_large() {
        // 40 + 55 = 95, edge = 5 cents
        let edge = CrossExchangeDetector::calculate_theoretical_edge(40, dec!(0.55));
        assert_eq!(edge, Some(dec!(0.05)));
    }

    // ==================== Opportunity Detection Tests ====================

    #[test]
    fn test_detect_opportunity_found() {
        let config = DetectorConfig::default()
            .with_min_net_edge(dec!(0.001)) // Very low threshold for testing
            .with_min_size(dec!(1));

        let detector = CrossExchangeDetector::with_config(config);
        let matched = create_matched_market();

        // Kalshi YES ask at 45 cents, NO ask at 55 cents (implicit)
        let kalshi_book = create_kalshi_orderbook(44, 100, 45, 100);

        // Polymarket: YES ask at 0.52, NO ask at 0.48
        let poly_yes_book = create_polymarket_orderbook(dec!(0.51), dec!(0.52), dec!(100));
        let poly_no_book = create_polymarket_orderbook(dec!(0.47), dec!(0.48), dec!(100));

        // Check: Kalshi YES (45c) + Poly NO (48c) = 93c < $1 -> 7% gross edge
        let opportunity = detector.detect(&matched, &kalshi_book, &poly_yes_book, &poly_no_book);

        assert!(opportunity.is_some());
        let opp = opportunity.unwrap();

        // Should buy Kalshi YES and Poly NO
        assert_eq!(opp.kalshi_side, Side::Yes);
        assert_eq!(opp.polymarket_side, Side::No);
        assert!(opp.is_profitable());
    }

    #[test]
    fn test_detect_no_opportunity_high_cost() {
        let detector = CrossExchangeDetector::new();
        let matched = create_matched_market();

        // Kalshi YES ask at 55 cents, YES bid at 54 cents
        // This means NO ask = 100 - 54 = 46 cents
        let kalshi_book = create_kalshi_orderbook(54, 100, 55, 100);

        // Polymarket YES ask at 55 cents, NO ask at 55 cents
        // Kalshi YES (55c) + Poly NO (55c) = 110c > $1 (no arb)
        // Kalshi NO (46c) + Poly YES (55c) = 101c > $1 (no arb)
        let poly_yes_book = create_polymarket_orderbook(dec!(0.54), dec!(0.55), dec!(100));
        let poly_no_book = create_polymarket_orderbook(dec!(0.54), dec!(0.55), dec!(100));

        let opportunity = detector.detect(&matched, &kalshi_book, &poly_yes_book, &poly_no_book);
        assert!(opportunity.is_none());
    }

    #[test]
    fn test_detect_below_min_size() {
        let config = DetectorConfig::default().with_min_size(dec!(1000));
        let detector = CrossExchangeDetector::with_config(config);
        let matched = create_matched_market();

        let kalshi_book = create_kalshi_orderbook(44, 50, 45, 50); // Only 50 contracts
        let poly_yes_book = create_polymarket_orderbook(dec!(0.51), dec!(0.52), dec!(50));
        let poly_no_book = create_polymarket_orderbook(dec!(0.47), dec!(0.48), dec!(50));

        let opportunity = detector.detect(&matched, &kalshi_book, &poly_yes_book, &poly_no_book);
        assert!(opportunity.is_none());
    }

    #[test]
    fn test_detect_below_net_edge_threshold() {
        let config = DetectorConfig::default()
            .with_min_net_edge(dec!(0.10)) // 10% minimum (very high)
            .with_min_size(dec!(1));

        let detector = CrossExchangeDetector::with_config(config);
        let matched = create_matched_market();

        // 45 + 52 = 97c, 3% gross edge
        let kalshi_book = create_kalshi_orderbook(44, 100, 45, 100);
        let poly_yes_book = create_polymarket_orderbook(dec!(0.51), dec!(0.52), dec!(100));
        let poly_no_book = create_polymarket_orderbook(dec!(0.51), dec!(0.52), dec!(100));

        let opportunity = detector.detect(&matched, &kalshi_book, &poly_yes_book, &poly_no_book);
        assert!(opportunity.is_none()); // 3% - fees < 10%
    }

    // ==================== Opportunity Properties Tests ====================

    #[test]
    fn test_opportunity_is_profitable() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched,
            kalshi_side: Side::Yes,
            kalshi_price: dec!(45),
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: dec!(0.48),
            polymarket_size: dec!(100),
            combined_cost: dec!(0.93),
            gross_edge: dec!(0.07),
            gross_edge_pct: dec!(7.0),
            kalshi_fee: dec!(0.32),
            polymarket_fee: dec!(1.0),
            net_edge: dec!(0.05),
            net_edge_pct: dec!(5.0),
            max_size: dec!(100),
            expected_profit: dec!(5.0),
            detected_at: Utc::now(),
        };

        assert!(opp.is_profitable());
        assert!(opp.is_valid());
    }

    #[test]
    fn test_opportunity_not_profitable() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched,
            kalshi_side: Side::Yes,
            kalshi_price: dec!(50),
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: dec!(0.50),
            polymarket_size: dec!(100),
            combined_cost: dec!(1.00),
            gross_edge: dec!(0.00),
            gross_edge_pct: dec!(0.0),
            kalshi_fee: dec!(0.35),
            polymarket_fee: dec!(1.0),
            net_edge: dec!(-0.0135),
            net_edge_pct: dec!(-1.35),
            max_size: dec!(100),
            expected_profit: dec!(-1.35),
            detected_at: Utc::now(),
        };

        assert!(!opp.is_profitable());
    }

    #[test]
    fn test_opportunity_roi() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched,
            kalshi_side: Side::Yes,
            kalshi_price: dec!(45),
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: dec!(0.50),
            polymarket_size: dec!(100),
            combined_cost: dec!(0.95),
            gross_edge: dec!(0.05),
            gross_edge_pct: dec!(5.0),
            kalshi_fee: dec!(0.32),
            polymarket_fee: dec!(1.0),
            net_edge: dec!(0.03),
            net_edge_pct: dec!(3.0),
            max_size: dec!(100),
            expected_profit: dec!(3.0),
            detected_at: Utc::now(),
        };

        // ROI = 0.03 / 0.95 * 100 = ~3.16%
        let roi = opp.roi_pct();
        assert!(roi > dec!(3) && roi < dec!(4));
    }

    #[test]
    fn test_opportunity_roi_zero_cost() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched,
            kalshi_side: Side::Yes,
            kalshi_price: Decimal::ZERO,
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: Decimal::ZERO,
            polymarket_size: dec!(100),
            combined_cost: Decimal::ZERO,
            gross_edge: Decimal::ONE,
            gross_edge_pct: dec!(100),
            kalshi_fee: Decimal::ZERO,
            polymarket_fee: Decimal::ZERO,
            net_edge: Decimal::ONE,
            net_edge_pct: dec!(100),
            max_size: dec!(100),
            expected_profit: dec!(100),
            detected_at: Utc::now(),
        };

        assert_eq!(opp.roi_pct(), Decimal::ZERO);
    }

    // ==================== Break Even Tests ====================

    #[test]
    fn test_estimate_break_even_size() {
        let detector = CrossExchangeDetector::new();

        // 5% gross edge, $10 fixed costs
        let break_even = detector.estimate_break_even_size(dec!(0.05), dec!(10));

        // Need $10 / $0.05 = 200 units to break even
        assert_eq!(break_even, dec!(200));
    }

    #[test]
    fn test_estimate_break_even_size_zero_edge() {
        let detector = CrossExchangeDetector::new();

        let break_even = detector.estimate_break_even_size(Decimal::ZERO, dec!(10));
        assert_eq!(break_even, Decimal::MAX);
    }

    // ==================== Summary Tests ====================

    #[test]
    fn test_opportunity_summary_from_opportunity() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched.clone(),
            kalshi_side: Side::Yes,
            kalshi_price: dec!(45),
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: dec!(0.48),
            polymarket_size: dec!(100),
            combined_cost: dec!(0.93),
            gross_edge: dec!(0.07),
            gross_edge_pct: dec!(7.0),
            kalshi_fee: dec!(0.32),
            polymarket_fee: dec!(1.0),
            net_edge: dec!(0.05),
            net_edge_pct: dec!(5.0),
            max_size: dec!(100),
            expected_profit: dec!(5.0),
            detected_at: Utc::now(),
        };

        let summary = OpportunitySummary::from(&opp);

        assert_eq!(summary.kalshi_ticker, matched.kalshi_ticker);
        assert_eq!(summary.net_edge_pct, dec!(5.0));
        assert_eq!(summary.expected_profit, dec!(5.0));
        assert_eq!(summary.max_size, dec!(100));
    }

    #[test]
    fn test_detection_summary_default() {
        let summary = DetectionSummary::default();

        assert_eq!(summary.markets_scanned, 0);
        assert_eq!(summary.gross_opportunities, 0);
        assert_eq!(summary.net_opportunities, 0);
        assert!(summary.best_opportunity.is_none());
        assert_eq!(summary.total_potential_profit, Decimal::ZERO);
    }

    // ==================== Serialization Tests ====================

    #[test]
    fn test_detector_config_serialization() {
        let config = DetectorConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DetectorConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.min_net_edge, deserialized.min_net_edge);
        assert_eq!(config.min_size, deserialized.min_size);
    }

    #[test]
    fn test_opportunity_serialization() {
        let matched = create_matched_market();
        let opp = CrossExchangeOpportunity {
            matched_market: matched,
            kalshi_side: Side::Yes,
            kalshi_price: dec!(45),
            kalshi_size: 100,
            polymarket_side: Side::No,
            polymarket_price: dec!(0.48),
            polymarket_size: dec!(100),
            combined_cost: dec!(0.93),
            gross_edge: dec!(0.07),
            gross_edge_pct: dec!(7.0),
            kalshi_fee: dec!(0.32),
            polymarket_fee: dec!(1.0),
            net_edge: dec!(0.05),
            net_edge_pct: dec!(5.0),
            max_size: dec!(100),
            expected_profit: dec!(5.0),
            detected_at: Utc::now(),
        };

        let json = serde_json::to_string(&opp).unwrap();
        let deserialized: CrossExchangeOpportunity = serde_json::from_str(&json).unwrap();

        assert_eq!(opp.kalshi_side, deserialized.kalshi_side);
        assert_eq!(opp.net_edge, deserialized.net_edge);
        assert_eq!(opp.expected_profit, deserialized.expected_profit);
    }

    #[test]
    fn test_detection_summary_serialization() {
        let summary = DetectionSummary {
            markets_scanned: 10,
            gross_opportunities: 3,
            net_opportunities: 1,
            best_opportunity: Some(OpportunitySummary {
                kalshi_ticker: "KXBTC-TEST".to_string(),
                net_edge_pct: dec!(5.0),
                expected_profit: dec!(50),
                max_size: dec!(1000),
            }),
            total_potential_profit: dec!(75),
        };

        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: DetectionSummary = serde_json::from_str(&json).unwrap();

        assert_eq!(summary.markets_scanned, deserialized.markets_scanned);
        assert_eq!(
            summary.total_potential_profit,
            deserialized.total_potential_profit
        );
    }
}
