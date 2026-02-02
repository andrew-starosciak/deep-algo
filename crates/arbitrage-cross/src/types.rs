//! Shared types for cross-exchange arbitrage operations.
//!
//! This module defines the core data structures used across the arbitrage system
//! for matching markets, detecting opportunities, and tracking execution.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// =============================================================================
// Side Types (unified across exchanges)
// =============================================================================

/// Unified side type for cross-exchange operations.
///
/// Maps to YES/NO on Kalshi and Up/Down on Polymarket 15-min markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Betting on price going up (YES on Kalshi, Up on Polymarket).
    Yes,
    /// Betting on price going down (NO on Kalshi, Down on Polymarket).
    No,
}

impl Side {
    /// Returns the opposite side.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Yes => Self::No,
            Self::No => Self::Yes,
        }
    }

    /// Returns the display string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "YES",
            Self::No => "NO",
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Exchange Identifiers
// =============================================================================

/// Identifies which exchange a position or order belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Exchange {
    /// Kalshi prediction market.
    Kalshi,
    /// Polymarket CLOB.
    Polymarket,
}

impl Exchange {
    /// Returns the display name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kalshi => "Kalshi",
            Self::Polymarket => "Polymarket",
        }
    }
}

impl std::fmt::Display for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Price Source Types
// =============================================================================

/// Price source used for market settlement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PriceSource {
    /// CF Benchmarks (used by Kalshi).
    CfBenchmarks,
    /// Binance spot price.
    Binance,
    /// CoinGecko.
    CoinGecko,
    /// Custom or unknown source.
    Other(String),
}

impl PriceSource {
    /// Returns true if two price sources are likely compatible.
    #[must_use]
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        match (self, other) {
            // Same source is always compatible
            (a, b) if a == b => true,
            // CF Benchmarks and major exchanges track closely
            (Self::CfBenchmarks, Self::Binance) | (Self::Binance, Self::CfBenchmarks) => true,
            // CoinGecko aggregates, so it's roughly compatible
            (Self::CoinGecko, _) | (_, Self::CoinGecko) => true,
            // Other sources need manual verification
            _ => false,
        }
    }
}

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CfBenchmarks => write!(f, "CF Benchmarks"),
            Self::Binance => write!(f, "Binance"),
            Self::CoinGecko => write!(f, "CoinGecko"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

// =============================================================================
// Comparison Types
// =============================================================================

/// Comparison type for settlement criteria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Comparison {
    /// Price must be above threshold.
    Above,
    /// Price must be below threshold.
    Below,
    /// Price must be between two thresholds.
    Between,
    /// Price must be at or above threshold.
    AtOrAbove,
    /// Price must be at or below threshold.
    AtOrBelow,
}

impl Comparison {
    /// Returns the display string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
            Self::Between => "between",
            Self::AtOrAbove => "at or above",
            Self::AtOrBelow => "at or below",
        }
    }

    /// Returns true if this comparison is compatible with another.
    ///
    /// Comparisons are compatible if they evaluate the same condition.
    #[must_use]
    pub fn is_compatible_with(self, other: Self) -> bool {
        match (self, other) {
            // Exact match
            (a, b) if a == b => true,
            // Above and AtOrAbove are nearly equivalent for arbitrage
            (Self::Above, Self::AtOrAbove) | (Self::AtOrAbove, Self::Above) => true,
            // Below and AtOrBelow are nearly equivalent for arbitrage
            (Self::Below, Self::AtOrBelow) | (Self::AtOrBelow, Self::Below) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for Comparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Settlement Criteria
// =============================================================================

/// Settlement criteria for a binary market.
///
/// Defines how the market will be resolved at settlement time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementCriteria {
    /// Price source for settlement.
    pub price_source: PriceSource,
    /// Time at which settlement occurs.
    pub settlement_time: DateTime<Utc>,
    /// Type of comparison.
    pub comparison: Comparison,
    /// Threshold value (e.g., $100,000).
    pub threshold: Decimal,
    /// Secondary threshold for Between comparisons.
    pub threshold_upper: Option<Decimal>,
}

impl SettlementCriteria {
    /// Creates settlement criteria for "BTC above $X" markets.
    #[must_use]
    pub fn btc_above(threshold: Decimal, settlement_time: DateTime<Utc>) -> Self {
        Self {
            price_source: PriceSource::CfBenchmarks,
            settlement_time,
            comparison: Comparison::Above,
            threshold,
            threshold_upper: None,
        }
    }

    /// Returns true if these criteria are compatible with another for arbitrage.
    ///
    /// Criteria are compatible if they would settle to the same outcome
    /// under the same market conditions.
    #[must_use]
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        // Price sources must be compatible
        if !self.price_source.is_compatible_with(&other.price_source) {
            return false;
        }

        // Comparisons must be compatible
        if !self.comparison.is_compatible_with(other.comparison) {
            return false;
        }

        // Thresholds must match exactly
        if self.threshold != other.threshold {
            return false;
        }

        // For Between comparisons, upper threshold must also match
        if (self.comparison == Comparison::Between || other.comparison == Comparison::Between)
            && self.threshold_upper != other.threshold_upper
        {
            return false;
        }

        true
    }

    /// Returns the time difference in seconds between two settlement times.
    #[must_use]
    pub fn settlement_time_diff_seconds(&self, other: &Self) -> i64 {
        (self.settlement_time - other.settlement_time)
            .num_seconds()
            .abs()
    }
}

// =============================================================================
// Matched Market
// =============================================================================

/// A market that has been matched across Kalshi and Polymarket.
///
/// Represents an event that can be traded on both exchanges with
/// verified settlement criteria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedMarket {
    /// Kalshi market ticker (e.g., "KXBTC-26FEB02-B100000").
    pub kalshi_ticker: String,

    /// Polymarket condition ID.
    pub polymarket_condition_id: String,

    /// Polymarket YES/Up token ID.
    pub polymarket_yes_token: String,

    /// Polymarket NO/Down token ID.
    pub polymarket_no_token: String,

    /// Underlying asset (e.g., "BTC").
    pub underlying: String,

    /// Strike price (e.g., $100,000).
    pub strike_price: Decimal,

    /// Settlement time.
    pub settlement_time: DateTime<Utc>,

    /// Confidence in the match (0.0 to 1.0).
    ///
    /// Based on how closely the settlement criteria align.
    pub match_confidence: f64,

    /// When the match was created.
    pub matched_at: DateTime<Utc>,

    /// Optional notes about the match.
    pub notes: Option<String>,
}

impl MatchedMarket {
    /// Creates a new matched market.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kalshi_ticker: String,
        polymarket_condition_id: String,
        polymarket_yes_token: String,
        polymarket_no_token: String,
        underlying: String,
        strike_price: Decimal,
        settlement_time: DateTime<Utc>,
        match_confidence: f64,
    ) -> Self {
        Self {
            kalshi_ticker,
            polymarket_condition_id,
            polymarket_yes_token,
            polymarket_no_token,
            underlying,
            strike_price,
            settlement_time,
            match_confidence,
            matched_at: Utc::now(),
            notes: None,
        }
    }

    /// Adds notes to the matched market.
    #[must_use]
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Returns true if the match confidence meets the threshold.
    #[must_use]
    pub fn meets_confidence_threshold(&self, threshold: f64) -> bool {
        self.match_confidence >= threshold
    }

    /// Returns the time until settlement.
    #[must_use]
    pub fn time_to_settlement(&self) -> chrono::Duration {
        self.settlement_time - Utc::now()
    }

    /// Returns true if the market is still tradeable (before settlement).
    #[must_use]
    pub fn is_tradeable(&self) -> bool {
        self.settlement_time > Utc::now()
    }
}

// =============================================================================
// Settlement Verification
// =============================================================================

/// Result of verifying settlement criteria match between exchanges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SettlementVerification {
    /// Settlement criteria are identical - safe to arbitrage.
    Identical,

    /// Settlement criteria are compatible with minor differences.
    Compatible {
        /// List of differences that were found.
        differences: Vec<String>,
        /// Adjusted confidence based on differences.
        adjusted_confidence: f64,
    },

    /// Settlement criteria are incompatible - DO NOT arbitrage.
    Incompatible {
        /// Reason for incompatibility.
        reason: String,
    },
}

impl SettlementVerification {
    /// Creates an identical verification.
    #[must_use]
    pub fn identical() -> Self {
        Self::Identical
    }

    /// Creates a compatible verification with differences.
    #[must_use]
    pub fn compatible(differences: Vec<String>, adjusted_confidence: f64) -> Self {
        Self::Compatible {
            differences,
            adjusted_confidence,
        }
    }

    /// Creates an incompatible verification.
    #[must_use]
    pub fn incompatible(reason: impl Into<String>) -> Self {
        Self::Incompatible {
            reason: reason.into(),
        }
    }

    /// Returns true if safe to arbitrage.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self, Self::Identical | Self::Compatible { .. })
    }

    /// Returns the confidence level for this verification.
    #[must_use]
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Identical => 1.0,
            Self::Compatible {
                adjusted_confidence,
                ..
            } => *adjusted_confidence,
            Self::Incompatible { .. } => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ==================== Side Tests ====================

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Yes.opposite(), Side::No);
        assert_eq!(Side::No.opposite(), Side::Yes);
    }

    #[test]
    fn test_side_as_str() {
        assert_eq!(Side::Yes.as_str(), "YES");
        assert_eq!(Side::No.as_str(), "NO");
    }

    #[test]
    fn test_side_display() {
        assert_eq!(format!("{}", Side::Yes), "YES");
        assert_eq!(format!("{}", Side::No), "NO");
    }

    // ==================== Exchange Tests ====================

    #[test]
    fn test_exchange_as_str() {
        assert_eq!(Exchange::Kalshi.as_str(), "Kalshi");
        assert_eq!(Exchange::Polymarket.as_str(), "Polymarket");
    }

    #[test]
    fn test_exchange_display() {
        assert_eq!(format!("{}", Exchange::Kalshi), "Kalshi");
        assert_eq!(format!("{}", Exchange::Polymarket), "Polymarket");
    }

    // ==================== PriceSource Tests ====================

    #[test]
    fn test_price_source_same_is_compatible() {
        assert!(PriceSource::CfBenchmarks.is_compatible_with(&PriceSource::CfBenchmarks));
        assert!(PriceSource::Binance.is_compatible_with(&PriceSource::Binance));
    }

    #[test]
    fn test_price_source_cf_binance_compatible() {
        assert!(PriceSource::CfBenchmarks.is_compatible_with(&PriceSource::Binance));
        assert!(PriceSource::Binance.is_compatible_with(&PriceSource::CfBenchmarks));
    }

    #[test]
    fn test_price_source_coingecko_compatible() {
        assert!(PriceSource::CoinGecko.is_compatible_with(&PriceSource::Binance));
        assert!(PriceSource::CoinGecko.is_compatible_with(&PriceSource::CfBenchmarks));
    }

    #[test]
    fn test_price_source_other_not_compatible() {
        let other1 = PriceSource::Other("CustomSource".to_string());
        let other2 = PriceSource::Other("AnotherSource".to_string());
        assert!(!other1.is_compatible_with(&other2));
        assert!(!other1.is_compatible_with(&PriceSource::Binance));
    }

    #[test]
    fn test_price_source_display() {
        assert_eq!(format!("{}", PriceSource::CfBenchmarks), "CF Benchmarks");
        assert_eq!(format!("{}", PriceSource::Binance), "Binance");
        assert_eq!(
            format!("{}", PriceSource::Other("Custom".to_string())),
            "Custom"
        );
    }

    // ==================== Comparison Tests ====================

    #[test]
    fn test_comparison_as_str() {
        assert_eq!(Comparison::Above.as_str(), "above");
        assert_eq!(Comparison::Below.as_str(), "below");
        assert_eq!(Comparison::Between.as_str(), "between");
    }

    #[test]
    fn test_comparison_same_is_compatible() {
        assert!(Comparison::Above.is_compatible_with(Comparison::Above));
        assert!(Comparison::Below.is_compatible_with(Comparison::Below));
    }

    #[test]
    fn test_comparison_above_at_or_above_compatible() {
        assert!(Comparison::Above.is_compatible_with(Comparison::AtOrAbove));
        assert!(Comparison::AtOrAbove.is_compatible_with(Comparison::Above));
    }

    #[test]
    fn test_comparison_below_at_or_below_compatible() {
        assert!(Comparison::Below.is_compatible_with(Comparison::AtOrBelow));
        assert!(Comparison::AtOrBelow.is_compatible_with(Comparison::Below));
    }

    #[test]
    fn test_comparison_above_below_not_compatible() {
        assert!(!Comparison::Above.is_compatible_with(Comparison::Below));
        assert!(!Comparison::Below.is_compatible_with(Comparison::Above));
    }

    #[test]
    fn test_comparison_display() {
        assert_eq!(format!("{}", Comparison::Above), "above");
        assert_eq!(format!("{}", Comparison::AtOrAbove), "at or above");
    }

    // ==================== SettlementCriteria Tests ====================

    #[test]
    fn test_settlement_criteria_btc_above() {
        let settlement_time = Utc::now() + chrono::Duration::hours(1);
        let criteria = SettlementCriteria::btc_above(dec!(100000), settlement_time);

        assert_eq!(criteria.price_source, PriceSource::CfBenchmarks);
        assert_eq!(criteria.comparison, Comparison::Above);
        assert_eq!(criteria.threshold, dec!(100000));
        assert!(criteria.threshold_upper.is_none());
    }

    #[test]
    fn test_settlement_criteria_compatible_same() {
        let time = Utc::now() + chrono::Duration::hours(1);
        let criteria1 = SettlementCriteria::btc_above(dec!(100000), time);
        let criteria2 = SettlementCriteria::btc_above(dec!(100000), time);

        assert!(criteria1.is_compatible_with(&criteria2));
    }

    #[test]
    fn test_settlement_criteria_compatible_different_source() {
        let time = Utc::now() + chrono::Duration::hours(1);
        let criteria1 = SettlementCriteria {
            price_source: PriceSource::CfBenchmarks,
            settlement_time: time,
            comparison: Comparison::Above,
            threshold: dec!(100000),
            threshold_upper: None,
        };
        let criteria2 = SettlementCriteria {
            price_source: PriceSource::Binance,
            settlement_time: time,
            comparison: Comparison::Above,
            threshold: dec!(100000),
            threshold_upper: None,
        };

        assert!(criteria1.is_compatible_with(&criteria2));
    }

    #[test]
    fn test_settlement_criteria_not_compatible_different_threshold() {
        let time = Utc::now() + chrono::Duration::hours(1);
        let criteria1 = SettlementCriteria::btc_above(dec!(100000), time);
        let criteria2 = SettlementCriteria::btc_above(dec!(105000), time);

        assert!(!criteria1.is_compatible_with(&criteria2));
    }

    #[test]
    fn test_settlement_criteria_not_compatible_different_comparison() {
        let time = Utc::now() + chrono::Duration::hours(1);
        let criteria1 = SettlementCriteria {
            price_source: PriceSource::CfBenchmarks,
            settlement_time: time,
            comparison: Comparison::Above,
            threshold: dec!(100000),
            threshold_upper: None,
        };
        let criteria2 = SettlementCriteria {
            price_source: PriceSource::CfBenchmarks,
            settlement_time: time,
            comparison: Comparison::Below,
            threshold: dec!(100000),
            threshold_upper: None,
        };

        assert!(!criteria1.is_compatible_with(&criteria2));
    }

    #[test]
    fn test_settlement_criteria_time_diff() {
        let time1 = Utc::now();
        let time2 = time1 + chrono::Duration::seconds(60);

        let criteria1 = SettlementCriteria::btc_above(dec!(100000), time1);
        let criteria2 = SettlementCriteria::btc_above(dec!(100000), time2);

        assert_eq!(criteria1.settlement_time_diff_seconds(&criteria2), 60);
        assert_eq!(criteria2.settlement_time_diff_seconds(&criteria1), 60);
    }

    // ==================== MatchedMarket Tests ====================

    fn sample_matched_market() -> MatchedMarket {
        MatchedMarket::new(
            "KXBTC-26FEB02-B100000".to_string(),
            "0xabc123".to_string(),
            "yes-token".to_string(),
            "no-token".to_string(),
            "BTC".to_string(),
            dec!(100000),
            Utc::now() + chrono::Duration::hours(1),
            0.95,
        )
    }

    #[test]
    fn test_matched_market_creation() {
        let market = sample_matched_market();

        assert_eq!(market.kalshi_ticker, "KXBTC-26FEB02-B100000");
        assert_eq!(market.polymarket_condition_id, "0xabc123");
        assert_eq!(market.underlying, "BTC");
        assert_eq!(market.strike_price, dec!(100000));
        assert!((market.match_confidence - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_matched_market_with_notes() {
        let market = sample_matched_market().with_notes("Settlement verified manually");

        assert_eq!(
            market.notes,
            Some("Settlement verified manually".to_string())
        );
    }

    #[test]
    fn test_matched_market_meets_confidence_threshold() {
        let market = sample_matched_market();

        assert!(market.meets_confidence_threshold(0.90));
        assert!(market.meets_confidence_threshold(0.95));
        assert!(!market.meets_confidence_threshold(0.99));
    }

    #[test]
    fn test_matched_market_is_tradeable() {
        let market = sample_matched_market();
        assert!(market.is_tradeable());

        // Create a market with past settlement time
        let past_market = MatchedMarket::new(
            "KXBTC-OLD".to_string(),
            "0xold".to_string(),
            "yes".to_string(),
            "no".to_string(),
            "BTC".to_string(),
            dec!(100000),
            Utc::now() - chrono::Duration::hours(1),
            0.95,
        );
        assert!(!past_market.is_tradeable());
    }

    #[test]
    fn test_matched_market_time_to_settlement() {
        let settlement_time = Utc::now() + chrono::Duration::hours(2);
        let market = MatchedMarket::new(
            "KXBTC-TEST".to_string(),
            "0xtest".to_string(),
            "yes".to_string(),
            "no".to_string(),
            "BTC".to_string(),
            dec!(100000),
            settlement_time,
            0.95,
        );

        let time_to_settlement = market.time_to_settlement();
        // Should be approximately 2 hours (allow 5 seconds tolerance)
        let hours = time_to_settlement.num_hours();
        assert!(hours >= 1 && hours <= 2);
    }

    // ==================== SettlementVerification Tests ====================

    #[test]
    fn test_settlement_verification_identical() {
        let verification = SettlementVerification::identical();

        assert!(verification.is_safe());
        assert!((verification.confidence() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_settlement_verification_compatible() {
        let verification =
            SettlementVerification::compatible(vec!["Different price source".to_string()], 0.9);

        assert!(verification.is_safe());
        assert!((verification.confidence() - 0.9).abs() < 0.001);

        if let SettlementVerification::Compatible { differences, .. } = verification {
            assert_eq!(differences.len(), 1);
        } else {
            panic!("Expected Compatible variant");
        }
    }

    #[test]
    fn test_settlement_verification_incompatible() {
        let verification = SettlementVerification::incompatible("Different threshold");

        assert!(!verification.is_safe());
        assert!((verification.confidence() - 0.0).abs() < 0.001);

        if let SettlementVerification::Incompatible { reason } = verification {
            assert_eq!(reason, "Different threshold");
        } else {
            panic!("Expected Incompatible variant");
        }
    }

    // ==================== Serialization Tests ====================

    #[test]
    fn test_side_serialization() {
        let side = Side::Yes;
        let json = serde_json::to_string(&side).unwrap();
        let deserialized: Side = serde_json::from_str(&json).unwrap();
        assert_eq!(side, deserialized);
    }

    #[test]
    fn test_exchange_serialization() {
        let exchange = Exchange::Kalshi;
        let json = serde_json::to_string(&exchange).unwrap();
        let deserialized: Exchange = serde_json::from_str(&json).unwrap();
        assert_eq!(exchange, deserialized);
    }

    #[test]
    fn test_matched_market_serialization() {
        let market = sample_matched_market();
        let json = serde_json::to_string(&market).unwrap();
        let deserialized: MatchedMarket = serde_json::from_str(&json).unwrap();

        assert_eq!(market.kalshi_ticker, deserialized.kalshi_ticker);
        assert_eq!(
            market.polymarket_condition_id,
            deserialized.polymarket_condition_id
        );
        assert_eq!(market.strike_price, deserialized.strike_price);
    }

    #[test]
    fn test_settlement_verification_serialization() {
        let verification =
            SettlementVerification::compatible(vec!["Minor difference".to_string()], 0.85);
        let json = serde_json::to_string(&verification).unwrap();
        let deserialized: SettlementVerification = serde_json::from_str(&json).unwrap();

        assert!(deserialized.is_safe());
        assert!((deserialized.confidence() - 0.85).abs() < 0.001);
    }
}
