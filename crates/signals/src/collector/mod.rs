//! Data collectors for the statistical trading engine.
//!
//! This module provides data collection infrastructure for real-time market data:
//! - Order book snapshots (1/sec aggregation from 100ms updates)
//! - Funding rates with rolling statistical context
//! - Liquidation events with rolling window aggregates
//! - News events with urgency scoring
//! - OHLCV historical data backfill from Binance Futures
//!
//! All collectors follow the actor pattern with channel-based output,
//! enabling backtest-live parity through the same processing pipeline.

mod funding_collector;
mod liquidation_collector;
mod news_collector;
mod ohlcv_collector;
mod orderbook_collector;
mod types;

pub use funding_collector::{FundingCollector, MarkPriceUpdate, RollingHistory};
pub use liquidation_collector::{
    ForceOrder, ForceOrderEvent, LiquidationCollector, LiquidationCollectorConfig, RollingWindows,
};
pub use news_collector::{
    calculate_urgency, categorize_news, determine_sentiment, NewsCategory, NewsCollector,
    NewsCollectorConfig,
};
pub use ohlcv_collector::{
    calculate_expected_candles, calculate_required_requests, BackfillStats, Interval,
    OhlcvCollector,
};
pub use orderbook_collector::{
    calculate_imbalance, calculate_mid_price, calculate_spread_bps, calculate_total_volume,
    parse_levels_to_json, DepthUpdate, OrderBookAggregator, OrderBookCollector, StreamWrapper,
};
pub use types::{CollectorConfig, CollectorEvent, CollectorStats};
