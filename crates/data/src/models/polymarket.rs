//! Polymarket odds data model.
//!
//! Captures binary outcome market prices from Polymarket CLOB.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A snapshot of Polymarket binary outcome prices.
///
/// Used for tracking market odds and finding trading opportunities.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PolymarketOddsRecord {
    /// Timestamp of the odds snapshot
    pub timestamp: DateTime<Utc>,
    /// Polymarket market ID
    pub market_id: String,
    /// Market question/title
    pub question: String,
    /// Price of "Yes" outcome (0.0 to 1.0)
    pub outcome_yes_price: Decimal,
    /// Price of "No" outcome (0.0 to 1.0)
    pub outcome_no_price: Decimal,
    /// 24-hour trading volume in USD
    pub volume_24h: Option<Decimal>,
    /// Current market liquidity in USD
    pub liquidity: Option<Decimal>,
    /// Market end/resolution date
    pub end_date: Option<DateTime<Utc>>,
}

impl PolymarketOddsRecord {
    /// Creates a new odds record.
    pub fn new(
        timestamp: DateTime<Utc>,
        market_id: String,
        question: String,
        outcome_yes_price: Decimal,
        outcome_no_price: Decimal,
    ) -> Self {
        Self {
            timestamp,
            market_id,
            question,
            outcome_yes_price,
            outcome_no_price,
            volume_24h: None,
            liquidity: None,
            end_date: None,
        }
    }

    /// Adds market metadata.
    pub fn with_metadata(
        mut self,
        volume_24h: Option<Decimal>,
        liquidity: Option<Decimal>,
        end_date: Option<DateTime<Utc>>,
    ) -> Self {
        self.volume_24h = volume_24h;
        self.liquidity = liquidity;
        self.end_date = end_date;
        self
    }

    /// Returns the implied probability of "Yes" outcome.
    /// For well-calibrated markets, this approximates the true probability.
    #[must_use]
    pub fn implied_yes_probability(&self) -> Decimal {
        self.outcome_yes_price
    }

    /// Returns the implied probability of "No" outcome.
    #[must_use]
    pub fn implied_no_probability(&self) -> Decimal {
        self.outcome_no_price
    }

    /// Returns the market spread (deviation from perfect pricing).
    /// A spread of 0 means yes_price + no_price = 1.0 (no overround).
    #[must_use]
    pub fn spread(&self) -> Decimal {
        (self.outcome_yes_price + self.outcome_no_price) - Decimal::ONE
    }

    /// Returns the mid price for the "Yes" outcome.
    /// Adjusts for the spread to get a fairer price estimate.
    #[must_use]
    pub fn mid_yes_price(&self) -> Decimal {
        let total = self.outcome_yes_price + self.outcome_no_price;
        if total > Decimal::ZERO {
            self.outcome_yes_price / total
        } else {
            self.outcome_yes_price
        }
    }

    /// Returns the mid price for the "No" outcome.
    #[must_use]
    pub fn mid_no_price(&self) -> Decimal {
        let total = self.outcome_yes_price + self.outcome_no_price;
        if total > Decimal::ZERO {
            self.outcome_no_price / total
        } else {
            self.outcome_no_price
        }
    }

    /// Calculates expected value for buying "Yes" given estimated probability.
    ///
    /// EV = p * (1 - price) - (1 - p) * price
    ///    = p - price
    ///
    /// Positive EV means favorable bet.
    #[must_use]
    pub fn ev_yes(&self, estimated_probability: Decimal) -> Decimal {
        estimated_probability - self.outcome_yes_price
    }

    /// Calculates expected value for buying "No" given estimated probability.
    #[must_use]
    pub fn ev_no(&self, estimated_no_probability: Decimal) -> Decimal {
        estimated_no_probability - self.outcome_no_price
    }

    /// Calculates Kelly fraction for "Yes" bet.
    ///
    /// Kelly: f* = (p(b+1) - 1) / b where b = (1-price)/price
    ///
    /// Returns None if no edge or negative edge.
    #[must_use]
    pub fn kelly_yes(&self, estimated_probability: Decimal) -> Option<Decimal> {
        let price = self.outcome_yes_price;
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return None;
        }

        // Odds: b = (1 - price) / price
        let b = (Decimal::ONE - price) / price;

        // Kelly: f* = (p(b+1) - 1) / b
        let kelly = (estimated_probability * (b + Decimal::ONE) - Decimal::ONE) / b;

        if kelly > Decimal::ZERO {
            Some(kelly)
        } else {
            None
        }
    }

    /// Calculates Kelly fraction for "No" bet.
    #[must_use]
    pub fn kelly_no(&self, estimated_no_probability: Decimal) -> Option<Decimal> {
        let price = self.outcome_no_price;
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return None;
        }

        let b = (Decimal::ONE - price) / price;
        let kelly = (estimated_no_probability * (b + Decimal::ONE) - Decimal::ONE) / b;

        if kelly > Decimal::ZERO {
            Some(kelly)
        } else {
            None
        }
    }

    /// Returns true if market has sufficient liquidity for trading.
    #[must_use]
    pub fn has_sufficient_liquidity(&self, min_liquidity: Decimal) -> bool {
        self.liquidity.map(|l| l >= min_liquidity).unwrap_or(false)
    }

    /// Returns time until market ends, if end_date is set.
    #[must_use]
    pub fn time_to_end(&self) -> Option<chrono::Duration> {
        self.end_date.map(|end| end - self.timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn test_odds_record_creation() {
        let record = PolymarketOddsRecord {
            timestamp: sample_timestamp(),
            market_id: "btc-100k".to_string(),
            question: "Will BTC exceed $100k?".to_string(),
            outcome_yes_price: dec!(0.65),
            outcome_no_price: dec!(0.36),
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
            end_date: Some(sample_timestamp()),
        };

        assert_eq!(record.market_id, "btc-100k");
        assert_eq!(record.outcome_yes_price, dec!(0.65));
    }

    #[test]
    fn test_odds_new() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "Will BTC exceed $100k?".to_string(),
            dec!(0.65),
            dec!(0.35),
        );

        assert_eq!(record.outcome_yes_price, dec!(0.65));
        assert_eq!(record.volume_24h, None);
    }

    #[test]
    fn test_with_metadata() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "Will BTC exceed $100k?".to_string(),
            dec!(0.65),
            dec!(0.35),
        )
        .with_metadata(Some(dec!(50000)), Some(dec!(100000)), None);

        assert_eq!(record.volume_24h, Some(dec!(50000)));
        assert_eq!(record.liquidity, Some(dec!(100000)));
    }

    #[test]
    fn test_implied_probabilities() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.65),
            dec!(0.35),
        );

        assert_eq!(record.implied_yes_probability(), dec!(0.65));
        assert_eq!(record.implied_no_probability(), dec!(0.35));
    }

    #[test]
    fn test_spread_perfect_pricing() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.65),
            dec!(0.35),
        );

        assert_eq!(record.spread(), Decimal::ZERO);
    }

    #[test]
    fn test_spread_with_overround() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.66),
            dec!(0.36),
        );

        assert_eq!(record.spread(), dec!(0.02));
    }

    #[test]
    fn test_mid_prices() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.66),
            dec!(0.36),
        );

        // Total = 1.02, mid_yes = 0.66/1.02 ~= 0.647
        let mid_yes = record.mid_yes_price();
        let mid_no = record.mid_no_price();

        assert!(mid_yes > dec!(0.64) && mid_yes < dec!(0.66));
        assert!(mid_no > dec!(0.34) && mid_no < dec!(0.36));
        assert_eq!(mid_yes + mid_no, Decimal::ONE);
    }

    #[test]
    fn test_ev_yes_positive() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.60),
            dec!(0.40),
        );

        // If we think true probability is 0.70, EV = 0.70 - 0.60 = 0.10
        let ev = record.ev_yes(dec!(0.70));
        assert_eq!(ev, dec!(0.10));
    }

    #[test]
    fn test_ev_yes_negative() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.70),
            dec!(0.30),
        );

        // If we think true probability is 0.60, EV = 0.60 - 0.70 = -0.10
        let ev = record.ev_yes(dec!(0.60));
        assert_eq!(ev, dec!(-0.10));
    }

    #[test]
    fn test_kelly_yes_with_edge() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.50),
            dec!(0.50),
        );

        // Price = 0.50, odds b = 1.0
        // If p = 0.60: kelly = (0.60 * 2 - 1) / 1 = 0.20
        let kelly = record.kelly_yes(dec!(0.60));
        assert_eq!(kelly, Some(dec!(0.20)));
    }

    #[test]
    fn test_kelly_yes_no_edge() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.60),
            dec!(0.40),
        );

        // Price = 0.60, odds b = 0.666...
        // If p = 0.50: kelly = (0.50 * 1.666... - 1) / 0.666... = negative
        let kelly = record.kelly_yes(dec!(0.50));
        assert_eq!(kelly, None);
    }

    #[test]
    fn test_kelly_yes_extreme_prices() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            Decimal::ZERO,
            Decimal::ONE,
        );

        assert_eq!(record.kelly_yes(dec!(0.50)), None);

        let record2 = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            Decimal::ONE,
            Decimal::ZERO,
        );

        assert_eq!(record2.kelly_yes(dec!(0.50)), None);
    }

    #[test]
    fn test_has_sufficient_liquidity() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.65),
            dec!(0.35),
        )
        .with_metadata(None, Some(dec!(50000)), None);

        assert!(record.has_sufficient_liquidity(dec!(10000)));
        assert!(!record.has_sufficient_liquidity(dec!(100000)));
    }

    #[test]
    fn test_has_sufficient_liquidity_no_data() {
        let record = PolymarketOddsRecord::new(
            sample_timestamp(),
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.65),
            dec!(0.35),
        );

        assert!(!record.has_sufficient_liquidity(dec!(10000)));
    }

    #[test]
    fn test_time_to_end() {
        let start = sample_timestamp();
        let end = start + chrono::Duration::hours(24);

        let record = PolymarketOddsRecord::new(
            start,
            "btc-100k".to_string(),
            "test".to_string(),
            dec!(0.65),
            dec!(0.35),
        )
        .with_metadata(None, None, Some(end));

        let duration = record.time_to_end().unwrap();
        assert_eq!(duration.num_hours(), 24);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = PolymarketOddsRecord {
            timestamp: sample_timestamp(),
            market_id: "btc-100k".to_string(),
            question: "Will BTC exceed $100k?".to_string(),
            outcome_yes_price: dec!(0.65),
            outcome_no_price: dec!(0.36),
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
            end_date: Some(sample_timestamp()),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: PolymarketOddsRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.market_id, deserialized.market_id);
        assert_eq!(record.outcome_yes_price, deserialized.outcome_yes_price);
        assert_eq!(record.volume_24h, deserialized.volume_24h);
    }
}
