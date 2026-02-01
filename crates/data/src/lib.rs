//! Data storage and management for the statistical trading engine.
//!
//! This crate provides:
//! - Database client for `PostgreSQL`/TimescaleDB
//! - Data models for all trading entities
//! - Repositories for typed database access
//! - CSV and Parquet storage utilities

pub mod chainlink;
pub mod csv_storage;
pub mod database;
pub mod models;
pub mod parquet_storage;
pub mod repositories;
pub mod settlement;

// Re-export commonly used types
pub use csv_storage::CsvStorage;
pub use database::{BacktestResultRecord, DatabaseClient, OhlcvRecord};
pub use parquet_storage::ParquetStorage;

// Re-export models
pub use models::{
    BinaryTradeRecord, CascadeDirection, FundingBias, FundingRateRecord, KellyCriterion,
    LiquidationAggregateRecord, LiquidationRecord, LiquidationSide, NewsEventRecord, NewsSentiment,
    NewsSignalDirection, OrderBookSnapshotRecord, PaperTradeDirection, PaperTradeRecord,
    PaperTradeStatus, PolymarketOddsRecord, SignalDirection, SignalSnapshotRecord, TradeDecision,
    TradeDirection, TradeOutcome,
};

// Re-export repositories
pub use repositories::{
    BinaryTradeRepository, FundingRateRepository, LiquidationRepository, NewsEventRepository,
    OhlcvRepository, OrderBookRepository, PaperTradeRepository, PaperTradeStatistics,
    PolymarketOddsRepository, Repositories, SignalSnapshotRepository, ValidationStats,
};

// Re-export Chainlink price feed types
pub use chainlink::{ChainlinkPriceData, ChainlinkPriceFeed, SettlementResult, WindowPrices};

// Re-export settlement service
pub use settlement::{
    calculate_window_end, calculate_window_start, LiveWindowTracker, SettlementService,
    TradeSettlementResult,
};
