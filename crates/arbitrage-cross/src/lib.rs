//! Cross-exchange arbitrage detection and orchestration.
//!
//! This crate provides tools for detecting and executing arbitrage opportunities
//! between Kalshi and Polymarket prediction markets.
//!
//! # Overview
//!
//! When the same event (e.g., "BTC above $100k at 3pm") is priced differently
//! across exchanges, buying opposing positions can guarantee profit:
//!
//! ```text
//! Kalshi:     YES @ $0.55,  NO @ $0.46  (internal spread)
//! Polymarket: YES @ $0.52,  NO @ $0.50  (internal spread)
//!
//! Cross-exchange opportunity:
//!   Buy Kalshi NO     @ $0.46
//!   Buy Polymarket YES @ $0.52
//!   Total cost:         $0.98
//!   Guaranteed payout:  $1.00
//!   Gross profit:       $0.02 (2.04%)
//! ```
//!
//! # Modules
//!
//! - [`types`]: Core types for cross-exchange operations
//! - [`fees`]: Fee calculations for both exchanges
//! - [`matcher`]: Match equivalent markets across exchanges
//! - [`detector`]: Detect arbitrage opportunities
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_arbitrage_cross::{
//!     CrossExchangeDetector, DetectorConfig,
//!     MarketMatcher, MatchConfig,
//!     FeeCalculator,
//! };
//!
//! // Create detector with conservative settings
//! let detector = CrossExchangeDetector::with_config(
//!     DetectorConfig::conservative()
//! );
//!
//! // Match markets
//! let matcher = MarketMatcher::with_config(MatchConfig::default());
//! let matches = matcher.find_btc_matches(&kalshi_markets, &poly_markets);
//!
//! // Detect opportunities
//! for matched in &matches {
//!     if let Some(opp) = detector.detect(
//!         &matched,
//!         &kalshi_book,
//!         &poly_yes_book,
//!         &poly_no_book,
//!     ) {
//!         println!(
//!             "Found opportunity: {} net edge, ${} expected profit",
//!             opp.net_edge_pct,
//!             opp.expected_profit
//!         );
//!     }
//! }
//! ```
//!
//! # Safety
//!
//! **CRITICAL**: Before executing arbitrage, always verify:
//!
//! 1. Settlement criteria match exactly (use [`MarketMatcher::verify_settlement_match`])
//! 2. Match confidence exceeds threshold (0.99 recommended)
//! 3. Both exchanges are operational
//! 4. Sufficient balance on both exchanges
//!
//! Mismatched settlement criteria can turn guaranteed arbitrage into a gamble.

pub mod detector;
pub mod fees;
pub mod matcher;
pub mod types;

// Re-export main types for convenience
pub use detector::{
    CrossExchangeDetector, CrossExchangeOpportunity, DetectionSummary, DetectorConfig,
    OpportunitySummary,
};
pub use fees::{ArbitrageFees, FeeCalculator, FeeConfig};
pub use matcher::{MarketMatcher, MatchConfig, ParsedKalshiMarket, ParsedPolymarketMarket};
pub use types::{
    Comparison, Exchange, MatchedMarket, PriceSource, SettlementCriteria, SettlementVerification,
    Side,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    #[test]
    fn test_public_api_exports() {
        // Verify main types are accessible
        let _ = FeeCalculator::new();
        let _ = CrossExchangeDetector::new();
        let _ = MarketMatcher::new();
        let _ = DetectorConfig::default();
        let _ = MatchConfig::default();
        let _ = FeeConfig::default();
    }

    #[test]
    fn test_types_accessible() {
        let _ = Side::Yes;
        let _ = Side::No;
        let _ = Exchange::Kalshi;
        let _ = Exchange::Polymarket;
        let _ = Comparison::Above;
        let _ = PriceSource::CfBenchmarks;
    }

    #[test]
    fn test_settlement_criteria_accessible() {
        let time = Utc::now() + chrono::Duration::hours(1);
        let criteria = SettlementCriteria::btc_above(dec!(100000), time);
        assert_eq!(criteria.threshold, dec!(100000));
    }

    #[test]
    fn test_matched_market_creation() {
        let settlement = Utc::now() + chrono::Duration::hours(1);
        let matched = MatchedMarket::new(
            "KXBTC-TEST".to_string(),
            "0xtest".to_string(),
            "yes-token".to_string(),
            "no-token".to_string(),
            "BTC".to_string(),
            dec!(100000),
            settlement,
            0.95,
        );

        assert_eq!(matched.underlying, "BTC");
        assert!(matched.is_tradeable());
    }

    #[test]
    fn test_fee_calculation() {
        let calc = FeeCalculator::new();
        let fees = calc.calculate_arbitrage_fees(dec!(50), dec!(100), dec!(0.50), dec!(100));

        assert!(fees.total_fee > Decimal::ZERO);
        assert!(fees.kalshi_fee > Decimal::ZERO);
        assert!(fees.polymarket_trading_fee > Decimal::ZERO);
    }

    #[test]
    fn test_integration_matcher_and_detector() {
        let matcher = MarketMatcher::with_config(MatchConfig::relaxed());
        let detector = CrossExchangeDetector::with_config(DetectorConfig::default());

        // Parse a Kalshi ticker
        let parsed = matcher.parse_kalshi_ticker("KXBTC-26FEB02-B100000");
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().underlying, "BTC");

        // Verify detector config is accessible
        assert!(detector.config().min_net_edge > Decimal::ZERO);
    }

    use rust_decimal::Decimal;
}
