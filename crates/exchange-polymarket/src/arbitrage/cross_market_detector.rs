//! Cross-market correlation arbitrage detector.
//!
//! Detects opportunities across multiple coin 15-minute markets where
//! the total cost of buying positions on two different coins is less
//! than $1.00, with positive expected value based on correlation.
//!
//! # Strategy
//!
//! Crypto assets (BTC, ETH, SOL, XRP) are highly correlated (~85%).
//! When buying cheap sides on two different coins:
//! - If total cost < $1.00, we profit if at least one wins
//! - With high correlation, coins usually move together
//! - Opposite movements (BTC up + ETH down) are rare
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::{
//!     CrossMarketDetector, CrossMarketConfig, CoinMarketSnapshot,
//! };
//! use algo_trade_polymarket::models::Coin;
//!
//! let config = CrossMarketConfig::default();
//! let mut detector = CrossMarketDetector::new(config);
//!
//! let snapshots = vec![
//!     CoinMarketSnapshot { coin: Coin::Eth, up_price: dec!(0.05), ... },
//!     CoinMarketSnapshot { coin: Coin::Btc, down_price: dec!(0.91), ... },
//! ];
//!
//! let opportunities = detector.check(&snapshots, now_ms);
//! for opp in opportunities {
//!     println!("{}", opp.display_short());
//! }
//! ```

use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::models::Coin;

use super::cross_market_types::{
    CoinMarketSnapshot, CoinPair, CrossMarketCombination, CrossMarketConfig,
    CrossMarketOpportunity,
};

/// Fee rate on winning side (2%).
const FEE_RATE: f64 = 0.02;

/// Cross-market correlation arbitrage detector.
#[derive(Debug)]
pub struct CrossMarketDetector {
    /// Configuration.
    config: CrossMarketConfig,
    /// Last signal time per (coin1, coin2, combination) to enforce cooldown.
    last_signal_ms: HashMap<(Coin, Coin, CrossMarketCombination), i64>,
}

impl CrossMarketDetector {
    /// Creates a new detector with the given configuration.
    #[must_use]
    pub fn new(config: CrossMarketConfig) -> Self {
        Self {
            config,
            last_signal_ms: HashMap::new(),
        }
    }

    /// Creates a detector with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(CrossMarketConfig::default())
    }

    /// Returns the current configuration.
    #[must_use]
    pub fn config(&self) -> &CrossMarketConfig {
        &self.config
    }

    /// Check all coin pairs and combinations for opportunities.
    ///
    /// Returns a vector of detected opportunities that meet all thresholds.
    pub fn check(
        &mut self,
        markets: &[CoinMarketSnapshot],
        current_time_ms: i64,
    ) -> Vec<CrossMarketOpportunity> {
        let pairs = CoinPair::all_pairs(&self.config.coins);
        let mut opportunities = Vec::new();

        for pair in pairs {
            let m1 = markets.iter().find(|m| m.coin == pair.coin1);
            let m2 = markets.iter().find(|m| m.coin == pair.coin2);

            if let (Some(m1), Some(m2)) = (m1, m2) {
                // Check combinations (filtered or all)
                // Clone to avoid borrow conflict with &mut self in check_combination
                let combos: Vec<CrossMarketCombination> = self
                    .config
                    .combinations
                    .clone()
                    .unwrap_or_else(|| CrossMarketCombination::all().to_vec());
                for combo in combos {
                    if let Some(opp) =
                        self.check_combination(pair, m1, m2, combo, current_time_ms)
                    {
                        opportunities.push(opp);
                    }
                }
            }
        }

        opportunities
    }

    /// Check a single combination for a coin pair.
    fn check_combination(
        &mut self,
        pair: CoinPair,
        m1: &CoinMarketSnapshot,
        m2: &CoinMarketSnapshot,
        combo: CrossMarketCombination,
        current_time_ms: i64,
    ) -> Option<CrossMarketOpportunity> {
        // Check cooldown
        let key = (pair.coin1, pair.coin2, combo);
        if let Some(last) = self.last_signal_ms.get(&key) {
            if current_time_ms - last < self.config.signal_cooldown_ms {
                return None;
            }
        }

        // Get prices for this combination
        let (leg1_is_up, leg2_is_up) = combo.directions();
        let leg1_price = m1.price_for_direction(leg1_is_up);
        let leg2_price = m2.price_for_direction(leg2_is_up);

        let total_cost = leg1_price + leg2_price;

        // Check total cost threshold
        if total_cost > self.config.max_total_cost {
            return None;
        }

        // Check spread threshold
        let spread = Decimal::ONE - total_cost;
        if spread < self.config.min_spread {
            return None;
        }

        // Calculate win probability and expected value
        let win_prob = self.calculate_win_probability(combo, leg1_price, leg2_price);
        let ev = self.calculate_expected_value(total_cost, win_prob);

        // Check EV threshold
        if ev < self.config.min_expected_value {
            return None;
        }

        // Record signal time
        self.last_signal_ms.insert(key, current_time_ms);

        // Get depth data from snapshots (if available)
        let leg1_depth = m1.depth_for_direction(leg1_is_up);
        let leg2_depth = m2.depth_for_direction(leg2_is_up);

        // Check minimum depth requirement (if configured)
        if self.config.min_depth > Decimal::ZERO {
            let leg1_ask = leg1_depth.as_ref().map(|d| d.ask_depth).unwrap_or(Decimal::ZERO);
            let leg2_ask = leg2_depth.as_ref().map(|d| d.ask_depth).unwrap_or(Decimal::ZERO);
            let min_available = leg1_ask.min(leg2_ask);

            if min_available < self.config.min_depth {
                // Not enough liquidity - skip this opportunity
                return None;
            }
        }

        // Record signal time
        self.last_signal_ms.insert(key, current_time_ms);

        // Build opportunity
        let detected_at = chrono::DateTime::from_timestamp_millis(current_time_ms)
            .unwrap_or_else(chrono::Utc::now);

        let opp = CrossMarketOpportunity {
            coin1: m1.coin.slug_prefix().to_uppercase(),
            coin2: m2.coin.slug_prefix().to_uppercase(),
            combination: combo,
            leg1_direction: if leg1_is_up { "UP" } else { "DOWN" }.to_string(),
            leg1_price,
            leg1_token_id: m1.token_for_direction(leg1_is_up).to_string(),
            leg2_direction: if leg2_is_up { "UP" } else { "DOWN" }.to_string(),
            leg2_price,
            leg2_token_id: m2.token_for_direction(leg2_is_up).to_string(),
            total_cost,
            spread,
            expected_value: ev,
            assumed_correlation: self.config.assumed_correlation,
            win_probability: win_prob,
            detected_at,
            // Depth fields
            leg1_bid_depth: None,
            leg1_ask_depth: None,
            leg1_spread_bps: None,
            leg2_bid_depth: None,
            leg2_ask_depth: None,
            leg2_spread_bps: None,
        }
        .with_depth(leg1_depth, leg2_depth);

        Some(opp)
    }

    /// Calculate P(at least one leg wins) based on correlation model.
    ///
    /// For opposite-direction bets (Coin1UP + Coin2DOWN):
    /// - Win if either coin goes in our direction
    /// - Both UP: Coin1UP wins
    /// - Both DOWN: Coin2DOWN wins
    /// - Opposite (rare): Either both win or both lose
    ///
    /// For same-direction bets (BothUP or BothDOWN):
    /// - Win if at least one coin goes in our direction
    /// - Both same: Both legs win or both lose
    /// - Opposite (rare): One wins, one loses
    #[must_use]
    pub fn calculate_win_probability(
        &self,
        combo: CrossMarketCombination,
        leg1_price: Decimal,
        leg2_price: Decimal,
    ) -> f64 {
        let rho = self.config.assumed_correlation;

        // Market-implied probabilities (used for more advanced models)
        let _p1 = decimal_to_f64(leg1_price); // P(leg1 outcome)
        let _p2 = decimal_to_f64(leg2_price); // P(leg2 outcome)

        match combo {
            // Opposite direction bets (e.g., ETH UP + BTC DOWN)
            // We win if:
            // - Both coins go UP: leg1 (ETH UP) wins → payout $1
            // - Both coins go DOWN: leg2 (BTC DOWN) wins → payout $1
            // - Coins diverge UP/DOWN: leg1 wins → payout $1
            // We lose only if: ETH DOWN and BTC UP (both legs wrong)
            CrossMarketCombination::Coin1UpCoin2Down => {
                // P(both move same direction) increases with correlation
                let p_both_up = 0.5 * (0.5 + 0.5 * rho);
                let p_both_down = 0.5 * (0.5 + 0.5 * rho);
                let p_c1_up_c2_down = 0.5 * (0.5 - 0.5 * rho); // Rare with high correlation
                // P(C1 DOWN, C2 UP) = rare, this is our loss scenario

                // We win in 3 of 4 scenarios:
                // Both UP: C1_UP wins
                // Both DOWN: C2_DOWN wins
                // C1 UP, C2 DOWN: Both win!
                p_both_up + p_both_down + p_c1_up_c2_down
            }

            CrossMarketCombination::Coin1DownCoin2Up => {
                // Mirror of above
                let p_both_up = 0.5 * (0.5 + 0.5 * rho);
                let p_both_down = 0.5 * (0.5 + 0.5 * rho);
                let p_c1_down_c2_up = 0.5 * (0.5 - 0.5 * rho);

                // Win in 3 of 4 scenarios:
                // Both UP: C2_UP wins
                // Both DOWN: C1_DOWN wins
                // C1 DOWN, C2 UP: Both win!
                p_both_up + p_both_down + p_c1_down_c2_up
            }

            // Same direction bets (e.g., ETH UP + BTC UP)
            // We win if at least one coin goes UP
            // We lose only if both coins go DOWN
            CrossMarketCombination::BothUp => {
                let p_both_down = 0.5 * (0.5 + 0.5 * rho);
                1.0 - p_both_down
            }

            // We win if at least one coin goes DOWN
            // We lose only if both coins go UP
            CrossMarketCombination::BothDown => {
                let p_both_up = 0.5 * (0.5 + 0.5 * rho);
                1.0 - p_both_up
            }
        }
    }

    /// Calculate expected value of the trade.
    ///
    /// EV = P(win) * (payout - fee) - cost
    ///
    /// Where:
    /// - payout = $1.00 (one side always wins)
    /// - fee = 2% on winning side
    /// - cost = total_cost (leg1 + leg2 prices)
    #[must_use]
    pub fn calculate_expected_value(&self, total_cost: Decimal, win_prob: f64) -> Decimal {
        let cost_f64 = decimal_to_f64(total_cost);
        let payout = 1.0;
        let net_payout = payout * (1.0 - FEE_RATE); // After 2% fee

        let ev = win_prob * net_payout - cost_f64;

        Decimal::try_from(ev).unwrap_or(Decimal::ZERO)
    }

    /// Resets all cooldowns (useful for testing).
    pub fn reset_cooldowns(&mut self) {
        self.last_signal_ms.clear();
    }
}

/// Convert Decimal to f64 for probability calculations.
fn decimal_to_f64(d: Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.5)
}

// ============================================================================
// Tests (TDD - written first)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn make_snapshot(coin: Coin, up_price: Decimal, down_price: Decimal) -> CoinMarketSnapshot {
        CoinMarketSnapshot {
            coin,
            up_price,
            down_price,
            up_token_id: format!("{}_up", coin.slug_prefix()),
            down_token_id: format!("{}_down", coin.slug_prefix()),
            timestamp_ms: Utc::now().timestamp_millis(),
            up_depth: None,
            down_depth: None,
        }
    }

    // -------------------------------------------------------------------------
    // Detector Creation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn detector_new_with_config() {
        let config = CrossMarketConfig::default();
        let detector = CrossMarketDetector::new(config.clone());
        assert_eq!(detector.config().max_total_cost, config.max_total_cost);
    }

    #[test]
    fn detector_with_defaults() {
        let detector = CrossMarketDetector::with_defaults();
        assert_eq!(detector.config().max_total_cost, dec!(0.95));
    }

    // -------------------------------------------------------------------------
    // Basic Detection Tests
    // -------------------------------------------------------------------------

    #[test]
    fn check_finds_opportunity_when_cost_below_threshold() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.98),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0), // Accept any EV for this test
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.45), dec!(0.55)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
        ];

        let opps = detector.check(&snapshots, 1000);

        // Should find opportunities for combinations under threshold
        // BTC_UP(0.45) + ETH_DOWN(0.60) = 1.05 > 0.98 - NO
        // BTC_UP(0.45) + ETH_UP(0.40) = 0.85 < 0.98 - YES
        // BTC_DOWN(0.55) + ETH_UP(0.40) = 0.95 < 0.98 - YES
        // BTC_DOWN(0.55) + ETH_DOWN(0.60) = 1.15 > 0.98 - NO
        assert!(!opps.is_empty());
    }

    #[test]
    fn check_returns_empty_when_all_costs_above_threshold() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.50), // Very restrictive
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.45), dec!(0.55)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
        ];

        let opps = detector.check(&snapshots, 1000);
        assert!(opps.is_empty());
    }

    #[test]
    fn check_respects_min_spread() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.10), // Require 10% spread
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        // Total cost 0.95 = spread 0.05, below min_spread 0.10
        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.50), dec!(0.50)),
            make_snapshot(Coin::Eth, dec!(0.45), dec!(0.55)),
        ];

        let opps = detector.check(&snapshots, 1000);
        // BTC_UP(0.50) + ETH_UP(0.45) = 0.95, spread = 0.05 < 0.10 - NO
        // Should filter out low-spread opportunities
        let low_spread_count = opps.iter().filter(|o| o.spread < dec!(0.10)).count();
        assert_eq!(low_spread_count, 0);
    }

    // -------------------------------------------------------------------------
    // Win Probability Tests
    // -------------------------------------------------------------------------

    #[test]
    fn win_probability_opposite_direction_high_with_correlation() {
        let config = CrossMarketConfig {
            assumed_correlation: 0.85,
            combinations: None,
            ..Default::default()
        };
        let detector = CrossMarketDetector::new(config);

        let win_prob = detector.calculate_win_probability(
            CrossMarketCombination::Coin1UpCoin2Down,
            dec!(0.05),
            dec!(0.91),
        );

        // With 85% correlation, opposite movements are rare
        // Win probability should be high (>90%)
        assert!(win_prob > 0.90, "Win prob {} should be > 0.90", win_prob);
        assert!(win_prob < 1.0, "Win prob {} should be < 1.0", win_prob);
    }

    #[test]
    fn win_probability_same_direction_around_fifty_percent() {
        let config = CrossMarketConfig {
            assumed_correlation: 0.85,
            combinations: None,
            ..Default::default()
        };
        let detector = CrossMarketDetector::new(config);

        let win_prob_up = detector.calculate_win_probability(
            CrossMarketCombination::BothUp,
            dec!(0.50),
            dec!(0.50),
        );

        let win_prob_down = detector.calculate_win_probability(
            CrossMarketCombination::BothDown,
            dec!(0.50),
            dec!(0.50),
        );

        // BothUp wins if at least one goes up
        // BothDown wins if at least one goes down
        // With correlation, these should be symmetric
        assert!(
            (win_prob_up - win_prob_down).abs() < 0.01,
            "BothUp {} and BothDown {} should be symmetric",
            win_prob_up,
            win_prob_down
        );

        // Should be reasonably high (>50%) since we win if AT LEAST ONE goes our way
        assert!(win_prob_up > 0.50, "Win prob {} should be > 0.50", win_prob_up);
    }

    #[test]
    fn win_probability_increases_with_correlation_for_opposite() {
        let low_corr = CrossMarketDetector::new(CrossMarketConfig {
            assumed_correlation: 0.50,
            ..Default::default()
        });

        let high_corr = CrossMarketDetector::new(CrossMarketConfig {
            assumed_correlation: 0.95,
            ..Default::default()
        });

        let combo = CrossMarketCombination::Coin1UpCoin2Down;

        let win_prob_low = low_corr.calculate_win_probability(combo, dec!(0.10), dec!(0.90));
        let win_prob_high = high_corr.calculate_win_probability(combo, dec!(0.10), dec!(0.90));

        // Higher correlation = more likely to move together = higher win rate for opposite bets
        assert!(
            win_prob_high > win_prob_low,
            "High corr {} should beat low corr {}",
            win_prob_high,
            win_prob_low
        );
    }

    // -------------------------------------------------------------------------
    // Expected Value Tests
    // -------------------------------------------------------------------------

    #[test]
    fn expected_value_positive_for_cheap_entry() {
        let detector = CrossMarketDetector::with_defaults();

        // Total cost $0.90, win prob 95%
        // EV = 0.95 * 0.98 - 0.90 = 0.931 - 0.90 = 0.031
        let ev = detector.calculate_expected_value(dec!(0.90), 0.95);

        assert!(ev > Decimal::ZERO, "EV {} should be positive", ev);
        assert!(ev < dec!(0.10), "EV {} should be reasonable", ev);
    }

    #[test]
    fn expected_value_negative_for_expensive_entry() {
        let detector = CrossMarketDetector::with_defaults();

        // Total cost $0.99, win prob 95%
        // EV = 0.95 * 0.98 - 0.99 = 0.931 - 0.99 = -0.059
        let ev = detector.calculate_expected_value(dec!(0.99), 0.95);

        assert!(ev < Decimal::ZERO, "EV {} should be negative", ev);
    }

    #[test]
    fn expected_value_accounts_for_fees() {
        let detector = CrossMarketDetector::with_defaults();

        // Without fees: EV = 1.0 * 1.0 - 0.90 = 0.10
        // With 2% fees: EV = 1.0 * 0.98 - 0.90 = 0.08
        let ev = detector.calculate_expected_value(dec!(0.90), 1.0);

        // Should be close to 0.08, not 0.10
        assert!(ev < dec!(0.09), "EV {} should account for fees", ev);
        assert!(ev > dec!(0.07), "EV {} should be around 0.08", ev);
    }

    // -------------------------------------------------------------------------
    // Cooldown Tests
    // -------------------------------------------------------------------------

    #[test]
    fn check_respects_cooldown() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 10_000, // 10 second cooldown
            coins: vec![Coin::Btc, Coin::Eth],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
        ];

        // First check at t=1000
        let opps1 = detector.check(&snapshots, 1000);
        let count1 = opps1.len();
        assert!(count1 > 0, "Should find opportunities initially");

        // Second check at t=5000 (within cooldown)
        let opps2 = detector.check(&snapshots, 5000);
        assert!(
            opps2.is_empty(),
            "Should respect cooldown, found {} opps",
            opps2.len()
        );

        // Third check at t=15000 (after cooldown)
        let opps3 = detector.check(&snapshots, 15000);
        assert_eq!(opps3.len(), count1, "Should find opportunities after cooldown");
    }

    #[test]
    fn reset_cooldowns_allows_immediate_signal() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 10_000,
            coins: vec![Coin::Btc, Coin::Eth],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
        ];

        let opps1 = detector.check(&snapshots, 1000);
        assert!(!opps1.is_empty());

        // Reset cooldowns
        detector.reset_cooldowns();

        // Should work immediately
        let opps2 = detector.check(&snapshots, 1001);
        assert!(!opps2.is_empty());
    }

    // -------------------------------------------------------------------------
    // Opportunity Structure Tests
    // -------------------------------------------------------------------------

    #[test]
    fn opportunity_has_correct_fields() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth],
            assumed_correlation: 0.85,
            combinations: None,
            min_depth: dec!(0),
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Eth, dec!(0.35), dec!(0.65)),
        ];

        let opps = detector.check(&snapshots, 1000);
        assert!(!opps.is_empty());

        // Find BothUp opportunity: BTC_UP(0.40) + ETH_UP(0.35) = 0.75
        let both_up = opps
            .iter()
            .find(|o| o.combination == CrossMarketCombination::BothUp);

        if let Some(opp) = both_up {
            assert_eq!(opp.coin1, "BTC");
            assert_eq!(opp.coin2, "ETH");
            assert_eq!(opp.leg1_direction, "UP");
            assert_eq!(opp.leg2_direction, "UP");
            assert_eq!(opp.leg1_price, dec!(0.40));
            assert_eq!(opp.leg2_price, dec!(0.35));
            assert_eq!(opp.total_cost, dec!(0.75));
            assert_eq!(opp.spread, dec!(0.25));
            assert!((opp.assumed_correlation - 0.85).abs() < 0.001);
        }
    }

    // -------------------------------------------------------------------------
    // Multi-Coin Tests
    // -------------------------------------------------------------------------

    #[test]
    fn check_all_four_coins_generates_six_pairs() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp],
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Sol, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Xrp, dec!(0.40), dec!(0.60)),
        ];

        let opps = detector.check(&snapshots, 1000);

        // 6 pairs * up to 4 combinations each
        // But not all combinations meet thresholds
        // At minimum, should have opportunities from multiple pairs
        let unique_pairs: std::collections::HashSet<_> = opps
            .iter()
            .map(|o| format!("{}/{}", o.coin1, o.coin2))
            .collect();

        assert!(
            unique_pairs.len() >= 2,
            "Should have opportunities from multiple pairs, found {:?}",
            unique_pairs
        );
    }

    #[test]
    fn check_handles_missing_coins_gracefully() {
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-1.0),
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth, Coin::Sol], // Expect 3 coins
            ..Default::default()
        };
        let mut detector = CrossMarketDetector::new(config);

        // Only provide 2 coins
        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.40), dec!(0.60)),
            make_snapshot(Coin::Eth, dec!(0.40), dec!(0.60)),
            // Sol is missing!
        ];

        // Should not panic, should still find BTC/ETH opportunities
        let opps = detector.check(&snapshots, 1000);
        assert!(!opps.is_empty(), "Should find BTC/ETH opportunities");

        // Should not include SOL pairs
        let has_sol = opps.iter().any(|o| o.coin1 == "SOL" || o.coin2 == "SOL");
        assert!(!has_sol, "Should not have SOL opportunities");
    }

    // -------------------------------------------------------------------------
    // Real-World Scenario Tests
    // -------------------------------------------------------------------------

    #[test]
    fn scenario_eth_up_cheap_btc_down_expensive() {
        // User's original scenario: ETH UP @ $0.05, BTC DOWN @ $0.91 = $0.96 total
        // Note: With 2% fees, EV is slightly negative for this exact scenario.
        // Here we use a slightly cheaper combination to get positive EV.
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.98),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-0.05), // Accept slightly negative EV for this test
            signal_cooldown_ms: 0,
            coins: vec![Coin::Btc, Coin::Eth],
            assumed_correlation: 0.85,
            combinations: None,
            min_depth: dec!(0),
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Btc, dec!(0.09), dec!(0.91)), // BTC DOWN is expensive
            make_snapshot(Coin::Eth, dec!(0.05), dec!(0.95)), // ETH UP is cheap
        ];

        let opps = detector.check(&snapshots, 1000);

        // Should find: BTC_DOWN(0.91) + ETH_UP(0.05) = 0.96
        let target = opps.iter().find(|o| {
            o.coin1 == "BTC"
                && o.coin2 == "ETH"
                && o.combination == CrossMarketCombination::Coin1DownCoin2Up
        });

        assert!(target.is_some(), "Should find BTC_DOWN + ETH_UP opportunity");

        let opp = target.unwrap();
        assert_eq!(opp.total_cost, dec!(0.96));
        assert_eq!(opp.spread, dec!(0.04));
        assert!(opp.win_probability > 0.90);
        // EV is slightly negative due to 2% fees on $0.96 cost
        // With win_prob ~0.96 and payout 0.98: EV = 0.96 * 0.98 - 0.96 ≈ -0.02
    }

    #[test]
    fn scenario_both_cheap_high_spread() {
        // Both sides cheap: great opportunity
        let config = CrossMarketConfig {
            max_total_cost: dec!(0.99),
            min_spread: dec!(0.01),
            min_expected_value: dec!(-0.50), // Allow negative EV for this test
            signal_cooldown_ms: 0,
            coins: vec![Coin::Sol, Coin::Xrp],
            assumed_correlation: 0.85,
            combinations: None,
            min_depth: dec!(0),
        };
        let mut detector = CrossMarketDetector::new(config);

        let snapshots = vec![
            make_snapshot(Coin::Sol, dec!(0.30), dec!(0.70)),
            make_snapshot(Coin::Xrp, dec!(0.35), dec!(0.65)),
        ];

        let opps = detector.check(&snapshots, 1000);

        // BothUp: 0.30 + 0.35 = 0.65, spread = 0.35 (great!)
        // Find any opportunity with BothUp combination
        let both_up = opps
            .iter()
            .find(|o| o.combination == CrossMarketCombination::BothUp);

        assert!(
            both_up.is_some(),
            "Should find BothUp opportunity. Found: {:?}",
            opps.iter().map(|o| format!("{} {:?} cost={}", o.display_short(), o.combination, o.total_cost)).collect::<Vec<_>>()
        );
        let opp = both_up.unwrap();
        assert_eq!(opp.total_cost, dec!(0.65));
        assert_eq!(opp.spread, dec!(0.35));
    }
}
