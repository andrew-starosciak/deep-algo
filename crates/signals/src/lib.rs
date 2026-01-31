pub mod collector;
pub mod common;
pub mod context_builder;
pub mod funding;
pub mod generator;
pub mod liquidations;
pub mod registry;
pub mod trades;
pub mod validation;

// Re-export signal generators for convenience
pub use generator::{
    adjust_weights_for_multicollinearity, calculate_correlation_matrix, calculate_imbalance_zscore,
    calculate_net_delta, calculate_news_impact, calculate_time_decay, calculate_wall_bias,
    calculate_weighted_imbalance, combine_bayesian, default_category_weights, detect_big_move,
    detect_exhaustion, detect_momentum_exhaustion, detect_reversal, detect_stall, detect_walls,
    is_cascade, is_funding_extreme_30d, parse_sentiment, percentile_signal, BigMoveResult,
    CascadeConfig, CombinationMethod, CompositeSignal, CorrelationMatrix, ExhaustionConfig,
    ExhaustionSignal, FundingPercentileConfig, FundingRateSignal, FundingReversalConfig,
    FundingSignalMode, LiquidationCascadeSignal, LiquidationSignalMode, MomentumExhaustionConfig,
    MomentumExhaustionSignal, NewsSignal, OrderBookImbalanceSignal, ReversalSignal, Side, Wall,
    WallBias, WallDetectionConfig, WallSemantics,
};

// Re-export collectors for convenience
pub use collector::{
    calculate_urgency, categorize_news, determine_sentiment, CollectorConfig, CollectorEvent,
    CollectorStats, FundingCollector, LiquidationCollector, LiquidationCollectorConfig,
    NewsCategory, NewsCollector, NewsCollectorConfig, OrderBookCollector, RollingWindows,
};

// Re-export registry
pub use registry::SignalRegistry;

// Re-export context builder
pub use context_builder::SignalContextBuilder;

// Re-export validation framework
pub use validation::{
    analyze_signal_correlation, calculate_ic, calculate_ranks, determine_recommendation,
    test_directional_accuracy, test_return_significance, CorrelationAnalysis, ICAnalysis,
    Recommendation, SignificanceTest, TestType, ValidationReport, ValidationResult,
};
