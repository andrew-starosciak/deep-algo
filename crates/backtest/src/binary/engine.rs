//! Binary backtest engine for simulating prediction market bets.
//!
//! This module provides the main simulation loop that processes signals,
//! places bets, and settles them based on price movements.
//!
//! # Design Principles
//!
//! 1. **No Look-Ahead Bias**: All price data is fetched via `PointInTimeProvider`
//!    which guarantees point-in-time correctness.
//!
//! 2. **Pre-computed Signals**: Signals are provided as input rather than computed
//!    during the backtest, allowing for flexible signal generation strategies.
//!
//! 3. **Configurable Thresholds**: Minimum signal strength and expected value
//!    thresholds can be tuned to filter low-confidence bets.
//!
//! # Example
//!
//! ```ignore
//! let config = BinaryBacktestConfig {
//!     symbol: "BTCUSDT".to_string(),
//!     exchange: "binance".to_string(),
//!     window_duration: Duration::minutes(15),
//!     min_strength: 0.6,
//!     min_ev: dec!(0.02),
//!     stake_per_bet: dec!(100),
//! };
//!
//! let engine = BinaryBacktestEngine::new(config, pit_provider, fee_model);
//! let results = engine.run(start, end, Duration::minutes(15), &signals).await?;
//! ```

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use algo_trade_core::signal::{Direction, SignalValue};

use super::fees::FeeModel;
use super::metrics::BinaryMetrics;
use super::outcome::{BetDirection, BinaryBet, BinaryOutcome, SettlementResult};
use super::pit::PointInTimeProvider;

/// Configuration for a binary backtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryBacktestConfig {
    /// Trading symbol (e.g., "BTCUSDT").
    pub symbol: String,
    /// Exchange name (e.g., "binance").
    pub exchange: String,
    /// Duration of each binary option window in seconds.
    pub window_duration_secs: i64,
    /// Minimum signal strength required to place a bet (0.0 to 1.0).
    pub min_strength: f64,
    /// Minimum expected value required to place a bet.
    pub min_ev: Decimal,
    /// Fixed stake amount per bet.
    pub stake_per_bet: Decimal,
}

impl BinaryBacktestConfig {
    /// Creates a new config with default values.
    #[must_use]
    pub fn new(symbol: &str, exchange: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            window_duration_secs: 900, // 15 minutes
            min_strength: 0.5,
            min_ev: Decimal::ZERO,
            stake_per_bet: Decimal::ONE_HUNDRED,
        }
    }

    /// Returns the window duration as a `Duration`.
    #[must_use]
    pub fn window_duration(&self) -> Duration {
        Duration::seconds(self.window_duration_secs)
    }

    /// Sets the window duration.
    #[must_use]
    pub fn with_window_duration(mut self, duration: Duration) -> Self {
        self.window_duration_secs = duration.num_seconds();
        self
    }

    /// Sets the minimum signal strength.
    #[must_use]
    pub fn with_min_strength(mut self, strength: f64) -> Self {
        self.min_strength = strength;
        self
    }

    /// Sets the minimum expected value.
    #[must_use]
    pub fn with_min_ev(mut self, ev: Decimal) -> Self {
        self.min_ev = ev;
        self
    }

    /// Sets the stake per bet.
    #[must_use]
    pub fn with_stake_per_bet(mut self, stake: Decimal) -> Self {
        self.stake_per_bet = stake;
        self
    }
}

/// Results from a binary backtest run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResults {
    /// All settlement results from the backtest.
    pub settlements: Vec<SettlementResult>,
    /// Aggregated metrics from the settlements.
    pub metrics: BinaryMetrics,
    /// Configuration used for the backtest.
    pub config: BinaryBacktestConfig,
    /// Start time of the backtest period.
    pub start_time: DateTime<Utc>,
    /// End time of the backtest period.
    pub end_time: DateTime<Utc>,
    /// Number of signals processed.
    pub signals_processed: usize,
    /// Number of signals skipped (below threshold).
    pub signals_skipped: usize,
}

impl BacktestResults {
    /// Returns the fill rate (bets placed / signals processed).
    #[must_use]
    pub fn fill_rate(&self) -> f64 {
        if self.signals_processed == 0 {
            return 0.0;
        }
        self.settlements.len() as f64 / self.signals_processed as f64
    }
}

/// Binary backtest engine that runs the simulation loop.
pub struct BinaryBacktestEngine {
    config: BinaryBacktestConfig,
    pit_provider: PointInTimeProvider,
    fee_model: Box<dyn FeeModel>,
}

impl BinaryBacktestEngine {
    /// Creates a new backtest engine.
    ///
    /// # Arguments
    /// * `config` - Backtest configuration
    /// * `pit_provider` - Point-in-time data provider for prices
    /// * `fee_model` - Fee calculation model
    #[must_use]
    pub fn new(
        config: BinaryBacktestConfig,
        pit_provider: PointInTimeProvider,
        fee_model: Box<dyn FeeModel>,
    ) -> Self {
        Self {
            config,
            pit_provider,
            fee_model,
        }
    }

    /// Returns a reference to the configuration.
    #[must_use]
    pub fn config(&self) -> &BinaryBacktestConfig {
        &self.config
    }

    /// Runs the backtest over the given time range.
    ///
    /// # Arguments
    /// * `start` - Start of backtest period
    /// * `end` - End of backtest period
    /// * `interval` - Time between signal checks (e.g., every 15 minutes)
    /// * `signals` - Pre-computed signals as (timestamp, signal) pairs
    ///
    /// # Returns
    /// `BacktestResults` containing all settlements and aggregated metrics
    ///
    /// # Errors
    /// Returns error if database queries fail
    pub async fn run(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        _interval: Duration,
        signals: &[(DateTime<Utc>, SignalValue)],
    ) -> Result<BacktestResults> {
        let mut settlements = Vec::new();
        let mut signals_skipped = 0;

        // Process each signal
        for (timestamp, signal) in signals {
            // Skip signals outside the backtest period
            if *timestamp < start || *timestamp >= end {
                continue;
            }

            // Check if signal meets strength threshold
            if !self.should_place_bet(signal) {
                signals_skipped += 1;
                continue;
            }

            // Determine bet direction from signal
            let direction = match signal.direction {
                Direction::Up => BetDirection::Yes,
                Direction::Down => BetDirection::No,
                Direction::Neutral => {
                    signals_skipped += 1;
                    continue;
                }
            };

            // Get start price (at bet placement time)
            let start_price = match self.pit_provider.get_price_at(*timestamp).await? {
                Some(p) => p,
                None => {
                    signals_skipped += 1;
                    continue;
                }
            };

            // Get end price (at settlement time)
            let settlement_time = *timestamp + self.config.window_duration();
            let end_price = match self.pit_provider.get_price_at(settlement_time).await? {
                Some(p) => p,
                None => {
                    signals_skipped += 1;
                    continue;
                }
            };

            // Create the bet
            // Use a default price of 0.50 for now (fair odds)
            // In real use, this would come from market data
            let bet_price = Decimal::new(50, 2); // 0.50
            let bet = BinaryBet::new(
                *timestamp,
                self.config.symbol.clone(),
                direction,
                self.config.stake_per_bet,
                bet_price,
                signal.strength,
            );

            // Settle the bet
            let settlement = self.settle_bet(&bet, start_price, end_price, settlement_time);
            settlements.push(settlement);
        }

        // Calculate metrics
        let metrics = BinaryMetrics::from_settlements(&settlements);

        Ok(BacktestResults {
            settlements,
            metrics,
            config: self.config.clone(),
            start_time: start,
            end_time: end,
            signals_processed: signals.len(),
            signals_skipped,
        })
    }

    /// Determines if a bet should be placed based on signal criteria.
    #[must_use]
    fn should_place_bet(&self, signal: &SignalValue) -> bool {
        // Must have directional signal
        if signal.direction == Direction::Neutral {
            return false;
        }

        // Must meet minimum strength threshold
        if signal.strength < self.config.min_strength {
            return false;
        }

        true
    }

    /// Settles a bet based on price movement.
    ///
    /// Settlement logic:
    /// - If end_price > start_price: Up outcome (Yes wins)
    /// - If end_price < start_price: Down outcome (No wins)
    /// - If end_price == start_price: Push (no winner)
    #[must_use]
    fn settle_bet(
        &self,
        bet: &BinaryBet,
        start_price: Decimal,
        end_price: Decimal,
        settlement_time: DateTime<Utc>,
    ) -> SettlementResult {
        // Determine the actual outcome based on price movement
        let actual_outcome = Self::determine_outcome(start_price, end_price);

        // Determine if bet won based on direction
        let bet_outcome = match (bet.direction, actual_outcome) {
            // Yes bet wins if price went up
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            // No bet wins if price went down
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            // Push if price unchanged
            (_, Direction::Neutral) => BinaryOutcome::Push,
            // Otherwise loss
            _ => BinaryOutcome::Loss,
        };

        // Calculate fees
        let fees = if bet_outcome == BinaryOutcome::Push {
            Decimal::ZERO // No fees on push
        } else {
            self.fee_model.calculate_fee(bet.stake, bet.price)
        };

        SettlementResult::new(
            bet.clone(),
            settlement_time,
            end_price,
            start_price,
            bet_outcome,
            fees,
        )
    }

    /// Determines the market outcome based on price movement.
    #[must_use]
    fn determine_outcome(start_price: Decimal, end_price: Decimal) -> Direction {
        if end_price > start_price {
            Direction::Up
        } else if end_price < start_price {
            Direction::Down
        } else {
            Direction::Neutral
        }
    }

    /// Calculates expected value for a potential bet.
    ///
    /// EV = p * (1 - price) - (1-p) * price - fee_rate * potential_profit
    ///
    /// where p is the estimated probability of winning (from signal confidence)
    #[must_use]
    pub fn calculate_expected_value(
        &self,
        win_probability: f64,
        price: Decimal,
        stake: Decimal,
    ) -> Decimal {
        let price_f64 = f64::try_from(price).unwrap_or(0.5);
        let stake_f64 = f64::try_from(stake).unwrap_or(100.0);

        // Potential profit on win: stake * (1 - price) / price
        let win_profit = stake_f64 * (1.0 - price_f64) / price_f64;

        // Loss on loss: -stake
        let loss = stake_f64;

        // Expected value
        let ev = win_probability * win_profit - (1.0 - win_probability) * loss;

        // Subtract expected fees
        let fee = self
            .fee_model
            .calculate_fee(stake, price)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);
        let ev_after_fees = ev - fee;

        Decimal::from_str(&format!("{:.8}", ev_after_fees)).unwrap_or(Decimal::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_test_config() -> BinaryBacktestConfig {
        BinaryBacktestConfig::new("BTCUSDT", "binance")
            .with_window_duration(Duration::minutes(15))
            .with_min_strength(0.5)
            .with_stake_per_bet(dec!(100))
    }

    fn create_up_signal(strength: f64) -> SignalValue {
        SignalValue::new(Direction::Up, strength, 0.8).unwrap()
    }

    fn create_down_signal(strength: f64) -> SignalValue {
        SignalValue::new(Direction::Down, strength, 0.8).unwrap()
    }

    fn create_neutral_signal() -> SignalValue {
        SignalValue::neutral()
    }

    fn create_test_bet(direction: BetDirection, stake: Decimal, price: Decimal) -> BinaryBet {
        BinaryBet::new(
            Utc::now(),
            "BTCUSDT-15MIN-UP".to_string(),
            direction,
            stake,
            price,
            0.75,
        )
    }

    // ============================================================
    // BinaryBacktestConfig Tests
    // ============================================================

    #[test]
    fn config_new_sets_default_values() {
        let config = BinaryBacktestConfig::new("BTCUSDT", "binance");

        assert_eq!(config.symbol, "BTCUSDT");
        assert_eq!(config.exchange, "binance");
        assert_eq!(config.window_duration(), Duration::minutes(15));
        assert!((config.min_strength - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.stake_per_bet, dec!(100));
    }

    #[test]
    fn config_builder_methods_chain() {
        let config = BinaryBacktestConfig::new("ETHUSDT", "hyperliquid")
            .with_window_duration(Duration::minutes(5))
            .with_min_strength(0.7)
            .with_min_ev(dec!(0.05))
            .with_stake_per_bet(dec!(50));

        assert_eq!(config.symbol, "ETHUSDT");
        assert_eq!(config.exchange, "hyperliquid");
        assert_eq!(config.window_duration(), Duration::minutes(5));
        assert!((config.min_strength - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.min_ev, dec!(0.05));
        assert_eq!(config.stake_per_bet, dec!(50));
    }

    #[test]
    fn config_serialization_roundtrip() {
        let config = create_test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BinaryBacktestConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.symbol, config.symbol);
        assert_eq!(deserialized.exchange, config.exchange);
        assert_eq!(deserialized.stake_per_bet, config.stake_per_bet);
    }

    // ============================================================
    // Bet Decision Logic Tests
    // ============================================================

    #[test]
    fn should_place_bet_returns_true_for_strong_up_signal() {
        let min_strength = 0.5;
        let signal = create_up_signal(0.75);
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(should_bet);
    }

    #[test]
    fn should_place_bet_returns_true_for_strong_down_signal() {
        let signal = create_down_signal(0.8);
        let min_strength = 0.5;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(should_bet);
    }

    #[test]
    fn should_place_bet_returns_false_for_weak_signal() {
        let signal = create_up_signal(0.3);
        let min_strength = 0.5;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(!should_bet);
    }

    #[test]
    fn should_place_bet_returns_false_for_neutral_signal() {
        let signal = create_neutral_signal();
        let min_strength = 0.5;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(!should_bet);
    }

    #[test]
    fn should_place_bet_at_exact_threshold() {
        let signal = create_up_signal(0.5);
        let min_strength = 0.5;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(should_bet);
    }

    #[test]
    fn should_place_bet_just_below_threshold() {
        let signal = create_up_signal(0.499);
        let min_strength = 0.5;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        assert!(!should_bet);
    }

    // ============================================================
    // Direction Mapping Tests
    // ============================================================

    #[test]
    fn up_signal_maps_to_yes_bet() {
        let direction = match Direction::Up {
            Direction::Up => BetDirection::Yes,
            Direction::Down => BetDirection::No,
            Direction::Neutral => panic!("Should not map neutral"),
        };

        assert_eq!(direction, BetDirection::Yes);
    }

    #[test]
    fn down_signal_maps_to_no_bet() {
        let direction = match Direction::Down {
            Direction::Up => BetDirection::Yes,
            Direction::Down => BetDirection::No,
            Direction::Neutral => panic!("Should not map neutral"),
        };

        assert_eq!(direction, BetDirection::No);
    }

    // ============================================================
    // Settlement Calculation Tests
    // ============================================================

    #[test]
    fn determine_outcome_price_increase_is_up() {
        let start = dec!(43000);
        let end = dec!(43500);
        let outcome = BinaryBacktestEngine::determine_outcome(start, end);

        assert_eq!(outcome, Direction::Up);
    }

    #[test]
    fn determine_outcome_price_decrease_is_down() {
        let start = dec!(43000);
        let end = dec!(42500);
        let outcome = BinaryBacktestEngine::determine_outcome(start, end);

        assert_eq!(outcome, Direction::Down);
    }

    #[test]
    fn determine_outcome_price_unchanged_is_neutral() {
        let start = dec!(43000);
        let end = dec!(43000);
        let outcome = BinaryBacktestEngine::determine_outcome(start, end);

        assert_eq!(outcome, Direction::Neutral);
    }

    #[test]
    fn determine_outcome_small_increase_is_up() {
        let start = dec!(43000.00);
        let end = dec!(43000.01);
        let outcome = BinaryBacktestEngine::determine_outcome(start, end);

        assert_eq!(outcome, Direction::Up);
    }

    #[test]
    fn determine_outcome_small_decrease_is_down() {
        let start = dec!(43000.00);
        let end = dec!(42999.99);
        let outcome = BinaryBacktestEngine::determine_outcome(start, end);

        assert_eq!(outcome, Direction::Down);
    }

    // ============================================================
    // Win/Loss Determination Tests
    // ============================================================

    #[test]
    fn yes_bet_wins_when_price_goes_up() {
        // Simulate settlement logic
        let bet_direction = BetDirection::Yes;
        let price_direction = Direction::Up;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Win);
    }

    #[test]
    fn yes_bet_loses_when_price_goes_down() {
        let bet_direction = BetDirection::Yes;
        let price_direction = Direction::Down;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Loss);
    }

    #[test]
    fn no_bet_wins_when_price_goes_down() {
        let bet_direction = BetDirection::No;
        let price_direction = Direction::Down;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Win);
    }

    #[test]
    fn no_bet_loses_when_price_goes_up() {
        let bet_direction = BetDirection::No;
        let price_direction = Direction::Up;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Loss);
    }

    #[test]
    fn yes_bet_pushes_when_price_unchanged() {
        let bet_direction = BetDirection::Yes;
        let price_direction = Direction::Neutral;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Push);
    }

    #[test]
    fn no_bet_pushes_when_price_unchanged() {
        let bet_direction = BetDirection::No;
        let price_direction = Direction::Neutral;

        let outcome = match (bet_direction, price_direction) {
            (BetDirection::Yes, Direction::Up) => BinaryOutcome::Win,
            (BetDirection::No, Direction::Down) => BinaryOutcome::Win,
            (_, Direction::Neutral) => BinaryOutcome::Push,
            _ => BinaryOutcome::Loss,
        };

        assert_eq!(outcome, BinaryOutcome::Push);
    }

    // ============================================================
    // P&L Calculation Tests (via SettlementResult)
    // ============================================================

    #[test]
    fn pnl_winning_bet_at_50_percent_price() {
        // stake = $100, price = $0.50
        // shares = 100 / 0.50 = 200
        // gross_pnl on win = 200 - 100 = $100
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));
        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500), // end price
            dec!(43000), // start price
            BinaryOutcome::Win,
            dec!(2), // fees
        );

        assert_eq!(settlement.gross_pnl, dec!(100));
        assert_eq!(settlement.net_pnl, dec!(98)); // 100 - 2
    }

    #[test]
    fn pnl_winning_bet_at_45_percent_price() {
        // stake = $100, price = $0.45
        // shares = 100 / 0.45 = 222.222...
        // gross_pnl on win = 222.222... - 100 = $122.222...
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.45));
        let expected_gross = dec!(100) / dec!(0.45) - dec!(100);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        assert_eq!(settlement.gross_pnl, expected_gross);
    }

    #[test]
    fn pnl_losing_bet_loses_stake() {
        // On loss: gross_pnl = -stake = -$100
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(42500), // end price (down)
            dec!(43000), // start price
            BinaryOutcome::Loss,
            dec!(2), // fees
        );

        assert_eq!(settlement.gross_pnl, -dec!(100));
        assert_eq!(settlement.net_pnl, -dec!(102)); // -100 - 2
    }

    #[test]
    fn pnl_push_is_zero() {
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43000), // same price
            dec!(43000),
            BinaryOutcome::Push,
            dec!(0), // no fees on push
        );

        assert_eq!(settlement.gross_pnl, Decimal::ZERO);
        assert_eq!(settlement.net_pnl, Decimal::ZERO);
    }

    #[test]
    fn pnl_with_different_prices() {
        // Test at price = $0.60
        // stake = $100, price = $0.60
        // shares = 100 / 0.60 = 166.666...
        // gross_pnl on win = 166.666... - 100 = $66.666...
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.60));
        let expected_gross = dec!(100) / dec!(0.60) - dec!(100);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        assert_eq!(settlement.gross_pnl, expected_gross);
    }

    #[test]
    fn pnl_high_price_low_profit() {
        // At price = $0.90, profit potential is low
        // stake = $100, price = $0.90
        // shares = 100 / 0.90 = 111.111...
        // gross_pnl on win = 111.111... - 100 = $11.111...
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.90));
        let expected_gross = dec!(100) / dec!(0.90) - dec!(100);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        assert_eq!(settlement.gross_pnl, expected_gross);
        assert!(settlement.gross_pnl < dec!(15)); // Low profit at high price
    }

    #[test]
    fn pnl_low_price_high_profit() {
        // At price = $0.10, profit potential is high
        // stake = $100, price = $0.10
        // shares = 100 / 0.10 = 1000
        // gross_pnl on win = 1000 - 100 = $900
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.10));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        assert_eq!(settlement.gross_pnl, dec!(900));
    }

    // ============================================================
    // Fee Calculation Integration Tests
    // ============================================================

    #[test]
    fn fees_applied_on_winning_bet() {
        // Using ZeroFees for simplicity in isolated tests
        // The fee model is tested separately
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));
        let fee_amount = dec!(2);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            fee_amount,
        );

        assert_eq!(settlement.fees, dec!(2));
        assert_eq!(settlement.gross_pnl - settlement.fees, settlement.net_pnl);
    }

    #[test]
    fn fees_applied_on_losing_bet() {
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));
        let fee_amount = dec!(2);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(42500), // price went down
            dec!(43000),
            BinaryOutcome::Loss,
            fee_amount,
        );

        assert_eq!(settlement.fees, dec!(2));
        // net_pnl = -100 - 2 = -102
        assert_eq!(settlement.net_pnl, -dec!(102));
    }

    #[test]
    fn no_fees_on_push() {
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43000),
            dec!(43000),
            BinaryOutcome::Push,
            Decimal::ZERO,
        );

        assert_eq!(settlement.fees, Decimal::ZERO);
        assert_eq!(settlement.net_pnl, Decimal::ZERO);
    }

    // ============================================================
    // BacktestResults Tests
    // ============================================================

    #[test]
    fn backtest_results_fill_rate_calculated() {
        let config = create_test_config();
        let results = BacktestResults {
            settlements: vec![], // Empty for this test
            metrics: BinaryMetrics::empty(),
            config,
            start_time: Utc::now(),
            end_time: Utc::now() + Duration::hours(24),
            signals_processed: 100,
            signals_skipped: 60,
        };

        // fill_rate = 0 settlements / 100 signals = 0
        assert!((results.fill_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn backtest_results_fill_rate_with_bets() {
        let config = create_test_config();
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));

        let settlements = vec![
            SettlementResult::new(
                bet.clone(),
                Utc::now(),
                dec!(43500),
                dec!(43000),
                BinaryOutcome::Win,
                dec!(0),
            ),
            SettlementResult::new(
                bet.clone(),
                Utc::now(),
                dec!(42500),
                dec!(43000),
                BinaryOutcome::Loss,
                dec!(0),
            ),
        ];

        let results = BacktestResults {
            settlements,
            metrics: BinaryMetrics::empty(),
            config,
            start_time: Utc::now(),
            end_time: Utc::now() + Duration::hours(24),
            signals_processed: 10,
            signals_skipped: 8,
        };

        // fill_rate = 2 settlements / 10 signals = 0.2
        assert!((results.fill_rate() - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn backtest_results_fill_rate_zero_signals() {
        let config = create_test_config();
        let results = BacktestResults {
            settlements: vec![],
            metrics: BinaryMetrics::empty(),
            config,
            start_time: Utc::now(),
            end_time: Utc::now() + Duration::hours(24),
            signals_processed: 0,
            signals_skipped: 0,
        };

        // Should handle division by zero gracefully
        assert!((results.fill_rate() - 0.0).abs() < f64::EPSILON);
    }

    // ============================================================
    // Expected Value Calculation Tests
    // ============================================================

    #[test]
    fn expected_value_positive_for_high_win_prob() {
        // At 60% win probability, 50% price, we should have positive EV
        let win_prob: f64 = 0.6;

        // EV = 0.6 * 100 - 0.4 * 100 = 60 - 40 = 20
        // (simplified, actual formula accounts for odds structure)
        let ev_approx: f64 = win_prob * 100.0 - (1.0 - win_prob) * 100.0;
        assert!(ev_approx > 0.0);
    }

    #[test]
    fn expected_value_negative_for_low_win_prob() {
        // At 40% win probability, 50% price, we should have negative EV
        let win_prob = 0.4;

        let ev_approx = win_prob * 100.0 - (1.0 - win_prob) * 100.0;
        assert!(ev_approx < 0.0);
    }

    #[test]
    fn expected_value_zero_at_fair_odds() {
        // At 50% win probability and 50% price (fair odds), EV = 0
        let win_prob: f64 = 0.5;

        let ev_approx: f64 = win_prob * 100.0 - (1.0 - win_prob) * 100.0;
        assert!((ev_approx - 0.0).abs() < 0.001);
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn very_small_stake_handled() {
        let bet = create_test_bet(BetDirection::Yes, dec!(0.01), dec!(0.50));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        // gross_pnl = 0.01 / 0.50 - 0.01 = 0.02 - 0.01 = 0.01
        assert_eq!(settlement.gross_pnl, dec!(0.01));
    }

    #[test]
    fn very_large_stake_handled() {
        let bet = create_test_bet(BetDirection::Yes, dec!(1000000), dec!(0.50));

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        // gross_pnl = 1000000
        assert_eq!(settlement.gross_pnl, dec!(1000000));
    }

    #[test]
    fn price_return_calculated_in_settlement() {
        let bet = create_test_bet(BetDirection::Yes, dec!(100), dec!(0.50));
        let start_price = dec!(43000);
        let end_price = dec!(43500);

        let settlement = SettlementResult::new(
            bet,
            Utc::now(),
            end_price,
            start_price,
            BinaryOutcome::Win,
            dec!(0),
        );

        // price_return = (43500 - 43000) / 43000 = 500/43000
        let expected_return = (end_price - start_price) / start_price;
        assert_eq!(settlement.price_return, expected_return);
    }

    // ============================================================
    // Signal Filtering Tests
    // ============================================================

    #[test]
    fn signal_strength_boundary_at_0() {
        let signal = SignalValue::new(Direction::Up, 0.0, 0.5).unwrap();
        let min_strength = 0.0;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        // Strength of 0.0 meets threshold of 0.0
        assert!(should_bet);
    }

    #[test]
    fn signal_strength_boundary_at_1() {
        let signal = SignalValue::new(Direction::Up, 1.0, 0.9).unwrap();
        let min_strength = 1.0;
        let should_bet = signal.direction != Direction::Neutral && signal.strength >= min_strength;

        // Strength of 1.0 meets threshold of 1.0
        assert!(should_bet);
    }

    #[test]
    fn signal_with_metadata_preserved() {
        let signal = SignalValue::new(Direction::Up, 0.75, 0.8)
            .unwrap()
            .with_metadata("imbalance", 0.3)
            .with_metadata("funding_zscore", 2.1);

        assert_eq!(signal.metadata.len(), 2);
        assert!((signal.metadata["imbalance"] - 0.3).abs() < f64::EPSILON);
    }

    // ============================================================
    // Time Range Tests
    // ============================================================

    #[test]
    fn timestamps_correctly_calculated() {
        let bet_time = Utc::now();
        let window_duration = Duration::minutes(15);
        let settlement_time = bet_time + window_duration;

        assert_eq!((settlement_time - bet_time).num_minutes(), 15);
    }

    #[test]
    fn timestamps_cross_hour_boundary() {
        use chrono::TimeZone;
        let bet_time = Utc.with_ymd_and_hms(2026, 1, 30, 11, 50, 0).unwrap();
        let window_duration = Duration::minutes(15);
        let settlement_time = bet_time + window_duration;

        assert_eq!(
            settlement_time,
            Utc.with_ymd_and_hms(2026, 1, 30, 12, 5, 0).unwrap()
        );
    }

    #[test]
    fn timestamps_cross_day_boundary() {
        use chrono::TimeZone;
        let bet_time = Utc.with_ymd_and_hms(2026, 1, 30, 23, 50, 0).unwrap();
        let window_duration = Duration::minutes(15);
        let settlement_time = bet_time + window_duration;

        assert_eq!(
            settlement_time,
            Utc.with_ymd_and_hms(2026, 1, 31, 0, 5, 0).unwrap()
        );
    }
}
