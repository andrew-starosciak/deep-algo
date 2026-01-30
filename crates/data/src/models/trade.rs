//! Binary trade data model.
//!
//! Tracks trades for backtesting and live execution on binary outcome markets.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// A binary outcome trade record.
///
/// Used for tracking both simulated (backtest) and live trades.
/// Stores signal snapshot for post-hoc analysis.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BinaryTradeRecord {
    /// Auto-generated trade ID
    pub id: i32,
    /// Timestamp when trade was placed
    pub timestamp: DateTime<Utc>,
    /// Market identifier (e.g., Polymarket market ID)
    pub market_id: String,
    /// Trade direction: "yes" or "no"
    pub direction: String,
    /// Number of shares purchased
    pub shares: Decimal,
    /// Price per share (0.0 to 1.0)
    pub price: Decimal,
    /// Total stake (shares * price)
    pub stake: Decimal,
    /// Snapshot of signals at time of trade for analysis
    pub signals_snapshot: Option<JsonValue>,
    /// Outcome after settlement: "win" or "loss"
    pub outcome: Option<String>,
    /// Profit/loss in USD
    pub pnl: Option<Decimal>,
    /// Timestamp when market settled
    pub settled_at: Option<DateTime<Utc>>,
}

impl BinaryTradeRecord {
    /// Creates a new trade record (pre-settlement).
    pub fn new(
        timestamp: DateTime<Utc>,
        market_id: String,
        direction: TradeDirection,
        shares: Decimal,
        price: Decimal,
    ) -> Self {
        let stake = shares * price;
        Self {
            id: 0, // Will be set by database
            timestamp,
            market_id,
            direction: direction.as_str().to_string(),
            shares,
            price,
            stake,
            signals_snapshot: None,
            outcome: None,
            pnl: None,
            settled_at: None,
        }
    }

    /// Adds signal snapshot to the trade.
    pub fn with_signals(mut self, signals: JsonValue) -> Self {
        self.signals_snapshot = Some(signals);
        self
    }

    /// Records the settlement outcome.
    pub fn settle(&mut self, won: bool, settled_at: DateTime<Utc>) {
        self.outcome = Some(if won {
            "win".to_string()
        } else {
            "loss".to_string()
        });

        // PnL calculation:
        // If win: pnl = shares * (1.0 - price) = shares - stake
        // If loss: pnl = -stake
        self.pnl = Some(if won {
            self.shares - self.stake
        } else {
            -self.stake
        });

        self.settled_at = Some(settled_at);
    }

    /// Returns true if this trade was a "yes" direction.
    #[must_use]
    pub fn is_yes(&self) -> bool {
        self.direction == "yes"
    }

    /// Returns true if this trade was a "no" direction.
    #[must_use]
    pub fn is_no(&self) -> bool {
        self.direction == "no"
    }

    /// Returns true if this trade has been settled.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.outcome.is_some()
    }

    /// Returns true if this trade was a win.
    #[must_use]
    pub fn is_win(&self) -> bool {
        self.outcome.as_ref().map(|o| o == "win").unwrap_or(false)
    }

    /// Returns true if this trade was a loss.
    #[must_use]
    pub fn is_loss(&self) -> bool {
        self.outcome.as_ref().map(|o| o == "loss").unwrap_or(false)
    }

    /// Returns the parsed trade direction.
    #[must_use]
    pub fn parsed_direction(&self) -> Option<TradeDirection> {
        match self.direction.as_str() {
            "yes" => Some(TradeDirection::Yes),
            "no" => Some(TradeDirection::No),
            _ => None,
        }
    }

    /// Calculates the return on investment (ROI) if settled.
    /// ROI = pnl / stake
    #[must_use]
    pub fn roi(&self) -> Option<Decimal> {
        let pnl = self.pnl?;
        if self.stake > Decimal::ZERO {
            Some(pnl / self.stake)
        } else {
            None
        }
    }

    /// Returns the potential payout if the trade wins.
    /// Payout = shares (since each winning share pays $1)
    #[must_use]
    pub fn potential_payout(&self) -> Decimal {
        self.shares
    }

    /// Returns the potential profit if the trade wins.
    /// Profit = shares - stake = shares * (1 - price)
    #[must_use]
    pub fn potential_profit(&self) -> Decimal {
        self.shares - self.stake
    }

    /// Calculates expected value at time of trade.
    /// EV = estimated_prob * potential_profit - (1 - estimated_prob) * stake
    #[must_use]
    pub fn expected_value(&self, estimated_probability: Decimal) -> Decimal {
        let profit = self.potential_profit();
        estimated_probability * profit - (Decimal::ONE - estimated_probability) * self.stake
    }
}

/// Direction of a binary trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDirection {
    Yes,
    No,
}

impl TradeDirection {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeDirection::Yes => "yes",
            TradeDirection::No => "no",
        }
    }
}

/// Trade outcome after settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeOutcome {
    Win,
    Loss,
}

impl TradeOutcome {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeOutcome::Win => "win",
            TradeOutcome::Loss => "loss",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn test_trade_record_creation() {
        let record = BinaryTradeRecord {
            id: 1,
            timestamp: sample_timestamp(),
            market_id: "btc-100k".to_string(),
            direction: "yes".to_string(),
            shares: dec!(100),
            price: dec!(0.65),
            stake: dec!(65),
            signals_snapshot: Some(json!({"imbalance": 0.15})),
            outcome: None,
            pnl: None,
            settled_at: None,
        };

        assert_eq!(record.market_id, "btc-100k");
        assert!(record.is_yes());
        assert!(!record.is_settled());
    }

    #[test]
    fn test_trade_new() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        assert_eq!(record.direction, "yes");
        assert_eq!(record.shares, dec!(100));
        assert_eq!(record.price, dec!(0.65));
        assert_eq!(record.stake, dec!(65)); // 100 * 0.65
    }

    #[test]
    fn test_with_signals() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        )
        .with_signals(json!({"imbalance": 0.15, "funding_zscore": 2.1}));

        assert!(record.signals_snapshot.is_some());
        let signals = record.signals_snapshot.unwrap();
        assert_eq!(signals["imbalance"], 0.15);
    }

    #[test]
    fn test_settle_win() {
        let mut record = BinaryTradeRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        let settlement_time = sample_timestamp() + chrono::Duration::hours(1);
        record.settle(true, settlement_time);

        assert!(record.is_settled());
        assert!(record.is_win());
        assert!(!record.is_loss());
        assert_eq!(record.outcome, Some("win".to_string()));
        // pnl = shares - stake = 100 - 65 = 35
        assert_eq!(record.pnl, Some(dec!(35)));
        assert_eq!(record.settled_at, Some(settlement_time));
    }

    #[test]
    fn test_settle_loss() {
        let mut record = BinaryTradeRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        let settlement_time = sample_timestamp() + chrono::Duration::hours(1);
        record.settle(false, settlement_time);

        assert!(record.is_settled());
        assert!(!record.is_win());
        assert!(record.is_loss());
        assert_eq!(record.outcome, Some("loss".to_string()));
        // pnl = -stake = -65
        assert_eq!(record.pnl, Some(dec!(-65)));
    }

    #[test]
    fn test_is_yes_no() {
        let yes_trade = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.5),
        );

        assert!(yes_trade.is_yes());
        assert!(!yes_trade.is_no());

        let no_trade = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::No,
            dec!(100),
            dec!(0.5),
        );

        assert!(!no_trade.is_yes());
        assert!(no_trade.is_no());
    }

    #[test]
    fn test_parsed_direction() {
        let yes_trade = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.5),
        );

        assert_eq!(yes_trade.parsed_direction(), Some(TradeDirection::Yes));

        let no_trade = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::No,
            dec!(100),
            dec!(0.5),
        );

        assert_eq!(no_trade.parsed_direction(), Some(TradeDirection::No));
    }

    #[test]
    fn test_roi_win() {
        let mut record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.50),
        );

        record.settle(true, sample_timestamp());

        // stake = 50, pnl = 100 - 50 = 50
        // roi = 50 / 50 = 1.0 (100% return)
        assert_eq!(record.roi(), Some(dec!(1.0)));
    }

    #[test]
    fn test_roi_loss() {
        let mut record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.50),
        );

        record.settle(false, sample_timestamp());

        // stake = 50, pnl = -50
        // roi = -50 / 50 = -1.0 (-100% return)
        assert_eq!(record.roi(), Some(dec!(-1.0)));
    }

    #[test]
    fn test_roi_unsettled() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.50),
        );

        assert_eq!(record.roi(), None);
    }

    #[test]
    fn test_potential_payout() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        assert_eq!(record.potential_payout(), dec!(100));
    }

    #[test]
    fn test_potential_profit() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.65),
        );

        // profit = shares - stake = 100 - 65 = 35
        assert_eq!(record.potential_profit(), dec!(35));
    }

    #[test]
    fn test_expected_value_positive() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.60),
        );

        // stake = 60, profit = 40
        // EV = 0.70 * 40 - 0.30 * 60 = 28 - 18 = 10
        let ev = record.expected_value(dec!(0.70));
        assert_eq!(ev, dec!(10));
    }

    #[test]
    fn test_expected_value_negative() {
        let record = BinaryTradeRecord::new(
            sample_timestamp(),
            "test".to_string(),
            TradeDirection::Yes,
            dec!(100),
            dec!(0.70),
        );

        // stake = 70, profit = 30
        // EV = 0.60 * 30 - 0.40 * 70 = 18 - 28 = -10
        let ev = record.expected_value(dec!(0.60));
        assert_eq!(ev, dec!(-10));
    }

    #[test]
    fn test_direction_as_str() {
        assert_eq!(TradeDirection::Yes.as_str(), "yes");
        assert_eq!(TradeDirection::No.as_str(), "no");
    }

    #[test]
    fn test_outcome_as_str() {
        assert_eq!(TradeOutcome::Win.as_str(), "win");
        assert_eq!(TradeOutcome::Loss.as_str(), "loss");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = BinaryTradeRecord {
            id: 1,
            timestamp: sample_timestamp(),
            market_id: "btc-100k".to_string(),
            direction: "yes".to_string(),
            shares: dec!(100),
            price: dec!(0.65),
            stake: dec!(65),
            signals_snapshot: Some(json!({"imbalance": 0.15})),
            outcome: Some("win".to_string()),
            pnl: Some(dec!(35)),
            settled_at: Some(sample_timestamp()),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: BinaryTradeRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.market_id, deserialized.market_id);
        assert_eq!(record.direction, deserialized.direction);
        assert_eq!(record.shares, deserialized.shares);
        assert_eq!(record.pnl, deserialized.pnl);
    }
}
