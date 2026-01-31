//! Monte Carlo simulation for binary outcome trading strategies.
//!
//! This module provides Monte Carlo simulation methods to estimate the distribution
//! of future equity outcomes, probability of ruin, and probability of reaching
//! profit targets. It supports both empirical (resampling from historical results)
//! and parametric (explicit win rate and odds) simulation methods.
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_backtest::binary::monte_carlo::{MonteCarloConfig, MonteCarloSimulator, BetSizing};
//! use rust_decimal_macros::dec;
//!
//! let config = MonteCarloConfig::default();
//! let simulator = MonteCarloSimulator::new(config);
//!
//! // Parametric simulation with known win rate
//! let results = simulator.simulate_parametric(0.55, dec!(0.50));
//! println!("Probability of ruin: {:.2}%", results.prob_ruin * 100.0);
//! println!("Probability of profit: {:.2}%", results.prob_profit * 100.0);
//! ```

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use super::outcome::{BinaryOutcome, SettlementResult};

/// Bet sizing strategy for Monte Carlo simulation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BetSizing {
    /// Fixed bet size in dollars.
    Fixed(Decimal),
    /// Fraction of current bankroll (e.g., 0.02 = 2%).
    FractionOfBankroll(Decimal),
    /// Kelly criterion with fraction multiplier and minimum edge requirement.
    Kelly {
        /// Fraction of full Kelly (e.g., 0.25 for quarter Kelly).
        fraction: Decimal,
        /// Minimum edge required to place a bet.
        min_edge: Decimal,
    },
}

impl Default for BetSizing {
    fn default() -> Self {
        Self::Fixed(dec!(100))
    }
}

/// Configuration for Monte Carlo simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonteCarloConfig {
    /// Number of simulation paths to run.
    pub n_simulations: usize,
    /// Number of bets per simulation path.
    pub n_bets: usize,
    /// Initial bankroll in dollars.
    pub initial_bankroll: Decimal,
    /// Bet sizing strategy.
    pub sizing: BetSizing,
    /// Bankroll threshold for ruin (as fraction of initial, e.g., 0.1 = 10%).
    pub ruin_threshold: Decimal,
    /// Optional seed for reproducible results.
    pub seed: Option<u64>,
}

impl Default for MonteCarloConfig {
    fn default() -> Self {
        Self {
            n_simulations: 10_000,
            n_bets: 100,
            initial_bankroll: dec!(10000),
            sizing: BetSizing::default(),
            ruin_threshold: dec!(0.1),
            seed: None,
        }
    }
}

impl MonteCarloConfig {
    /// Creates a new configuration with specified parameters.
    #[must_use]
    pub fn new(n_simulations: usize, n_bets: usize, initial_bankroll: Decimal) -> Self {
        Self {
            n_simulations,
            n_bets,
            initial_bankroll,
            ..Default::default()
        }
    }

    /// Sets the bet sizing strategy.
    #[must_use]
    pub fn with_sizing(mut self, sizing: BetSizing) -> Self {
        self.sizing = sizing;
        self
    }

    /// Sets a seed for reproducible simulations.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Sets the ruin threshold as a fraction of initial bankroll.
    #[must_use]
    pub fn with_ruin_threshold(mut self, threshold: Decimal) -> Self {
        self.ruin_threshold = threshold;
        self
    }
}

/// Summary statistics for a distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionSummary {
    /// Minimum value.
    pub min: Decimal,
    /// Maximum value.
    pub max: Decimal,
    /// Mean value.
    pub mean: Decimal,
    /// Median value (50th percentile).
    pub median: Decimal,
    /// Standard deviation.
    pub std_dev: Decimal,
    /// Key percentiles (5th, 10th, 25th, 50th, 75th, 90th, 95th).
    pub percentiles: Vec<(f64, Decimal)>,
}

impl DistributionSummary {
    /// Creates a summary from a slice of values.
    #[must_use]
    pub fn from_values(values: &[Decimal]) -> Self {
        if values.is_empty() {
            return Self::empty();
        }

        let mut sorted = values.to_vec();
        sorted.sort();

        let n = sorted.len();
        let sum: Decimal = sorted.iter().copied().sum();
        let mean = sum / Decimal::from(n);

        // Calculate variance
        let variance: Decimal = sorted
            .iter()
            .map(|&x| {
                let diff = x - mean;
                diff * diff
            })
            .sum::<Decimal>()
            / Decimal::from(n.max(1));

        // Approximate sqrt using Newton-Raphson
        let std_dev = decimal_sqrt(variance);

        let min = sorted[0];
        let max = sorted[n - 1];
        let median = percentile_decimal(&sorted, 0.50);

        let percentiles = vec![
            (0.05, percentile_decimal(&sorted, 0.05)),
            (0.10, percentile_decimal(&sorted, 0.10)),
            (0.25, percentile_decimal(&sorted, 0.25)),
            (0.50, median),
            (0.75, percentile_decimal(&sorted, 0.75)),
            (0.90, percentile_decimal(&sorted, 0.90)),
            (0.95, percentile_decimal(&sorted, 0.95)),
        ];

        Self {
            min,
            max,
            mean,
            median,
            std_dev,
            percentiles,
        }
    }

    /// Returns an empty summary.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            min: Decimal::ZERO,
            max: Decimal::ZERO,
            mean: Decimal::ZERO,
            median: Decimal::ZERO,
            std_dev: Decimal::ZERO,
            percentiles: vec![],
        }
    }
}

/// Results from Monte Carlo simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonteCarloResults {
    /// Probability of hitting ruin threshold.
    pub prob_ruin: f64,
    /// Probability of ending with profit (equity > initial).
    pub prob_profit: f64,
    /// Probability of doubling the bankroll.
    pub prob_double: f64,
    /// Median final equity across all simulations.
    pub median_equity: Decimal,
    /// Mean final equity across all simulations.
    pub mean_equity: Decimal,
    /// 5th percentile of final equity.
    pub equity_p5: Decimal,
    /// 95th percentile of final equity.
    pub equity_p95: Decimal,
    /// Full distribution summary of final equities.
    pub distributions: DistributionSummary,
    /// Number of simulations run.
    pub n_simulations: usize,
    /// Number of bets per simulation.
    pub n_bets: usize,
}

/// Monte Carlo simulator for trading strategies.
pub struct MonteCarloSimulator {
    config: MonteCarloConfig,
}

impl MonteCarloSimulator {
    /// Creates a new simulator with the given configuration.
    #[must_use]
    pub fn new(config: MonteCarloConfig) -> Self {
        Self { config }
    }

    /// Creates a simulator with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(MonteCarloConfig::default())
    }

    /// Returns a reference to the configuration.
    #[must_use]
    pub fn config(&self) -> &MonteCarloConfig {
        &self.config
    }

    /// Simulates a single path and returns the equity curve.
    ///
    /// # Arguments
    /// * `win_prob` - Probability of winning each bet
    /// * `price` - Price per share (determines payout odds)
    /// * `rng` - Random number generator
    ///
    /// # Returns
    /// Vector of equity values at each step
    pub fn simulate_path(
        &self,
        win_prob: f64,
        price: Decimal,
        rng: &mut ChaCha8Rng,
    ) -> Vec<Decimal> {
        let mut equity = self.config.initial_bankroll;
        let mut path = Vec::with_capacity(self.config.n_bets + 1);
        path.push(equity);

        let ruin_level = self.config.initial_bankroll * self.config.ruin_threshold;

        for _ in 0..self.config.n_bets {
            // Check for ruin
            if equity <= ruin_level {
                // Pad remaining path with ruin value
                while path.len() <= self.config.n_bets {
                    path.push(equity);
                }
                break;
            }

            // Calculate bet size
            let bet_size = self.calculate_bet_size(equity, win_prob, price);
            if bet_size <= Decimal::ZERO {
                path.push(equity);
                continue;
            }

            // Simulate bet outcome
            let is_win = rng.gen::<f64>() < win_prob;

            if is_win {
                // Win: payout = stake / price, profit = payout - stake
                let payout = bet_size / price;
                let profit = payout - bet_size;
                equity += profit;
            } else {
                // Loss: lose the stake
                equity -= bet_size;
            }

            path.push(equity);
        }

        path
    }

    /// Calculates bet size based on the sizing strategy.
    fn calculate_bet_size(&self, equity: Decimal, win_prob: f64, price: Decimal) -> Decimal {
        match &self.config.sizing {
            BetSizing::Fixed(amount) => (*amount).min(equity),
            BetSizing::FractionOfBankroll(fraction) => equity * *fraction,
            BetSizing::Kelly { fraction, min_edge } => {
                // Calculate edge
                let p = Decimal::try_from(win_prob).unwrap_or(dec!(0.5));
                let break_even = price;
                let edge = p - break_even;

                // Check minimum edge
                if edge < *min_edge {
                    return Decimal::ZERO;
                }

                // Kelly formula: f* = (p(b+1) - 1) / b
                // where b = (1 - price) / price (net odds)
                let b = (Decimal::ONE - price) / price;
                let full_kelly = (p * (b + Decimal::ONE) - Decimal::ONE) / b;

                // Apply fraction and clamp
                let bet = full_kelly * *fraction * equity;
                bet.max(Decimal::ZERO).min(equity)
            }
        }
    }

    /// Runs Monte Carlo simulation using empirical distribution from settlements.
    ///
    /// This resamples from historical bet outcomes to simulate future paths.
    #[must_use]
    pub fn simulate_from_settlements(&self, settlements: &[SettlementResult]) -> MonteCarloResults {
        if settlements.is_empty() {
            return MonteCarloResults::empty(self.config.n_simulations, self.config.n_bets);
        }

        // Calculate empirical win rate
        let wins = settlements
            .iter()
            .filter(|s| s.outcome == BinaryOutcome::Win)
            .count();
        let non_push = settlements
            .iter()
            .filter(|s| s.outcome != BinaryOutcome::Push)
            .count();

        if non_push == 0 {
            return MonteCarloResults::empty(self.config.n_simulations, self.config.n_bets);
        }

        let win_prob = wins as f64 / non_push as f64;

        // Calculate average price from settlements
        let total_bets = settlements.len();
        let avg_price: Decimal =
            settlements.iter().map(|s| s.bet.price).sum::<Decimal>() / Decimal::from(total_bets);

        self.simulate_parametric(win_prob, avg_price)
    }

    /// Runs Monte Carlo simulation with explicit parameters.
    ///
    /// # Arguments
    /// * `win_prob` - Probability of winning each bet
    /// * `price` - Average price per share
    #[must_use]
    pub fn simulate_parametric(&self, win_prob: f64, price: Decimal) -> MonteCarloResults {
        let mut rng = match self.config.seed {
            Some(seed) => ChaCha8Rng::seed_from_u64(seed),
            None => ChaCha8Rng::from_entropy(),
        };

        let mut final_equities = Vec::with_capacity(self.config.n_simulations);
        let ruin_level = self.config.initial_bankroll * self.config.ruin_threshold;
        let double_level = self.config.initial_bankroll * dec!(2);

        let mut ruin_count = 0;
        let mut profit_count = 0;
        let mut double_count = 0;

        for _ in 0..self.config.n_simulations {
            let path = self.simulate_path(win_prob, price, &mut rng);
            let final_equity = *path.last().unwrap_or(&self.config.initial_bankroll);

            // Check outcomes
            if path.iter().any(|&e| e <= ruin_level) {
                ruin_count += 1;
            }
            if final_equity > self.config.initial_bankroll {
                profit_count += 1;
            }
            if final_equity >= double_level {
                double_count += 1;
            }

            final_equities.push(final_equity);
        }

        let distributions = DistributionSummary::from_values(&final_equities);

        MonteCarloResults {
            prob_ruin: ruin_count as f64 / self.config.n_simulations as f64,
            prob_profit: profit_count as f64 / self.config.n_simulations as f64,
            prob_double: double_count as f64 / self.config.n_simulations as f64,
            median_equity: distributions.median,
            mean_equity: distributions.mean,
            equity_p5: percentile_decimal_from_summary(&distributions, 0.05),
            equity_p95: percentile_decimal_from_summary(&distributions, 0.95),
            distributions,
            n_simulations: self.config.n_simulations,
            n_bets: self.config.n_bets,
        }
    }
}

impl MonteCarloResults {
    /// Returns empty results.
    #[must_use]
    pub fn empty(n_simulations: usize, n_bets: usize) -> Self {
        Self {
            prob_ruin: 0.0,
            prob_profit: 0.0,
            prob_double: 0.0,
            median_equity: Decimal::ZERO,
            mean_equity: Decimal::ZERO,
            equity_p5: Decimal::ZERO,
            equity_p95: Decimal::ZERO,
            distributions: DistributionSummary::empty(),
            n_simulations,
            n_bets,
        }
    }

    /// Returns true if the strategy shows positive expected outcomes.
    #[must_use]
    pub fn is_favorable(&self) -> bool {
        self.prob_profit > 0.5 && self.prob_ruin < 0.1
    }
}

/// Calculates a percentile from a sorted slice of Decimal values.
fn percentile_decimal(sorted: &[Decimal], p: f64) -> Decimal {
    if sorted.is_empty() {
        return Decimal::ZERO;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let n = sorted.len();
    let idx = (p * (n - 1) as f64).round() as usize;
    sorted[idx.min(n - 1)]
}

/// Extracts a percentile from a DistributionSummary.
fn percentile_decimal_from_summary(summary: &DistributionSummary, p: f64) -> Decimal {
    for (pct, value) in &summary.percentiles {
        if (*pct - p).abs() < 0.001 {
            return *value;
        }
    }
    summary.median
}

/// Approximates square root for Decimal using Newton-Raphson method.
fn decimal_sqrt(x: Decimal) -> Decimal {
    if x <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    // Initial guess
    let mut guess = x / dec!(2);
    if guess == Decimal::ZERO {
        guess = dec!(1);
    }

    // Newton-Raphson iterations
    for _ in 0..20 {
        let new_guess = (guess + x / guess) / dec!(2);
        if (new_guess - guess).abs() < dec!(0.0000001) {
            return new_guess;
        }
        guess = new_guess;
    }

    guess
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet};
    use chrono::Utc;

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

    fn create_mixed_settlements(wins: usize, losses: usize) -> Vec<SettlementResult> {
        let mut settlements = Vec::with_capacity(wins + losses);
        for _ in 0..wins {
            settlements.push(create_winning_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        for _ in 0..losses {
            settlements.push(create_losing_settlement(dec!(100), dec!(0.50), dec!(0)));
        }
        settlements
    }

    // ============================================================
    // MonteCarloConfig Tests
    // ============================================================

    #[test]
    fn config_default_has_expected_values() {
        let config = MonteCarloConfig::default();

        assert_eq!(config.n_simulations, 10_000);
        assert_eq!(config.n_bets, 100);
        assert_eq!(config.initial_bankroll, dec!(10000));
        assert_eq!(config.sizing, BetSizing::Fixed(dec!(100)));
        assert_eq!(config.ruin_threshold, dec!(0.1));
        assert!(config.seed.is_none());
    }

    #[test]
    fn config_new_sets_parameters() {
        let config = MonteCarloConfig::new(5000, 200, dec!(5000));

        assert_eq!(config.n_simulations, 5000);
        assert_eq!(config.n_bets, 200);
        assert_eq!(config.initial_bankroll, dec!(5000));
    }

    #[test]
    fn config_with_sizing_sets_sizing() {
        let config =
            MonteCarloConfig::default().with_sizing(BetSizing::FractionOfBankroll(dec!(0.02)));

        assert_eq!(config.sizing, BetSizing::FractionOfBankroll(dec!(0.02)));
    }

    #[test]
    fn config_with_seed_sets_seed() {
        let config = MonteCarloConfig::default().with_seed(42);

        assert_eq!(config.seed, Some(42));
    }

    #[test]
    fn config_with_ruin_threshold_sets_threshold() {
        let config = MonteCarloConfig::default().with_ruin_threshold(dec!(0.05));

        assert_eq!(config.ruin_threshold, dec!(0.05));
    }

    // ============================================================
    // BetSizing Tests
    // ============================================================

    #[test]
    fn bet_sizing_fixed_variant() {
        let sizing = BetSizing::Fixed(dec!(50));
        assert_eq!(sizing, BetSizing::Fixed(dec!(50)));
    }

    #[test]
    fn bet_sizing_fraction_variant() {
        let sizing = BetSizing::FractionOfBankroll(dec!(0.05));
        assert_eq!(sizing, BetSizing::FractionOfBankroll(dec!(0.05)));
    }

    #[test]
    fn bet_sizing_kelly_variant() {
        let sizing = BetSizing::Kelly {
            fraction: dec!(0.25),
            min_edge: dec!(0.02),
        };
        match sizing {
            BetSizing::Kelly { fraction, min_edge } => {
                assert_eq!(fraction, dec!(0.25));
                assert_eq!(min_edge, dec!(0.02));
            }
            _ => panic!("Expected Kelly variant"),
        }
    }

    #[test]
    fn bet_sizing_default_is_fixed_100() {
        let sizing = BetSizing::default();
        assert_eq!(sizing, BetSizing::Fixed(dec!(100)));
    }

    // ============================================================
    // simulate_path Tests
    // ============================================================

    #[test]
    fn simulate_path_returns_correct_length() {
        let config = MonteCarloConfig::new(1, 50, dec!(10000)).with_seed(42);
        let simulator = MonteCarloSimulator::new(config);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let path = simulator.simulate_path(0.55, dec!(0.50), &mut rng);

        // Path should have n_bets + 1 entries (initial + each bet)
        assert_eq!(path.len(), 51);
    }

    #[test]
    fn simulate_path_starts_with_initial_bankroll() {
        let config = MonteCarloConfig::new(1, 10, dec!(5000)).with_seed(42);
        let simulator = MonteCarloSimulator::new(config);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let path = simulator.simulate_path(0.50, dec!(0.50), &mut rng);

        assert_eq!(path[0], dec!(5000));
    }

    #[test]
    fn simulate_path_equity_changes_with_bets() {
        let config = MonteCarloConfig::new(1, 10, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let path = simulator.simulate_path(0.50, dec!(0.50), &mut rng);

        // Check that equity changes
        let changes_exist = path.windows(2).any(|w| w[0] != w[1]);
        assert!(changes_exist, "Equity should change with bets");
    }

    #[test]
    fn simulate_path_win_rate_100_always_profits() {
        let config = MonteCarloConfig::new(1, 20, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let path = simulator.simulate_path(1.0, dec!(0.50), &mut rng);

        // Every step should increase equity
        for i in 1..path.len() {
            assert!(
                path[i] >= path[i - 1],
                "Equity should never decrease with 100% win rate"
            );
        }

        // Final equity should be higher than initial
        assert!(path.last().unwrap() > &dec!(10000));
    }

    #[test]
    fn simulate_path_win_rate_0_always_loses() {
        let config = MonteCarloConfig::new(1, 20, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42)
            .with_ruin_threshold(dec!(0.01)); // Very low ruin threshold
        let simulator = MonteCarloSimulator::new(config);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let path = simulator.simulate_path(0.0, dec!(0.50), &mut rng);

        // Every step should decrease equity (until ruin)
        let mut decreasing = true;
        for i in 1..path.len() {
            if path[i] > path[i - 1] {
                decreasing = false;
                break;
            }
        }
        assert!(decreasing, "Equity should always decrease with 0% win rate");
    }

    // ============================================================
    // prob_ruin Tests
    // ============================================================

    #[test]
    fn prob_ruin_is_zero_when_win_rate_100() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(1.0, dec!(0.50));

        assert!(
            (results.prob_ruin - 0.0).abs() < f64::EPSILON,
            "prob_ruin was {}",
            results.prob_ruin
        );
    }

    #[test]
    fn prob_ruin_is_high_when_win_rate_low() {
        let config = MonteCarloConfig::new(1000, 100, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(500))) // Large bets
            .with_ruin_threshold(dec!(0.1))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.30, dec!(0.50));

        // With 30% win rate and large bets, ruin should be very likely
        assert!(
            results.prob_ruin > 0.5,
            "prob_ruin was {}, expected high",
            results.prob_ruin
        );
    }

    // ============================================================
    // prob_profit Tests
    // ============================================================

    #[test]
    fn prob_profit_is_1_when_win_rate_100() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(1.0, dec!(0.50));

        assert!(
            (results.prob_profit - 1.0).abs() < f64::EPSILON,
            "prob_profit was {}",
            results.prob_profit
        );
    }

    #[test]
    fn prob_profit_is_low_when_win_rate_low() {
        let config = MonteCarloConfig::new(1000, 100, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.40, dec!(0.50));

        // With 40% win rate at even odds, profit should be unlikely
        assert!(
            results.prob_profit < 0.3,
            "prob_profit was {}, expected low",
            results.prob_profit
        );
    }

    #[test]
    fn prob_profit_reasonable_for_edge_strategy() {
        let config = MonteCarloConfig::new(1000, 100, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        // With 55% win rate at even odds, profit should be likely
        assert!(
            results.prob_profit > 0.5,
            "prob_profit was {}, expected > 0.5",
            results.prob_profit
        );
    }

    // ============================================================
    // simulate_from_settlements Tests
    // ============================================================

    #[test]
    fn simulate_from_settlements_uses_empirical_distribution() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // 60% win rate from settlements
        let settlements = create_mixed_settlements(60, 40);

        let results = simulator.simulate_from_settlements(&settlements);

        // With 60% empirical win rate, should have positive expectation
        assert!(
            results.prob_profit > 0.5,
            "prob_profit was {}",
            results.prob_profit
        );
    }

    #[test]
    fn simulate_from_settlements_empty_returns_zeros() {
        let config = MonteCarloConfig::new(100, 50, dec!(10000)).with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let settlements: Vec<SettlementResult> = vec![];

        let results = simulator.simulate_from_settlements(&settlements);

        assert!((results.prob_ruin - 0.0).abs() < f64::EPSILON);
        assert!((results.prob_profit - 0.0).abs() < f64::EPSILON);
        assert_eq!(results.median_equity, Decimal::ZERO);
    }

    #[test]
    fn simulate_from_settlements_all_wins_high_profit_prob() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let settlements = create_mixed_settlements(100, 0);

        let results = simulator.simulate_from_settlements(&settlements);

        assert!(
            (results.prob_profit - 1.0).abs() < f64::EPSILON,
            "prob_profit was {}",
            results.prob_profit
        );
    }

    #[test]
    fn simulate_from_settlements_all_losses_high_ruin_prob() {
        let config = MonteCarloConfig::new(1000, 100, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(500)))
            .with_ruin_threshold(dec!(0.1))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let settlements = create_mixed_settlements(0, 100);

        let results = simulator.simulate_from_settlements(&settlements);

        // 0% win rate should lead to ruin
        assert!(
            results.prob_ruin > 0.9,
            "prob_ruin was {}, expected high",
            results.prob_ruin
        );
    }

    // ============================================================
    // simulate_parametric Tests
    // ============================================================

    #[test]
    fn simulate_parametric_basic() {
        let config = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        // Basic sanity checks
        assert!(results.prob_ruin >= 0.0 && results.prob_ruin <= 1.0);
        assert!(results.prob_profit >= 0.0 && results.prob_profit <= 1.0);
        assert!(results.prob_double >= 0.0 && results.prob_double <= 1.0);
        assert!(results.median_equity > Decimal::ZERO);
    }

    #[test]
    fn simulate_parametric_returns_correct_counts() {
        let config = MonteCarloConfig::new(500, 50, dec!(10000)).with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        assert_eq!(results.n_simulations, 500);
        assert_eq!(results.n_bets, 50);
    }

    // ============================================================
    // Kelly Sizing Tests
    // ============================================================

    #[test]
    fn kelly_sizing_respects_fraction() {
        let config = MonteCarloConfig::new(100, 20, dec!(10000))
            .with_sizing(BetSizing::Kelly {
                fraction: dec!(0.25), // Quarter Kelly
                min_edge: dec!(0.0),
            })
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // With 60% win rate at 50% odds, full Kelly would be aggressive
        // Quarter Kelly should be more conservative
        let results = simulator.simulate_parametric(0.60, dec!(0.50));

        // Should still profit with edge but be more stable
        assert!(
            results.prob_profit > 0.5,
            "prob_profit was {}",
            results.prob_profit
        );
    }

    #[test]
    fn kelly_sizing_respects_min_edge() {
        let config = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Kelly {
                fraction: dec!(0.25),
                min_edge: dec!(0.10), // Require 10% edge
            })
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // 55% win rate at 50% odds = 5% edge, below threshold
        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        // No bets should be placed, equity stays at initial
        assert_eq!(
            results.median_equity,
            dec!(10000),
            "No bets should be placed when edge below threshold"
        );
    }

    #[test]
    fn kelly_sizing_bets_when_edge_sufficient() {
        let config = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Kelly {
                fraction: dec!(0.25),
                min_edge: dec!(0.05), // Require 5% edge
            })
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // 60% win rate at 50% odds = 10% edge, above threshold
        let results = simulator.simulate_parametric(0.60, dec!(0.50));

        // Bets should be placed and equity should change
        assert_ne!(
            results.median_equity,
            dec!(10000),
            "Bets should be placed when edge above threshold"
        );
    }

    // ============================================================
    // Reproducibility Tests
    // ============================================================

    #[test]
    fn simulation_reproducible_with_seed() {
        let config1 = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(12345);
        let simulator1 = MonteCarloSimulator::new(config1);
        let results1 = simulator1.simulate_parametric(0.55, dec!(0.50));

        let config2 = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(12345);
        let simulator2 = MonteCarloSimulator::new(config2);
        let results2 = simulator2.simulate_parametric(0.55, dec!(0.50));

        assert!(
            (results1.prob_ruin - results2.prob_ruin).abs() < f64::EPSILON,
            "prob_ruin should be identical"
        );
        assert!(
            (results1.prob_profit - results2.prob_profit).abs() < f64::EPSILON,
            "prob_profit should be identical"
        );
        assert_eq!(results1.median_equity, results2.median_equity);
    }

    #[test]
    fn simulation_different_with_different_seeds() {
        let config1 = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(11111);
        let simulator1 = MonteCarloSimulator::new(config1);
        let results1 = simulator1.simulate_parametric(0.55, dec!(0.50));

        let config2 = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(22222);
        let simulator2 = MonteCarloSimulator::new(config2);
        let results2 = simulator2.simulate_parametric(0.55, dec!(0.50));

        // Results should differ (though it's possible they're equal by chance)
        let differs = results1.median_equity != results2.median_equity
            || results1.prob_ruin != results2.prob_ruin;
        assert!(differs, "Results should differ with different seeds");
    }

    // ============================================================
    // DistributionSummary Tests
    // ============================================================

    #[test]
    fn distribution_summary_empty() {
        let values: Vec<Decimal> = vec![];
        let summary = DistributionSummary::from_values(&values);

        assert_eq!(summary.min, Decimal::ZERO);
        assert_eq!(summary.max, Decimal::ZERO);
        assert_eq!(summary.mean, Decimal::ZERO);
        assert_eq!(summary.median, Decimal::ZERO);
    }

    #[test]
    fn distribution_summary_single_value() {
        let values = vec![dec!(100)];
        let summary = DistributionSummary::from_values(&values);

        assert_eq!(summary.min, dec!(100));
        assert_eq!(summary.max, dec!(100));
        assert_eq!(summary.mean, dec!(100));
        assert_eq!(summary.median, dec!(100));
    }

    #[test]
    fn distribution_summary_calculates_min_max() {
        let values = vec![dec!(50), dec!(100), dec!(150), dec!(200), dec!(250)];
        let summary = DistributionSummary::from_values(&values);

        assert_eq!(summary.min, dec!(50));
        assert_eq!(summary.max, dec!(250));
    }

    #[test]
    fn distribution_summary_calculates_mean() {
        let values = vec![dec!(100), dec!(200), dec!(300)];
        let summary = DistributionSummary::from_values(&values);

        // Mean = (100 + 200 + 300) / 3 = 200
        assert_eq!(summary.mean, dec!(200));
    }

    #[test]
    fn distribution_summary_calculates_median_odd() {
        let values = vec![dec!(10), dec!(20), dec!(30), dec!(40), dec!(50)];
        let summary = DistributionSummary::from_values(&values);

        // Median of 5 values is the 3rd value = 30
        assert_eq!(summary.median, dec!(30));
    }

    #[test]
    fn distribution_summary_calculates_percentiles() {
        let values: Vec<Decimal> = (1..=100).map(|i| Decimal::from(i)).collect();
        let summary = DistributionSummary::from_values(&values);

        // Check that percentiles exist
        assert!(!summary.percentiles.is_empty());

        // 5th percentile should be around 5
        let p5 = summary
            .percentiles
            .iter()
            .find(|(p, _)| (*p - 0.05).abs() < 0.01);
        assert!(p5.is_some());
    }

    // ============================================================
    // MonteCarloResults Tests
    // ============================================================

    #[test]
    fn results_is_favorable_positive_expectation() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // 60% win rate should be favorable
        let results = simulator.simulate_parametric(0.60, dec!(0.50));

        assert!(results.is_favorable(), "60% win rate should be favorable");
    }

    #[test]
    fn results_not_favorable_negative_expectation() {
        let config = MonteCarloConfig::new(1000, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        // 40% win rate should not be favorable
        let results = simulator.simulate_parametric(0.40, dec!(0.50));

        assert!(
            !results.is_favorable(),
            "40% win rate should not be favorable"
        );
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn handles_very_small_bankroll() {
        let config = MonteCarloConfig::new(100, 10, dec!(100))
            .with_sizing(BetSizing::Fixed(dec!(10)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        // Should not panic
        assert!(results.prob_ruin >= 0.0);
    }

    #[test]
    fn handles_very_large_bet_size() {
        let config = MonteCarloConfig::new(100, 10, dec!(1000))
            .with_sizing(BetSizing::Fixed(dec!(500))) // 50% of bankroll per bet
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.50, dec!(0.50));

        // With large bets at even odds, ruin should be likely
        assert!(results.prob_ruin > 0.0);
    }

    #[test]
    fn handles_fraction_of_bankroll_sizing() {
        let config = MonteCarloConfig::new(100, 50, dec!(10000))
            .with_sizing(BetSizing::FractionOfBankroll(dec!(0.02))) // 2% per bet
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        // 2% betting with edge should have low ruin
        assert!(
            results.prob_ruin < 0.1,
            "prob_ruin was {}",
            results.prob_ruin
        );
    }

    #[test]
    fn prob_double_reasonable() {
        let config = MonteCarloConfig::new(1000, 100, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(200)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);

        let results = simulator.simulate_parametric(0.60, dec!(0.50));

        // With 60% win rate and decent bet sizes, doubling should be possible
        assert!(
            results.prob_double > 0.0,
            "prob_double was {}",
            results.prob_double
        );
        // But not guaranteed
        assert!(
            results.prob_double < 1.0,
            "prob_double was {}",
            results.prob_double
        );
    }

    // ============================================================
    // Serialization Tests
    // ============================================================

    #[test]
    fn config_serialization_roundtrip() {
        let config = MonteCarloConfig::new(500, 100, dec!(5000))
            .with_sizing(BetSizing::Kelly {
                fraction: dec!(0.25),
                min_edge: dec!(0.02),
            })
            .with_seed(42);

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MonteCarloConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.n_simulations, 500);
        assert_eq!(deserialized.n_bets, 100);
        assert_eq!(deserialized.initial_bankroll, dec!(5000));
        assert_eq!(deserialized.seed, Some(42));
    }

    #[test]
    fn results_serialization_roundtrip() {
        let config = MonteCarloConfig::new(100, 20, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);
        let simulator = MonteCarloSimulator::new(config);
        let results = simulator.simulate_parametric(0.55, dec!(0.50));

        let json = serde_json::to_string(&results).unwrap();
        let deserialized: MonteCarloResults = serde_json::from_str(&json).unwrap();

        assert!((deserialized.prob_ruin - results.prob_ruin).abs() < f64::EPSILON);
        assert!((deserialized.prob_profit - results.prob_profit).abs() < f64::EPSILON);
        assert_eq!(deserialized.median_equity, results.median_equity);
    }

    // ============================================================
    // Statistical Property Tests
    // ============================================================

    #[test]
    fn higher_win_rate_leads_to_higher_profit_prob() {
        let config = MonteCarloConfig::new(500, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(100)))
            .with_seed(42);

        let simulator_50 = MonteCarloSimulator::new(config.clone());
        let results_50 = simulator_50.simulate_parametric(0.50, dec!(0.50));

        let simulator_60 = MonteCarloSimulator::new(config.clone());
        let results_60 = simulator_60.simulate_parametric(0.60, dec!(0.50));

        assert!(
            results_60.prob_profit > results_50.prob_profit,
            "Higher win rate should have higher profit probability"
        );
    }

    #[test]
    fn larger_bet_size_increases_volatility() {
        let small_bet_config = MonteCarloConfig::new(500, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(50)))
            .with_seed(42);
        let small_bet_simulator = MonteCarloSimulator::new(small_bet_config);
        let small_results = small_bet_simulator.simulate_parametric(0.55, dec!(0.50));

        let large_bet_config = MonteCarloConfig::new(500, 50, dec!(10000))
            .with_sizing(BetSizing::Fixed(dec!(500)))
            .with_seed(42);
        let large_bet_simulator = MonteCarloSimulator::new(large_bet_config);
        let large_results = large_bet_simulator.simulate_parametric(0.55, dec!(0.50));

        // Larger bets should have wider equity distribution (higher std dev)
        assert!(
            large_results.distributions.std_dev > small_results.distributions.std_dev,
            "Larger bets should increase volatility"
        );
    }
}
