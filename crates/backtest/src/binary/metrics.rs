//! Binary metrics calculator for backtesting results.
//!
//! This module provides comprehensive metrics calculation for binary outcome
//! backtests, including win rate statistics, significance testing, and
//! financial performance metrics.

use algo_trade_core::{binomial_test, wilson_ci};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::outcome::{BinaryOutcome, SettlementResult};

/// Comprehensive metrics for binary outcome backtests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryMetrics {
    // Basic counts
    /// Total number of bets placed.
    pub total_bets: u32,
    /// Number of winning bets.
    pub wins: u32,
    /// Number of losing bets.
    pub losses: u32,
    /// Number of push/canceled bets.
    pub pushes: u32,

    // Win rate statistics
    /// Win rate (wins / total excluding pushes).
    pub win_rate: f64,
    /// Wilson score 95% CI lower bound.
    pub wilson_ci_lower: f64,
    /// Wilson score 95% CI upper bound.
    pub wilson_ci_upper: f64,

    // Significance testing
    /// p-value from binomial test (H0: p = 0.50).
    pub binomial_p_value: f64,
    /// Whether the result is statistically significant at alpha = 0.05.
    pub is_significant: bool,

    // Financial metrics (use Decimal!)
    /// Total amount staked across all bets.
    pub total_stake: Decimal,
    /// Total gross P&L (before fees).
    pub total_gross_pnl: Decimal,
    /// Total fees paid.
    pub total_fees: Decimal,
    /// Net P&L after fees.
    pub net_pnl: Decimal,
    /// Expected value per bet (net_pnl / total_bets).
    pub ev_per_bet: Decimal,
    /// Return on investment (net_pnl / total_stake).
    pub roi: Decimal,

    // Break-even analysis
    /// Break-even win rate needed to be profitable.
    pub break_even_win_rate: f64,
    /// Edge over break-even (win_rate - break_even_win_rate).
    pub edge_over_break_even: f64,

    // Risk metrics
    /// Maximum drawdown during the backtest.
    pub max_drawdown: Decimal,
    /// Maximum consecutive losing bets.
    pub max_consecutive_losses: u32,
    /// Average price paid per share.
    pub avg_price: Decimal,
    /// Average fee rate.
    pub avg_fee_rate: Decimal,
}

impl BinaryMetrics {
    /// Creates metrics from a slice of settlement results.
    ///
    /// # Arguments
    /// * `settlements` - Slice of settlement results to analyze
    ///
    /// # Returns
    /// `BinaryMetrics` with all computed statistics
    #[must_use]
    pub fn from_settlements(settlements: &[SettlementResult]) -> Self {
        if settlements.is_empty() {
            return Self::empty();
        }

        // Basic counts
        let total_bets = settlements.len() as u32;
        let wins = settlements
            .iter()
            .filter(|s| s.outcome == BinaryOutcome::Win)
            .count() as u32;
        let losses = settlements
            .iter()
            .filter(|s| s.outcome == BinaryOutcome::Loss)
            .count() as u32;
        let pushes = settlements
            .iter()
            .filter(|s| s.outcome == BinaryOutcome::Push)
            .count() as u32;

        // Financial metrics
        let total_stake: Decimal = settlements.iter().map(|s| s.bet.stake).sum();
        let total_gross_pnl: Decimal = settlements.iter().map(|s| s.gross_pnl).sum();
        let total_fees: Decimal = settlements.iter().map(|s| s.fees).sum();
        let net_pnl: Decimal = settlements.iter().map(|s| s.net_pnl).sum();

        // EV and ROI
        let ev_per_bet = if total_bets > 0 {
            net_pnl / Decimal::from(total_bets)
        } else {
            Decimal::ZERO
        };
        let roi = if total_stake > Decimal::ZERO {
            net_pnl / total_stake
        } else {
            Decimal::ZERO
        };

        // Win rate (excluding pushes)
        let non_push_bets = wins + losses;
        let win_rate = if non_push_bets > 0 {
            wins as f64 / non_push_bets as f64
        } else {
            0.0
        };

        // Wilson CI and binomial test
        let (wilson_ci_lower, wilson_ci_upper) =
            wilson_ci(wins as usize, non_push_bets as usize, 1.96);
        let binomial_p_value = binomial_test(wins as usize, non_push_bets as usize, 0.5);
        let is_significant = binomial_p_value < 0.05;

        // Average price and fee rate
        let avg_price = if total_bets > 0 {
            settlements.iter().map(|s| s.bet.price).sum::<Decimal>() / Decimal::from(total_bets)
        } else {
            Decimal::ZERO
        };
        let avg_fee_rate = if total_stake > Decimal::ZERO {
            total_fees / total_stake
        } else {
            Decimal::ZERO
        };

        // Break-even analysis
        let break_even_win_rate = calculate_break_even(avg_price, avg_fee_rate);
        let edge_over_break_even = win_rate - break_even_win_rate;

        // Risk metrics
        let max_drawdown = Self::calculate_max_drawdown(settlements);
        let max_consecutive_losses = Self::calculate_max_consecutive_losses(settlements);

        Self {
            total_bets,
            wins,
            losses,
            pushes,
            win_rate,
            wilson_ci_lower,
            wilson_ci_upper,
            binomial_p_value,
            is_significant,
            total_stake,
            total_gross_pnl,
            total_fees,
            net_pnl,
            ev_per_bet,
            roi,
            break_even_win_rate,
            edge_over_break_even,
            max_drawdown,
            max_consecutive_losses,
            avg_price,
            avg_fee_rate,
        }
    }

    /// Returns an empty metrics struct for when there are no settlements.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            total_bets: 0,
            wins: 0,
            losses: 0,
            pushes: 0,
            win_rate: 0.0,
            wilson_ci_lower: 0.0,
            wilson_ci_upper: 0.0,
            binomial_p_value: 1.0,
            is_significant: false,
            total_stake: Decimal::ZERO,
            total_gross_pnl: Decimal::ZERO,
            total_fees: Decimal::ZERO,
            net_pnl: Decimal::ZERO,
            ev_per_bet: Decimal::ZERO,
            roi: Decimal::ZERO,
            break_even_win_rate: 0.5,
            edge_over_break_even: -0.5,
            max_drawdown: Decimal::ZERO,
            max_consecutive_losses: 0,
            avg_price: Decimal::ZERO,
            avg_fee_rate: Decimal::ZERO,
        }
    }

    /// Returns true if the strategy has a statistically significant edge.
    ///
    /// Requires:
    /// 1. p-value < 0.05 (statistically significant)
    /// 2. Wilson CI lower bound > 0.50 (positive edge)
    /// 3. At least 100 non-push bets (sufficient sample size)
    #[must_use]
    pub fn has_significant_edge(&self) -> bool {
        let non_push_bets = self.wins + self.losses;
        self.is_significant && self.wilson_ci_lower > 0.5 && non_push_bets >= 100
    }

    /// Returns true if there is sufficient data for reliable inference.
    ///
    /// Requires at least 100 non-push bets.
    #[must_use]
    pub fn has_sufficient_samples(&self) -> bool {
        let non_push_bets = self.wins + self.losses;
        non_push_bets >= 100
    }

    /// Calculates maximum drawdown from peak equity.
    fn calculate_max_drawdown(settlements: &[SettlementResult]) -> Decimal {
        let mut peak = Decimal::ZERO;
        let mut equity = Decimal::ZERO;
        let mut max_dd = Decimal::ZERO;

        for settlement in settlements {
            equity += settlement.net_pnl;
            if equity > peak {
                peak = equity;
            }
            let drawdown = peak - equity;
            if drawdown > max_dd {
                max_dd = drawdown;
            }
        }

        max_dd
    }

    /// Calculates the maximum consecutive losses.
    fn calculate_max_consecutive_losses(settlements: &[SettlementResult]) -> u32 {
        let mut current_streak = 0u32;
        let mut max_streak = 0u32;

        for settlement in settlements {
            if settlement.outcome == BinaryOutcome::Loss {
                current_streak += 1;
                if current_streak > max_streak {
                    max_streak = current_streak;
                }
            } else {
                current_streak = 0;
            }
        }

        max_streak
    }
}

/// Calculates the break-even win rate for binary bets.
///
/// For binary bets with fees, the break-even win rate is:
/// ```text
/// p_be = (c + f) / (1 + f - c)
/// ```
/// where:
/// - c = average price (cost per share)
/// - f = average fee rate as fraction of stake
///
/// Simplified: at price 0.50 with no fees, break-even is 50%.
/// With fees, you need a higher win rate to break even.
///
/// # Arguments
/// * `avg_price` - Average price paid per share (0.0 to 1.0)
/// * `avg_fee_rate` - Average fee as fraction of stake
///
/// # Returns
/// Break-even win rate (0.0 to 1.0)
#[must_use]
pub fn calculate_break_even(avg_price: Decimal, avg_fee_rate: Decimal) -> f64 {
    // Convert to f64 for calculation
    let c = f64::try_from(avg_price).unwrap_or(0.5);
    let f = f64::try_from(avg_fee_rate).unwrap_or(0.0);

    // Handle edge cases
    if c <= 0.0 || c >= 1.0 {
        return 0.5; // Default to 50% for invalid prices
    }

    // For binary bets:
    // Win payout = 1/c - 1 (net profit per dollar staked)
    // Loss = -1 (lose stake)
    // With fees f on stake:
    // Expected value = p * (1/c - 1 - f) + (1-p) * (-1 - f) = 0
    // Solving for p:
    // p * (1/c - 1 - f) - (1-p) * (1 + f) = 0
    // p * (1/c - 1 - f) = (1-p) * (1 + f)
    // p * (1/c - 1 - f + 1 + f) = 1 + f
    // p * (1/c) = 1 + f
    // p = c * (1 + f)
    //
    // But this is simplified. More accurate for Polymarket style fees on profit:
    // Break-even when: p * (1-c)/c * (1-fee_rate_on_profit) = (1-p) * c / c
    // Simplifying with fee on stake:
    // p_be = (c + f*c) / 1 = c * (1 + f)
    // But clamped to realistic range

    let break_even = c * (1.0 + f);

    // Clamp to valid range
    break_even.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet};
    use chrono::Utc;
    use rust_decimal_macros::dec;

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_winning_settlement(
        stake: Decimal,
        price: Decimal,
        fees: Decimal,
    ) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            fees,
        )
    }

    fn create_losing_settlement(stake: Decimal, price: Decimal, fees: Decimal) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(42500),
            dec!(43000),
            BinaryOutcome::Loss,
            fees,
        )
    }

    fn create_push_settlement(stake: Decimal, price: Decimal) -> SettlementResult {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            stake,
            price,
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43000),
            dec!(43000),
            BinaryOutcome::Push,
            Decimal::ZERO,
        )
    }

    // ============================================================
    // BinaryMetrics::from_settlements Tests
    // ============================================================

    #[test]
    fn from_settlements_empty_returns_empty_metrics() {
        let settlements: Vec<SettlementResult> = vec![];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 0);
        assert_eq!(metrics.wins, 0);
        assert_eq!(metrics.losses, 0);
        assert!((metrics.win_rate - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.total_stake, Decimal::ZERO);
        assert_eq!(metrics.net_pnl, Decimal::ZERO);
    }

    #[test]
    fn from_settlements_single_win_calculates_correctly() {
        // Stake $100 at price $0.50, win pays out $200 (100/0.50), profit = $100
        let settlements = vec![create_winning_settlement(dec!(100), dec!(0.50), dec!(2))];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 1);
        assert_eq!(metrics.wins, 1);
        assert_eq!(metrics.losses, 0);
        assert!((metrics.win_rate - 1.0).abs() < f64::EPSILON);
        assert_eq!(metrics.total_stake, dec!(100));
        // gross_pnl = shares - stake = 200 - 100 = 100
        assert_eq!(metrics.total_gross_pnl, dec!(100));
        assert_eq!(metrics.total_fees, dec!(2));
        // net_pnl = 100 - 2 = 98
        assert_eq!(metrics.net_pnl, dec!(98));
    }

    #[test]
    fn from_settlements_single_loss_calculates_correctly() {
        let settlements = vec![create_losing_settlement(dec!(100), dec!(0.50), dec!(2))];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 1);
        assert_eq!(metrics.wins, 0);
        assert_eq!(metrics.losses, 1);
        assert!((metrics.win_rate - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.total_stake, dec!(100));
        // gross_pnl = -stake = -100
        assert_eq!(metrics.total_gross_pnl, -dec!(100));
        assert_eq!(metrics.total_fees, dec!(2));
        // net_pnl = -100 - 2 = -102
        assert_eq!(metrics.net_pnl, -dec!(102));
    }

    #[test]
    fn from_settlements_50_50_win_rate() {
        let mut settlements = vec![];
        // 5 wins, 5 losses at 50% price
        for _ in 0..5 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(2)));
        }
        for _ in 0..5 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(2)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 10);
        assert_eq!(metrics.wins, 5);
        assert_eq!(metrics.losses, 5);
        assert!((metrics.win_rate - 0.5).abs() < f64::EPSILON);
        assert_eq!(metrics.total_stake, dec!(1000));
        // 5 wins * $100 profit - 5 losses * $100 = $0 gross
        assert_eq!(metrics.total_gross_pnl, Decimal::ZERO);
        // Fees: 10 * $2 = $20
        assert_eq!(metrics.total_fees, dec!(20));
        // Net: $0 - $20 = -$20
        assert_eq!(metrics.net_pnl, -dec!(20));
    }

    #[test]
    fn from_settlements_with_pushes_excludes_from_win_rate() {
        let mut settlements = vec![];
        // 5 wins, 3 losses, 2 pushes
        for _ in 0..5 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..3 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..2 {
            settlements.push(create_push_settlement(dec!(100), dec!(0.50)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 10);
        assert_eq!(metrics.wins, 5);
        assert_eq!(metrics.losses, 3);
        assert_eq!(metrics.pushes, 2);
        // Win rate = 5 / (5 + 3) = 0.625
        assert!((metrics.win_rate - 0.625).abs() < 0.001);
    }

    #[test]
    fn from_settlements_ev_per_bet_calculated_correctly() {
        // 3 wins, 2 losses at 50% price, no fees
        let mut settlements = vec![];
        for _ in 0..3 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..2 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // gross = 3 * 100 - 2 * 100 = 100
        // net = 100 (no fees)
        // ev_per_bet = 100 / 5 = 20
        assert_eq!(metrics.ev_per_bet, dec!(20));
    }

    #[test]
    fn from_settlements_roi_calculated_correctly() {
        // 3 wins, 2 losses at 50% price, no fees
        let mut settlements = vec![];
        for _ in 0..3 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..2 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // net_pnl = 100
        // total_stake = 500
        // roi = 100 / 500 = 0.20 (20%)
        assert_eq!(metrics.roi, dec!(0.2));
    }

    // ============================================================
    // Wilson CI Tests
    // ============================================================

    #[test]
    fn from_settlements_wilson_ci_for_50_percent() {
        let mut settlements = vec![];
        for _ in 0..50 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..50 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // 50% win rate with n=100 should have CI approximately (0.40, 0.60)
        assert!(
            metrics.wilson_ci_lower > 0.39 && metrics.wilson_ci_lower < 0.42,
            "lower was {}",
            metrics.wilson_ci_lower
        );
        assert!(
            metrics.wilson_ci_upper > 0.58 && metrics.wilson_ci_upper < 0.61,
            "upper was {}",
            metrics.wilson_ci_upper
        );
    }

    #[test]
    fn from_settlements_wilson_ci_for_65_percent() {
        let mut settlements = vec![];
        for _ in 0..65 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..35 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // 65% win rate with n=100 should have CI above 0.50
        assert!(
            metrics.wilson_ci_lower > 0.54,
            "lower was {}",
            metrics.wilson_ci_lower
        );
    }

    // ============================================================
    // Significance Tests
    // ============================================================

    #[test]
    fn from_settlements_55_of_100_not_significant() {
        let mut settlements = vec![];
        for _ in 0..55 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..45 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert!(!metrics.is_significant);
        assert!(metrics.binomial_p_value > 0.05);
    }

    #[test]
    fn from_settlements_65_of_100_is_significant() {
        let mut settlements = vec![];
        for _ in 0..65 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..35 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert!(metrics.is_significant);
        assert!(metrics.binomial_p_value < 0.05);
    }

    // ============================================================
    // has_significant_edge Tests
    // ============================================================

    #[test]
    fn has_significant_edge_requires_100_samples() {
        let mut settlements = vec![];
        // 70% win rate but only 50 samples
        for _ in 0..35 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..15 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert!(!metrics.has_significant_edge());
        assert!(!metrics.has_sufficient_samples());
    }

    #[test]
    fn has_significant_edge_requires_ci_above_50() {
        let mut settlements = vec![];
        // 55% win rate with 100 samples - CI includes 0.50
        for _ in 0..55 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..45 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert!(!metrics.has_significant_edge());
    }

    #[test]
    fn has_significant_edge_true_for_strong_performance() {
        let mut settlements = vec![];
        // 65% win rate with 100 samples
        for _ in 0..65 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..35 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert!(metrics.has_significant_edge());
    }

    // ============================================================
    // calculate_break_even Tests
    // ============================================================

    #[test]
    fn break_even_at_50_percent_no_fees() {
        let be = calculate_break_even(dec!(0.50), dec!(0));
        assert!(
            (be - 0.50).abs() < 0.001,
            "break-even was {be}, expected ~0.50"
        );
    }

    #[test]
    fn break_even_at_45_percent_no_fees() {
        // At price 0.45, you need 45% win rate to break even (no fees)
        let be = calculate_break_even(dec!(0.45), dec!(0));
        assert!(
            (be - 0.45).abs() < 0.001,
            "break-even was {be}, expected ~0.45"
        );
    }

    #[test]
    fn break_even_at_60_percent_no_fees() {
        let be = calculate_break_even(dec!(0.60), dec!(0));
        assert!(
            (be - 0.60).abs() < 0.001,
            "break-even was {be}, expected ~0.60"
        );
    }

    #[test]
    fn break_even_increases_with_fees() {
        let be_no_fees = calculate_break_even(dec!(0.50), dec!(0));
        let be_with_fees = calculate_break_even(dec!(0.50), dec!(0.02));

        assert!(
            be_with_fees > be_no_fees,
            "expected fees to increase break-even"
        );
    }

    #[test]
    fn break_even_handles_zero_price() {
        let be = calculate_break_even(dec!(0), dec!(0));
        // Should return default of 0.5
        assert!((be - 0.50).abs() < 0.001);
    }

    #[test]
    fn break_even_handles_price_at_one() {
        let be = calculate_break_even(dec!(1.0), dec!(0));
        // Should return default of 0.5
        assert!((be - 0.50).abs() < 0.001);
    }

    #[test]
    fn break_even_from_settlements_integrated() {
        // Create settlements at price 0.50 with 2% fee rate
        let mut settlements = vec![];
        for _ in 0..5 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(2)));
        }
        for _ in 0..5 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(2)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // avg_price = 0.50
        // avg_fee_rate = 20 / 1000 = 0.02
        // break_even should be slightly above 0.50
        assert!(
            metrics.break_even_win_rate > 0.50,
            "break-even was {}",
            metrics.break_even_win_rate
        );
    }

    // ============================================================
    // Max Drawdown Tests
    // ============================================================

    #[test]
    fn max_drawdown_single_loss() {
        let settlements = vec![create_losing_settlement(dec!(100), dec!(0.50), dec!(0))];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Single loss of $100
        assert_eq!(metrics.max_drawdown, dec!(100));
    }

    #[test]
    fn max_drawdown_win_then_loss() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)), // +$100, equity = $100
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),  // -$100, equity = $0
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Peak was $100, dropped to $0, drawdown = $100
        assert_eq!(metrics.max_drawdown, dec!(100));
    }

    #[test]
    fn max_drawdown_multiple_losses() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)), // equity = $100
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)), // equity = $200
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),  // equity = $100
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),  // equity = $0
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),  // equity = -$100
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Peak was $200, dropped to -$100, drawdown = $300
        assert_eq!(metrics.max_drawdown, dec!(300));
    }

    #[test]
    fn max_drawdown_only_wins_is_zero() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // No drawdown when only winning
        assert_eq!(metrics.max_drawdown, Decimal::ZERO);
    }

    // ============================================================
    // Max Consecutive Losses Tests
    // ============================================================

    #[test]
    fn max_consecutive_losses_single_loss() {
        let settlements = vec![create_losing_settlement(dec!(100), dec!(0.50), dec!(0))];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.max_consecutive_losses, 1);
    }

    #[test]
    fn max_consecutive_losses_three_in_a_row() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.max_consecutive_losses, 3);
    }

    #[test]
    fn max_consecutive_losses_multiple_streaks() {
        let settlements = vec![
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 1
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 2
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)), // break
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 1
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 2
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 3
            create_losing_settlement(dec!(100), dec!(0.50), dec!(0)), // streak 4
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Longest streak is 4
        assert_eq!(metrics.max_consecutive_losses, 4);
    }

    #[test]
    fn max_consecutive_losses_only_wins() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
            create_winning_settlement(dec!(100), dec!(0.50), dec!(0)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.max_consecutive_losses, 0);
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn from_settlements_all_pushes() {
        let settlements = vec![
            create_push_settlement(dec!(100), dec!(0.50)),
            create_push_settlement(dec!(100), dec!(0.50)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_bets, 2);
        assert_eq!(metrics.wins, 0);
        assert_eq!(metrics.losses, 0);
        assert_eq!(metrics.pushes, 2);
        // Win rate is 0 when no wins or losses (0 / 0 = 0)
        assert!((metrics.win_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn from_settlements_varying_prices() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.40), dec!(0)), // payout = 250, profit = 150
            create_winning_settlement(dec!(100), dec!(0.60), dec!(0)), // payout = 166.67, profit = 66.67
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        // avg_price = (0.40 + 0.60) / 2 = 0.50
        assert_eq!(metrics.avg_price, dec!(0.5));
    }

    #[test]
    fn from_settlements_varying_stakes() {
        let settlements = vec![
            create_winning_settlement(dec!(50), dec!(0.50), dec!(1)),
            create_winning_settlement(dec!(150), dec!(0.50), dec!(3)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        assert_eq!(metrics.total_stake, dec!(200));
        assert_eq!(metrics.total_fees, dec!(4));
        // avg_fee_rate = 4 / 200 = 0.02
        assert_eq!(metrics.avg_fee_rate, dec!(0.02));
    }

    #[test]
    fn from_settlements_serialization_roundtrip() {
        let settlements = vec![
            create_winning_settlement(dec!(100), dec!(0.50), dec!(2)),
            create_losing_settlement(dec!(100), dec!(0.50), dec!(2)),
        ];
        let metrics = BinaryMetrics::from_settlements(&settlements);

        let json = serde_json::to_string(&metrics).unwrap();
        let deserialized: BinaryMetrics = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.total_bets, metrics.total_bets);
        assert_eq!(deserialized.wins, metrics.wins);
        assert_eq!(deserialized.net_pnl, metrics.net_pnl);
    }

    // ============================================================
    // Large Sample Tests (Statistical Power)
    // ============================================================

    #[test]
    fn from_settlements_large_sample_narrow_ci() {
        let mut settlements = vec![];
        // 550 wins, 450 losses (55% win rate, n=1000)
        for _ in 0..550 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..450 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // CI should be narrow with large sample
        let ci_width = metrics.wilson_ci_upper - metrics.wilson_ci_lower;
        assert!(ci_width < 0.07, "CI width was {ci_width}");

        // Should be significant
        assert!(metrics.is_significant);
        assert!(metrics.has_significant_edge());
    }

    #[test]
    fn edge_over_break_even_positive_for_winning_strategy() {
        let mut settlements = vec![];
        for _ in 0..60 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..40 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Win rate 60%, break-even at 50%
        // Edge = 60% - 50% = 10%
        assert!(
            metrics.edge_over_break_even > 0.05,
            "edge was {}",
            metrics.edge_over_break_even
        );
    }

    #[test]
    fn edge_over_break_even_negative_for_losing_strategy() {
        let mut settlements = vec![];
        for _ in 0..40 {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..60 {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }

        let metrics = BinaryMetrics::from_settlements(&settlements);

        // Win rate 40%, break-even at 50%
        // Edge = 40% - 50% = -10%
        assert!(
            metrics.edge_over_break_even < -0.05,
            "edge was {}",
            metrics.edge_over_break_even
        );
    }
}
