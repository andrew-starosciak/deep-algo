//! Signal-Strategy Bridge Module
//!
//! This module bridges microstructure `SignalGenerator` implementations with traditional
//! `Strategy` implementations, enabling four key use cases:
//!
//! 1. **Entry Filters**: Block entries when microstructure signals conflict with strategy direction
//! 2. **Exit Triggers**: Force exits based on extreme microstructure conditions
//! 3. **Position Sizing**: Adjust size based on market stress indicators
//! 4. **Entry Timing**: Delay entries until order book support materializes
//!
//! # Architecture
//!
//! ```text
//! MicrostructureOrchestrator (background task)
//!         |
//!         v
//! CachedMicroSignals (shared state via RwLock)
//!         ^
//!         |
//! EnhancedStrategy<S: Strategy> (wraps existing strategy)
//!         |
//!         v
//! MicrostructureFilter (decision logic)
//! ```
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_signals::bridge::{
//!     CachedMicroSignals, EnhancedStrategy, MicrostructureFilterConfig,
//!     SharedMicroSignals,
//! };
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//!
//! // Create shared signal cache
//! let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));
//!
//! // Wrap strategy with microstructure filtering
//! let enhanced = EnhancedStrategy::new(
//!     base_strategy,
//!     signals,
//!     MicrostructureFilterConfig::default(),
//! );
//! ```

mod cached_signals;
mod enhanced_strategy;
mod filter;
mod orchestrator;

pub use cached_signals::{CachedMicroSignals, SharedMicroSignals};
pub use enhanced_strategy::EnhancedStrategy;
pub use filter::{FilterResult, MicrostructureFilter, MicrostructureFilterConfig};
pub use orchestrator::{MicrostructureOrchestrator, OrchestratorCommand};
