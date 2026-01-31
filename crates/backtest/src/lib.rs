pub mod binary;
pub mod data_provider;
pub mod execution;
pub mod metrics;

pub use binary::{
    BetDirection, BinaryBet, BinaryOutcome, FeeModel, FeeTier, PointInTimeProvider, PolymarketFees,
    SettlementResult, DEFAULT_MAX_LOOKBACK_SECONDS,
};
pub use data_provider::HistoricalDataProvider;
pub use execution::SimulatedExecutionHandler;
pub use metrics::{MetricsCalculator, PerformanceMetrics};
