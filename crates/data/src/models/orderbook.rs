//! Order book snapshot data model.
//!
//! Captures order book state at 1/sec frequency for signal generation.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// A snapshot of the order book at a point in time.
///
/// Used for computing order book imbalance signals.
/// Bid/ask levels stored as JSONB for flexibility with varying depth.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OrderBookSnapshotRecord {
    /// Timestamp of the snapshot
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// Bid price levels as JSON array of [price, quantity] pairs
    pub bid_levels: JsonValue,
    /// Ask price levels as JSON array of [price, quantity] pairs
    pub ask_levels: JsonValue,
    /// Total bid volume across all levels
    pub bid_volume: Decimal,
    /// Total ask volume across all levels
    pub ask_volume: Decimal,
    /// Order book imbalance: (bid_vol - ask_vol) / (bid_vol + ask_vol)
    pub imbalance: Decimal,
    /// Mid price: (best_bid + best_ask) / 2
    pub mid_price: Option<Decimal>,
    /// Spread in basis points
    pub spread_bps: Option<Decimal>,
}

impl OrderBookSnapshotRecord {
    /// Creates a new order book snapshot with calculated derived fields.
    ///
    /// # Arguments
    /// * `timestamp` - When the snapshot was taken
    /// * `symbol` - Trading pair symbol
    /// * `exchange` - Exchange name
    /// * `bid_levels` - Bid price levels as JSON
    /// * `ask_levels` - Ask price levels as JSON
    pub fn new(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        bid_levels: JsonValue,
        ask_levels: JsonValue,
    ) -> Self {
        let (bid_volume, best_bid) = Self::parse_levels(&bid_levels);
        let (ask_volume, best_ask) = Self::parse_levels(&ask_levels);

        let imbalance = if bid_volume + ask_volume > Decimal::ZERO {
            (bid_volume - ask_volume) / (bid_volume + ask_volume)
        } else {
            Decimal::ZERO
        };

        let (mid_price, spread_bps) = match (best_bid, best_ask) {
            (Some(bid), Some(ask)) if bid > Decimal::ZERO => {
                let mid = (bid + ask) / Decimal::TWO;
                let spread = ((ask - bid) / mid) * Decimal::from(10000);
                (Some(mid), Some(spread))
            }
            _ => (None, None),
        };

        Self {
            timestamp,
            symbol,
            exchange,
            bid_levels,
            ask_levels,
            bid_volume,
            ask_volume,
            imbalance,
            mid_price,
            spread_bps,
        }
    }

    /// Parses price levels and returns (total_volume, best_price).
    fn parse_levels(levels: &JsonValue) -> (Decimal, Option<Decimal>) {
        if let Some(arr) = levels.as_array() {
            let mut total_volume = Decimal::ZERO;
            let mut best_price: Option<Decimal> = None;

            for level in arr {
                if let Some(level_arr) = level.as_array() {
                    if level_arr.len() >= 2 {
                        if let (Some(price_str), Some(qty_str)) =
                            (level_arr[0].as_str(), level_arr[1].as_str())
                        {
                            if let (Ok(price), Ok(qty)) =
                                (price_str.parse::<Decimal>(), qty_str.parse::<Decimal>())
                            {
                                total_volume += qty;
                                if best_price.is_none() {
                                    best_price = Some(price);
                                }
                            }
                        }
                    }
                }
            }
            (total_volume, best_price)
        } else {
            (Decimal::ZERO, None)
        }
    }

    /// Returns true if the order book shows bullish imbalance (more bids than asks).
    #[must_use]
    pub fn is_bullish(&self, threshold: Decimal) -> bool {
        self.imbalance > threshold
    }

    /// Returns true if the order book shows bearish imbalance (more asks than bids).
    #[must_use]
    pub fn is_bearish(&self, threshold: Decimal) -> bool {
        self.imbalance < -threshold
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
    fn test_orderbook_snapshot_creation() {
        let record = OrderBookSnapshotRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: json!([["50000", "1.5"], ["49999", "2.0"]]),
            ask_levels: json!([["50001", "1.0"], ["50002", "1.5"]]),
            bid_volume: dec!(3.5),
            ask_volume: dec!(2.5),
            imbalance: dec!(0.166666666),
            mid_price: Some(dec!(50000.5)),
            spread_bps: Some(dec!(0.2)),
        };

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert!(record.imbalance > Decimal::ZERO);
    }

    #[test]
    fn test_orderbook_new_calculates_imbalance() {
        let bids = json!([["50000", "10.0"], ["49999", "5.0"]]);
        let asks = json!([["50001", "5.0"], ["50002", "2.5"]]);

        let record = OrderBookSnapshotRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            bids,
            asks,
        );

        // bid_volume = 15.0, ask_volume = 7.5
        // imbalance = (15 - 7.5) / (15 + 7.5) = 7.5 / 22.5 = 0.333...
        assert_eq!(record.bid_volume, dec!(15.0));
        assert_eq!(record.ask_volume, dec!(7.5));
        assert!(record.imbalance > dec!(0.33));
        assert!(record.imbalance < dec!(0.34));
    }

    #[test]
    fn test_orderbook_new_calculates_mid_price() {
        let bids = json!([["50000", "1.0"]]);
        let asks = json!([["50010", "1.0"]]);

        let record = OrderBookSnapshotRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            bids,
            asks,
        );

        // mid = (50000 + 50010) / 2 = 50005
        assert_eq!(record.mid_price, Some(dec!(50005)));
    }

    #[test]
    fn test_orderbook_new_calculates_spread() {
        let bids = json!([["50000", "1.0"]]);
        let asks = json!([["50010", "1.0"]]);

        let record = OrderBookSnapshotRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            bids,
            asks,
        );

        // spread_bps = ((50010 - 50000) / 50005) * 10000 ~= 2.0
        let spread = record.spread_bps.unwrap();
        assert!(spread > dec!(1.9));
        assert!(spread < dec!(2.1));
    }

    #[test]
    fn test_orderbook_handles_empty_levels() {
        let record = OrderBookSnapshotRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            json!([]),
            json!([]),
        );

        assert_eq!(record.bid_volume, Decimal::ZERO);
        assert_eq!(record.ask_volume, Decimal::ZERO);
        assert_eq!(record.imbalance, Decimal::ZERO);
        assert_eq!(record.mid_price, None);
        assert_eq!(record.spread_bps, None);
    }

    #[test]
    fn test_is_bullish() {
        let record = OrderBookSnapshotRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: json!([]),
            ask_levels: json!([]),
            bid_volume: dec!(100.0),
            ask_volume: dec!(50.0),
            imbalance: dec!(0.333),
            mid_price: None,
            spread_bps: None,
        };

        assert!(record.is_bullish(dec!(0.1)));
        assert!(record.is_bullish(dec!(0.3)));
        assert!(!record.is_bullish(dec!(0.5)));
    }

    #[test]
    fn test_is_bearish() {
        let record = OrderBookSnapshotRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: json!([]),
            ask_levels: json!([]),
            bid_volume: dec!(50.0),
            ask_volume: dec!(100.0),
            imbalance: dec!(-0.333),
            mid_price: None,
            spread_bps: None,
        };

        assert!(record.is_bearish(dec!(0.1)));
        assert!(record.is_bearish(dec!(0.3)));
        assert!(!record.is_bearish(dec!(0.5)));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let record = OrderBookSnapshotRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: json!([["50000", "1.0"]]),
            ask_levels: json!([["50001", "1.0"]]),
            bid_volume: dec!(1.0),
            ask_volume: dec!(1.0),
            imbalance: dec!(0.0),
            mid_price: Some(dec!(50000.5)),
            spread_bps: Some(dec!(0.2)),
        };

        let json_str = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: OrderBookSnapshotRecord =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(record.symbol, deserialized.symbol);
        assert_eq!(record.exchange, deserialized.exchange);
        assert_eq!(record.imbalance, deserialized.imbalance);
        assert_eq!(record.mid_price, deserialized.mid_price);
    }
}
