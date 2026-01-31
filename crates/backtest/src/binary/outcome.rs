//! Binary outcome types for prediction market backtesting.
//!
//! This module defines the core types for representing binary bets,
//! their outcomes, and settlement results.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Direction of a binary bet (Yes or No on the outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BetDirection {
    /// Betting that the outcome will occur (e.g., BTC > price at settlement).
    Yes,
    /// Betting that the outcome will NOT occur.
    No,
}

/// The result of a binary bet after settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOutcome {
    /// The bet was correct - pays out at $1.00.
    Win,
    /// The bet was incorrect - pays out $0.00.
    Loss,
    /// The bet was canceled or settled at entry price (rare).
    Push,
}

/// A single binary bet placed in the market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryBet {
    /// Unique identifier for this bet.
    pub id: Uuid,
    /// Timestamp when the bet was placed.
    pub timestamp: DateTime<Utc>,
    /// Symbol being bet on (e.g., "BTCUSD-15MIN-UP").
    pub symbol: String,
    /// Direction of the bet (Yes or No).
    pub direction: BetDirection,
    /// Amount staked in USD.
    pub stake: Decimal,
    /// Price paid per share (0.0 to 1.0).
    pub price: Decimal,
    /// Signal strength that triggered this bet (0.0 to 1.0).
    pub signal_strength: f64,
    /// Additional metadata from signals.
    pub signal_metadata: HashMap<String, f64>,
}

impl BinaryBet {
    /// Creates a new binary bet with a generated UUID.
    #[must_use]
    pub fn new(
        timestamp: DateTime<Utc>,
        symbol: String,
        direction: BetDirection,
        stake: Decimal,
        price: Decimal,
        signal_strength: f64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp,
            symbol,
            direction,
            stake,
            price,
            signal_strength,
            signal_metadata: HashMap::new(),
        }
    }

    /// Creates a new binary bet with specified metadata.
    #[must_use]
    pub fn with_metadata(
        timestamp: DateTime<Utc>,
        symbol: String,
        direction: BetDirection,
        stake: Decimal,
        price: Decimal,
        signal_strength: f64,
        metadata: HashMap<String, f64>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp,
            symbol,
            direction,
            stake,
            price,
            signal_strength,
            signal_metadata: metadata,
        }
    }

    /// Calculates the number of shares purchased.
    ///
    /// shares = stake / price
    #[must_use]
    pub fn shares(&self) -> Decimal {
        if self.price == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.stake / self.price
    }

    /// Calculates the maximum potential payout (if bet wins).
    ///
    /// For a winning bet, shares pay out at $1.00 each.
    /// max_payout = shares * $1.00 = stake / price
    #[must_use]
    pub fn max_payout(&self) -> Decimal {
        self.shares()
    }

    /// Calculates the maximum potential profit (before fees).
    ///
    /// max_profit = max_payout - stake = shares - stake
    #[must_use]
    pub fn max_profit(&self) -> Decimal {
        self.max_payout() - self.stake
    }
}

/// The result of settling a binary bet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// The original bet that was settled.
    pub bet: BinaryBet,
    /// Timestamp when the bet was settled.
    pub settlement_time: DateTime<Utc>,
    /// The settlement price (actual price at expiration).
    pub settlement_price: Decimal,
    /// The starting price at bet entry (for calculating return).
    pub start_price: Decimal,
    /// The price return from start to settlement.
    pub price_return: Decimal,
    /// The outcome of the bet.
    pub outcome: BinaryOutcome,
    /// Gross P&L before fees.
    pub gross_pnl: Decimal,
    /// Total fees charged.
    pub fees: Decimal,
    /// Net P&L after fees.
    pub net_pnl: Decimal,
}

impl SettlementResult {
    /// Creates a new settlement result.
    #[must_use]
    pub fn new(
        bet: BinaryBet,
        settlement_time: DateTime<Utc>,
        settlement_price: Decimal,
        start_price: Decimal,
        outcome: BinaryOutcome,
        fees: Decimal,
    ) -> Self {
        let price_return = if start_price != Decimal::ZERO {
            (settlement_price - start_price) / start_price
        } else {
            Decimal::ZERO
        };

        let gross_pnl = match outcome {
            BinaryOutcome::Win => bet.max_payout() - bet.stake,
            BinaryOutcome::Loss => -bet.stake,
            BinaryOutcome::Push => Decimal::ZERO,
        };

        let net_pnl = gross_pnl - fees;

        Self {
            bet,
            settlement_time,
            settlement_price,
            start_price,
            price_return,
            outcome,
            gross_pnl,
            fees,
            net_pnl,
        }
    }

    /// Returns true if the bet was profitable (net_pnl > 0).
    #[must_use]
    pub fn is_profitable(&self) -> bool {
        self.net_pnl > Decimal::ZERO
    }

    /// Calculates return on investment (ROI).
    ///
    /// ROI = net_pnl / stake
    #[must_use]
    pub fn roi(&self) -> Decimal {
        if self.bet.stake == Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.net_pnl / self.bet.stake
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================================
    // BetDirection Tests
    // ============================================================

    #[test]
    fn bet_direction_yes_and_no_are_distinct() {
        let yes = BetDirection::Yes;
        let no = BetDirection::No;

        assert_ne!(yes, no);
        assert_eq!(yes, BetDirection::Yes);
        assert_eq!(no, BetDirection::No);
    }

    #[test]
    fn bet_direction_serializes_correctly() {
        let yes = BetDirection::Yes;
        let no = BetDirection::No;

        let yes_json = serde_json::to_string(&yes).unwrap();
        let no_json = serde_json::to_string(&no).unwrap();

        assert_eq!(yes_json, r#""Yes""#);
        assert_eq!(no_json, r#""No""#);
    }

    #[test]
    fn bet_direction_deserializes_correctly() {
        let yes: BetDirection = serde_json::from_str(r#""Yes""#).unwrap();
        let no: BetDirection = serde_json::from_str(r#""No""#).unwrap();

        assert_eq!(yes, BetDirection::Yes);
        assert_eq!(no, BetDirection::No);
    }

    // ============================================================
    // BinaryOutcome Tests
    // ============================================================

    #[test]
    fn binary_outcome_variants_are_distinct() {
        let win = BinaryOutcome::Win;
        let loss = BinaryOutcome::Loss;
        let push = BinaryOutcome::Push;

        assert_ne!(win, loss);
        assert_ne!(win, push);
        assert_ne!(loss, push);
    }

    #[test]
    fn binary_outcome_serializes_correctly() {
        let win = BinaryOutcome::Win;
        let loss = BinaryOutcome::Loss;
        let push = BinaryOutcome::Push;

        assert_eq!(serde_json::to_string(&win).unwrap(), r#""Win""#);
        assert_eq!(serde_json::to_string(&loss).unwrap(), r#""Loss""#);
        assert_eq!(serde_json::to_string(&push).unwrap(), r#""Push""#);
    }

    // ============================================================
    // BinaryBet Tests
    // ============================================================

    fn create_test_bet() -> BinaryBet {
        BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),  // stake: $100
            dec!(0.45), // price: $0.45 per share
            0.75,       // signal_strength: 75%
        )
    }

    #[test]
    fn binary_bet_new_creates_unique_ids() {
        let bet1 = create_test_bet();
        let bet2 = create_test_bet();

        assert_ne!(bet1.id, bet2.id);
    }

    #[test]
    fn binary_bet_new_sets_fields_correctly() {
        let timestamp = Utc::now();
        let bet = BinaryBet::new(
            timestamp,
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::No,
            dec!(50),
            dec!(0.60),
            0.80,
        );

        assert_eq!(bet.symbol, "BTCUSD-15MIN-UP");
        assert_eq!(bet.direction, BetDirection::No);
        assert_eq!(bet.stake, dec!(50));
        assert_eq!(bet.price, dec!(0.60));
        assert!((bet.signal_strength - 0.80).abs() < f64::EPSILON);
        assert!(bet.signal_metadata.is_empty());
    }

    #[test]
    fn binary_bet_with_metadata_includes_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("order_book_imbalance".to_string(), 0.65);
        metadata.insert("funding_rate".to_string(), -0.02);

        let bet = BinaryBet::with_metadata(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50),
            0.70,
            metadata,
        );

        assert_eq!(bet.signal_metadata.len(), 2);
        assert!((bet.signal_metadata["order_book_imbalance"] - 0.65).abs() < f64::EPSILON);
        assert!((bet.signal_metadata["funding_rate"] - (-0.02)).abs() < f64::EPSILON);
    }

    #[test]
    fn binary_bet_shares_calculated_correctly() {
        // stake = $100, price = $0.45
        // shares = 100 / 0.45 = 222.222...
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.45),
            0.75,
        );

        let expected = dec!(100) / dec!(0.45);
        assert_eq!(bet.shares(), expected);
    }

    #[test]
    fn binary_bet_shares_handles_zero_price() {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0), // zero price
            0.75,
        );

        assert_eq!(bet.shares(), Decimal::ZERO);
    }

    #[test]
    fn binary_bet_max_payout_equals_shares() {
        let bet = create_test_bet();

        // max_payout = shares * $1 = shares
        assert_eq!(bet.max_payout(), bet.shares());
    }

    #[test]
    fn binary_bet_max_profit_calculated_correctly() {
        // stake = $100, price = $0.45
        // shares = 222.222...
        // max_profit = shares - stake = 222.222... - 100 = 122.222...
        let bet = create_test_bet();

        let expected = bet.shares() - dec!(100);
        assert_eq!(bet.max_profit(), expected);
    }

    #[test]
    fn binary_bet_max_profit_at_even_odds() {
        // At price = 0.50, profit potential = stake
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50), // even odds
            0.75,
        );

        // shares = 100 / 0.50 = 200
        // max_profit = 200 - 100 = 100
        assert_eq!(bet.max_profit(), dec!(100));
    }

    #[test]
    fn binary_bet_max_profit_at_high_price() {
        // At price = 0.90, profit potential is low
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.90),
            0.75,
        );

        // shares = 100 / 0.90 = 111.111...
        // max_profit = 111.111... - 100 = 11.111...
        let expected = dec!(100) / dec!(0.90) - dec!(100);
        assert_eq!(bet.max_profit(), expected);
    }

    // ============================================================
    // SettlementResult Tests
    // ============================================================

    fn create_test_settlement_win() -> SettlementResult {
        let bet = create_test_bet();
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);

        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500), // settlement_price
            dec!(43000), // start_price
            BinaryOutcome::Win,
            dec!(2), // fees
        )
    }

    fn create_test_settlement_loss() -> SettlementResult {
        let bet = create_test_bet();
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);

        SettlementResult::new(
            bet,
            settlement_time,
            dec!(42500), // settlement_price (went down)
            dec!(43000), // start_price
            BinaryOutcome::Loss,
            dec!(2), // fees
        )
    }

    #[test]
    fn settlement_result_win_calculates_gross_pnl_correctly() {
        let result = create_test_settlement_win();

        // For a win: gross_pnl = max_payout - stake = shares - stake
        let expected_gross = result.bet.shares() - result.bet.stake;
        assert_eq!(result.gross_pnl, expected_gross);
    }

    #[test]
    fn settlement_result_win_calculates_net_pnl_correctly() {
        let result = create_test_settlement_win();

        // net_pnl = gross_pnl - fees
        let expected_net = result.gross_pnl - dec!(2);
        assert_eq!(result.net_pnl, expected_net);
    }

    #[test]
    fn settlement_result_loss_calculates_gross_pnl_correctly() {
        let result = create_test_settlement_loss();

        // For a loss: gross_pnl = -stake
        assert_eq!(result.gross_pnl, -dec!(100));
    }

    #[test]
    fn settlement_result_loss_calculates_net_pnl_correctly() {
        let result = create_test_settlement_loss();

        // net_pnl = -stake - fees = -100 - 2 = -102
        assert_eq!(result.net_pnl, -dec!(102));
    }

    #[test]
    fn settlement_result_push_has_zero_gross_pnl() {
        let bet = create_test_bet();
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);

        let result = SettlementResult::new(
            bet,
            settlement_time,
            dec!(43000), // same as start
            dec!(43000), // start_price
            BinaryOutcome::Push,
            dec!(0), // no fees on push
        );

        assert_eq!(result.gross_pnl, Decimal::ZERO);
        assert_eq!(result.net_pnl, Decimal::ZERO);
    }

    #[test]
    fn settlement_result_calculates_price_return_correctly() {
        let result = create_test_settlement_win();

        // price_return = (settlement - start) / start = (43500 - 43000) / 43000
        let expected = (dec!(43500) - dec!(43000)) / dec!(43000);
        assert_eq!(result.price_return, expected);
    }

    #[test]
    fn settlement_result_handles_zero_start_price() {
        let bet = create_test_bet();
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);

        let result = SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(0), // zero start price
            BinaryOutcome::Win,
            dec!(2),
        );

        assert_eq!(result.price_return, Decimal::ZERO);
    }

    #[test]
    fn settlement_result_is_profitable_for_winning_bet() {
        let result = create_test_settlement_win();

        assert!(result.is_profitable());
    }

    #[test]
    fn settlement_result_not_profitable_for_losing_bet() {
        let result = create_test_settlement_loss();

        assert!(!result.is_profitable());
    }

    #[test]
    fn settlement_result_roi_calculated_correctly_for_win() {
        let result = create_test_settlement_win();

        // roi = net_pnl / stake
        let expected = result.net_pnl / result.bet.stake;
        assert_eq!(result.roi(), expected);
    }

    #[test]
    fn settlement_result_roi_calculated_correctly_for_loss() {
        let result = create_test_settlement_loss();

        // roi = -102 / 100 = -1.02
        let expected = dec!(-102) / dec!(100);
        assert_eq!(result.roi(), expected);
    }

    #[test]
    fn settlement_result_roi_handles_zero_stake() {
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(0), // zero stake
            dec!(0.45),
            0.75,
        );
        let settlement_time = bet.timestamp + chrono::Duration::minutes(15);

        let result = SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            BinaryOutcome::Win,
            dec!(0),
        );

        assert_eq!(result.roi(), Decimal::ZERO);
    }

    // ============================================================
    // Edge Case Tests
    // ============================================================

    #[test]
    fn binary_bet_with_very_low_price() {
        // Price of $0.01 means high potential payout
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.01), // very low price
            0.90,
        );

        // shares = 100 / 0.01 = 10000
        assert_eq!(bet.shares(), dec!(10000));
        // max_profit = 10000 - 100 = 9900
        assert_eq!(bet.max_profit(), dec!(9900));
    }

    #[test]
    fn binary_bet_with_very_high_price() {
        // Price of $0.99 means low potential payout
        let bet = BinaryBet::new(
            Utc::now(),
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.99), // very high price
            0.10,
        );

        // shares = 100 / 0.99 = 101.0101...
        let expected_shares = dec!(100) / dec!(0.99);
        assert_eq!(bet.shares(), expected_shares);
        // max_profit is small
        assert!(bet.max_profit() < dec!(2));
    }

    #[test]
    fn settlement_result_serialization_roundtrip() {
        let result = create_test_settlement_win();

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SettlementResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.outcome, BinaryOutcome::Win);
        assert_eq!(deserialized.net_pnl, result.net_pnl);
        assert_eq!(deserialized.bet.stake, result.bet.stake);
    }
}
