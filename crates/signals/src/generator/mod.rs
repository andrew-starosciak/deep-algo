//! Signal generators for statistical trading.
//!
//! This module contains implementations of various signal generators
//! that produce trading signals based on market data analysis.

mod composite;
mod funding_rate;
pub mod liquidation_cascade;
pub mod news_signal;
mod orderbook_imbalance;

pub use composite::{
    adjust_weights_for_multicollinearity, calculate_correlation_matrix, combine_bayesian,
    CombinationMethod, CompositeSignal, CorrelationMatrix,
};
pub use funding_rate::{
    detect_reversal, percentile_signal, FundingRateSignal, FundingReversalConfig,
    FundingSignalMode, ReversalSignal,
};
pub use liquidation_cascade::{
    calculate_net_delta, detect_exhaustion, is_cascade, CascadeConfig, ExhaustionConfig,
    ExhaustionSignal, LiquidationCascadeSignal, LiquidationSignalMode,
};
pub use news_signal::{
    calculate_news_impact, calculate_time_decay, default_category_weights, parse_sentiment,
    NewsSignal,
};
pub use orderbook_imbalance::{
    calculate_imbalance_zscore, calculate_weighted_imbalance, detect_walls,
    OrderBookImbalanceSignal, Side, Wall, WallDetectionConfig,
};
