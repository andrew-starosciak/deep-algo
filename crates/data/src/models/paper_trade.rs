//! Paper trade data model for simulated Polymarket trading.
//!
//! Tracks paper trades for testing strategies before live deployment.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Status of a paper trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperTradeStatus {
    /// Trade is pending, waiting for market resolution.
    Pending,
    /// Trade has been settled (won or lost).
    Settled,
    /// Trade was cancelled.
    Cancelled,
}

impl PaperTradeStatus {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses from string representation.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "settled" => Some(Self::Settled),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Direction of the paper trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperTradeDirection {
    /// Betting "Yes" (price will go up).
    Yes,
    /// Betting "No" (price will go down).
    No,
}

impl PaperTradeDirection {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }

    /// Parses from string representation.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "yes" => Some(Self::Yes),
            "no" => Some(Self::No),
            _ => None,
        }
    }
}

/// A paper trade record for simulated Polymarket trading.
///
/// Used for paper trading before live deployment.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PaperTradeRecord {
    /// Auto-generated trade ID.
    pub id: i32,
    /// Timestamp when trade was placed.
    pub timestamp: DateTime<Utc>,
    /// Market identifier (Polymarket condition_id).
    pub market_id: String,
    /// Market question for reference.
    pub market_question: String,
    /// Trade direction: "yes" or "no".
    pub direction: String,
    /// Number of simulated shares.
    pub shares: Decimal,
    /// Price per share at entry (0.0 to 1.0).
    pub entry_price: Decimal,
    /// Total stake (shares * entry_price).
    pub stake: Decimal,
    /// Estimated probability from signal (0.0 to 1.0).
    pub estimated_prob: Decimal,
    /// Expected value at time of trade.
    pub expected_value: Decimal,
    /// Kelly fraction used for sizing.
    pub kelly_fraction: Decimal,
    /// Signal strength (0.0 to 1.0).
    pub signal_strength: Decimal,
    /// Composite signal snapshot for analysis.
    pub signals_snapshot: Option<JsonValue>,
    /// Trade status: "pending", "settled", "cancelled".
    pub status: String,
    /// Outcome after settlement: "win", "loss", or null.
    pub outcome: Option<String>,
    /// Profit/loss in USD.
    pub pnl: Option<Decimal>,
    /// Fees paid.
    pub fees: Option<Decimal>,
    /// Timestamp when market settled.
    pub settled_at: Option<DateTime<Utc>>,
    /// Session identifier for grouping trades.
    pub session_id: String,
}

impl PaperTradeRecord {
    /// Creates a new paper trade record (pre-settlement).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timestamp: DateTime<Utc>,
        market_id: String,
        market_question: String,
        direction: PaperTradeDirection,
        shares: Decimal,
        entry_price: Decimal,
        estimated_prob: Decimal,
        kelly_fraction: Decimal,
        signal_strength: Decimal,
        session_id: String,
    ) -> Self {
        let stake = shares * entry_price;

        // EV = estimated_prob * (1 - entry_price) - (1 - estimated_prob) * entry_price
        let expected_value = Self::calculate_ev(estimated_prob, entry_price, stake);

        Self {
            id: 0, // Will be set by database
            timestamp,
            market_id,
            market_question,
            direction: direction.as_str().to_string(),
            shares,
            entry_price,
            stake,
            estimated_prob,
            expected_value,
            kelly_fraction,
            signal_strength,
            signals_snapshot: None,
            status: PaperTradeStatus::Pending.as_str().to_string(),
            outcome: None,
            pnl: None,
            fees: None,
            settled_at: None,
            session_id,
        }
    }

    /// Calculates expected value for a trade.
    ///
    /// EV = p * profit_if_win - (1-p) * loss_if_lose
    ///    = p * stake * (1-price)/price - (1-p) * stake
    #[must_use]
    pub fn calculate_ev(estimated_prob: Decimal, price: Decimal, stake: Decimal) -> Decimal {
        if price == Decimal::ZERO || price >= Decimal::ONE {
            return Decimal::ZERO;
        }

        let profit_if_win = stake * (Decimal::ONE - price) / price;
        let loss_if_lose = stake;

        estimated_prob * profit_if_win - (Decimal::ONE - estimated_prob) * loss_if_lose
    }

    /// Adds signal snapshot to the trade.
    #[must_use]
    pub fn with_signals(mut self, signals: JsonValue) -> Self {
        self.signals_snapshot = Some(signals);
        self
    }

    /// Settles the trade with the final outcome.
    pub fn settle(&mut self, won: bool, fees: Decimal, settled_at: DateTime<Utc>) {
        self.status = PaperTradeStatus::Settled.as_str().to_string();
        self.outcome = Some(if won { "win" } else { "loss" }.to_string());
        self.fees = Some(fees);
        self.settled_at = Some(settled_at);

        // Calculate P&L
        // Win: pnl = shares - stake - fees = stake * (1-price)/price - fees
        // Loss: pnl = -stake - fees
        self.pnl = Some(if won {
            self.shares - self.stake - fees
        } else {
            -self.stake - fees
        });
    }

    /// Cancels the trade.
    pub fn cancel(&mut self) {
        self.status = PaperTradeStatus::Cancelled.as_str().to_string();
    }

    /// Returns true if this trade is a "yes" direction.
    #[must_use]
    pub fn is_yes(&self) -> bool {
        self.direction == "yes"
    }

    /// Returns true if this trade is a "no" direction.
    #[must_use]
    pub fn is_no(&self) -> bool {
        self.direction == "no"
    }

    /// Returns true if this trade is pending.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    /// Returns true if this trade has been settled.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.status == "settled"
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
    pub fn parsed_direction(&self) -> Option<PaperTradeDirection> {
        PaperTradeDirection::parse(&self.direction)
    }

    /// Returns the parsed trade status.
    #[must_use]
    pub fn parsed_status(&self) -> Option<PaperTradeStatus> {
        PaperTradeStatus::parse(&self.status)
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

    /// Returns the potential profit if the trade wins (before fees).
    /// Profit = shares - stake = stake * (1 - price) / price
    #[must_use]
    pub fn potential_profit(&self) -> Decimal {
        self.shares - self.stake
    }
}

/// Kelly criterion calculator for binary outcomes.
#[derive(Debug, Clone)]
pub struct KellyCriterion {
    /// Fraction of full Kelly to use (e.g., 0.25 for quarter Kelly).
    pub fraction: Decimal,
    /// Maximum bet as fraction of bankroll.
    pub max_bet_fraction: Decimal,
    /// Minimum edge required to bet.
    pub min_edge: Decimal,
}

impl KellyCriterion {
    /// Creates a new Kelly criterion calculator.
    #[must_use]
    pub fn new(fraction: Decimal, max_bet_fraction: Decimal, min_edge: Decimal) -> Self {
        Self {
            fraction,
            max_bet_fraction,
            min_edge,
        }
    }

    /// Creates a quarter Kelly calculator with sensible defaults.
    #[must_use]
    pub fn quarter_kelly() -> Self {
        use rust_decimal_macros::dec;
        Self {
            fraction: dec!(0.25),
            max_bet_fraction: dec!(0.05),
            min_edge: dec!(0.02),
        }
    }

    /// Creates a half Kelly calculator with sensible defaults.
    #[must_use]
    pub fn half_kelly() -> Self {
        use rust_decimal_macros::dec;
        Self {
            fraction: dec!(0.50),
            max_bet_fraction: dec!(0.10),
            min_edge: dec!(0.02),
        }
    }

    /// Calculates the Kelly bet size for a binary outcome.
    ///
    /// Kelly formula for binary bets: f* = (p(b+1) - 1) / b
    /// where:
    /// - p = estimated probability of winning
    /// - b = net odds = (1 - price) / price
    ///
    /// Returns None if the edge is below minimum or bet size is non-positive.
    #[must_use]
    pub fn calculate_bet_size(
        &self,
        estimated_prob: Decimal,
        price: Decimal,
        bankroll: Decimal,
    ) -> Option<Decimal> {
        use rust_decimal_macros::dec;

        // Validate inputs
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return None;
        }
        if estimated_prob <= Decimal::ZERO || estimated_prob >= Decimal::ONE {
            return None;
        }
        if bankroll <= Decimal::ZERO {
            return None;
        }

        // Calculate edge (EV per dollar risked)
        let edge = estimated_prob - price;
        if edge < self.min_edge {
            return None;
        }

        // Calculate net odds: b = (1 - price) / price
        let b = (Decimal::ONE - price) / price;

        // Full Kelly: f* = (p(b+1) - 1) / b
        let full_kelly = (estimated_prob * (b + Decimal::ONE) - Decimal::ONE) / b;

        // Apply fraction and ensure non-negative
        let fractional_kelly = full_kelly * self.fraction;
        if fractional_kelly <= dec!(0) {
            return None;
        }

        // Cap at max bet fraction
        let capped_fraction = fractional_kelly.min(self.max_bet_fraction);

        // Calculate actual bet size
        let bet_size = capped_fraction * bankroll;

        Some(bet_size)
    }

    /// Calculates the number of shares to buy given a bet size.
    ///
    /// shares = bet_size / price
    #[must_use]
    pub fn calculate_shares(&self, bet_size: Decimal, price: Decimal) -> Decimal {
        if price <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        bet_size / price
    }
}

/// Decision from the trading engine.
#[derive(Debug, Clone)]
pub struct TradeDecision {
    /// Whether to place a trade.
    pub should_trade: bool,
    /// Direction of the trade (if trading).
    pub direction: Option<PaperTradeDirection>,
    /// Number of shares to buy.
    pub shares: Decimal,
    /// Stake amount.
    pub stake: Decimal,
    /// Kelly fraction used.
    pub kelly_fraction: Decimal,
    /// Expected value of the trade.
    pub expected_value: Decimal,
    /// Reason for the decision.
    pub reason: String,
}

impl TradeDecision {
    /// Creates a "no trade" decision.
    #[must_use]
    pub fn no_trade(reason: &str) -> Self {
        Self {
            should_trade: false,
            direction: None,
            shares: Decimal::ZERO,
            stake: Decimal::ZERO,
            kelly_fraction: Decimal::ZERO,
            expected_value: Decimal::ZERO,
            reason: reason.to_string(),
        }
    }

    /// Creates a "trade" decision.
    #[must_use]
    pub fn trade(
        direction: PaperTradeDirection,
        shares: Decimal,
        stake: Decimal,
        kelly_fraction: Decimal,
        expected_value: Decimal,
    ) -> Self {
        Self {
            should_trade: true,
            direction: Some(direction),
            shares,
            stake,
            kelly_fraction,
            expected_value,
            reason: "Signal meets criteria".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use serde_json::json;

    // =========================================================================
    // Test Helpers
    // =========================================================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 31, 12, 0, 0).unwrap()
    }

    fn sample_paper_trade() -> PaperTradeRecord {
        PaperTradeRecord::new(
            sample_timestamp(),
            "btc-100k-feb".to_string(),
            "Will Bitcoin exceed $100k by Feb 2025?".to_string(),
            PaperTradeDirection::Yes,
            dec!(100),
            dec!(0.60),
            dec!(0.70), // estimated prob
            dec!(0.25), // kelly fraction
            dec!(0.75), // signal strength
            "session-123".to_string(),
        )
    }

    // =========================================================================
    // PaperTradeStatus Tests
    // =========================================================================

    #[test]
    fn test_paper_trade_status_as_str() {
        assert_eq!(PaperTradeStatus::Pending.as_str(), "pending");
        assert_eq!(PaperTradeStatus::Settled.as_str(), "settled");
        assert_eq!(PaperTradeStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_paper_trade_status_from_str() {
        assert_eq!(
            PaperTradeStatus::parse("pending"),
            Some(PaperTradeStatus::Pending)
        );
        assert_eq!(
            PaperTradeStatus::parse("SETTLED"),
            Some(PaperTradeStatus::Settled)
        );
        assert_eq!(
            PaperTradeStatus::parse("Cancelled"),
            Some(PaperTradeStatus::Cancelled)
        );
        assert_eq!(PaperTradeStatus::parse("invalid"), None);
    }

    // =========================================================================
    // PaperTradeDirection Tests
    // =========================================================================

    #[test]
    fn test_paper_trade_direction_as_str() {
        assert_eq!(PaperTradeDirection::Yes.as_str(), "yes");
        assert_eq!(PaperTradeDirection::No.as_str(), "no");
    }

    #[test]
    fn test_paper_trade_direction_from_str() {
        assert_eq!(
            PaperTradeDirection::parse("yes"),
            Some(PaperTradeDirection::Yes)
        );
        assert_eq!(
            PaperTradeDirection::parse("NO"),
            Some(PaperTradeDirection::No)
        );
        assert_eq!(PaperTradeDirection::parse("invalid"), None);
    }

    // =========================================================================
    // PaperTradeRecord Creation Tests
    // =========================================================================

    #[test]
    fn test_paper_trade_record_new() {
        let trade = sample_paper_trade();

        assert_eq!(trade.market_id, "btc-100k-feb");
        assert_eq!(trade.direction, "yes");
        assert_eq!(trade.shares, dec!(100));
        assert_eq!(trade.entry_price, dec!(0.60));
        assert_eq!(trade.stake, dec!(60)); // 100 * 0.60
        assert_eq!(trade.estimated_prob, dec!(0.70));
        assert_eq!(trade.kelly_fraction, dec!(0.25));
        assert_eq!(trade.signal_strength, dec!(0.75));
        assert_eq!(trade.status, "pending");
        assert!(trade.outcome.is_none());
        assert!(trade.pnl.is_none());
        assert_eq!(trade.session_id, "session-123");
    }

    #[test]
    fn test_paper_trade_stake_calculation() {
        let trade = PaperTradeRecord::new(
            sample_timestamp(),
            "market-1".to_string(),
            "Test market".to_string(),
            PaperTradeDirection::Yes,
            dec!(200),  // shares
            dec!(0.45), // price
            dec!(0.55),
            dec!(0.25),
            dec!(0.70),
            "session-1".to_string(),
        );

        // stake = shares * price = 200 * 0.45 = 90
        assert_eq!(trade.stake, dec!(90));
    }

    #[test]
    fn test_paper_trade_ev_calculation() {
        // EV = p * profit_if_win - (1-p) * loss_if_lose
        // profit_if_win = stake * (1-price)/price = 60 * 0.40/0.60 = 40
        // loss_if_lose = stake = 60
        // EV = 0.70 * 40 - 0.30 * 60 = 28 - 18 = 10
        let trade = sample_paper_trade();

        let expected_ev = dec!(10);
        assert_eq!(trade.expected_value, expected_ev);
    }

    #[test]
    fn test_calculate_ev_edge_cases() {
        // Zero price
        assert_eq!(
            PaperTradeRecord::calculate_ev(dec!(0.50), dec!(0), dec!(100)),
            dec!(0)
        );

        // Price = 1.0
        assert_eq!(
            PaperTradeRecord::calculate_ev(dec!(0.50), dec!(1), dec!(100)),
            dec!(0)
        );

        // Price > 1.0
        assert_eq!(
            PaperTradeRecord::calculate_ev(dec!(0.50), dec!(1.5), dec!(100)),
            dec!(0)
        );
    }

    // =========================================================================
    // PaperTradeRecord Method Tests
    // =========================================================================

    #[test]
    fn test_paper_trade_with_signals() {
        let trade = sample_paper_trade().with_signals(json!({
            "imbalance": 0.15,
            "funding_zscore": 2.1,
            "composite": 0.75
        }));

        assert!(trade.signals_snapshot.is_some());
        let signals = trade.signals_snapshot.unwrap();
        assert_eq!(signals["imbalance"], 0.15);
        assert_eq!(signals["composite"], 0.75);
    }

    #[test]
    fn test_paper_trade_settle_win() {
        let mut trade = sample_paper_trade();
        let settlement_time = sample_timestamp() + chrono::Duration::hours(1);

        trade.settle(true, dec!(2), settlement_time);

        assert!(trade.is_settled());
        assert!(trade.is_win());
        assert!(!trade.is_loss());
        assert!(!trade.is_pending());
        assert_eq!(trade.outcome, Some("win".to_string()));
        assert_eq!(trade.fees, Some(dec!(2)));
        assert_eq!(trade.settled_at, Some(settlement_time));

        // pnl = shares - stake - fees = 100 - 60 - 2 = 38
        assert_eq!(trade.pnl, Some(dec!(38)));
    }

    #[test]
    fn test_paper_trade_settle_loss() {
        let mut trade = sample_paper_trade();
        let settlement_time = sample_timestamp() + chrono::Duration::hours(1);

        trade.settle(false, dec!(2), settlement_time);

        assert!(trade.is_settled());
        assert!(!trade.is_win());
        assert!(trade.is_loss());
        assert_eq!(trade.outcome, Some("loss".to_string()));

        // pnl = -stake - fees = -60 - 2 = -62
        assert_eq!(trade.pnl, Some(dec!(-62)));
    }

    #[test]
    fn test_paper_trade_cancel() {
        let mut trade = sample_paper_trade();
        trade.cancel();

        assert!(!trade.is_pending());
        assert!(!trade.is_settled());
        assert_eq!(trade.status, "cancelled");
    }

    #[test]
    fn test_paper_trade_direction_helpers() {
        let yes_trade = sample_paper_trade();
        assert!(yes_trade.is_yes());
        assert!(!yes_trade.is_no());

        let no_trade = PaperTradeRecord::new(
            sample_timestamp(),
            "market-1".to_string(),
            "Test".to_string(),
            PaperTradeDirection::No,
            dec!(100),
            dec!(0.40),
            dec!(0.30),
            dec!(0.25),
            dec!(0.70),
            "session-1".to_string(),
        );
        assert!(!no_trade.is_yes());
        assert!(no_trade.is_no());
    }

    #[test]
    fn test_paper_trade_parsed_direction() {
        let yes_trade = sample_paper_trade();
        assert_eq!(yes_trade.parsed_direction(), Some(PaperTradeDirection::Yes));
    }

    #[test]
    fn test_paper_trade_parsed_status() {
        let trade = sample_paper_trade();
        assert_eq!(trade.parsed_status(), Some(PaperTradeStatus::Pending));
    }

    #[test]
    fn test_paper_trade_roi_win() {
        let mut trade = sample_paper_trade();
        trade.settle(true, dec!(0), sample_timestamp());

        // pnl = 100 - 60 = 40, stake = 60
        // roi = 40 / 60 = 0.666...
        let roi = trade.roi().unwrap();
        let expected = dec!(40) / dec!(60);
        assert_eq!(roi, expected);
    }

    #[test]
    fn test_paper_trade_roi_loss() {
        let mut trade = sample_paper_trade();
        trade.settle(false, dec!(0), sample_timestamp());

        // pnl = -60, stake = 60
        // roi = -60 / 60 = -1.0
        assert_eq!(trade.roi(), Some(dec!(-1)));
    }

    #[test]
    fn test_paper_trade_roi_unsettled() {
        let trade = sample_paper_trade();
        assert_eq!(trade.roi(), None);
    }

    #[test]
    fn test_paper_trade_potential_payout() {
        let trade = sample_paper_trade();
        assert_eq!(trade.potential_payout(), dec!(100));
    }

    #[test]
    fn test_paper_trade_potential_profit() {
        let trade = sample_paper_trade();
        // profit = shares - stake = 100 - 60 = 40
        assert_eq!(trade.potential_profit(), dec!(40));
    }

    // =========================================================================
    // KellyCriterion Tests
    // =========================================================================

    #[test]
    fn test_kelly_quarter_kelly_defaults() {
        let kelly = KellyCriterion::quarter_kelly();
        assert_eq!(kelly.fraction, dec!(0.25));
        assert_eq!(kelly.max_bet_fraction, dec!(0.05));
        assert_eq!(kelly.min_edge, dec!(0.02));
    }

    #[test]
    fn test_kelly_half_kelly_defaults() {
        let kelly = KellyCriterion::half_kelly();
        assert_eq!(kelly.fraction, dec!(0.50));
        assert_eq!(kelly.max_bet_fraction, dec!(0.10));
        assert_eq!(kelly.min_edge, dec!(0.02));
    }

    #[test]
    fn test_kelly_calculate_bet_size_positive_edge() {
        let kelly = KellyCriterion::new(dec!(0.25), dec!(0.10), dec!(0.02));

        // p = 0.60, price = 0.50, bankroll = 1000
        // edge = 0.60 - 0.50 = 0.10 (above min_edge of 0.02)
        // b = (1 - 0.50) / 0.50 = 1.0
        // full_kelly = (0.60 * 2.0 - 1.0) / 1.0 = 0.20
        // fractional = 0.20 * 0.25 = 0.05
        // bet = 0.05 * 1000 = 50
        let bet = kelly
            .calculate_bet_size(dec!(0.60), dec!(0.50), dec!(1000))
            .unwrap();
        assert_eq!(bet, dec!(50));
    }

    #[test]
    fn test_kelly_calculate_bet_size_caps_at_max() {
        let kelly = KellyCriterion::new(dec!(1.0), dec!(0.05), dec!(0.02)); // Full Kelly, 5% max

        // p = 0.80, price = 0.50, bankroll = 1000
        // edge = 0.30 (above min)
        // b = 1.0
        // full_kelly = (0.80 * 2.0 - 1.0) / 1.0 = 0.60
        // Capped at 0.05
        // bet = 0.05 * 1000 = 50
        let bet = kelly
            .calculate_bet_size(dec!(0.80), dec!(0.50), dec!(1000))
            .unwrap();
        assert_eq!(bet, dec!(50));
    }

    #[test]
    fn test_kelly_calculate_bet_size_below_min_edge() {
        let kelly = KellyCriterion::new(dec!(0.25), dec!(0.10), dec!(0.05));

        // p = 0.52, price = 0.50, edge = 0.02 (below min_edge of 0.05)
        let bet = kelly.calculate_bet_size(dec!(0.52), dec!(0.50), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_bet_size_negative_edge() {
        let kelly = KellyCriterion::new(dec!(0.25), dec!(0.10), dec!(0.02));

        // p = 0.40, price = 0.50, edge = -0.10 (negative)
        let bet = kelly.calculate_bet_size(dec!(0.40), dec!(0.50), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_bet_size_invalid_price_zero() {
        let kelly = KellyCriterion::quarter_kelly();
        let bet = kelly.calculate_bet_size(dec!(0.60), dec!(0), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_bet_size_invalid_price_one() {
        let kelly = KellyCriterion::quarter_kelly();
        let bet = kelly.calculate_bet_size(dec!(0.60), dec!(1.0), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_bet_size_invalid_prob() {
        let kelly = KellyCriterion::quarter_kelly();

        // p = 0
        let bet = kelly.calculate_bet_size(dec!(0), dec!(0.50), dec!(1000));
        assert!(bet.is_none());

        // p = 1
        let bet = kelly.calculate_bet_size(dec!(1.0), dec!(0.50), dec!(1000));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_bet_size_zero_bankroll() {
        let kelly = KellyCriterion::quarter_kelly();
        let bet = kelly.calculate_bet_size(dec!(0.60), dec!(0.50), dec!(0));
        assert!(bet.is_none());
    }

    #[test]
    fn test_kelly_calculate_shares() {
        let kelly = KellyCriterion::quarter_kelly();

        // bet_size = 100, price = 0.50
        // shares = 100 / 0.50 = 200
        let shares = kelly.calculate_shares(dec!(100), dec!(0.50));
        assert_eq!(shares, dec!(200));
    }

    #[test]
    fn test_kelly_calculate_shares_zero_price() {
        let kelly = KellyCriterion::quarter_kelly();
        let shares = kelly.calculate_shares(dec!(100), dec!(0));
        assert_eq!(shares, dec!(0));
    }

    // =========================================================================
    // TradeDecision Tests
    // =========================================================================

    #[test]
    fn test_trade_decision_no_trade() {
        let decision = TradeDecision::no_trade("Edge too low");

        assert!(!decision.should_trade);
        assert!(decision.direction.is_none());
        assert_eq!(decision.shares, dec!(0));
        assert_eq!(decision.stake, dec!(0));
        assert_eq!(decision.reason, "Edge too low");
    }

    #[test]
    fn test_trade_decision_trade() {
        let decision = TradeDecision::trade(
            PaperTradeDirection::Yes,
            dec!(100),
            dec!(60),
            dec!(0.25),
            dec!(10),
        );

        assert!(decision.should_trade);
        assert_eq!(decision.direction, Some(PaperTradeDirection::Yes));
        assert_eq!(decision.shares, dec!(100));
        assert_eq!(decision.stake, dec!(60));
        assert_eq!(decision.kelly_fraction, dec!(0.25));
        assert_eq!(decision.expected_value, dec!(10));
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_paper_trade_serialization_roundtrip() {
        let trade = sample_paper_trade().with_signals(json!({"test": "value"}));

        let json_str = serde_json::to_string(&trade).expect("serialization failed");
        let deserialized: PaperTradeRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(trade.market_id, deserialized.market_id);
        assert_eq!(trade.direction, deserialized.direction);
        assert_eq!(trade.shares, deserialized.shares);
        assert_eq!(trade.stake, deserialized.stake);
        assert_eq!(trade.session_id, deserialized.session_id);
    }

    #[test]
    fn test_paper_trade_status_serialization() {
        let status = PaperTradeStatus::Settled;
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: PaperTradeStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, deserialized);
    }

    #[test]
    fn test_paper_trade_direction_serialization() {
        let direction = PaperTradeDirection::Yes;
        let json = serde_json::to_string(&direction).unwrap();
        let deserialized: PaperTradeDirection = serde_json::from_str(&json).unwrap();
        assert_eq!(direction, deserialized);
    }
}
