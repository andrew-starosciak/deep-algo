//! Liquidation data models.
//!
//! Captures individual liquidation events and rolling window aggregates
//! for cascade detection signals.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// An individual liquidation event.
///
/// Liquidation events above the USD threshold (typically $3K) are captured
/// for analysis of market stress and cascade potential.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LiquidationRecord {
    /// Timestamp of the liquidation
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// Side that was liquidated: "long" or "short"
    pub side: String,
    /// Quantity liquidated in base currency
    pub quantity: Decimal,
    /// Price at which liquidation occurred
    pub price: Decimal,
    /// USD value of the liquidation
    pub usd_value: Decimal,
}

impl LiquidationRecord {
    /// Creates a new liquidation record.
    pub fn new(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        side: LiquidationSide,
        quantity: Decimal,
        price: Decimal,
    ) -> Self {
        let usd_value = quantity * price;
        Self {
            timestamp,
            symbol,
            exchange,
            side: side.as_str().to_string(),
            quantity,
            price,
            usd_value,
        }
    }

    /// Returns true if this is a long liquidation.
    #[must_use]
    pub fn is_long(&self) -> bool {
        self.side == "long"
    }

    /// Returns true if this is a short liquidation.
    #[must_use]
    pub fn is_short(&self) -> bool {
        self.side == "short"
    }

    /// Returns true if this is a significant liquidation (above threshold).
    #[must_use]
    pub fn is_significant(&self, threshold_usd: Decimal) -> bool {
        self.usd_value >= threshold_usd
    }
}

/// Side of a liquidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidationSide {
    Long,
    Short,
}

impl LiquidationSide {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            LiquidationSide::Long => "long",
            LiquidationSide::Short => "short",
        }
    }
}

/// Rolling window aggregate of liquidations.
///
/// Aggregates liquidation data over configurable time windows
/// for detecting cascade patterns.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LiquidationAggregateRecord {
    /// End timestamp of the window
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol
    pub symbol: String,
    /// Exchange name
    pub exchange: String,
    /// Window size in minutes
    pub window_minutes: i32,
    /// Total USD volume of long liquidations
    pub long_volume: Decimal,
    /// Total USD volume of short liquidations
    pub short_volume: Decimal,
    /// Net delta: long_volume - short_volume
    pub net_delta: Decimal,
    /// Count of long liquidation events
    pub count_long: i32,
    /// Count of short liquidation events
    pub count_short: i32,
}

impl LiquidationAggregateRecord {
    /// Creates a new aggregate from a list of liquidations.
    pub fn from_liquidations(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        window_minutes: i32,
        liquidations: &[LiquidationRecord],
    ) -> Self {
        let mut long_volume = Decimal::ZERO;
        let mut short_volume = Decimal::ZERO;
        let mut count_long = 0;
        let mut count_short = 0;

        for liq in liquidations {
            if liq.is_long() {
                long_volume += liq.usd_value;
                count_long += 1;
            } else {
                short_volume += liq.usd_value;
                count_short += 1;
            }
        }

        let net_delta = long_volume - short_volume;

        Self {
            timestamp,
            symbol,
            exchange,
            window_minutes,
            long_volume,
            short_volume,
            net_delta,
            count_long,
            count_short,
        }
    }

    /// Returns the total liquidation volume (long + short).
    #[must_use]
    pub fn total_volume(&self) -> Decimal {
        self.long_volume + self.short_volume
    }

    /// Returns the imbalance ratio: (long - short) / (long + short).
    #[must_use]
    pub fn imbalance_ratio(&self) -> Option<Decimal> {
        let total = self.total_volume();
        if total > Decimal::ZERO {
            Some(self.net_delta / total)
        } else {
            None
        }
    }

    /// Returns true if there's a potential long cascade (high long liquidations).
    #[must_use]
    pub fn is_long_cascade(&self, volume_threshold: Decimal, imbalance_threshold: Decimal) -> bool {
        self.long_volume >= volume_threshold
            && self
                .imbalance_ratio()
                .map(|r| r > imbalance_threshold)
                .unwrap_or(false)
    }

    /// Returns true if there's a potential short cascade (high short liquidations).
    #[must_use]
    pub fn is_short_cascade(
        &self,
        volume_threshold: Decimal,
        imbalance_threshold: Decimal,
    ) -> bool {
        self.short_volume >= volume_threshold
            && self
                .imbalance_ratio()
                .map(|r| r < -imbalance_threshold)
                .unwrap_or(false)
    }

    /// Returns the cascade direction if thresholds are met.
    #[must_use]
    pub fn cascade_direction(
        &self,
        volume_threshold: Decimal,
        imbalance_threshold: Decimal,
    ) -> Option<CascadeDirection> {
        if self.is_long_cascade(volume_threshold, imbalance_threshold) {
            Some(CascadeDirection::LongCascade)
        } else if self.is_short_cascade(volume_threshold, imbalance_threshold) {
            Some(CascadeDirection::ShortCascade)
        } else {
            None
        }
    }
}

/// Direction of a liquidation cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeDirection {
    /// Longs being liquidated, price likely to continue falling
    LongCascade,
    /// Shorts being liquidated, price likely to continue rising
    ShortCascade,
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
    fn test_liquidation_record_creation() {
        let record = LiquidationRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            side: "long".to_string(),
            quantity: dec!(1.5),
            price: dec!(50000),
            usd_value: dec!(75000),
        };

        assert_eq!(record.symbol, "BTCUSDT");
        assert!(record.is_long());
        assert!(!record.is_short());
    }

    #[test]
    fn test_liquidation_record_new() {
        let record = LiquidationRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            LiquidationSide::Long,
            dec!(1.5),
            dec!(50000),
        );

        assert_eq!(record.usd_value, dec!(75000));
        assert!(record.is_long());
    }

    #[test]
    fn test_is_significant() {
        let record = LiquidationRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            LiquidationSide::Long,
            dec!(0.1),
            dec!(50000),
        );

        // usd_value = 5000
        assert!(record.is_significant(dec!(3000)));
        assert!(!record.is_significant(dec!(10000)));
    }

    #[test]
    fn test_aggregate_from_liquidations() {
        let liquidations = vec![
            LiquidationRecord::new(
                sample_timestamp(),
                "BTCUSDT".to_string(),
                "binance".to_string(),
                LiquidationSide::Long,
                dec!(1.0),
                dec!(50000),
            ),
            LiquidationRecord::new(
                sample_timestamp(),
                "BTCUSDT".to_string(),
                "binance".to_string(),
                LiquidationSide::Long,
                dec!(0.5),
                dec!(50000),
            ),
            LiquidationRecord::new(
                sample_timestamp(),
                "BTCUSDT".to_string(),
                "binance".to_string(),
                LiquidationSide::Short,
                dec!(0.2),
                dec!(50000),
            ),
        ];

        let agg = LiquidationAggregateRecord::from_liquidations(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            5,
            &liquidations,
        );

        assert_eq!(agg.long_volume, dec!(75000)); // (1.0 + 0.5) * 50000
        assert_eq!(agg.short_volume, dec!(10000)); // 0.2 * 50000
        assert_eq!(agg.net_delta, dec!(65000));
        assert_eq!(agg.count_long, 2);
        assert_eq!(agg.count_short, 1);
    }

    #[test]
    fn test_aggregate_total_volume() {
        let agg = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(50000),
            short_volume: dec!(30000),
            net_delta: dec!(20000),
            count_long: 5,
            count_short: 3,
        };

        assert_eq!(agg.total_volume(), dec!(80000));
    }

    #[test]
    fn test_aggregate_imbalance_ratio() {
        let agg = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(75000),
            short_volume: dec!(25000),
            net_delta: dec!(50000),
            count_long: 5,
            count_short: 3,
        };

        // imbalance = 50000 / 100000 = 0.5
        assert_eq!(agg.imbalance_ratio(), Some(dec!(0.5)));
    }

    #[test]
    fn test_aggregate_imbalance_ratio_empty() {
        let agg = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: Decimal::ZERO,
            short_volume: Decimal::ZERO,
            net_delta: Decimal::ZERO,
            count_long: 0,
            count_short: 0,
        };

        assert_eq!(agg.imbalance_ratio(), None);
    }

    #[test]
    fn test_is_long_cascade() {
        let agg = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(100000),
            short_volume: dec!(10000),
            net_delta: dec!(90000),
            count_long: 10,
            count_short: 1,
        };

        // imbalance = 90000 / 110000 = 0.818...
        assert!(agg.is_long_cascade(dec!(50000), dec!(0.5)));
        assert!(!agg.is_long_cascade(dec!(200000), dec!(0.5))); // volume too low
        assert!(!agg.is_long_cascade(dec!(50000), dec!(0.9))); // imbalance too low
    }

    #[test]
    fn test_is_short_cascade() {
        let agg = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(10000),
            short_volume: dec!(100000),
            net_delta: dec!(-90000),
            count_long: 1,
            count_short: 10,
        };

        // imbalance = -90000 / 110000 = -0.818...
        assert!(agg.is_short_cascade(dec!(50000), dec!(0.5)));
    }

    #[test]
    fn test_cascade_direction() {
        let long_cascade = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(100000),
            short_volume: dec!(10000),
            net_delta: dec!(90000),
            count_long: 10,
            count_short: 1,
        };

        assert_eq!(
            long_cascade.cascade_direction(dec!(50000), dec!(0.5)),
            Some(CascadeDirection::LongCascade)
        );

        let short_cascade = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(10000),
            short_volume: dec!(100000),
            net_delta: dec!(-90000),
            count_long: 1,
            count_short: 10,
        };

        assert_eq!(
            short_cascade.cascade_direction(dec!(50000), dec!(0.5)),
            Some(CascadeDirection::ShortCascade)
        );

        let neutral = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(50000),
            short_volume: dec!(50000),
            net_delta: dec!(0),
            count_long: 5,
            count_short: 5,
        };

        assert_eq!(neutral.cascade_direction(dec!(50000), dec!(0.5)), None);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = LiquidationRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            side: "long".to_string(),
            quantity: dec!(1.5),
            price: dec!(50000),
            usd_value: dec!(75000),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: LiquidationRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.symbol, deserialized.symbol);
        assert_eq!(record.side, deserialized.side);
        assert_eq!(record.usd_value, deserialized.usd_value);
    }

    #[test]
    fn test_aggregate_serialization_roundtrip() {
        let record = LiquidationAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(100000),
            short_volume: dec!(50000),
            net_delta: dec!(50000),
            count_long: 10,
            count_short: 5,
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: LiquidationAggregateRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.symbol, deserialized.symbol);
        assert_eq!(record.net_delta, deserialized.net_delta);
    }
}
