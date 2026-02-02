//! Data models for the statistical trading engine.
//!
//! All models use `rust_decimal::Decimal` for financial precision.
//! Models derive `sqlx::FromRow` for database compatibility.

pub mod cvd_aggregate;
pub mod funding;
pub mod liquidation;
pub mod news;
pub mod orderbook;
pub mod paper_trade;
pub mod polymarket;
pub mod signal_snapshot;
pub mod trade;
pub mod trade_tick;

pub use cvd_aggregate::{
    calculate_cumulative_cvd, calculate_rolling_cvd, extract_close_prices, CvdAggregateRecord,
};
pub use funding::{FundingBias, FundingRateRecord};
pub use liquidation::{
    CascadeDirection, LiquidationAggregateRecord, LiquidationRecord, LiquidationSide,
};
pub use news::{NewsEventRecord, NewsSentiment, NewsSignalDirection};
pub use orderbook::OrderBookSnapshotRecord;
pub use paper_trade::{
    KellyCriterion, PaperTradeDirection, PaperTradeRecord, PaperTradeStatus, TradeDecision,
};
pub use polymarket::PolymarketOddsRecord;
pub use signal_snapshot::{SignalDirection, SignalSnapshotRecord};
pub use trade::{BinaryTradeRecord, TradeDirection, TradeOutcome};
pub use trade_tick::{TradeSide, TradeTickRecord};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rust_decimal_macros::dec;

    #[test]
    fn test_all_models_are_exported() {
        // This test verifies that all models are properly exported
        // and can be constructed. It will fail if any model is missing.
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 29, 12, 0, 0).unwrap();

        let _orderbook = OrderBookSnapshotRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            bid_levels: serde_json::json!([]),
            ask_levels: serde_json::json!([]),
            bid_volume: dec!(100.0),
            ask_volume: dec!(100.0),
            imbalance: dec!(0.0),
            mid_price: Some(dec!(50000.0)),
            spread_bps: Some(dec!(1.0)),
        };

        let _funding = FundingRateRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            funding_rate: dec!(0.0001),
            annual_rate: Some(dec!(3.65)),
            rate_percentile: Some(dec!(0.75)),
            rate_zscore: Some(dec!(1.5)),
        };

        let _liquidation = LiquidationRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            side: "long".to_string(),
            quantity: dec!(1.5),
            price: dec!(50000.0),
            usd_value: dec!(75000.0),
        };

        let _liq_agg = LiquidationAggregateRecord {
            timestamp,
            symbol: "BTCUSDT".to_string(),
            exchange: "binance".to_string(),
            window_minutes: 5,
            long_volume: dec!(100000.0),
            short_volume: dec!(50000.0),
            net_delta: dec!(50000.0),
            count_long: 10,
            count_short: 5,
        };

        let _polymarket = PolymarketOddsRecord {
            timestamp,
            market_id: "test-market-123".to_string(),
            question: "Will BTC exceed 100k?".to_string(),
            outcome_yes_price: dec!(0.65),
            outcome_no_price: dec!(0.35),
            volume_24h: Some(dec!(50000.0)),
            liquidity: Some(dec!(100000.0)),
            end_date: Some(timestamp),
        };

        let _news = NewsEventRecord {
            timestamp,
            source: "cryptopanic".to_string(),
            title: "Bitcoin hits new ATH".to_string(),
            url: Some("https://example.com".to_string()),
            categories: Some(vec!["bitcoin".to_string()]),
            currencies: Some(vec!["BTC".to_string()]),
            sentiment: Some("positive".to_string()),
            urgency_score: Some(dec!(0.85)),
            raw_data: Some(serde_json::json!({})),
        };

        let _trade = BinaryTradeRecord {
            id: 1,
            timestamp,
            market_id: "test-market-123".to_string(),
            direction: "yes".to_string(),
            shares: dec!(100.0),
            price: dec!(0.65),
            stake: dec!(65.0),
            signals_snapshot: Some(serde_json::json!({"imbalance": 0.15})),
            outcome: None,
            pnl: None,
            settled_at: None,
        };
    }
}
