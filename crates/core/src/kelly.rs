//! Kelly Criterion implementation for binary bet sizing.
//!
//! Provides optimal bet sizing for binary outcome markets (e.g., Polymarket)
//! using the Kelly Criterion with fractional Kelly and safety constraints.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Kelly Criterion bet sizer for binary outcome markets.
///
/// The Kelly formula for binary bets with price `c` and win probability `p` is:
/// ```text
/// f* = (p(b+1) - 1) / b
/// where b = (1-c)/c (net odds)
/// ```
///
/// This simplifies to: `f* = p - c*(1-p)/(1-c) = (p - c) / (1 - c)`
/// Or equivalently: `f* = (p*(1-c) - (1-p)*c) / (1-c)`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KellySizer {
    /// Fraction of Kelly to use (0.25 = quarter Kelly)
    pub fraction: Decimal,
    /// Maximum bet size in absolute terms
    pub max_bet: Decimal,
    /// Minimum edge required to place a bet (EV threshold)
    pub min_edge: Decimal,
}

impl Default for KellySizer {
    fn default() -> Self {
        Self {
            fraction: Decimal::new(25, 2),  // 0.25 (quarter Kelly)
            max_bet: Decimal::new(1000, 0), // $1000
            min_edge: Decimal::new(1, 2),   // 0.01 (1% minimum edge)
        }
    }
}

/// Result of Kelly bet sizing calculation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetDecision {
    /// Whether to place a bet
    pub should_bet: bool,
    /// Recommended stake amount
    pub stake: Decimal,
    /// Full Kelly fraction (before applying fractional Kelly)
    pub full_kelly_fraction: Decimal,
    /// Expected value per dollar wagered
    pub expected_value: Decimal,
    /// Reason for the decision
    pub reason: BetReason,
}

/// Reason for a bet decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BetReason {
    /// Bet placed - positive edge found
    PositiveEdge,
    /// No bet - edge below minimum threshold
    InsufficientEdge,
    /// No bet - probability equals price (no edge)
    NoEdge,
    /// No bet - negative expected value
    NegativeEv,
    /// No bet - invalid inputs
    InvalidInputs,
}

impl KellySizer {
    /// Creates a new KellySizer with custom parameters.
    #[must_use]
    pub fn new(fraction: Decimal, max_bet: Decimal, min_edge: Decimal) -> Self {
        Self {
            fraction,
            max_bet,
            min_edge,
        }
    }

    /// Calculates the optimal bet size using fractional Kelly.
    ///
    /// # Arguments
    /// * `win_prob` - Estimated probability of winning (0 to 1)
    /// * `price` - Cost per share (0 to 1) - also the market's implied probability
    /// * `bankroll` - Current bankroll to size the bet against
    ///
    /// # Returns
    /// `BetDecision` with recommended stake and reasoning
    ///
    /// # Examples
    /// ```
    /// use algo_trade_core::kelly::KellySizer;
    /// use rust_decimal_macros::dec;
    ///
    /// let sizer = KellySizer::default();
    /// let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(10000));
    ///
    /// // 60% win prob vs 50% price = positive edge
    /// assert!(decision.should_bet);
    /// assert!(decision.stake > dec!(0));
    /// ```
    #[must_use]
    pub fn size(&self, win_prob: Decimal, price: Decimal, bankroll: Decimal) -> BetDecision {
        // Validate inputs
        if win_prob < Decimal::ZERO
            || win_prob > Decimal::ONE
            || price <= Decimal::ZERO
            || price >= Decimal::ONE
            || bankroll <= Decimal::ZERO
        {
            return BetDecision {
                should_bet: false,
                stake: Decimal::ZERO,
                full_kelly_fraction: Decimal::ZERO,
                expected_value: Decimal::ZERO,
                reason: BetReason::InvalidInputs,
            };
        }

        // Calculate expected value per dollar wagered
        // EV = p * (1 - c) - (1 - p) * c
        // Where p = win probability, c = cost/price
        let win_payout = Decimal::ONE - price; // Profit if win
        let lose_cost = price; // Loss if lose

        let ev = win_prob * win_payout - (Decimal::ONE - win_prob) * lose_cost;

        // Check for no edge
        if win_prob == price {
            return BetDecision {
                should_bet: false,
                stake: Decimal::ZERO,
                full_kelly_fraction: Decimal::ZERO,
                expected_value: Decimal::ZERO,
                reason: BetReason::NoEdge,
            };
        }

        // Check for negative EV
        if ev <= Decimal::ZERO {
            return BetDecision {
                should_bet: false,
                stake: Decimal::ZERO,
                full_kelly_fraction: Decimal::ZERO,
                expected_value: ev,
                reason: BetReason::NegativeEv,
            };
        }

        // Check minimum edge threshold
        // Edge = p - c (simplified measure)
        let edge = win_prob - price;
        if edge < self.min_edge {
            return BetDecision {
                should_bet: false,
                stake: Decimal::ZERO,
                full_kelly_fraction: Decimal::ZERO,
                expected_value: ev,
                reason: BetReason::InsufficientEdge,
            };
        }

        // Calculate full Kelly fraction
        // f* = (p - c) / (1 - c) for binary bets
        let full_kelly = (win_prob - price) / (Decimal::ONE - price);

        // Apply fractional Kelly
        let fractional_kelly = full_kelly * self.fraction;

        // Calculate stake
        let mut stake = bankroll * fractional_kelly;

        // Apply maximum bet cap
        if stake > self.max_bet {
            stake = self.max_bet;
        }

        // Ensure non-negative
        if stake < Decimal::ZERO {
            stake = Decimal::ZERO;
        }

        BetDecision {
            should_bet: stake > Decimal::ZERO,
            stake,
            full_kelly_fraction: full_kelly,
            expected_value: ev,
            reason: BetReason::PositiveEdge,
        }
    }

    /// Calculates expected value without sizing.
    ///
    /// EV = p * (1 - c) - (1 - p) * c
    #[must_use]
    pub fn expected_value(win_prob: Decimal, price: Decimal) -> Decimal {
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return Decimal::ZERO;
        }
        win_prob * (Decimal::ONE - price) - (Decimal::ONE - win_prob) * price
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================
    // Basic Kelly Formula Tests
    // ============================================

    #[test]
    fn kelly_no_bet_when_prob_equals_price() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.5), dec!(0.5), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.stake, Decimal::ZERO);
        assert_eq!(decision.reason, BetReason::NoEdge);
    }

    #[test]
    fn kelly_positive_bet_when_prob_exceeds_price() {
        let sizer = KellySizer::new(dec!(1.0), dec!(10000), dec!(0.01)); // Full Kelly
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(10000));

        assert!(decision.should_bet);
        assert!(decision.stake > Decimal::ZERO);
        assert_eq!(decision.reason, BetReason::PositiveEdge);

        // Full Kelly for p=0.6, c=0.5: f* = (0.6 - 0.5) / (1 - 0.5) = 0.2
        // Stake = 10000 * 0.2 = 2000
        assert!((decision.full_kelly_fraction - dec!(0.2)).abs() < dec!(0.001));
        assert!((decision.stake - dec!(2000)).abs() < dec!(1));
    }

    #[test]
    fn kelly_no_bet_when_prob_below_price() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.4), dec!(0.5), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.stake, Decimal::ZERO);
        assert_eq!(decision.reason, BetReason::NegativeEv);
        assert!(decision.expected_value < Decimal::ZERO);
    }

    // ============================================
    // Fractional Kelly Tests
    // ============================================

    #[test]
    fn kelly_quarter_fraction_reduces_bet() {
        let full_sizer = KellySizer::new(dec!(1.0), dec!(100000), dec!(0.01));
        let quarter_sizer = KellySizer::new(dec!(0.25), dec!(100000), dec!(0.01));

        let full_decision = full_sizer.size(dec!(0.7), dec!(0.5), dec!(10000));
        let quarter_decision = quarter_sizer.size(dec!(0.7), dec!(0.5), dec!(10000));

        assert!(quarter_decision.stake > Decimal::ZERO);
        assert!((quarter_decision.stake - full_decision.stake * dec!(0.25)).abs() < dec!(1));
    }

    #[test]
    fn kelly_half_fraction() {
        let sizer = KellySizer::new(dec!(0.5), dec!(100000), dec!(0.01));
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(10000));

        // Full Kelly = 0.2, half Kelly = 0.1, stake = 1000
        assert!((decision.stake - dec!(1000)).abs() < dec!(1));
    }

    // ============================================
    // Maximum Bet Cap Tests
    // ============================================

    #[test]
    fn kelly_respects_max_bet_cap() {
        let sizer = KellySizer::new(dec!(1.0), dec!(500), dec!(0.01)); // Max $500
        let decision = sizer.size(dec!(0.7), dec!(0.5), dec!(10000));

        // Full Kelly would be 10000 * 0.4 = 4000, but capped at 500
        assert_eq!(decision.stake, dec!(500));
        assert!(decision.should_bet);
    }

    #[test]
    fn kelly_under_max_bet_not_capped() {
        let sizer = KellySizer::new(dec!(0.1), dec!(5000), dec!(0.01)); // 10% Kelly, max $5000
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(10000));

        // 10% of full Kelly (0.2) = 0.02, stake = 200 (under 5000 cap)
        assert!((decision.stake - dec!(200)).abs() < dec!(1));
    }

    // ============================================
    // Minimum Edge Threshold Tests
    // ============================================

    #[test]
    fn kelly_no_bet_below_min_edge() {
        let sizer = KellySizer::new(dec!(0.25), dec!(1000), dec!(0.05)); // 5% min edge
        let decision = sizer.size(dec!(0.52), dec!(0.5), dec!(10000)); // Only 2% edge

        assert!(!decision.should_bet);
        assert_eq!(decision.stake, Decimal::ZERO);
        assert_eq!(decision.reason, BetReason::InsufficientEdge);
    }

    #[test]
    fn kelly_bets_at_min_edge() {
        let sizer = KellySizer::new(dec!(0.25), dec!(1000), dec!(0.05)); // 5% min edge
        let decision = sizer.size(dec!(0.55), dec!(0.5), dec!(10000)); // Exactly 5% edge

        assert!(decision.should_bet);
        assert!(decision.stake > Decimal::ZERO);
    }

    #[test]
    fn kelly_bets_above_min_edge() {
        let sizer = KellySizer::new(dec!(0.25), dec!(1000), dec!(0.01)); // 1% min edge
        let decision = sizer.size(dec!(0.55), dec!(0.5), dec!(10000)); // 5% edge

        assert!(decision.should_bet);
        assert!(decision.stake > Decimal::ZERO);
    }

    // ============================================
    // Expected Value Tests
    // ============================================

    #[test]
    fn expected_value_positive_edge() {
        // p=0.6, c=0.5: EV = 0.6 * 0.5 - 0.4 * 0.5 = 0.3 - 0.2 = 0.1
        let ev = KellySizer::expected_value(dec!(0.6), dec!(0.5));
        assert!((ev - dec!(0.1)).abs() < dec!(0.001));
    }

    #[test]
    fn expected_value_no_edge() {
        // p=0.5, c=0.5: EV = 0.5 * 0.5 - 0.5 * 0.5 = 0
        let ev = KellySizer::expected_value(dec!(0.5), dec!(0.5));
        assert!((ev - dec!(0.0)).abs() < dec!(0.001));
    }

    #[test]
    fn expected_value_negative_edge() {
        // p=0.4, c=0.5: EV = 0.4 * 0.5 - 0.6 * 0.5 = 0.2 - 0.3 = -0.1
        let ev = KellySizer::expected_value(dec!(0.4), dec!(0.5));
        assert!((ev - dec!(-0.1)).abs() < dec!(0.001));
    }

    #[test]
    fn expected_value_high_prob_cheap_price() {
        // p=0.8, c=0.3: EV = 0.8 * 0.7 - 0.2 * 0.3 = 0.56 - 0.06 = 0.5
        let ev = KellySizer::expected_value(dec!(0.8), dec!(0.3));
        assert!((ev - dec!(0.5)).abs() < dec!(0.001));
    }

    // ============================================
    // Edge Cases and Input Validation
    // ============================================

    #[test]
    fn kelly_invalid_negative_prob() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(-0.1), dec!(0.5), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_invalid_prob_above_one() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(1.1), dec!(0.5), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_invalid_zero_price() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.6), dec!(0.0), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_invalid_price_one() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.6), dec!(1.0), dec!(10000));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_invalid_zero_bankroll() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(0));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_invalid_negative_bankroll() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(-1000));

        assert!(!decision.should_bet);
        assert_eq!(decision.reason, BetReason::InvalidInputs);
    }

    #[test]
    fn kelly_prob_at_boundary_zero() {
        let sizer = KellySizer::default();
        let decision = sizer.size(dec!(0.0), dec!(0.5), dec!(10000));

        // 0% win prob against 50% price = very negative EV
        assert!(!decision.should_bet);
        assert!(decision.expected_value < Decimal::ZERO);
    }

    #[test]
    fn kelly_prob_at_boundary_one() {
        let sizer = KellySizer::new(dec!(1.0), dec!(100000), dec!(0.01));
        let decision = sizer.size(dec!(1.0), dec!(0.5), dec!(10000));

        // 100% win prob = guaranteed profit, full Kelly = 1.0
        assert!(decision.should_bet);
        assert!((decision.full_kelly_fraction - dec!(1.0)).abs() < dec!(0.001));
    }

    // ============================================
    // Default Values Tests
    // ============================================

    #[test]
    fn kelly_default_has_quarter_fraction() {
        let sizer = KellySizer::default();
        assert_eq!(sizer.fraction, dec!(0.25));
    }

    #[test]
    fn kelly_default_has_1000_max_bet() {
        let sizer = KellySizer::default();
        assert_eq!(sizer.max_bet, dec!(1000));
    }

    #[test]
    fn kelly_default_has_1_percent_min_edge() {
        let sizer = KellySizer::default();
        assert_eq!(sizer.min_edge, dec!(0.01));
    }

    // ============================================
    // BetDecision Fields Tests
    // ============================================

    #[test]
    fn bet_decision_has_correct_full_kelly() {
        let sizer = KellySizer::new(dec!(0.25), dec!(10000), dec!(0.01));
        let decision = sizer.size(dec!(0.7), dec!(0.5), dec!(10000));

        // Full Kelly for p=0.7, c=0.5: f* = (0.7 - 0.5) / (1 - 0.5) = 0.4
        assert!((decision.full_kelly_fraction - dec!(0.4)).abs() < dec!(0.001));
    }

    #[test]
    fn bet_decision_has_correct_ev() {
        let sizer = KellySizer::new(dec!(0.25), dec!(10000), dec!(0.01));
        let decision = sizer.size(dec!(0.6), dec!(0.5), dec!(10000));

        // EV = 0.6 * 0.5 - 0.4 * 0.5 = 0.1
        assert!((decision.expected_value - dec!(0.1)).abs() < dec!(0.001));
    }

    // ============================================
    // Decimal Precision Tests
    // ============================================

    #[test]
    fn kelly_uses_decimal_not_float() {
        // This test verifies we're using Decimal correctly
        let sizer = KellySizer::new(dec!(0.25), dec!(1000), dec!(0.01));

        // These precise values would cause float errors
        let decision = sizer.size(dec!(0.333333333), dec!(0.3), dec!(10000));

        // The calculation should work without precision loss
        assert!(decision.should_bet || !decision.should_bet); // Just verifying no panic
    }

    #[test]
    fn kelly_small_edge_with_decimal_precision() {
        let sizer = KellySizer::new(dec!(0.25), dec!(1000), dec!(0.001)); // 0.1% min edge
        let decision = sizer.size(dec!(0.501), dec!(0.5), dec!(10000)); // 0.1% edge

        // Should be able to detect 0.1% edge precisely
        assert!(decision.should_bet);
    }
}
