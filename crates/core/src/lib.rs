pub mod config;
pub mod config_loader;
pub mod config_watcher;
pub mod engine;
pub mod events;
pub mod kelly;
pub mod metrics_formatter;
pub mod position;
pub mod position_sizing;
pub mod signal;
pub mod traits;
pub mod validation;

pub use config::{AppConfig, DatabaseConfig, HyperliquidConfig, ServerConfig};
pub use config_loader::ConfigLoader;
pub use config_watcher::ConfigWatcher;
pub use engine::{PerformanceMetrics, ProcessingCycleEvents, TradingSystem};
pub use events::{FillEvent, MarketEvent, OrderEvent, SignalDirection, SignalEvent};
pub use kelly::{BetDecision, BetReason, KellySizer};
pub use metrics_formatter::MetricsFormatter;
pub use position::{Position, PositionTracker};
pub use position_sizing::{calculate_position_size, calculate_required_margin};
pub use signal::{
    Direction, HistoricalFundingRate, LiquidationAggregate, NewsEvent, OrderBookSnapshot,
    PriceLevel, SignalContext, SignalGenerator, SignalValue,
};
pub use traits::{DataProvider, ExecutionHandler, RiskManager, Strategy};
pub use validation::{binomial_test, information_coefficient, wilson_ci, SignalValidation};
