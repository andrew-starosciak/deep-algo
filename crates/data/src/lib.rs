//! Data storage and management for the statistical trading engine.
//!
//! This crate provides:
//! - Database client for `PostgreSQL`/TimescaleDB
//! - Data models for all trading entities
//! - Repositories for typed database access
//! - CSV and Parquet storage utilities

pub mod csv_storage;
pub mod database;
pub mod models;
pub mod parquet_storage;
pub mod repositories;

// Re-export commonly used types
pub use csv_storage::CsvStorage;
pub use database::{BacktestResultRecord, DatabaseClient, OhlcvRecord};
pub use parquet_storage::ParquetStorage;

// Re-export models
pub use models::{
    BinaryTradeRecord, CascadeDirection, FundingBias, FundingRateRecord,
    LiquidationAggregateRecord, LiquidationRecord, LiquidationSide, NewsEventRecord, NewsSentiment,
    NewsSignalDirection, OrderBookSnapshotRecord, PolymarketOddsRecord, SignalDirection,
    SignalSnapshotRecord, TradeDirection, TradeOutcome,
};

// Re-export repositories
pub use repositories::{
    BinaryTradeRepository, FundingRateRepository, LiquidationRepository, NewsEventRepository,
    OrderBookRepository, PolymarketOddsRepository, Repositories, SignalSnapshotRepository,
    ValidationStats,
};
