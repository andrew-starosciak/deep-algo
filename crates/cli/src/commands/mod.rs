//! CLI commands for the statistical trading engine.

pub mod backfill_ohlcv;
pub mod backfill_signals;
pub mod calculate_returns;
pub mod collect_signals;
pub mod validate_signals;

pub use backfill_ohlcv::{run_backfill_ohlcv, BackfillOhlcvArgs};
pub use backfill_signals::{run_backfill_signals, BackfillSignalsArgs};
pub use calculate_returns::{run_calculate_returns, CalculateReturnsArgs};
pub use collect_signals::{run_collect_signals, CollectSignalsArgs};
pub use validate_signals::{run_validate_signals, ValidateSignalsArgs};
