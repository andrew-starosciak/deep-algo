//! Trade tick data models.
//!
//! Captures individual trade executions from exchanges for CVD (Cumulative Volume Delta)
//! calculation. Trade side is determined by whether the buyer was the maker or taker.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Side of a trade determined by the aggressor.
///
/// In a trade, the aggressor is the party that crossed the spread to execute.
/// - `Buy`: Buyer was the aggressor (taker), lifted the ask
/// - `Sell`: Seller was the aggressor (taker), hit the bid
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TradeSide {
    /// Buyer was the aggressor (taker)
    Buy,
    /// Seller was the aggressor (taker)
    Sell,
}

impl TradeSide {
    /// Converts from Binance's `m` (buyer is maker) flag.
    ///
    /// Binance reports whether the buyer was the maker:
    /// - `m=true`: Buyer was maker, so SELLER was taker/aggressor -> Sell
    /// - `m=false`: Buyer was taker/aggressor -> Buy
    ///
    /// # Examples
    ///
    /// ```
    /// use algo_trade_data::models::trade_tick::TradeSide;
    ///
    /// // Buyer was maker, seller was aggressor
    /// assert_eq!(TradeSide::from_binance_maker_flag(true), TradeSide::Sell);
    ///
    /// // Buyer was taker/aggressor
    /// assert_eq!(TradeSide::from_binance_maker_flag(false), TradeSide::Buy);
    /// ```
    #[must_use]
    pub const fn from_binance_maker_flag(buyer_is_maker: bool) -> Self {
        if buyer_is_maker {
            // Buyer was maker, so seller was taker/aggressor
            Self::Sell
        } else {
            // Buyer was taker/aggressor
            Self::Buy
        }
    }

    /// Returns the string representation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// Returns the opposite side.
    #[must_use]
    pub const fn opposite(&self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

/// An individual trade tick record.
///
/// Represents a single trade execution from an exchange, used for
/// calculating CVD (Cumulative Volume Delta) signals.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TradeTickRecord {
    /// Timestamp of the trade
    pub timestamp: DateTime<Utc>,
    /// Trading pair symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange name (e.g., "binance")
    pub exchange: String,
    /// Trade ID from the exchange
    pub trade_id: i64,
    /// Trade price
    pub price: Decimal,
    /// Trade quantity in base currency
    pub quantity: Decimal,
    /// Trade side determined by aggressor
    pub side: String,
    /// USD value of the trade (quantity * price)
    pub usd_value: Decimal,
}

impl TradeTickRecord {
    /// Creates a new trade tick record.
    pub fn new(
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        trade_id: i64,
        price: Decimal,
        quantity: Decimal,
        side: TradeSide,
    ) -> Self {
        let usd_value = quantity * price;
        Self {
            timestamp,
            symbol,
            exchange,
            trade_id,
            price,
            quantity,
            side: side.as_str().to_string(),
            usd_value,
        }
    }

    /// Creates a trade record from Binance aggTrade data.
    ///
    /// # Arguments
    /// * `timestamp` - Trade timestamp
    /// * `symbol` - Trading pair symbol
    /// * `trade_id` - Aggregate trade ID
    /// * `price` - Trade price
    /// * `quantity` - Trade quantity
    /// * `buyer_is_maker` - Binance's `m` flag
    pub fn from_binance_agg_trade(
        timestamp: DateTime<Utc>,
        symbol: String,
        trade_id: i64,
        price: Decimal,
        quantity: Decimal,
        buyer_is_maker: bool,
    ) -> Self {
        let side = TradeSide::from_binance_maker_flag(buyer_is_maker);
        Self::new(
            timestamp,
            symbol,
            "binance".to_string(),
            trade_id,
            price,
            quantity,
            side,
        )
    }

    /// Returns true if this is a buy trade.
    #[must_use]
    pub fn is_buy(&self) -> bool {
        self.side == "buy"
    }

    /// Returns true if this is a sell trade.
    #[must_use]
    pub fn is_sell(&self) -> bool {
        self.side == "sell"
    }

    /// Returns the parsed trade side.
    #[must_use]
    pub fn parsed_side(&self) -> Option<TradeSide> {
        match self.side.as_str() {
            "buy" => Some(TradeSide::Buy),
            "sell" => Some(TradeSide::Sell),
            _ => None,
        }
    }

    /// Returns the signed volume for CVD calculation.
    ///
    /// Buy trades contribute positive volume, sell trades negative.
    #[must_use]
    pub fn signed_volume(&self) -> Decimal {
        if self.is_buy() {
            self.quantity
        } else {
            -self.quantity
        }
    }

    /// Returns the signed USD value for CVD calculation.
    ///
    /// Buy trades contribute positive value, sell trades negative.
    #[must_use]
    pub fn signed_usd_value(&self) -> Decimal {
        if self.is_buy() {
            self.usd_value
        } else {
            -self.usd_value
        }
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

    // ============================================
    // TradeSide Classification Tests (TDD RED -> GREEN)
    // ============================================

    #[test]
    fn test_buyer_maker_means_sell_aggressor() {
        // m=true means buyer was maker, so seller was taker/aggressor
        assert_eq!(TradeSide::from_binance_maker_flag(true), TradeSide::Sell);
    }

    #[test]
    fn test_buyer_taker_means_buy_aggressor() {
        // m=false means buyer was taker/aggressor
        assert_eq!(TradeSide::from_binance_maker_flag(false), TradeSide::Buy);
    }

    #[test]
    fn test_trade_side_as_str() {
        assert_eq!(TradeSide::Buy.as_str(), "buy");
        assert_eq!(TradeSide::Sell.as_str(), "sell");
    }

    #[test]
    fn test_trade_side_opposite() {
        assert_eq!(TradeSide::Buy.opposite(), TradeSide::Sell);
        assert_eq!(TradeSide::Sell.opposite(), TradeSide::Buy);
    }

    #[test]
    fn test_trade_side_serialization() {
        let buy = TradeSide::Buy;
        let json = serde_json::to_string(&buy).unwrap();
        assert_eq!(json, "\"Buy\"");

        let sell = TradeSide::Sell;
        let json = serde_json::to_string(&sell).unwrap();
        assert_eq!(json, "\"Sell\"");
    }

    #[test]
    fn test_trade_side_deserialization() {
        let buy: TradeSide = serde_json::from_str("\"Buy\"").unwrap();
        assert_eq!(buy, TradeSide::Buy);

        let sell: TradeSide = serde_json::from_str("\"Sell\"").unwrap();
        assert_eq!(sell, TradeSide::Sell);
    }

    // ============================================
    // TradeTickRecord Creation Tests
    // ============================================

    #[test]
    fn test_trade_tick_new_buy() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            12345,
            dec!(50000),
            dec!(0.5),
            TradeSide::Buy,
        );

        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange, "binance");
        assert_eq!(record.trade_id, 12345);
        assert_eq!(record.price, dec!(50000));
        assert_eq!(record.quantity, dec!(0.5));
        assert_eq!(record.side, "buy");
        assert_eq!(record.usd_value, dec!(25000)); // 0.5 * 50000
        assert!(record.is_buy());
        assert!(!record.is_sell());
    }

    #[test]
    fn test_trade_tick_new_sell() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            12346,
            dec!(50000),
            dec!(1.0),
            TradeSide::Sell,
        );

        assert_eq!(record.side, "sell");
        assert_eq!(record.usd_value, dec!(50000)); // 1.0 * 50000
        assert!(!record.is_buy());
        assert!(record.is_sell());
    }

    #[test]
    fn test_trade_tick_from_binance_buy() {
        // buyer_is_maker=false means buyer was aggressor
        let record = TradeTickRecord::from_binance_agg_trade(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            12345,
            dec!(50000),
            dec!(0.5),
            false, // buyer was NOT maker, so buyer was aggressor
        );

        assert!(record.is_buy());
        assert_eq!(record.exchange, "binance");
    }

    #[test]
    fn test_trade_tick_from_binance_sell() {
        // buyer_is_maker=true means seller was aggressor
        let record = TradeTickRecord::from_binance_agg_trade(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            12345,
            dec!(50000),
            dec!(0.5),
            true, // buyer WAS maker, so seller was aggressor
        );

        assert!(record.is_sell());
    }

    #[test]
    fn test_trade_tick_parsed_side() {
        let buy_record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.0),
            TradeSide::Buy,
        );
        assert_eq!(buy_record.parsed_side(), Some(TradeSide::Buy));

        let sell_record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            2,
            dec!(50000),
            dec!(1.0),
            TradeSide::Sell,
        );
        assert_eq!(sell_record.parsed_side(), Some(TradeSide::Sell));
    }

    // ============================================
    // Signed Volume Tests (for CVD calculation)
    // ============================================

    #[test]
    fn test_signed_volume_buy_is_positive() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.5),
            TradeSide::Buy,
        );

        assert_eq!(record.signed_volume(), dec!(1.5));
    }

    #[test]
    fn test_signed_volume_sell_is_negative() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.5),
            TradeSide::Sell,
        );

        assert_eq!(record.signed_volume(), dec!(-1.5));
    }

    #[test]
    fn test_signed_usd_value_buy_is_positive() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.0),
            TradeSide::Buy,
        );

        assert_eq!(record.signed_usd_value(), dec!(50000));
    }

    #[test]
    fn test_signed_usd_value_sell_is_negative() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(1.0),
            TradeSide::Sell,
        );

        assert_eq!(record.signed_usd_value(), dec!(-50000));
    }

    // ============================================
    // Edge Cases
    // ============================================

    #[test]
    fn test_trade_tick_zero_quantity() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            Decimal::ZERO,
            TradeSide::Buy,
        );

        assert_eq!(record.usd_value, Decimal::ZERO);
        assert_eq!(record.signed_volume(), Decimal::ZERO);
        assert_eq!(record.signed_usd_value(), Decimal::ZERO);
    }

    #[test]
    fn test_trade_tick_small_quantity() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            1,
            dec!(50000),
            dec!(0.00001),
            TradeSide::Buy,
        );

        assert_eq!(record.usd_value, dec!(0.5)); // 0.00001 * 50000
    }

    // ============================================
    // Serialization Tests
    // ============================================

    #[test]
    fn test_trade_tick_serialization_roundtrip() {
        let record = TradeTickRecord::new(
            sample_timestamp(),
            "BTCUSDT".to_string(),
            "binance".to_string(),
            12345,
            dec!(50000.50),
            dec!(1.5),
            TradeSide::Buy,
        );

        let json = serde_json::to_string(&record).expect("serialization failed");
        let deserialized: TradeTickRecord =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(record.symbol, deserialized.symbol);
        assert_eq!(record.trade_id, deserialized.trade_id);
        assert_eq!(record.price, deserialized.price);
        assert_eq!(record.quantity, deserialized.quantity);
        assert_eq!(record.side, deserialized.side);
        assert_eq!(record.usd_value, deserialized.usd_value);
    }
}
