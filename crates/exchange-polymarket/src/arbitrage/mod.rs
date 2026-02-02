//! Arbitrage detection and execution for Polymarket binary markets.
//!
//! This module provides infrastructure for identifying and executing arbitrage
//! opportunities in Polymarket's BTC 15-minute binary options markets.
//!
//! # Overview
//!
//! In binary prediction markets, arbitrage exists when the combined cost of
//! buying both YES and NO outcomes is less than $1.00 (the guaranteed payout).
//!
//! For example, if YES trades at $0.48 and NO trades at $0.48:
//! - Pair cost: $0.96
//! - Guaranteed payout: $1.00
//! - Gross profit: $0.04 (4.2% return)
//!
//! # Modules
//!
//! - [`types`]: Core data structures for arbitrage
//! - [`orderbook`]: Order book walking and fill simulation
//! - [`detector`]: Arbitrage opportunity detection
//! - [`execution`]: Order execution traits and types (Phase 3)
//!
//! # Example
//!
//! ```
//! use algo_trade_polymarket::arbitrage::{ArbitrageDetector, L2OrderBook, Side};
//! use rust_decimal_macros::dec;
//!
//! // Create order books
//! let mut yes_book = L2OrderBook::new("yes-token-123".to_string());
//! yes_book.apply_snapshot(
//!     vec![(dec!(0.46), dec!(200))],  // bids
//!     vec![(dec!(0.48), dec!(500))],  // asks
//! );
//!
//! let mut no_book = L2OrderBook::new("no-token-456".to_string());
//! no_book.apply_snapshot(
//!     vec![(dec!(0.46), dec!(200))],
//!     vec![(dec!(0.48), dec!(500))],
//! );
//!
//! // Detect arbitrage
//! let detector = ArbitrageDetector::default();
//! if let Some(opportunity) = detector.detect("market-1", &yes_book, &no_book, dec!(100)) {
//!     println!("Found arbitrage!");
//!     println!("  Pair cost: {}", opportunity.pair_cost);
//!     println!("  Net profit per pair: {}", opportunity.net_profit_per_pair);
//!     println!("  ROI: {}%", opportunity.roi);
//! }
//! ```
//!
//! # Fee Model
//!
//! Polymarket charges a 2% fee on profit from the winning side:
//! - E[Fee] = 0.01 * (2 - pair_cost)
//!
//! With typical Polygon gas costs (~$0.007 per transaction), the break-even
//! pair cost is approximately $0.975.
//!
//! # Risk Considerations
//!
//! - **Execution risk**: One leg may fill while the other fails
//! - **Slippage**: Large orders may walk through multiple price levels
//! - **Timing**: Opportunities may disappear before execution
//! - **Imbalance**: Partial fills create directional exposure

pub mod book_feed;
pub mod circuit_breaker;
pub mod detector;
pub mod dual_leg_executor;
pub mod execution;
pub mod live_executor;
pub mod metrics;
pub mod orderbook;
pub mod paper_executor;
pub mod phase1_config;
pub mod rate_limiter;
pub mod sdk_client;
pub mod session;
pub mod signer;
pub mod types;

// Re-export main types for convenience
pub use detector::ArbitrageDetector;
pub use orderbook::{depth_at_price, price_impact, simulate_fill};
pub use types::{
    ArbitrageOpportunity, ArbitragePosition, FillSimulation, L2OrderBook, OrderType,
    PositionStatus, Side,
};

// Execution layer re-exports (Phase 3)
pub use execution::{
    ArbitragePositionSnapshot, ExecutionError, ExecutionResult, ExecutorConfig, OrderParams,
    OrderResult, OrderStatus, PolymarketExecutor, Position, RiskLimit,
};
// Note: execution::Side and execution::OrderType are intentionally not re-exported
// to avoid conflicts with types::Side and types::OrderType. Use the full path
// (execution::Side, execution::OrderType) when working with the execution layer.

// Paper trading executor
pub use paper_executor::{PaperExecutor, PaperExecutorConfig};

// Live trading executor
pub use live_executor::{
    HardLimits, LiveExecutor, LiveExecutorConfig, POLYMARKET_MAINNET_URL, POLYMARKET_TESTNET_URL,
};

// Rate limiting
pub use rate_limiter::{ClobRateLimiter, RateLimiterConfig};

// Secure wallet for order signing
pub use signer::{Wallet, WalletConfig, WalletError};

// Phase 1 arbitrage configuration
pub use phase1_config::{
    Phase1Config, ValidationReason, ValidationResult, MAX_PAIR_COST, MAX_POSITION_VALUE,
    MIN_EDGE_AFTER_FEES, MIN_LIQUIDITY, MIN_VALIDATION_TRADES,
};

// Dual-leg executor for simultaneous YES+NO execution
pub use dual_leg_executor::{DualLegExecutor, DualLegResult, SlippageMetrics, UnwindResult};

// Session state and Go/No-Go tracking
pub use session::{ArbitrageSession, ExecutionRecord, Recommendation, SessionSummary, TradingMode};

// Circuit breaker for trading safety
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError};

// CLOB API client
pub use sdk_client::{ClobClient, ClobClientConfig, ClobError};

// Real-time order book feed
pub use book_feed::{BookFeed, BookFeedConfig, BookFeedError, BookFeedManager};
