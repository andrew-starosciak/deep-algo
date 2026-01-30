//! Funding rate data model.
//!
//! Captures perpetual futures funding rates with statistical context
//! for funding rate reversal signals.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A funding rate observation with statistical context.
///
/// Funding rates are paid/received every 8 hours on most perpetual exchanges.
/// The rate_percentile and rate_zscore provide historical context for the rate.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FundingRateRecord {
    /// Timestamp of the funding rate
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// The 8-hour funding rate (e.g., 0.0001 = 0.01%)
    pub funding_rate: Decimal,
    /// Annualized funding rate (rate * 3 * 365)
    pub annual_rate: Option<Decimal>,
    /// Percentile rank in historical distribution (0.0 to 1.0)
    pub rate_percentile: Option<Decimal>,
    /// Z-score relative to historical mean
    pub rate_zscore: Option<Decimal>,
}

impl FundingRateRecord {
    /// Creates a new funding rate record with calculated annual rate.
    ///
    /// # Arguments
    /// * `timestamp` - When the funding rate was observed
    /// * `symbol` - Trading pair symbol
    /// * `exchange` - Exchange name
    /// * `funding_rate` - The 8-hour funding rate
    pub fn new(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        funding_rate: Decimal,
    ) -> Self {
        // Annualize: 3 funding periods per day * 365 days
        let annual_rate = Some(funding_rate * Decimal::from(3 * 365));

        Self {
            timestamp,
            symbol,
            exchange,
            funding_rate,
            annual_rate,
            rate_percentile: None,
            rate_zscore: None,
        }
    }

    /// Updates the statistical context for this funding rate.
    pub fn with_statistics(mut self, percentile: Decimal, zscore: Decimal) -> Self {
        self.rate_percentile = Some(percentile);
        self.rate_zscore = Some(zscore);
        self
    }

    /// Returns true if funding rate is extremely positive (longs pay shorts).
    /// This often precedes a price drop as overleveraged longs get squeezed.
    #[must_use]
    pub fn is_extremely_positive(&self, zscore_threshold: Decimal) -> bool {
        self.rate_zscore
            .map(|z| z > zscore_threshold)
            .unwrap_or(false)
    }

    /// Returns true if funding rate is extremely negative (shorts pay longs).
    /// This often precedes a price rise as overleveraged shorts get squeezed.
    #[must_use]
    pub fn is_extremely_negative(&self, zscore_threshold: Decimal) -> bool {
        self.rate_zscore
            .map(|z| z < -zscore_threshold)
            .unwrap_or(false)
    }

    /// Returns the direction bias based on extreme funding rates.
    /// Extreme positive funding suggests bearish bias (reversal expected).
    /// Extreme negative funding suggests bullish bias (reversal expected).
    #[must_use]
    pub fn reversal_bias(&self, zscore_threshold: Decimal) -> Option<FundingBias> {
        let zscore = self.rate_zscore?;
        if zscore > zscore_threshold {
            Some(FundingBias::Bearish)
        } else if zscore < -zscore_threshold {
            Some(FundingBias::Bullish)
        } else {
            None
        }
    }
}

/// Direction bias from funding rate analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingBias {
    /// Expect price to rise (extreme negative funding)
    Bullish,
    /// Expect price to fall (extreme positive funding)
    Bearish,
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
    fn test_funding_rate_creation() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.0001),
            annual_rate: Some(dec!(0.1095)),
            rate_percentile: Some(dec!(0.75)),
            rate_zscore: Some(dec!(1.5)),
        };

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.funding_rate, dec!(0.0001));
    }

    #[test]
    fn test_funding_rate_new_calculates_annual() {
        let record = FundingRateRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        );

        // annual = 0.0001 * 3 * 365 = 0.1095
        let annual = record.annual_rate.unwrap();
        assert_eq!(annual, dec!(0.1095));
    }

    #[test]
    fn test_with_statistics() {
        let record = FundingRateRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        )
        .with_statistics(dec!(0.85), dec!(2.1));

        assert_eq!(record.rate_percentile, Some(dec!(0.85)));
        assert_eq!(record.rate_zscore, Some(dec!(2.1)));
    }

    #[test]
    fn test_is_extremely_positive() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.001),
            annual_rate: None,
            rate_percentile: Some(dec!(0.99)),
            rate_zscore: Some(dec!(2.5)),
        };

        assert!(record.is_extremely_positive(dec!(2.0)));
        assert!(!record.is_extremely_positive(dec!(3.0)));
    }

    #[test]
    fn test_is_extremely_negative() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(-0.001),
            annual_rate: None,
            rate_percentile: Some(dec!(0.01)),
            rate_zscore: Some(dec!(-2.5)),
        };

        assert!(record.is_extremely_negative(dec!(2.0)));
        assert!(!record.is_extremely_negative(dec!(3.0)));
    }

    #[test]
    fn test_reversal_bias_bullish() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(-0.001),
            annual_rate: None,
            rate_percentile: None,
            rate_zscore: Some(dec!(-2.5)),
        };

        assert_eq!(record.reversal_bias(dec!(2.0)), Some(FundingBias::Bullish));
    }

    #[test]
    fn test_reversal_bias_bearish() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.001),
            annual_rate: None,
            rate_percentile: None,
            rate_zscore: Some(dec!(2.5)),
        };

        assert_eq!(record.reversal_bias(dec!(2.0)), Some(FundingBias::Bearish));
    }

    #[test]
    fn test_reversal_bias_neutral() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.0001),
            annual_rate: None,
            rate_percentile: None,
            rate_zscore: Some(dec!(0.5)),
        };

        assert_eq!(record.reversal_bias(dec!(2.0)), None);
    }

    #[test]
    fn test_reversal_bias_no_zscore() {
        let record = FundingRateRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            dec!(0.0001),
        );

        assert_eq!(record.reversal_bias(dec!(2.0)), None);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = FundingRateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.0001),
            annual_rate: Some(dec!(0.1095)),
            rate_percentile: Some(dec!(0.75)),
            rate_zscore: Some(dec!(1.5)),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: FundingRateRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.symbol, deserialized.symbol);
        assert_eq!(record.funding_rate, deserialized.funding_rate);
        assert_eq!(record.rate_zscore, deserialized.rate_zscore);
    }
}
