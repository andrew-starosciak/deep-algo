pub mod collector;
pub mod common;
pub mod funding;
pub mod generator;
pub mod liquidations;
pub mod trades;

// Re-export signal generators for convenience
pub use generator::{
    CompositeSignal, FundingRateSignal, LiquidationCascadeSignal, OrderBookImbalanceSignal,
};

// Re-export collectors for convenience
pub use collector::{
    calculate_urgency, categorize_news, determine_sentiment, CollectorConfig, CollectorEvent,
    CollectorStats, FundingCollector, LiquidationCollector, LiquidationCollectorConfig,
    NewsCategory, NewsCollector, NewsCollectorConfig, OrderBookCollector, RollingWindows,
};
