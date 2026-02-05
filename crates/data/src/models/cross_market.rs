//! Cross-market correlation opportunity data model.
//!
//! Records cross-market arbitrage opportunities for analysis and outcome tracking.

use chrono::{DateTime, Duration, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Settlement status for an opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementStatus {
    /// Awaiting 15-min window to close.
    Pending,
    /// Outcome determined and recorded.
    Settled,
    /// Market expired without settlement data.
    Expired,
    /// Error during settlement lookup.
    Error,
}

impl SettlementStatus {
    /// Converts to database string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Error => "error",
        }
    }

    /// Parses from database string.
    #[must_use]
    pub fn from_str(s: &str) -> Self {
        match s {
            "settled" => Self::Settled,
            "expired" => Self::Expired,
            "error" => Self::Error,
            _ => Self::Pending,
        }
    }
}

/// Trade result after settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeResult {
    /// One leg won (expected case with correlation).
    Win,
    /// Both legs won (rare divergence case - jackpot!).
    DoubleWin,
    /// Both legs lost (rare - correlation broke).
    Lose,
}

impl TradeResult {
    /// Converts to database string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Win => "WIN",
            Self::DoubleWin => "DOUBLE_WIN",
            Self::Lose => "LOSE",
        }
    }

    /// Parses from database string.
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "WIN" => Some(Self::Win),
            "DOUBLE_WIN" => Some(Self::DoubleWin),
            "LOSE" => Some(Self::Lose),
            _ => None,
        }
    }
}

/// A cross-market opportunity record for database storage.
///
/// Represents a detected opportunity where buying positions on two
/// different coins totals less than $1.00.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CrossMarketOpportunityRecord {
    /// Auto-generated ID.
    pub id: i32,
    /// Timestamp when opportunity was detected.
    pub timestamp: DateTime<Utc>,

    /// First coin (e.g., "BTC", "ETH").
    pub coin1: String,
    /// Second coin (e.g., "ETH", "SOL").
    pub coin2: String,
    /// Combination type (e.g., "Coin1UpCoin2Down").
    pub combination: String,

    /// Direction for leg 1: "UP" or "DOWN".
    pub leg1_direction: String,
    /// Price for leg 1 (0.0 to 1.0).
    pub leg1_price: Decimal,
    /// Token ID for leg 1.
    pub leg1_token_id: String,

    /// Direction for leg 2: "UP" or "DOWN".
    pub leg2_direction: String,
    /// Price for leg 2 (0.0 to 1.0).
    pub leg2_price: Decimal,
    /// Token ID for leg 2.
    pub leg2_token_id: String,

    /// Total cost of both legs.
    pub total_cost: Decimal,
    /// Spread (1.0 - total_cost).
    pub spread: Decimal,
    /// Expected value based on correlation.
    pub expected_value: Decimal,
    /// Win probability based on correlation model.
    pub win_probability: Decimal,
    /// Assumed correlation between coins.
    pub assumed_correlation: Decimal,

    /// Optional session ID for grouping.
    #[sqlx(default)]
    pub session_id: Option<String>,

    // === Outcome Tracking Fields ===

    /// Settlement status: pending, settled, expired, error.
    #[sqlx(default)]
    pub status: Option<String>,

    /// When the 15-min window expires.
    #[sqlx(default)]
    pub window_end: Option<DateTime<Utc>>,

    /// Actual outcome for coin1: UP or DOWN.
    #[sqlx(default)]
    pub coin1_outcome: Option<String>,

    /// Actual outcome for coin2: UP or DOWN.
    #[sqlx(default)]
    pub coin2_outcome: Option<String>,

    /// Trade result: WIN, DOUBLE_WIN, or LOSE.
    #[sqlx(default)]
    pub trade_result: Option<String>,

    /// Actual P&L if traded (payout - cost - fees).
    #[sqlx(default)]
    pub actual_pnl: Option<Decimal>,

    /// Whether correlation held (both coins moved same direction).
    #[sqlx(default)]
    pub correlation_correct: Option<bool>,

    /// When the opportunity was settled.
    #[sqlx(default)]
    pub settled_at: Option<DateTime<Utc>>,

    // === Order Book Depth Fields (for fill probability analysis) ===

    /// Leg 1 bid-side depth (total $ available).
    #[sqlx(default)]
    pub leg1_bid_depth: Option<Decimal>,

    /// Leg 1 ask-side depth (total $ available).
    #[sqlx(default)]
    pub leg1_ask_depth: Option<Decimal>,

    /// Leg 1 bid-ask spread in basis points.
    #[sqlx(default)]
    pub leg1_spread_bps: Option<Decimal>,

    /// Leg 2 bid-side depth (total $ available).
    #[sqlx(default)]
    pub leg2_bid_depth: Option<Decimal>,

    /// Leg 2 ask-side depth (total $ available).
    #[sqlx(default)]
    pub leg2_ask_depth: Option<Decimal>,

    /// Leg 2 bid-ask spread in basis points.
    #[sqlx(default)]
    pub leg2_spread_bps: Option<Decimal>,

    // === Execution Tracking ===

    /// Whether a trade was actually executed.
    #[sqlx(default)]
    pub executed: Option<bool>,

    /// Actual fill price for leg 1 (if executed).
    #[sqlx(default)]
    pub leg1_fill_price: Option<Decimal>,

    /// Actual fill price for leg 2 (if executed).
    #[sqlx(default)]
    pub leg2_fill_price: Option<Decimal>,

    /// Slippage from expected price (if executed).
    #[sqlx(default)]
    pub slippage: Option<Decimal>,
}

impl CrossMarketOpportunityRecord {
    /// Creates a new record from an opportunity.
    ///
    /// Note: `id` will be set by the database on insert.
    #[must_use]
    pub fn new(
        timestamp: DateTime<Utc>,
        coin1: String,
        coin2: String,
        combination: String,
        leg1_direction: String,
        leg1_price: Decimal,
        leg1_token_id: String,
        leg2_direction: String,
        leg2_price: Decimal,
        leg2_token_id: String,
        total_cost: Decimal,
        spread: Decimal,
        expected_value: Decimal,
        win_probability: f64,
        assumed_correlation: f64,
    ) -> Self {
        // Calculate window_end as next 15-min boundary + 15 minutes
        let window_end = Self::calculate_window_end(timestamp);

        Self {
            id: 0, // Set by database
            timestamp,
            coin1,
            coin2,
            combination,
            leg1_direction,
            leg1_price,
            leg1_token_id,
            leg2_direction,
            leg2_price,
            leg2_token_id,
            total_cost,
            spread,
            expected_value,
            win_probability: Decimal::from_f64_retain(win_probability)
                .unwrap_or(Decimal::ZERO),
            assumed_correlation: Decimal::from_f64_retain(assumed_correlation)
                .unwrap_or(Decimal::ZERO),
            session_id: None,
            // Outcome tracking fields
            status: Some("pending".to_string()),
            window_end: Some(window_end),
            coin1_outcome: None,
            coin2_outcome: None,
            trade_result: None,
            actual_pnl: None,
            correlation_correct: None,
            settled_at: None,
            // Order book depth fields (set via with_depth())
            leg1_bid_depth: None,
            leg1_ask_depth: None,
            leg1_spread_bps: None,
            leg2_bid_depth: None,
            leg2_ask_depth: None,
            leg2_spread_bps: None,
            // Execution tracking
            executed: Some(false),
            leg1_fill_price: None,
            leg2_fill_price: None,
            slippage: None,
        }
    }

    /// Sets order book depth data for both legs.
    #[must_use]
    pub fn with_depth(
        mut self,
        leg1_bid_depth: Decimal,
        leg1_ask_depth: Decimal,
        leg1_spread_bps: Decimal,
        leg2_bid_depth: Decimal,
        leg2_ask_depth: Decimal,
        leg2_spread_bps: Decimal,
    ) -> Self {
        self.leg1_bid_depth = Some(leg1_bid_depth);
        self.leg1_ask_depth = Some(leg1_ask_depth);
        self.leg1_spread_bps = Some(leg1_spread_bps);
        self.leg2_bid_depth = Some(leg2_bid_depth);
        self.leg2_ask_depth = Some(leg2_ask_depth);
        self.leg2_spread_bps = Some(leg2_spread_bps);
        self
    }

    /// Records execution details after a trade.
    pub fn record_execution(
        &mut self,
        leg1_fill_price: Decimal,
        leg2_fill_price: Decimal,
    ) {
        self.executed = Some(true);
        self.leg1_fill_price = Some(leg1_fill_price);
        self.leg2_fill_price = Some(leg2_fill_price);

        // Calculate slippage (difference from expected prices)
        let expected_total = self.leg1_price + self.leg2_price;
        let actual_total = leg1_fill_price + leg2_fill_price;
        self.slippage = Some(actual_total - expected_total);
    }

    /// Returns the minimum available depth across both legs.
    #[must_use]
    pub fn min_depth(&self) -> Option<Decimal> {
        match (
            self.leg1_bid_depth,
            self.leg1_ask_depth,
            self.leg2_bid_depth,
            self.leg2_ask_depth,
        ) {
            (Some(l1b), Some(l1a), Some(l2b), Some(l2a)) => {
                Some(l1b.min(l1a).min(l2b).min(l2a))
            }
            _ => None,
        }
    }

    /// Returns true if there's sufficient depth for a given trade size.
    #[must_use]
    pub fn has_sufficient_depth(&self, trade_size: Decimal) -> bool {
        self.min_depth().map_or(false, |depth| depth >= trade_size)
    }

    /// Calculates when the 15-min window ends for a given timestamp.
    ///
    /// Polymarket 15-min markets close at :00, :15, :30, :45 boundaries.
    #[must_use]
    pub fn calculate_window_end(timestamp: DateTime<Utc>) -> DateTime<Utc> {
        let minutes = timestamp.minute();
        let next_boundary = match minutes {
            0..=14 => 15,
            15..=29 => 30,
            30..=44 => 45,
            _ => 60, // Will roll to next hour
        };

        let base = timestamp
            .with_second(0)
            .unwrap_or(timestamp)
            .with_nanosecond(0)
            .unwrap_or(timestamp);

        if next_boundary == 60 {
            // Roll to next hour
            base.with_minute(0)
                .unwrap_or(base)
                + Duration::hours(1)
        } else {
            base.with_minute(next_boundary).unwrap_or(base)
        }
    }

    /// Sets the session ID.
    #[must_use]
    pub fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Settles the opportunity with actual outcomes.
    ///
    /// # Arguments
    /// * `coin1_outcome` - Actual outcome for coin1: "UP" or "DOWN"
    /// * `coin2_outcome` - Actual outcome for coin2: "UP" or "DOWN"
    /// * `fee_rate` - Fee rate on winnings (e.g., 0.02 for 2%)
    pub fn settle(&mut self, coin1_outcome: &str, coin2_outcome: &str, fee_rate: Decimal) {
        self.coin1_outcome = Some(coin1_outcome.to_string());
        self.coin2_outcome = Some(coin2_outcome.to_string());

        // Check if each leg won
        let leg1_won = self.leg1_direction == coin1_outcome;
        let leg2_won = self.leg2_direction == coin2_outcome;

        // Determine trade result
        let result = match (leg1_won, leg2_won) {
            (true, true) => TradeResult::DoubleWin,
            (true, false) | (false, true) => TradeResult::Win,
            (false, false) => TradeResult::Lose,
        };
        self.trade_result = Some(result.as_str().to_string());

        // Calculate P&L
        let payout = match result {
            TradeResult::DoubleWin => Decimal::TWO,
            TradeResult::Win => Decimal::ONE,
            TradeResult::Lose => Decimal::ZERO,
        };
        let fees = payout * fee_rate;
        self.actual_pnl = Some(payout - fees - self.total_cost);

        // Check if correlation held (both coins moved in same direction)
        self.correlation_correct = Some(coin1_outcome == coin2_outcome);

        // Mark as settled
        self.status = Some("settled".to_string());
        self.settled_at = Some(Utc::now());
    }

    /// Marks the opportunity as expired (no settlement data available).
    pub fn mark_expired(&mut self) {
        self.status = Some("expired".to_string());
        self.settled_at = Some(Utc::now());
    }

    /// Marks the opportunity as having a settlement error.
    pub fn mark_error(&mut self) {
        self.status = Some("error".to_string());
        self.settled_at = Some(Utc::now());
    }

    /// Returns true if this opportunity is ready for settlement.
    #[must_use]
    pub fn is_ready_for_settlement(&self) -> bool {
        if let (Some(status), Some(window_end)) = (&self.status, self.window_end) {
            status == "pending" && Utc::now() >= window_end
        } else {
            false
        }
    }

    /// Returns the settlement status.
    #[must_use]
    pub fn settlement_status(&self) -> SettlementStatus {
        self.status
            .as_ref()
            .map(|s| SettlementStatus::from_str(s))
            .unwrap_or(SettlementStatus::Pending)
    }

    /// Returns the trade result if settled.
    #[must_use]
    pub fn trade_result_enum(&self) -> Option<TradeResult> {
        self.trade_result.as_ref().and_then(|s| TradeResult::from_str(s))
    }

    /// Returns true if this was a winning trade.
    #[must_use]
    pub fn is_win(&self) -> bool {
        matches!(
            self.trade_result_enum(),
            Some(TradeResult::Win) | Some(TradeResult::DoubleWin)
        )
    }

    /// Returns the pair name (e.g., "BTC/ETH").
    #[must_use]
    pub fn pair_name(&self) -> String {
        format!("{}/{}", self.coin1, self.coin2)
    }

    /// Returns the ROI as a percentage.
    #[must_use]
    pub fn roi_pct(&self) -> f64 {
        if self.total_cost > Decimal::ZERO {
            let spread_f64: f64 = self.spread.try_into().unwrap_or(0.0);
            let cost_f64: f64 = self.total_cost.try_into().unwrap_or(1.0);
            spread_f64 / cost_f64 * 100.0
        } else {
            0.0
        }
    }

    /// Returns a short display string.
    #[must_use]
    pub fn display_short(&self) -> String {
        format!(
            "{}/{} {:?}: ${} + ${} = ${} (spread ${}, EV ${})",
            self.coin1,
            self.coin2,
            self.combination,
            self.leg1_price,
            self.leg2_price,
            self.total_cost,
            self.spread,
            self.expected_value
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 2, 1, 12, 7, 30).unwrap()
    }

    #[test]
    fn record_new() {
        let record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up_token".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down_token".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        );

        assert_eq!(record.coin1, "ETH");
        assert_eq!(record.coin2, "BTC");
        assert_eq!(record.total_cost, dec!(0.96));
        assert_eq!(record.spread, dec!(0.04));
        assert_eq!(record.status, Some("pending".to_string()));
    }

    #[test]
    fn record_with_session() {
        let record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up_token".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down_token".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        )
        .with_session("test-session".to_string());

        assert_eq!(record.session_id, Some("test-session".to_string()));
    }

    #[test]
    fn record_pair_name() {
        let record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "BothUp".to_string(),
            "UP".to_string(),
            dec!(0.50),
            "eth_up".to_string(),
            "UP".to_string(),
            dec!(0.45),
            "btc_up".to_string(),
            dec!(0.95),
            dec!(0.05),
            dec!(0.03),
            0.50,
            0.85,
        );

        assert_eq!(record.pair_name(), "ETH/BTC");
    }

    #[test]
    fn record_roi_pct() {
        let record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up".to_string(),
            "DOWN".to_string(),
            dec!(0.90),
            "btc_down".to_string(),
            dec!(0.95),
            dec!(0.05),
            dec!(0.03),
            0.95,
            0.85,
        );

        // ROI = 0.05 / 0.95 * 100 = ~5.26%
        let roi = record.roi_pct();
        assert!(roi > 5.0 && roi < 6.0);
    }

    #[test]
    fn record_roi_zero_cost() {
        let mut record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "BothUp".to_string(),
            "UP".to_string(),
            dec!(0.0),
            "eth_up".to_string(),
            "UP".to_string(),
            dec!(0.0),
            "btc_up".to_string(),
            dec!(0.0),
            dec!(0.0),
            dec!(0.0),
            0.5,
            0.85,
        );
        record.total_cost = Decimal::ZERO;

        assert!((record.roi_pct() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn record_display_short() {
        let record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        );

        let display = record.display_short();
        assert!(display.contains("ETH/BTC"));
        assert!(display.contains("0.96"));
        assert!(display.contains("spread $0.04"));
    }

    #[test]
    fn window_end_calculation() {
        // At 12:07:30, window should end at 12:15:00
        let ts = Utc.with_ymd_and_hms(2025, 2, 1, 12, 7, 30).unwrap();
        let end = CrossMarketOpportunityRecord::calculate_window_end(ts);
        assert_eq!(end.minute(), 15);
        assert_eq!(end.second(), 0);

        // At 12:45:00, window should end at 13:00:00
        let ts2 = Utc.with_ymd_and_hms(2025, 2, 1, 12, 45, 0).unwrap();
        let end2 = CrossMarketOpportunityRecord::calculate_window_end(ts2);
        assert_eq!(end2.hour(), 13);
        assert_eq!(end2.minute(), 0);

        // At 12:30:00, window should end at 12:45:00
        let ts3 = Utc.with_ymd_and_hms(2025, 2, 1, 12, 30, 0).unwrap();
        let end3 = CrossMarketOpportunityRecord::calculate_window_end(ts3);
        assert_eq!(end3.minute(), 45);
    }

    #[test]
    fn settle_win_one_leg() {
        let mut record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        );

        // Both coins went DOWN - BTC DOWN wins, ETH UP loses
        record.settle("DOWN", "DOWN", dec!(0.02));

        assert_eq!(record.trade_result, Some("WIN".to_string()));
        assert_eq!(record.correlation_correct, Some(true)); // Both moved same direction
        assert!(record.is_win());

        // P&L = $1.00 payout - 2% fee - $0.96 cost = 1.00 - 0.02 - 0.96 = $0.02
        let pnl = record.actual_pnl.unwrap();
        assert!(pnl > dec!(0.0));
    }

    #[test]
    fn settle_double_win() {
        let mut record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        );

        // ETH went UP, BTC went DOWN - both legs win!
        record.settle("UP", "DOWN", dec!(0.02));

        assert_eq!(record.trade_result, Some("DOUBLE_WIN".to_string()));
        assert_eq!(record.correlation_correct, Some(false)); // Coins diverged
        assert!(record.is_win());

        // P&L = $2.00 payout - 2% fee - $0.96 cost = 2.00 - 0.04 - 0.96 = $1.00
        let pnl = record.actual_pnl.unwrap();
        assert!(pnl > dec!(0.9));
    }

    #[test]
    fn settle_lose() {
        let mut record = CrossMarketOpportunityRecord::new(
            sample_timestamp(),
            "ETH".to_string(),
            "BTC".to_string(),
            "Coin1UpCoin2Down".to_string(),
            "UP".to_string(),
            dec!(0.05),
            "eth_up".to_string(),
            "DOWN".to_string(),
            dec!(0.91),
            "btc_down".to_string(),
            dec!(0.96),
            dec!(0.04),
            dec!(0.02),
            0.95,
            0.85,
        );

        // ETH went DOWN, BTC went UP - both legs lose (rare divergence)
        record.settle("DOWN", "UP", dec!(0.02));

        assert_eq!(record.trade_result, Some("LOSE".to_string()));
        assert_eq!(record.correlation_correct, Some(false)); // Coins diverged
        assert!(!record.is_win());

        // P&L = $0 - $0.96 = -$0.96
        let pnl = record.actual_pnl.unwrap();
        assert_eq!(pnl, dec!(-0.96));
    }

    #[test]
    fn settlement_status_parsing() {
        assert_eq!(SettlementStatus::from_str("pending"), SettlementStatus::Pending);
        assert_eq!(SettlementStatus::from_str("settled"), SettlementStatus::Settled);
        assert_eq!(SettlementStatus::from_str("expired"), SettlementStatus::Expired);
        assert_eq!(SettlementStatus::from_str("error"), SettlementStatus::Error);
        assert_eq!(SettlementStatus::from_str("unknown"), SettlementStatus::Pending);
    }

    #[test]
    fn trade_result_parsing() {
        assert_eq!(TradeResult::from_str("WIN"), Some(TradeResult::Win));
        assert_eq!(TradeResult::from_str("DOUBLE_WIN"), Some(TradeResult::DoubleWin));
        assert_eq!(TradeResult::from_str("LOSE"), Some(TradeResult::Lose));
        assert_eq!(TradeResult::from_str("unknown"), None);
    }
}
