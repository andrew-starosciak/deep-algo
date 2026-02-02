//! CVD (Cumulative Volume Delta) aggregate data models.
//!
//! CVD measures the difference between buying and selling volume over time.
//! Positive CVD indicates net buying pressure, negative indicates net selling.
//! Divergences between CVD and price can signal potential reversals.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::trade_tick::TradeTickRecord;

/// Aggregated CVD data over a time window.
///
/// Captures buy/sell volume breakdown and cumulative volume delta
/// for divergence detection signals.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CvdAggregateRecord {
    /// End timestamp of the aggregation window
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// Window size in seconds
    pub window_seconds: i32,
    /// Total buy volume in base currency
    pub buy_volume: Decimal,
    /// Total sell volume in base currency
    pub sell_volume: Decimal,
    /// CVD = buy_volume - sell_volume
    pub cvd: Decimal,
    /// Total number of trades in the window
    pub trade_count: i32,
    /// Average price during the window
    pub avg_price: Option<Decimal>,
    /// Closing price of the window (last trade price)
    pub close_price: Option<Decimal>,
}

impl CvdAggregateRecord {
    /// Creates a new CVD aggregate from a list of trade ticks.
    ///
    /// # Arguments
    /// * `timestamp` - End timestamp of the window
    /// * `symbol` - Trading pair symbol
    /// * `exchange` - Exchange name
    /// * `window_seconds` - Duration of the aggregation window
    /// * `trades` - Trade ticks to aggregate
    pub fn from_trades(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        window_seconds: i32,
        trades: &[TradeTickRecord],
    ) -> Self {
        let mut buy_volume = Decimal::ZERO;
        let mut sell_volume = Decimal::ZERO;
        let mut total_value = Decimal::ZERO;
        let mut total_quantity = Decimal::ZERO;
        let mut close_price: Option<Decimal> = None;

        for trade in trades {
            if trade.is_buy() {
                buy_volume += trade.quantity;
            } else {
                sell_volume += trade.quantity;
            }
            total_value += trade.usd_value;
            total_quantity += trade.quantity;
            close_price = Some(trade.price);
        }

        let cvd = buy_volume - sell_volume;
        // Saturating conversion to prevent integer overflow
        let trade_count = i32::try_from(trades.len()).unwrap_or(i32::MAX);

        let avg_price = if total_quantity > Decimal::ZERO {
            Some(total_value / total_quantity)
        } else {
            None
        };

        Self {
            timestamp,
            symbol,
            exchange,
            window_seconds,
            buy_volume,
            sell_volume,
            cvd,
            trade_count,
            avg_price,
            close_price,
        }
    }

    /// Creates an empty aggregate (no trades).
    pub fn empty(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        window_seconds: i32,
    ) -> Self {
        Self {
            timestamp,
            symbol,
            exchange,
            window_seconds,
            buy_volume: Decimal::ZERO,
            sell_volume: Decimal::ZERO,
            cvd: Decimal::ZERO,
            trade_count: 0,
            avg_price: None,
            close_price: None,
        }
    }

    /// Returns the total volume (buy + sell).
    #[must_use]
    pub fn total_volume(&self) -> Decimal {
        self.buy_volume + self.sell_volume
    }

    /// Returns the buy/sell ratio.
    ///
    /// Returns None if sell_volume is zero.
    #[must_use]
    pub fn buy_sell_ratio(&self) -> Option<Decimal> {
        if self.sell_volume > Decimal::ZERO {
            Some(self.buy_volume / self.sell_volume)
        } else if self.buy_volume > Decimal::ZERO {
            None // Infinite ratio
        } else {
            Some(Decimal::ONE) // Both zero, balanced
        }
    }

    /// Returns the volume imbalance ratio: (buy - sell) / (buy + sell).
    ///
    /// Returns None if total volume is zero.
    #[must_use]
    pub fn imbalance_ratio(&self) -> Option<Decimal> {
        let total = self.total_volume();
        if total > Decimal::ZERO {
            Some(self.cvd / total)
        } else {
            None
        }
    }

    /// Returns true if this is a buy-dominant window.
    #[must_use]
    pub fn is_buy_dominant(&self) -> bool {
        self.cvd > Decimal::ZERO
    }

    /// Returns true if this is a sell-dominant window.
    #[must_use]
    pub fn is_sell_dominant(&self) -> bool {
        self.cvd < Decimal::ZERO
    }
}

/// Calculates cumulative CVD from a series of aggregates.
///
/// # Arguments
/// * `aggregates` - CVD aggregates in chronological order
///
/// # Returns
/// Vector of cumulative CVD values
pub fn calculate_cumulative_cvd(aggregates: &[CvdAggregateRecord]) -> Vec<Decimal> {
    let mut cumulative = Vec::with_capacity(aggregates.len());
    let mut running_cvd = Decimal::ZERO;

    for agg in aggregates {
        running_cvd += agg.cvd;
        cumulative.push(running_cvd);
    }

    cumulative
}

/// Calculates rolling CVD over a lookback window.
///
/// # Arguments
/// * `aggregates` - CVD aggregates in chronological order
/// * `lookback` - Number of periods to include in rolling sum
///
/// # Returns
/// Vector of rolling CVD values (first `lookback-1` entries may use fewer periods)
pub fn calculate_rolling_cvd(aggregates: &[CvdAggregateRecord], lookback: usize) -> Vec<Decimal> {
    if lookback == 0 {
        return vec![Decimal::ZERO; aggregates.len()];
    }

    let mut rolling = Vec::with_capacity(aggregates.len());

    for i in 0..aggregates.len() {
        let start = i.saturating_sub(lookback.saturating_sub(1));
        let sum: Decimal = aggregates[start..=i].iter().map(|a| a.cvd).sum();
        rolling.push(sum);
    }

    rolling
}

/// Extracts close prices from aggregates for divergence analysis.
///
/// # Arguments
/// * `aggregates` - CVD aggregates in chronological order
///
/// # Returns
/// Vector of close prices (uses Decimal::ZERO if price is None)
pub fn extract_close_prices(aggregates: &[CvdAggregateRecord]) -> Vec<Decimal> {
    aggregates
        .iter()
        .map(|a| a.close_price.unwrap_or(Decimal::ZERO))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::trade_tick::TradeSide;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap()
    }

    fn make_trade(quantity: Decimal, price: Decimal, side: TradeSide) -> TradeTickRecord {
        TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            price,
            quantity,
            side,
        )
    }

    // ============================================
    // CVD Calculation Tests (TDD RED -> GREEN)
    // ============================================

    #[test]
    fn test_cvd_all_buys_positive() {
        // All buy trades should produce positive CVD
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(0.5), dec!(50100), TradeSide::Buy),
            make_trade(dec!(0.25), dec!(50200), TradeSide::Buy),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_volume, dec!(1.75)); // 1.0 + 0.5 + 0.25
        assert_eq!(agg.sell_volume, Decimal::ZERO);
        assert_eq!(agg.cvd, dec!(1.75)); // All buys
        assert!(agg.cvd > Decimal::ZERO);
        assert!(agg.is_buy_dominant());
        assert!(!agg.is_sell_dominant());
    }

    #[test]
    fn test_cvd_all_sells_negative() {
        // All sell trades should produce negative CVD
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
            make_trade(dec!(0.5), dec!(49900), TradeSide::Sell),
            make_trade(dec!(0.25), dec!(49800), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_volume, Decimal::ZERO);
        assert_eq!(agg.sell_volume, dec!(1.75));
        assert_eq!(agg.cvd, dec!(-1.75)); // All sells
        assert!(agg.cvd < Decimal::ZERO);
        assert!(!agg.is_buy_dominant());
        assert!(agg.is_sell_dominant());
    }

    #[test]
    fn test_cvd_balanced_near_zero() {
        // Equal buy/sell should produce CVD near zero
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_volume, dec!(1.0));
        assert_eq!(agg.sell_volume, dec!(1.0));
        assert_eq!(agg.cvd, Decimal::ZERO);
        assert!(!agg.is_buy_dominant());
        assert!(!agg.is_sell_dominant());
    }

    #[test]
    fn test_cvd_slight_buy_pressure() {
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(0.8), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.cvd, dec!(0.2)); // 1.0 - 0.8
        assert!(agg.is_buy_dominant());
    }

    #[test]
    fn test_cvd_slight_sell_pressure() {
        let trades = vec![
            make_trade(dec!(0.8), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.cvd, dec!(-0.2)); // 0.8 - 1.0
        assert!(agg.is_sell_dominant());
    }

    // ============================================
    // Empty and Edge Cases
    // ============================================

    #[test]
    fn test_cvd_empty_trades() {
        let trades: Vec<TradeTickRecord> = vec![];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_volume, Decimal::ZERO);
        assert_eq!(agg.sell_volume, Decimal::ZERO);
        assert_eq!(agg.cvd, Decimal::ZERO);
        assert_eq!(agg.trade_count, 0);
        assert!(agg.avg_price.is_none());
        assert!(agg.close_price.is_none());
    }

    #[test]
    fn test_cvd_empty_constructor() {
        let agg = CvdAggregateRecord::empty(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
        );

        assert_eq!(agg.cvd, Decimal::ZERO);
        assert_eq!(agg.trade_count, 0);
    }

    #[test]
    fn test_total_volume() {
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(0.5), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.total_volume(), dec!(1.5));
    }

    // ============================================
    // Ratio Tests
    // ============================================

    #[test]
    fn test_buy_sell_ratio() {
        let trades = vec![
            make_trade(dec!(2.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.buy_sell_ratio(), Some(dec!(2.0)));
    }

    #[test]
    fn test_buy_sell_ratio_zero_sell() {
        let trades = vec![make_trade(dec!(1.0), dec!(50000), TradeSide::Buy)];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        // Infinite ratio when no sells
        assert!(agg.buy_sell_ratio().is_none());
    }

    #[test]
    fn test_buy_sell_ratio_both_zero() {
        let agg = CvdAggregateRecord::empty(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
        );

        // Both zero returns 1 (balanced)
        assert_eq!(agg.buy_sell_ratio(), Some(Decimal::ONE));
    }

    #[test]
    fn test_imbalance_ratio() {
        let trades = vec![
            make_trade(dec!(3.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        // (3.0 - 1.0) / (3.0 + 1.0) = 2.0 / 4.0 = 0.5
        assert_eq!(agg.imbalance_ratio(), Some(dec!(0.5)));
    }

    #[test]
    fn test_imbalance_ratio_zero_volume() {
        let agg = CvdAggregateRecord::empty(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
        );

        assert!(agg.imbalance_ratio().is_none());
    }

    // ============================================
    // Price Tracking Tests
    // ============================================

    #[test]
    fn test_close_price_is_last_trade() {
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50100), TradeSide::Sell),
            make_trade(dec!(1.0), dec!(50200), TradeSide::Buy),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.close_price, Some(dec!(50200)));
    }

    #[test]
    fn test_avg_price_calculation() {
        // Trade 1: 1.0 @ 50000 = $50000
        // Trade 2: 1.0 @ 50200 = $50200
        // Total value: $100200, Total qty: 2.0
        // VWAP: 100200 / 2.0 = 50100
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50200), TradeSide::Sell),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.avg_price, Some(dec!(50100)));
    }

    #[test]
    fn test_trade_count() {
        let trades = vec![
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Sell),
            make_trade(dec!(1.0), dec!(50000), TradeSide::Buy),
        ];

        let agg = CvdAggregateRecord::from_trades(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
            &trades,
        );

        assert_eq!(agg.trade_count, 3);
    }

    // ============================================
    // Cumulative CVD Tests
    // ============================================

    #[test]
    fn test_cumulative_cvd() {
        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(2.0),
                sell_volume: dec!(1.0),
                cvd: dec!(1.0), // +1.0
                trade_count: 10,
                avg_price: Some(dec!(50000)),
                close_price: Some(dec!(50000)),
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(1.0),
                sell_volume: dec!(2.0),
                cvd: dec!(-1.0), // -1.0
                trade_count: 10,
                avg_price: Some(dec!(50100)),
                close_price: Some(dec!(50100)),
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(3.0),
                sell_volume: dec!(1.0),
                cvd: dec!(2.0), // +2.0
                trade_count: 10,
                avg_price: Some(dec!(50200)),
                close_price: Some(dec!(50200)),
            },
        ];

        let cumulative = calculate_cumulative_cvd(&aggregates);

        assert_eq!(cumulative.len(), 3);
        assert_eq!(cumulative[0], dec!(1.0)); // 1.0
        assert_eq!(cumulative[1], dec!(0.0)); // 1.0 + (-1.0) = 0.0
        assert_eq!(cumulative[2], dec!(2.0)); // 0.0 + 2.0 = 2.0
    }

    #[test]
    fn test_cumulative_cvd_empty() {
        let aggregates: Vec<CvdAggregateRecord> = vec![];
        let cumulative = calculate_cumulative_cvd(&aggregates);
        assert!(cumulative.is_empty());
    }

    // ============================================
    // Rolling CVD Tests
    // ============================================

    #[test]
    fn test_rolling_cvd() {
        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(1.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(1.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(2.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(2.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: dec!(3.0),
                sell_volume: Decimal::ZERO,
                cvd: dec!(3.0),
                trade_count: 1,
                avg_price: None,
                close_price: None,
            },
        ];

        // 2-period rolling
        let rolling = calculate_rolling_cvd(&aggregates, 2);

        assert_eq!(rolling.len(), 3);
        assert_eq!(rolling[0], dec!(1.0)); // Just first (no lookback)
        assert_eq!(rolling[1], dec!(3.0)); // 1.0 + 2.0
        assert_eq!(rolling[2], dec!(5.0)); // 2.0 + 3.0
    }

    #[test]
    fn test_rolling_cvd_zero_lookback() {
        let aggregates = vec![CvdAggregateRecord::empty(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            60,
        )];

        let rolling = calculate_rolling_cvd(&aggregates, 0);
        assert_eq!(rolling, vec![Decimal::ZERO]);
    }

    // ============================================
    // Extract Close Prices Tests
    // ============================================

    #[test]
    fn test_extract_close_prices() {
        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                cvd: Decimal::ZERO,
                trade_count: 0,
                avg_price: None,
                close_price: Some(dec!(100)),
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                cvd: Decimal::ZERO,
                trade_count: 0,
                avg_price: None,
                close_price: Some(dec!(105)),
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                cvd: Decimal::ZERO,
                trade_count: 0,
                avg_price: None,
                close_price: Some(dec!(110)),
            },
        ];

        let prices = extract_close_prices(&aggregates);
        assert_eq!(prices, vec![dec!(100), dec!(105), dec!(110)]);
    }

    #[test]
    fn test_extract_close_prices_with_none() {
        let aggregates = vec![
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                cvd: Decimal::ZERO,
                trade_count: 0,
                avg_price: None,
                close_price: Some(dec!(100)),
            },
            CvdAggregateRecord {
                timestamp: sample_timestamp(),
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_seconds: 60,
                buy_volume: Decimal::ZERO,
                sell_volume: Decimal::ZERO,
                cvd: Decimal::ZERO,
                trade_count: 0,
                avg_price: None,
                close_price: None, // Missing price
            },
        ];

        let prices = extract_close_prices(&aggregates);
        assert_eq!(prices, vec![dec!(100), Decimal::ZERO]);
    }

    // ============================================
    // Serialization Tests
    // ============================================

    #[test]
    fn test_cvd_aggregate_serialization_roundtrip() {
        let agg = CvdAggregateRecord {
            timestamp: sample_timestamp(),
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_seconds: 60,
            buy_volume: dec!(100.5),
            sell_volume: dec!(80.25),
            cvd: dec!(20.25),
            trade_count: 150,
            avg_price: Some(dec!(50100.50)),
            close_price: Some(dec!(50150.00)),
        };

        let json = serde_json::to_string(&agg).expect("serialization failed");
        let deserialized: CvdAggregateRecord =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(agg.symbol, deserialized.symbol);
        assert_eq!(agg.buy_volume, deserialized.buy_volume);
        assert_eq!(agg.sell_volume, deserialized.sell_volume);
        assert_eq!(agg.cvd, deserialized.cvd);
        assert_eq!(agg.trade_count, deserialized.trade_count);
        assert_eq!(agg.avg_price, deserialized.avg_price);
        assert_eq!(agg.close_price, deserialized.close_price);
    }
}
