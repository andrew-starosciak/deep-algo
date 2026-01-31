pub mod binary;
pub mod data_provider;
pub mod execution;
pub mod metrics;

pub use binary::{
    create_entry_strategy, BetDirection, BinaryBet, BinaryOutcome, EntryContext, EntryDecision,
    EntryResult, EntryStrategy, EntryStrategyConfig, EntryStrategyType, FallbackEntry, FeeModel,
    FeeTier, FixedTimeEntry, ImmediateEntry, PointInTimeProvider, PolymarketFees, SettlementResult,
    DEFAULT_MAX_LOOKBACK_SECONDS,
};
pub use data_provider::HistoricalDataProvider;
pub use execution::SimulatedExecutionHandler;
pub use metrics::{MetricsCalculator, PerformanceMetrics};
