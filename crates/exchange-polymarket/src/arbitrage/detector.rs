//! Arbitrage detection engine for Polymarket binary markets.
//!
//! This module provides the [`ArbitrageDetector`] struct for identifying
//! profitable arbitrage opportunities where buying both YES and NO tokens
//! costs less than $1.00, guaranteeing profit regardless of outcome.
//!
//! # Key Concepts
//!
//! - **Pair Cost**: Total cost to buy 1 YES + 1 NO share. If < $1.00, arbitrage exists.
//! - **Break-even Threshold**: ~$0.983 after accounting for fees and gas.
//! - **Conservative Threshold**: $0.97 (3% safety margin).
//!
//! # Fee Calculation
//!
//! Polymarket charges 2% fee on profit from the winning side:
//! - E[Fee] = 0.01 * (2 - pair_cost)
//!
//! This is derived from: E[Fee] = 0.02 * (1 - p_yes) * p_yes + 0.02 * p_no * (1 - p_no)
//! where the prices represent the market's implied probabilities.
//!
//! # Example
//!
//! ```
//! use algo_trade_polymarket::arbitrage::{ArbitrageDetector, L2OrderBook, Side};
//! use rust_decimal_macros::dec;
//!
//! let detector = ArbitrageDetector::default();
//!
//! // Set up order books
//! let mut yes_book = L2OrderBook::new("yes-token".to_string());
//! yes_book.apply_snapshot(vec![], vec![(dec!(0.48), dec!(500))]);
//!
//! let mut no_book = L2OrderBook::new("no-token".to_string());
//! no_book.apply_snapshot(vec![], vec![(dec!(0.48), dec!(500))]);
//!
//! // Detect opportunity (pair cost = 0.96, should be profitable)
//! if let Some(opp) = detector.detect("market-1", &yes_book, &no_book, dec!(100)) {
//!     println!("Found opportunity! Net profit: {}", opp.net_profit_per_pair);
//! }
//! ```

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::orderbook::simulate_fill;
use super::types::{ArbitrageOpportunity, FillSimulation, L2OrderBook, Side};

/// Configuration for arbitrage detection.
///
/// Provides configurable thresholds for identifying profitable opportunities
/// while accounting for fees, gas costs, and risk tolerance.
#[derive(Debug, Clone)]
pub struct ArbitrageDetector {
    /// Maximum pair cost to consider an opportunity (e.g., 0.97 for 3% margin).
    ///
    /// Opportunities with pair cost above this threshold are rejected.
    /// Default: 0.97
    pub target_pair_cost: Decimal,

    /// Minimum net profit per pair after fees and gas.
    ///
    /// Filters out opportunities that are technically profitable but
    /// not worth the execution risk. Default: 0.005 ($0.50 per 100 pairs)
    pub min_profit_threshold: Decimal,

    /// Maximum position size per opportunity.
    ///
    /// Caps the recommended size to limit exposure. Default: 1000
    pub max_position_size: Decimal,

    /// Gas cost per transaction on Polygon.
    ///
    /// Applied twice (once for YES, once for NO). Default: 0.007
    pub gas_cost: Decimal,
}

impl Default for ArbitrageDetector {
    fn default() -> Self {
        Self {
            target_pair_cost: dec!(0.97),
            min_profit_threshold: dec!(0.005), // 0.5 cents
            max_position_size: dec!(1000),
            gas_cost: dec!(0.007),
        }
    }
}

impl ArbitrageDetector {
    /// Creates a new detector with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a detector with a custom pair cost threshold.
    #[must_use]
    pub fn with_target_pair_cost(mut self, threshold: Decimal) -> Self {
        self.target_pair_cost = threshold;
        self
    }

    /// Creates a detector with a custom minimum profit threshold.
    #[must_use]
    pub fn with_min_profit_threshold(mut self, threshold: Decimal) -> Self {
        self.min_profit_threshold = threshold;
        self
    }

    /// Creates a detector with a custom maximum position size.
    #[must_use]
    pub fn with_max_position_size(mut self, size: Decimal) -> Self {
        self.max_position_size = size;
        self
    }

    /// Creates a detector with a custom gas cost.
    #[must_use]
    pub fn with_gas_cost(mut self, cost: Decimal) -> Self {
        self.gas_cost = cost;
        self
    }

    /// Detect arbitrage opportunity from YES and NO order books.
    ///
    /// Simulates fills for both sides and calculates profitability
    /// using worst-case fill prices to ensure the opportunity is real.
    ///
    /// # Arguments
    ///
    /// * `market_id` - Market condition ID
    /// * `yes_book` - Order book for the YES token
    /// * `no_book` - Order book for the NO token
    /// * `order_size` - Desired position size to evaluate
    ///
    /// # Returns
    ///
    /// Returns `Some(ArbitrageOpportunity)` if a profitable opportunity exists,
    /// `None` otherwise.
    ///
    /// # Rejection Reasons
    ///
    /// - Insufficient depth on either side
    /// - Pair cost exceeds threshold
    /// - Net profit below minimum threshold
    #[must_use]
    pub fn detect(
        &self,
        market_id: &str,
        yes_book: &L2OrderBook,
        no_book: &L2OrderBook,
        order_size: Decimal,
    ) -> Option<ArbitrageOpportunity> {
        // Simulate fills for both sides (buying into the ask)
        let yes_fill = simulate_fill(yes_book, Side::Buy, order_size)?;
        let no_fill = simulate_fill(no_book, Side::Buy, order_size)?;

        // Check sufficient depth on both sides
        if !yes_fill.sufficient_depth || !no_fill.sufficient_depth {
            return None;
        }

        // Calculate pair cost using WORST fill prices (most conservative)
        let pair_cost = yes_fill.worst_price + no_fill.worst_price;

        // Check threshold - reject if too expensive
        if pair_cost > self.target_pair_cost {
            return None;
        }

        // Calculate gross profit per pair
        // One side always pays $1.00 at settlement
        let gross_profit = Decimal::ONE - pair_cost;

        // Calculate expected fee
        // Fee is 2% of PROFIT on winning side
        // E[Fee] = 0.01 * (2 - pair_cost)
        //
        // Derivation:
        // - If YES wins: Fee = 0.02 * (1 - p_yes) where p_yes = yes_price
        // - If NO wins: Fee = 0.02 * (1 - p_no) where p_no = no_price
        // - E[Fee] = p_yes * 0.02 * (1 - p_yes) + p_no * 0.02 * (1 - p_no)
        // - Since p_yes + p_no = pair_cost and we're buying at worst prices:
        // - E[Fee] simplifies to 0.01 * (2 - pair_cost)
        let expected_fee = dec!(0.01) * (dec!(2) - pair_cost);

        // Gas for 2 transactions (YES buy + NO buy)
        let total_gas = self.gas_cost * dec!(2);

        // Net profit per pair after all costs
        let net_profit = gross_profit - expected_fee - total_gas;

        // Check minimum profit threshold
        if net_profit < self.min_profit_threshold {
            return None;
        }

        // Calculate recommended size (capped by max position)
        let size = order_size.min(self.max_position_size);
        let total_investment = size * pair_cost;
        let guaranteed_payout = size; // One side always pays $1

        // Calculate ROI as percentage
        let roi = if total_investment > Decimal::ZERO {
            net_profit / pair_cost * dec!(100)
        } else {
            Decimal::ZERO
        };

        // Calculate risk score (0.0 = low risk, 1.0 = high risk)
        let risk_score = self.calculate_risk_score(&yes_fill, &no_fill, pair_cost);

        Some(ArbitrageOpportunity {
            market_id: market_id.to_string(),
            yes_token_id: yes_book.token_id.clone(),
            no_token_id: no_book.token_id.clone(),
            yes_worst_fill: yes_fill.worst_price,
            no_worst_fill: no_fill.worst_price,
            pair_cost,
            gross_profit_per_pair: gross_profit,
            expected_fee,
            gas_cost: total_gas,
            net_profit_per_pair: net_profit,
            roi,
            recommended_size: size,
            total_investment,
            guaranteed_payout,
            // Use total available book depth for liquidity validation, not filled amount
            yes_depth: yes_book.total_ask_depth(),
            no_depth: no_book.total_ask_depth(),
            risk_score,
            detected_at: Utc::now(),
        })
    }

    /// Detect opportunity at multiple size points to find optimal sizing.
    ///
    /// Evaluates the opportunity at various sizes and returns all profitable
    /// configurations, sorted by total profit (descending).
    ///
    /// # Arguments
    ///
    /// * `market_id` - Market condition ID
    /// * `yes_book` - Order book for the YES token
    /// * `no_book` - Order book for the NO token
    /// * `sizes` - List of sizes to evaluate
    ///
    /// # Returns
    ///
    /// Vector of profitable opportunities at different sizes.
    #[must_use]
    pub fn detect_at_sizes(
        &self,
        market_id: &str,
        yes_book: &L2OrderBook,
        no_book: &L2OrderBook,
        sizes: &[Decimal],
    ) -> Vec<ArbitrageOpportunity> {
        let mut opportunities: Vec<ArbitrageOpportunity> = sizes
            .iter()
            .filter_map(|&size| self.detect(market_id, yes_book, no_book, size))
            .collect();

        // Sort by total profit (size * net_profit_per_pair), descending
        opportunities.sort_by(|a, b| {
            let profit_a = a.recommended_size * a.net_profit_per_pair;
            let profit_b = b.recommended_size * b.net_profit_per_pair;
            profit_b.cmp(&profit_a)
        });

        opportunities
    }

    /// Calculate risk score for an opportunity.
    ///
    /// Considers multiple risk factors:
    /// - Slippage between best and worst price (higher = riskier)
    /// - Margin to threshold (closer = riskier)
    /// - Depth imbalance between YES/NO (larger = riskier)
    ///
    /// # Returns
    ///
    /// Risk score from 0.0 (low risk) to 1.0 (high risk)
    fn calculate_risk_score(
        &self,
        yes_fill: &FillSimulation,
        no_fill: &FillSimulation,
        pair_cost: Decimal,
    ) -> f64 {
        let mut risk = 0.0;

        // 1. Slippage risk: difference between best and worst price
        // Higher slippage indicates thin liquidity at good prices
        let yes_slippage = (yes_fill.worst_price - yes_fill.best_price).abs();
        let no_slippage = (no_fill.worst_price - no_fill.best_price).abs();
        let total_slippage = yes_slippage + no_slippage;

        // Convert to f64 and scale (max contribution: 0.3)
        let slippage_f64 = total_slippage.to_string().parse::<f64>().unwrap_or(0.0);
        risk += (slippage_f64 * 10.0).min(0.3);

        // 2. Margin risk: closer to threshold = higher risk
        // If price moves against us, we might lose profitability
        let margin = self.target_pair_cost - pair_cost;
        let margin_f64 = margin.to_string().parse::<f64>().unwrap_or(0.0);

        if margin_f64 < 0.01 {
            risk += 0.3; // Very thin margin
        } else if margin_f64 < 0.02 {
            risk += 0.15; // Moderate margin
        }
        // >= 0.02 margin adds no risk

        // 3. Depth imbalance risk: imbalanced books are harder to execute
        // If one side has much less depth, partial fills are more likely
        let depth_ratio = if yes_fill.filled > no_fill.filled {
            if yes_fill.filled == Decimal::ZERO {
                1.0
            } else {
                no_fill
                    .filled
                    .checked_div(yes_fill.filled)
                    .unwrap_or(Decimal::ONE)
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(1.0)
            }
        } else if no_fill.filled == Decimal::ZERO {
            1.0
        } else {
            yes_fill
                .filled
                .checked_div(no_fill.filled)
                .unwrap_or(Decimal::ONE)
                .to_string()
                .parse::<f64>()
                .unwrap_or(1.0)
        };

        if depth_ratio < 0.5 {
            risk += 0.2; // Significant imbalance
        } else if depth_ratio < 0.8 {
            risk += 0.1; // Moderate imbalance
        }

        risk.min(1.0)
    }

    /// Check if a pair cost would be profitable with current settings.
    ///
    /// Quick check without full fill simulation.
    #[must_use]
    pub fn is_pair_cost_profitable(&self, pair_cost: Decimal) -> bool {
        if pair_cost > self.target_pair_cost {
            return false;
        }

        let gross_profit = Decimal::ONE - pair_cost;
        let expected_fee = dec!(0.01) * (dec!(2) - pair_cost);
        let total_gas = self.gas_cost * dec!(2);
        let net_profit = gross_profit - expected_fee - total_gas;

        net_profit >= self.min_profit_threshold
    }

    /// Calculate break-even pair cost for current fee and gas settings.
    ///
    /// Returns the pair cost at which net profit equals zero.
    #[must_use]
    pub fn break_even_pair_cost(&self) -> Decimal {
        // Net profit = (1 - pair_cost) - 0.01 * (2 - pair_cost) - 2 * gas_cost
        // 0 = 1 - pair_cost - 0.02 + 0.01 * pair_cost - 2 * gas_cost
        // 0 = 0.98 - 0.99 * pair_cost - 2 * gas_cost
        // 0.99 * pair_cost = 0.98 - 2 * gas_cost
        // pair_cost = (0.98 - 2 * gas_cost) / 0.99
        let total_gas = self.gas_cost * dec!(2);
        (dec!(0.98) - total_gas) / dec!(0.99)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_balanced_books(
        yes_price: Decimal,
        no_price: Decimal,
        depth: Decimal,
    ) -> (L2OrderBook, L2OrderBook) {
        let mut yes_book = L2OrderBook::new("yes-token".to_string());
        yes_book.apply_snapshot(vec![], vec![(yes_price, depth)]);

        let mut no_book = L2OrderBook::new("no-token".to_string());
        no_book.apply_snapshot(vec![], vec![(no_price, depth)]);

        (yes_book, no_book)
    }

    fn create_multi_level_books() -> (L2OrderBook, L2OrderBook) {
        let mut yes_book = L2OrderBook::new("yes-token".to_string());
        yes_book.apply_snapshot(
            vec![],
            vec![
                (dec!(0.46), dec!(100)), // Best level
                (dec!(0.48), dec!(200)),
                (dec!(0.50), dec!(300)), // Worst level
            ],
        );

        let mut no_book = L2OrderBook::new("no-token".to_string());
        no_book.apply_snapshot(
            vec![],
            vec![
                (dec!(0.46), dec!(100)), // Best level
                (dec!(0.48), dec!(200)),
                (dec!(0.50), dec!(300)), // Worst level
            ],
        );

        (yes_book, no_book)
    }

    #[test]
    fn test_detector_default() {
        let detector = ArbitrageDetector::default();
        assert_eq!(detector.target_pair_cost, dec!(0.97));
        assert_eq!(detector.min_profit_threshold, dec!(0.005));
        assert_eq!(detector.max_position_size, dec!(1000));
        assert_eq!(detector.gas_cost, dec!(0.007));
    }

    #[test]
    fn test_detector_builder_pattern() {
        let detector = ArbitrageDetector::new()
            .with_target_pair_cost(dec!(0.96))
            .with_min_profit_threshold(dec!(0.01))
            .with_max_position_size(dec!(500))
            .with_gas_cost(dec!(0.01));

        assert_eq!(detector.target_pair_cost, dec!(0.96));
        assert_eq!(detector.min_profit_threshold, dec!(0.01));
        assert_eq!(detector.max_position_size, dec!(500));
        assert_eq!(detector.gas_cost, dec!(0.01));
    }

    #[test]
    fn test_detect_profitable_opportunity() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .expect("Should find opportunity");

        assert_eq!(opp.market_id, "market-1");
        assert_eq!(opp.pair_cost, dec!(0.96));
        assert_eq!(opp.gross_profit_per_pair, dec!(0.04));
        assert_eq!(opp.yes_worst_fill, dec!(0.48));
        assert_eq!(opp.no_worst_fill, dec!(0.48));
        assert!(opp.net_profit_per_pair > Decimal::ZERO);
        assert_eq!(opp.recommended_size, dec!(100));
    }

    #[test]
    fn test_detect_no_opportunity_high_pair_cost() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.50), dec!(0.50), dec!(500));

        // Pair cost = 1.00, no arbitrage
        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_detect_no_opportunity_exceeds_threshold() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.49), dec!(0.49), dec!(500));

        // Pair cost = 0.98, above 0.97 threshold
        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_detect_no_opportunity_insufficient_depth() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(50));

        // Only 50 depth available, requesting 100
        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_detect_no_opportunity_below_profit_threshold() {
        let detector = ArbitrageDetector::new().with_min_profit_threshold(dec!(0.05)); // Very high threshold

        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_detect_with_slippage() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_multi_level_books();

        // Request 600 shares, will need all levels
        // YES worst: 0.50, NO worst: 0.50, pair_cost: 1.00 > 0.97 threshold
        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(600));
        assert!(opp.is_none()); // Should fail due to slippage

        // Request 100 shares, only first level
        // YES worst: 0.46, NO worst: 0.46, pair_cost: 0.92 < 0.97 threshold
        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .expect("Should find opportunity at small size");

        assert_eq!(opp.pair_cost, dec!(0.92));
        assert_eq!(opp.yes_worst_fill, dec!(0.46));
        assert_eq!(opp.no_worst_fill, dec!(0.46));
    }

    #[test]
    fn test_expected_fee_calculation() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // E[Fee] = 0.01 * (2 - 0.96) = 0.01 * 1.04 = 0.0104
        assert_eq!(opp.expected_fee, dec!(0.0104));
    }

    #[test]
    fn test_gas_cost_included() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // Gas = 0.007 * 2 = 0.014
        assert_eq!(opp.gas_cost, dec!(0.014));

        // Net profit = gross - fee - gas
        // = 0.04 - 0.0104 - 0.014 = 0.0156
        assert_eq!(opp.net_profit_per_pair, dec!(0.0156));
    }

    #[test]
    fn test_max_position_size_cap() {
        let detector = ArbitrageDetector::new().with_max_position_size(dec!(50));

        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // Should be capped at 50
        assert_eq!(opp.recommended_size, dec!(50));
    }

    #[test]
    fn test_roi_calculation() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // ROI = net_profit / pair_cost * 100
        // = 0.0156 / 0.96 * 100 = 1.625%
        let expected_roi = dec!(0.0156) / dec!(0.96) * dec!(100);
        assert_eq!(opp.roi, expected_roi);
    }

    #[test]
    fn test_detect_at_sizes() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_multi_level_books();

        // Test with sizes that span from profitable to unprofitable
        // With the multi-level books:
        // - 100 shares: pair_cost = 0.92 (profitable)
        // - 300 shares: pair_cost = 0.96 (profitable)
        // - 600 shares: pair_cost = 1.00 (not profitable)
        // - 700 shares: insufficient depth
        let sizes = vec![dec!(100), dec!(300), dec!(600), dec!(700)];
        let opportunities = detector.detect_at_sizes("market-1", &yes_book, &no_book, &sizes);

        // Only smaller sizes should be profitable (due to slippage and depth)
        assert!(!opportunities.is_empty());
        assert!(opportunities.len() < sizes.len()); // Some should be filtered out

        // Should be sorted by total profit descending
        for window in opportunities.windows(2) {
            let profit_a = window[0].recommended_size * window[0].net_profit_per_pair;
            let profit_b = window[1].recommended_size * window[1].net_profit_per_pair;
            assert!(profit_a >= profit_b);
        }
    }

    #[test]
    fn test_risk_score_low_slippage() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.45), dec!(0.45), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // Single level, no slippage, good margin = low risk
        assert!(opp.risk_score < 0.2);
    }

    #[test]
    fn test_risk_score_thin_margin() {
        let detector = ArbitrageDetector::new().with_target_pair_cost(dec!(0.965)); // Allow 0.96 through with thin margin

        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // Margin is only 0.005 (< 0.01), should add risk
        assert!(opp.risk_score >= 0.3);
    }

    #[test]
    fn test_risk_score_depth_imbalance() {
        let detector = ArbitrageDetector::default();

        // Create books with imbalanced depth where we request more than the minimum
        // YES has plenty of depth at good prices
        let mut yes_book = L2OrderBook::new("yes-token".to_string());
        yes_book.apply_snapshot(vec![], vec![(dec!(0.45), dec!(100))]);

        // NO has depth split across worse prices (creates slippage)
        let mut no_book = L2OrderBook::new("no-token".to_string());
        no_book.apply_snapshot(
            vec![],
            vec![
                (dec!(0.45), dec!(20)), // Only 20 at best price
                (dec!(0.46), dec!(80)), // Rest at worse price
            ],
        );

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // NO side has slippage (worst = 0.46, best = 0.45), adding risk
        // Slippage of 0.01 * 10 = 0.1 risk contribution
        assert!(opp.risk_score >= 0.1);
    }

    #[test]
    fn test_is_pair_cost_profitable() {
        let detector = ArbitrageDetector::default();

        assert!(detector.is_pair_cost_profitable(dec!(0.95)));
        assert!(detector.is_pair_cost_profitable(dec!(0.96)));
        assert!(!detector.is_pair_cost_profitable(dec!(0.98))); // Above threshold
        assert!(!detector.is_pair_cost_profitable(dec!(1.00))); // No profit
    }

    #[test]
    fn test_break_even_pair_cost() {
        let detector = ArbitrageDetector::default();

        let break_even = detector.break_even_pair_cost();

        // With gas_cost = 0.007, total_gas = 0.014
        // break_even = (0.98 - 0.014) / 0.99 = 0.966 / 0.99 ≈ 0.9757
        assert!(break_even > dec!(0.97));
        assert!(break_even < dec!(0.98));

        // Verify: at break_even, net profit should be ~0
        assert!(!detector.is_pair_cost_profitable(break_even));
    }

    #[test]
    fn test_empty_orderbook() {
        let detector = ArbitrageDetector::default();

        let yes_book = L2OrderBook::new("yes-token".to_string());
        let no_book = L2OrderBook::new("no-token".to_string());

        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_one_sided_orderbook() {
        let detector = ArbitrageDetector::default();

        let mut yes_book = L2OrderBook::new("yes-token".to_string());
        yes_book.apply_snapshot(vec![], vec![(dec!(0.48), dec!(500))]);

        let no_book = L2OrderBook::new("no-token".to_string()); // Empty

        let opp = detector.detect("market-1", &yes_book, &no_book, dec!(100));
        assert!(opp.is_none());
    }

    #[test]
    fn test_opportunity_fields_complete() {
        let detector = ArbitrageDetector::default();
        let (yes_book, no_book) = create_balanced_books(dec!(0.48), dec!(0.48), dec!(500));

        let opp = detector
            .detect("market-1", &yes_book, &no_book, dec!(100))
            .unwrap();

        // Verify all fields are populated correctly
        assert_eq!(opp.market_id, "market-1");
        assert_eq!(opp.yes_token_id, "yes-token");
        assert_eq!(opp.no_token_id, "no-token");
        assert_eq!(opp.total_investment, opp.recommended_size * opp.pair_cost);
        assert_eq!(opp.guaranteed_payout, opp.recommended_size);
        assert_eq!(opp.yes_depth, dec!(500)); // Total book depth
        assert_eq!(opp.no_depth, dec!(500));
        assert!(opp.detected_at <= Utc::now());
    }
}
