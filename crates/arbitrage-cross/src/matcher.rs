//! Market matching for cross-exchange arbitrage.
//!
//! This module provides functionality to match equivalent markets across
//! Kalshi and Polymarket exchanges, ensuring settlement criteria alignment.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::types::{Comparison, MatchedMarket, SettlementCriteria, SettlementVerification};

// =============================================================================
// Match Configuration
// =============================================================================

/// Configuration for market matching.
#[derive(Debug, Clone)]
pub struct MatchConfig {
    /// Maximum time difference in seconds between settlement times.
    pub max_settlement_time_diff_seconds: i64,

    /// Minimum match confidence required (0.0 to 1.0).
    pub min_confidence_threshold: f64,

    /// Whether to allow markets with different price sources.
    pub allow_different_price_sources: bool,

    /// Strike price tolerance as a percentage (e.g., 0.001 for 0.1%).
    pub strike_price_tolerance_pct: Decimal,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            // Allow up to 5 minutes difference in settlement time
            max_settlement_time_diff_seconds: 300,
            min_confidence_threshold: 0.90,
            allow_different_price_sources: true,
            // Strike prices must be within 0.1%
            strike_price_tolerance_pct: dec!(0.001),
        }
    }
}

impl MatchConfig {
    /// Creates a strict configuration requiring exact matches.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            max_settlement_time_diff_seconds: 60,
            min_confidence_threshold: 0.99,
            allow_different_price_sources: false,
            strike_price_tolerance_pct: Decimal::ZERO,
        }
    }

    /// Creates a relaxed configuration for broader matching.
    #[must_use]
    pub fn relaxed() -> Self {
        Self {
            max_settlement_time_diff_seconds: 900, // 15 minutes
            min_confidence_threshold: 0.80,
            allow_different_price_sources: true,
            strike_price_tolerance_pct: dec!(0.01), // 1%
        }
    }

    /// Sets the maximum settlement time difference.
    #[must_use]
    pub fn with_max_settlement_diff(mut self, seconds: i64) -> Self {
        self.max_settlement_time_diff_seconds = seconds;
        self
    }

    /// Sets the minimum confidence threshold.
    #[must_use]
    pub fn with_min_confidence(mut self, threshold: f64) -> Self {
        self.min_confidence_threshold = threshold;
        self
    }
}

// =============================================================================
// Parsed Market Info
// =============================================================================

/// Parsed information from a Kalshi market ticker.
#[derive(Debug, Clone)]
pub struct ParsedKalshiMarket {
    /// Original ticker.
    pub ticker: String,
    /// Underlying asset (e.g., "BTC").
    pub underlying: String,
    /// Strike price.
    pub strike_price: Decimal,
    /// Direction (Above or Below).
    pub direction: Comparison,
    /// Settlement time (if parseable from ticker).
    pub settlement_hint: Option<DateTime<Utc>>,
}

/// Parsed information from a Polymarket market.
#[derive(Debug, Clone)]
pub struct ParsedPolymarketMarket {
    /// Condition ID.
    pub condition_id: String,
    /// Yes/Up token ID.
    pub yes_token_id: String,
    /// No/Down token ID.
    pub no_token_id: String,
    /// Underlying asset (if identifiable).
    pub underlying: Option<String>,
    /// Strike price (if parseable).
    pub strike_price: Option<Decimal>,
    /// Settlement time.
    pub settlement_time: Option<DateTime<Utc>>,
}

// =============================================================================
// Market Matcher
// =============================================================================

/// Matches equivalent markets across Kalshi and Polymarket.
#[derive(Debug)]
pub struct MarketMatcher {
    config: MatchConfig,
    /// Cache of known market mappings.
    known_mappings: HashMap<String, String>,
}

impl MarketMatcher {
    /// Creates a new market matcher with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: MatchConfig::default(),
            known_mappings: HashMap::new(),
        }
    }

    /// Creates a new market matcher with custom configuration.
    #[must_use]
    pub fn with_config(config: MatchConfig) -> Self {
        Self {
            config,
            known_mappings: HashMap::new(),
        }
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &MatchConfig {
        &self.config
    }

    /// Adds a known mapping between a Kalshi ticker and Polymarket condition ID.
    pub fn add_known_mapping(&mut self, kalshi_ticker: &str, polymarket_condition_id: &str) {
        self.known_mappings.insert(
            kalshi_ticker.to_string(),
            polymarket_condition_id.to_string(),
        );
    }

    /// Parses a Kalshi market ticker to extract market information.
    ///
    /// Kalshi BTC tickers follow patterns like:
    /// - `KXBTC-26FEB02-B100000` (BTC above $100,000)
    /// - `KXBTC-26FEB02-B95000` (BTC above $95,000)
    #[must_use]
    pub fn parse_kalshi_ticker(&self, ticker: &str) -> Option<ParsedKalshiMarket> {
        // Split by dashes
        let parts: Vec<&str> = ticker.split('-').collect();
        if parts.len() < 3 {
            return None;
        }

        // Check if it's a BTC market
        let underlying = if parts[0].contains("BTC") || parts[0].contains("btc") {
            "BTC".to_string()
        } else if parts[0].contains("ETH") || parts[0].contains("eth") {
            "ETH".to_string()
        } else {
            return None;
        };

        // Parse the direction and strike price
        // B = Above (Bullish), format: B{price}
        let price_part = parts.last()?;
        let (direction, strike_str) = if price_part.starts_with('B') || price_part.starts_with('b')
        {
            (Comparison::Above, &price_part[1..])
        } else if price_part.starts_with('A') || price_part.starts_with('a') {
            // A for At or Above
            (Comparison::AtOrAbove, &price_part[1..])
        } else {
            return None;
        };

        let strike_price: Decimal = strike_str.parse().ok()?;

        Some(ParsedKalshiMarket {
            ticker: ticker.to_string(),
            underlying,
            strike_price,
            direction,
            settlement_hint: None,
        })
    }

    /// Parses a Polymarket market to extract relevant information.
    #[must_use]
    pub fn parse_polymarket_market(
        &self,
        condition_id: &str,
        yes_token_id: &str,
        no_token_id: &str,
        question: Option<&str>,
        settlement_time: Option<DateTime<Utc>>,
    ) -> ParsedPolymarketMarket {
        let mut underlying = None;
        let mut strike_price = None;

        if let Some(q) = question {
            let q_lower = q.to_lowercase();

            // Detect underlying asset
            if q_lower.contains("bitcoin") || q_lower.contains("btc") {
                underlying = Some("BTC".to_string());
            } else if q_lower.contains("ethereum") || q_lower.contains("eth") {
                underlying = Some("ETH".to_string());
            }

            // Try to extract strike price from common patterns
            // e.g., "above $100,000", "above $100k", "above 100000"
            if let Some(idx) = q_lower.find("above") {
                let after = &q[idx + 5..];
                strike_price = extract_price_from_text(after);
            } else if let Some(idx) = q_lower.find("below") {
                let after = &q[idx + 5..];
                strike_price = extract_price_from_text(after);
            }
        }

        ParsedPolymarketMarket {
            condition_id: condition_id.to_string(),
            yes_token_id: yes_token_id.to_string(),
            no_token_id: no_token_id.to_string(),
            underlying,
            strike_price,
            settlement_time,
        }
    }

    /// Attempts to match a Kalshi market with a Polymarket market.
    ///
    /// Returns a matched market if the criteria align within tolerance.
    #[must_use]
    pub fn try_match(
        &self,
        kalshi: &ParsedKalshiMarket,
        polymarket: &ParsedPolymarketMarket,
        kalshi_settlement_time: DateTime<Utc>,
    ) -> Option<MatchedMarket> {
        // Check known mappings first
        if let Some(known_poly_id) = self.known_mappings.get(&kalshi.ticker) {
            if known_poly_id == &polymarket.condition_id {
                debug!(
                    kalshi_ticker = %kalshi.ticker,
                    polymarket_id = %polymarket.condition_id,
                    "Found known mapping"
                );
                return Some(MatchedMarket::new(
                    kalshi.ticker.clone(),
                    polymarket.condition_id.clone(),
                    polymarket.yes_token_id.clone(),
                    polymarket.no_token_id.clone(),
                    kalshi.underlying.clone(),
                    kalshi.strike_price,
                    kalshi_settlement_time,
                    1.0, // Perfect confidence for known mappings
                ));
            }
        }

        // Check underlying asset match
        if let Some(ref poly_underlying) = polymarket.underlying {
            if poly_underlying != &kalshi.underlying {
                debug!(
                    kalshi_underlying = %kalshi.underlying,
                    polymarket_underlying = %poly_underlying,
                    "Underlying asset mismatch"
                );
                return None;
            }
        } else {
            // Can't verify underlying
            return None;
        }

        // Check strike price match
        if let Some(poly_strike) = polymarket.strike_price {
            let tolerance = kalshi.strike_price * self.config.strike_price_tolerance_pct;
            let diff = (kalshi.strike_price - poly_strike).abs();
            if diff > tolerance {
                debug!(
                    kalshi_strike = %kalshi.strike_price,
                    polymarket_strike = %poly_strike,
                    tolerance = %tolerance,
                    "Strike price mismatch"
                );
                return None;
            }
        } else {
            // Can't verify strike price
            return None;
        }

        // Check settlement time match
        if let Some(poly_settlement) = polymarket.settlement_time {
            let time_diff = (kalshi_settlement_time - poly_settlement)
                .num_seconds()
                .abs();
            if time_diff > self.config.max_settlement_time_diff_seconds {
                debug!(
                    kalshi_settlement = %kalshi_settlement_time,
                    polymarket_settlement = %poly_settlement,
                    diff_seconds = time_diff,
                    max_diff = self.config.max_settlement_time_diff_seconds,
                    "Settlement time mismatch"
                );
                return None;
            }
        } else {
            // Can't verify settlement time
            return None;
        }

        // Calculate confidence based on match quality
        let confidence =
            self.calculate_match_confidence(kalshi, polymarket, kalshi_settlement_time);

        if confidence < self.config.min_confidence_threshold {
            debug!(
                confidence = confidence,
                threshold = self.config.min_confidence_threshold,
                "Confidence below threshold"
            );
            return None;
        }

        info!(
            kalshi_ticker = %kalshi.ticker,
            polymarket_id = %polymarket.condition_id,
            underlying = %kalshi.underlying,
            strike = %kalshi.strike_price,
            confidence = confidence,
            "Market match found"
        );

        Some(MatchedMarket::new(
            kalshi.ticker.clone(),
            polymarket.condition_id.clone(),
            polymarket.yes_token_id.clone(),
            polymarket.no_token_id.clone(),
            kalshi.underlying.clone(),
            kalshi.strike_price,
            kalshi_settlement_time,
            confidence,
        ))
    }

    /// Calculates the confidence score for a market match.
    fn calculate_match_confidence(
        &self,
        kalshi: &ParsedKalshiMarket,
        polymarket: &ParsedPolymarketMarket,
        kalshi_settlement: DateTime<Utc>,
    ) -> f64 {
        let mut confidence = 1.0;

        // Reduce confidence for strike price difference
        if let Some(poly_strike) = polymarket.strike_price {
            let diff_pct = ((kalshi.strike_price - poly_strike).abs() / kalshi.strike_price)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            confidence -= diff_pct * 10.0; // 1% diff = 10% confidence reduction
        }

        // Reduce confidence for settlement time difference
        if let Some(poly_settlement) = polymarket.settlement_time {
            let time_diff = (kalshi_settlement - poly_settlement).num_seconds().abs();
            // Reduce confidence by 1% per minute of difference
            let time_penalty = (time_diff as f64 / 60.0) * 0.01;
            confidence -= time_penalty;
        }

        confidence.clamp(0.0, 1.0)
    }

    /// Verifies that settlement criteria match for a matched market.
    ///
    /// This is a critical safety check before executing arbitrage.
    #[must_use]
    pub fn verify_settlement_match(
        &self,
        kalshi_criteria: &SettlementCriteria,
        polymarket_criteria: &SettlementCriteria,
    ) -> SettlementVerification {
        let mut differences = Vec::new();
        let mut confidence_penalty = 0.0;

        // Check price source compatibility
        if !kalshi_criteria
            .price_source
            .is_compatible_with(&polymarket_criteria.price_source)
        {
            if !self.config.allow_different_price_sources {
                return SettlementVerification::incompatible(format!(
                    "Incompatible price sources: {} vs {}",
                    kalshi_criteria.price_source, polymarket_criteria.price_source
                ));
            }
            differences.push(format!(
                "Different price sources: {} vs {}",
                kalshi_criteria.price_source, polymarket_criteria.price_source
            ));
            confidence_penalty += 0.05;
        }

        // Check comparison type compatibility
        if !kalshi_criteria
            .comparison
            .is_compatible_with(polymarket_criteria.comparison)
        {
            return SettlementVerification::incompatible(format!(
                "Incompatible comparison types: {} vs {}",
                kalshi_criteria.comparison, polymarket_criteria.comparison
            ));
        }

        if kalshi_criteria.comparison != polymarket_criteria.comparison {
            differences.push(format!(
                "Slightly different comparison: {} vs {}",
                kalshi_criteria.comparison, polymarket_criteria.comparison
            ));
            confidence_penalty += 0.02;
        }

        // Check threshold match
        if kalshi_criteria.threshold != polymarket_criteria.threshold {
            let diff = (kalshi_criteria.threshold - polymarket_criteria.threshold).abs();
            let tolerance = kalshi_criteria.threshold * self.config.strike_price_tolerance_pct;

            if diff > tolerance {
                return SettlementVerification::incompatible(format!(
                    "Threshold mismatch: {} vs {} (diff: {})",
                    kalshi_criteria.threshold, polymarket_criteria.threshold, diff
                ));
            }
            differences.push(format!(
                "Minor threshold difference: {} vs {}",
                kalshi_criteria.threshold, polymarket_criteria.threshold
            ));
            confidence_penalty += 0.01;
        }

        // Check settlement time
        let time_diff = kalshi_criteria.settlement_time_diff_seconds(polymarket_criteria);
        if time_diff > self.config.max_settlement_time_diff_seconds {
            return SettlementVerification::incompatible(format!(
                "Settlement time difference too large: {} seconds",
                time_diff
            ));
        }

        if time_diff > 0 {
            differences.push(format!("Settlement time difference: {} seconds", time_diff));
            // 1% penalty per minute
            confidence_penalty += (time_diff as f64 / 60.0) * 0.01;
        }

        if differences.is_empty() {
            SettlementVerification::identical()
        } else {
            let adjusted_confidence = (1.0 - confidence_penalty).max(0.0);
            SettlementVerification::compatible(differences, adjusted_confidence)
        }
    }

    /// Finds all matching BTC markets from provided lists.
    ///
    /// # Arguments
    /// * `kalshi_markets` - List of parsed Kalshi markets with settlement times
    /// * `polymarket_markets` - List of parsed Polymarket markets
    ///
    /// # Returns
    /// Vector of matched markets that meet the confidence threshold.
    pub fn find_btc_matches(
        &self,
        kalshi_markets: &[(ParsedKalshiMarket, DateTime<Utc>)],
        polymarket_markets: &[ParsedPolymarketMarket],
    ) -> Vec<MatchedMarket> {
        let mut matches = Vec::new();

        for (kalshi, kalshi_settlement) in kalshi_markets {
            if kalshi.underlying != "BTC" {
                continue;
            }

            for poly in polymarket_markets {
                if let Some(matched) = self.try_match(kalshi, poly, *kalshi_settlement) {
                    matches.push(matched);
                }
            }
        }

        info!(
            kalshi_count = kalshi_markets.len(),
            polymarket_count = polymarket_markets.len(),
            matches_found = matches.len(),
            "BTC market matching complete"
        );

        matches
    }
}

impl Default for MarketMatcher {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extracts a price value from text.
///
/// Handles formats like "$100,000", "$100k", "100000", etc.
fn extract_price_from_text(text: &str) -> Option<Decimal> {
    // Remove common formatting
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == 'k' || *c == 'K')
        .collect();

    if cleaned.is_empty() {
        return None;
    }

    // Handle "k" suffix for thousands
    if cleaned.to_lowercase().ends_with('k') {
        let num_part = &cleaned[..cleaned.len() - 1];
        let value: Decimal = num_part.parse().ok()?;
        return Some(value * dec!(1000));
    }

    cleaned.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PriceSource;

    // ==================== MatchConfig Tests ====================

    #[test]
    fn test_match_config_default() {
        let config = MatchConfig::default();

        assert_eq!(config.max_settlement_time_diff_seconds, 300);
        assert!((config.min_confidence_threshold - 0.90).abs() < 0.001);
        assert!(config.allow_different_price_sources);
    }

    #[test]
    fn test_match_config_strict() {
        let config = MatchConfig::strict();

        assert_eq!(config.max_settlement_time_diff_seconds, 60);
        assert!((config.min_confidence_threshold - 0.99).abs() < 0.001);
        assert!(!config.allow_different_price_sources);
    }

    #[test]
    fn test_match_config_relaxed() {
        let config = MatchConfig::relaxed();

        assert_eq!(config.max_settlement_time_diff_seconds, 900);
        assert!((config.min_confidence_threshold - 0.80).abs() < 0.001);
    }

    #[test]
    fn test_match_config_builder() {
        let config = MatchConfig::default()
            .with_max_settlement_diff(600)
            .with_min_confidence(0.85);

        assert_eq!(config.max_settlement_time_diff_seconds, 600);
        assert!((config.min_confidence_threshold - 0.85).abs() < 0.001);
    }

    // ==================== Kalshi Ticker Parsing Tests ====================

    #[test]
    fn test_parse_kalshi_ticker_btc_above() {
        let matcher = MarketMatcher::new();
        let parsed = matcher.parse_kalshi_ticker("KXBTC-26FEB02-B100000");

        assert!(parsed.is_some());
        let market = parsed.unwrap();
        assert_eq!(market.underlying, "BTC");
        assert_eq!(market.strike_price, dec!(100000));
        assert_eq!(market.direction, Comparison::Above);
    }

    #[test]
    fn test_parse_kalshi_ticker_btc_different_strike() {
        let matcher = MarketMatcher::new();
        let parsed = matcher.parse_kalshi_ticker("KXBTC-26FEB02-B95000");

        assert!(parsed.is_some());
        let market = parsed.unwrap();
        assert_eq!(market.strike_price, dec!(95000));
    }

    #[test]
    fn test_parse_kalshi_ticker_eth() {
        let matcher = MarketMatcher::new();
        let parsed = matcher.parse_kalshi_ticker("KXETH-26FEB02-B3500");

        assert!(parsed.is_some());
        let market = parsed.unwrap();
        assert_eq!(market.underlying, "ETH");
        assert_eq!(market.strike_price, dec!(3500));
    }

    #[test]
    fn test_parse_kalshi_ticker_invalid_format() {
        let matcher = MarketMatcher::new();

        assert!(matcher.parse_kalshi_ticker("INVALID").is_none());
        assert!(matcher.parse_kalshi_ticker("").is_none());
        assert!(matcher.parse_kalshi_ticker("KXBTC").is_none());
    }

    #[test]
    fn test_parse_kalshi_ticker_lowercase() {
        let matcher = MarketMatcher::new();
        let parsed = matcher.parse_kalshi_ticker("kxbtc-26feb02-b100000");

        assert!(parsed.is_some());
        let market = parsed.unwrap();
        assert_eq!(market.underlying, "BTC");
    }

    // ==================== Polymarket Parsing Tests ====================

    #[test]
    fn test_parse_polymarket_market_btc() {
        let matcher = MarketMatcher::new();
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let parsed = matcher.parse_polymarket_market(
            "0xabc123",
            "yes-token",
            "no-token",
            Some("Will Bitcoin be above $100,000?"),
            Some(settlement),
        );

        assert_eq!(parsed.condition_id, "0xabc123");
        assert_eq!(parsed.underlying, Some("BTC".to_string()));
        assert_eq!(parsed.strike_price, Some(dec!(100000)));
        assert_eq!(parsed.settlement_time, Some(settlement));
    }

    #[test]
    fn test_parse_polymarket_market_eth() {
        let matcher = MarketMatcher::new();

        let parsed = matcher.parse_polymarket_market(
            "0xdef456",
            "yes",
            "no",
            Some("Will ETH be above $3,500?"),
            None,
        );

        assert_eq!(parsed.underlying, Some("ETH".to_string()));
        assert_eq!(parsed.strike_price, Some(dec!(3500)));
    }

    #[test]
    fn test_parse_polymarket_market_no_question() {
        let matcher = MarketMatcher::new();

        let parsed = matcher.parse_polymarket_market("0xtest", "yes", "no", None, None);

        assert!(parsed.underlying.is_none());
        assert!(parsed.strike_price.is_none());
    }

    // ==================== Market Matching Tests ====================

    #[test]
    fn test_try_match_success() {
        let matcher = MarketMatcher::with_config(MatchConfig::relaxed());
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-26FEB02-B100000".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0xabc".to_string(),
            yes_token_id: "yes-token".to_string(),
            no_token_id: "no-token".to_string(),
            underlying: Some("BTC".to_string()),
            strike_price: Some(dec!(100000)),
            settlement_time: Some(settlement),
        };

        let matched = matcher.try_match(&kalshi, &poly, settlement);
        assert!(matched.is_some());

        let m = matched.unwrap();
        assert_eq!(m.kalshi_ticker, "KXBTC-26FEB02-B100000");
        assert_eq!(m.polymarket_condition_id, "0xabc");
        assert_eq!(m.strike_price, dec!(100000));
        assert!(m.match_confidence >= 0.80);
    }

    #[test]
    fn test_try_match_underlying_mismatch() {
        let matcher = MarketMatcher::new();
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-TEST".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0xabc".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: Some("ETH".to_string()),
            strike_price: Some(dec!(100000)),
            settlement_time: Some(settlement),
        };

        let matched = matcher.try_match(&kalshi, &poly, settlement);
        assert!(matched.is_none());
    }

    #[test]
    fn test_try_match_strike_price_mismatch() {
        let matcher = MarketMatcher::new();
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-TEST".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0xabc".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: Some("BTC".to_string()),
            strike_price: Some(dec!(95000)), // Different strike
            settlement_time: Some(settlement),
        };

        let matched = matcher.try_match(&kalshi, &poly, settlement);
        assert!(matched.is_none());
    }

    #[test]
    fn test_try_match_settlement_time_mismatch() {
        let matcher = MarketMatcher::new();
        let kalshi_settlement = Utc::now() + chrono::Duration::hours(1);
        let poly_settlement = Utc::now() + chrono::Duration::hours(2); // 1 hour difference

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-TEST".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0xabc".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: Some("BTC".to_string()),
            strike_price: Some(dec!(100000)),
            settlement_time: Some(poly_settlement),
        };

        let matched = matcher.try_match(&kalshi, &poly, kalshi_settlement);
        assert!(matched.is_none()); // 1 hour > 5 minute default
    }

    #[test]
    fn test_try_match_known_mapping() {
        let mut matcher = MarketMatcher::new();
        matcher.add_known_mapping("KXBTC-KNOWN", "0xknown");

        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-KNOWN".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0xknown".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: None, // Unknown, but mapping exists
            strike_price: None,
            settlement_time: None,
        };

        let matched = matcher.try_match(&kalshi, &poly, settlement);
        assert!(matched.is_some());
        assert!((matched.unwrap().match_confidence - 1.0).abs() < 0.001);
    }

    // ==================== Settlement Verification Tests ====================

    #[test]
    fn test_verify_settlement_identical() {
        let matcher = MarketMatcher::new();
        let time = Utc::now() + chrono::Duration::hours(1);

        let kalshi_criteria = SettlementCriteria::btc_above(dec!(100000), time);
        let poly_criteria = SettlementCriteria::btc_above(dec!(100000), time);

        let verification = matcher.verify_settlement_match(&kalshi_criteria, &poly_criteria);
        assert!(matches!(verification, SettlementVerification::Identical));
        assert!(verification.is_safe());
    }

    #[test]
    fn test_verify_settlement_compatible() {
        let matcher = MarketMatcher::new();
        let time = Utc::now() + chrono::Duration::hours(1);
        let time2 = time + chrono::Duration::seconds(60);

        let kalshi_criteria = SettlementCriteria::btc_above(dec!(100000), time);
        let poly_criteria = SettlementCriteria::btc_above(dec!(100000), time2);

        let verification = matcher.verify_settlement_match(&kalshi_criteria, &poly_criteria);
        assert!(verification.is_safe());
        assert!(verification.confidence() < 1.0);
    }

    #[test]
    fn test_verify_settlement_incompatible_threshold() {
        let matcher = MarketMatcher::new();
        let time = Utc::now() + chrono::Duration::hours(1);

        let kalshi_criteria = SettlementCriteria::btc_above(dec!(100000), time);
        let poly_criteria = SettlementCriteria::btc_above(dec!(90000), time);

        let verification = matcher.verify_settlement_match(&kalshi_criteria, &poly_criteria);
        assert!(!verification.is_safe());
        assert!(matches!(
            verification,
            SettlementVerification::Incompatible { .. }
        ));
    }

    #[test]
    fn test_verify_settlement_incompatible_comparison() {
        let matcher = MarketMatcher::new();
        let time = Utc::now() + chrono::Duration::hours(1);

        let kalshi_criteria = SettlementCriteria {
            price_source: PriceSource::CfBenchmarks,
            settlement_time: time,
            comparison: Comparison::Above,
            threshold: dec!(100000),
            threshold_upper: None,
        };

        let poly_criteria = SettlementCriteria {
            price_source: PriceSource::CfBenchmarks,
            settlement_time: time,
            comparison: Comparison::Below,
            threshold: dec!(100000),
            threshold_upper: None,
        };

        let verification = matcher.verify_settlement_match(&kalshi_criteria, &poly_criteria);
        assert!(!verification.is_safe());
    }

    // ==================== Find BTC Matches Tests ====================

    #[test]
    fn test_find_btc_matches() {
        let matcher = MarketMatcher::with_config(MatchConfig::relaxed());
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi_markets = vec![
            (
                ParsedKalshiMarket {
                    ticker: "KXBTC-1".to_string(),
                    underlying: "BTC".to_string(),
                    strike_price: dec!(100000),
                    direction: Comparison::Above,
                    settlement_hint: None,
                },
                settlement,
            ),
            (
                ParsedKalshiMarket {
                    ticker: "KXBTC-2".to_string(),
                    underlying: "BTC".to_string(),
                    strike_price: dec!(105000),
                    direction: Comparison::Above,
                    settlement_hint: None,
                },
                settlement,
            ),
        ];

        let poly_markets = vec![
            ParsedPolymarketMarket {
                condition_id: "0x1".to_string(),
                yes_token_id: "yes1".to_string(),
                no_token_id: "no1".to_string(),
                underlying: Some("BTC".to_string()),
                strike_price: Some(dec!(100000)),
                settlement_time: Some(settlement),
            },
            ParsedPolymarketMarket {
                condition_id: "0x2".to_string(),
                yes_token_id: "yes2".to_string(),
                no_token_id: "no2".to_string(),
                underlying: Some("ETH".to_string()), // Won't match
                strike_price: Some(dec!(3500)),
                settlement_time: Some(settlement),
            },
        ];

        let matches = matcher.find_btc_matches(&kalshi_markets, &poly_markets);

        // Should find 1 match (KXBTC-1 with 0x1)
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kalshi_ticker, "KXBTC-1");
        assert_eq!(matches[0].polymarket_condition_id, "0x1");
    }

    #[test]
    fn test_find_btc_matches_empty() {
        let matcher = MarketMatcher::new();

        let matches = matcher.find_btc_matches(&[], &[]);
        assert!(matches.is_empty());
    }

    // ==================== Helper Function Tests ====================

    #[test]
    fn test_extract_price_from_text_dollars() {
        assert_eq!(extract_price_from_text("$100,000"), Some(dec!(100000)));
        assert_eq!(extract_price_from_text(" $100000"), Some(dec!(100000)));
    }

    #[test]
    fn test_extract_price_from_text_k_suffix() {
        assert_eq!(extract_price_from_text("$100k"), Some(dec!(100000)));
        assert_eq!(extract_price_from_text("100K"), Some(dec!(100000)));
    }

    #[test]
    fn test_extract_price_from_text_plain() {
        assert_eq!(extract_price_from_text("100000"), Some(dec!(100000)));
        assert_eq!(extract_price_from_text("95000"), Some(dec!(95000)));
    }

    #[test]
    fn test_extract_price_from_text_decimal() {
        assert_eq!(extract_price_from_text("99.5k"), Some(dec!(99500)));
    }

    #[test]
    fn test_extract_price_from_text_invalid() {
        assert!(extract_price_from_text("").is_none());
        assert!(extract_price_from_text("no numbers").is_none());
    }

    // ==================== Confidence Calculation Tests ====================

    #[test]
    fn test_match_confidence_perfect() {
        let matcher = MarketMatcher::new();
        let settlement = Utc::now() + chrono::Duration::hours(1);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-TEST".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0x1".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: Some("BTC".to_string()),
            strike_price: Some(dec!(100000)),
            settlement_time: Some(settlement),
        };

        let confidence = matcher.calculate_match_confidence(&kalshi, &poly, settlement);
        assert!((confidence - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_match_confidence_with_time_diff() {
        let matcher = MarketMatcher::with_config(MatchConfig::relaxed());
        let settlement = Utc::now() + chrono::Duration::hours(1);
        let poly_settlement = settlement + chrono::Duration::minutes(2);

        let kalshi = ParsedKalshiMarket {
            ticker: "KXBTC-TEST".to_string(),
            underlying: "BTC".to_string(),
            strike_price: dec!(100000),
            direction: Comparison::Above,
            settlement_hint: None,
        };

        let poly = ParsedPolymarketMarket {
            condition_id: "0x1".to_string(),
            yes_token_id: "yes".to_string(),
            no_token_id: "no".to_string(),
            underlying: Some("BTC".to_string()),
            strike_price: Some(dec!(100000)),
            settlement_time: Some(poly_settlement),
        };

        let confidence = matcher.calculate_match_confidence(&kalshi, &poly, settlement);
        // 2 minutes diff = 2 * 0.01 = 2% penalty
        assert!(confidence < 1.0);
        assert!(confidence > 0.95);
    }
}
