//! Signal generators for statistical trading.
//!
//! This module contains implementations of various signal generators
//! that produce trading signals based on market data analysis.

pub mod clob_velocity;
mod composite;
pub mod cvd_divergence;
mod funding_rate;
pub mod liquidation_cascade;
pub mod liquidation_ratio;
pub mod momentum;
pub mod news_signal;
mod orderbook_imbalance;

pub use clob_velocity::{ClobVelocityConfig, ClobVelocitySignal};
pub use composite::{
    adjust_weights_for_multicollinearity, calculate_correlation_matrix, combine_bayesian,
    CombinationMethod, CompositeSignal, CorrelationMatrix,
};
pub use cvd_divergence::{
    calculate_divergence_strength, detect_absorption, detect_bearish_divergence,
    detect_bullish_divergence, detect_divergence, AbsorptionType, CvdDivergenceConfig,
    CvdDivergenceSignal, DivergenceType,
};
pub use funding_rate::{
    detect_reversal, is_funding_extreme_30d, percentile_signal, FundingPercentileConfig,
    FundingRateSignal, FundingReversalConfig, FundingSignalMode, ReversalSignal,
};
pub use liquidation_cascade::{
    calculate_net_delta, detect_exhaustion, is_cascade, CascadeConfig, ExhaustionConfig,
    ExhaustionSignal, LiquidationCascadeSignal, LiquidationSignalMode,
};
pub use liquidation_ratio::{
    calculate_ratio_signal, LiquidationAggregate24h, LiquidationRatioConfig, LiquidationRatioSignal,
};
pub use momentum::{
    detect_big_move, detect_momentum_exhaustion, detect_stall, BigMoveResult,
    MomentumExhaustionConfig, MomentumExhaustionSignal,
};
pub use news_signal::{
    calculate_news_impact, calculate_time_decay, default_category_weights, parse_sentiment,
    NewsSignal,
};
pub use orderbook_imbalance::{
    calculate_imbalance_zscore, calculate_wall_bias, calculate_weighted_imbalance, detect_walls,
    OrderBookImbalanceSignal, Side, Wall, WallBias, WallDetectionConfig, WallSemantics,
};
