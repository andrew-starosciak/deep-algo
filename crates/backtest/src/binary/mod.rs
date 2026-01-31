//! Binary outcome backtesting module for Polymarket-style prediction markets.
//!
//! This module provides types and utilities for simulating binary outcome bets,
//! calculating fees, tracking settlement results, and computing comprehensive metrics.

pub mod bootstrap;
pub mod edge;
pub mod engine;
pub mod fees;
pub mod metrics;
pub mod monte_carlo;
pub mod outcome;
pub mod pit;
pub mod regimes;
pub mod walk_forward;

pub use bootstrap::{
    percentile_ci, BootstrapConfig, BootstrapMetrics, BootstrapResampler, BootstrapResult,
};
pub use edge::{
    linear_regression, ConditionalEdge, EdgeAnalysis, EdgeAnalyzer, EdgeAnalyzerConfig,
    EdgeClassification, EdgeDecay, EdgeMeasurement, EdgeSummary, LinearRegression, RollingMetric,
    TimeOfDayEdge, VolatilityEdge,
};
pub use engine::{BacktestResults, BinaryBacktestConfig, BinaryBacktestEngine};
pub use fees::{FeeModel, FeeTier, FlatFees, PolymarketFees, ZeroFees};
pub use metrics::{calculate_break_even, BinaryMetrics};
pub use monte_carlo::{
    BetSizing, DistributionSummary, MonteCarloConfig, MonteCarloResults, MonteCarloSimulator,
};
pub use outcome::{BetDirection, BinaryBet, BinaryOutcome, SettlementResult};
pub use pit::{PointInTimeProvider, DEFAULT_MAX_LOOKBACK_SECONDS};
pub use regimes::{
    RegimeAnalyzer, RegimeCombination, RegimeConfig, RegimeLabel, RegimeMetrics, TimePeriod,
    TrendRegime, VolatilityRegime,
};
pub use walk_forward::{
    FoldPeriod, OverfittingRisk, PerformanceDegradation, SignificanceTest, TrainTestSplit,
    WalkForwardConfig, WalkForwardFold, WalkForwardOptimizer, WalkForwardResults,
};
