//! CLI commands for the statistical trading engine.

pub mod backfill_funding;
pub mod backfill_ohlcv;
pub mod backfill_signals;
pub mod binary_backtest;
pub mod calculate_returns;
pub mod collect_polymarket;
pub mod collect_signals;
pub mod data_status;
pub mod entry_strategy_sim;
pub mod polymarket_paper_trade;
pub mod validate_signals;

pub use backfill_funding::{run_backfill_funding, BackfillFundingArgs};
pub use backfill_ohlcv::{run_backfill_ohlcv, BackfillOhlcvArgs};
pub use backfill_signals::{run_backfill_signals, BackfillSignalsArgs};
pub use binary_backtest::{run_binary_backtest, BinaryBacktestArgs};
pub use calculate_returns::{run_calculate_returns, CalculateReturnsArgs};
pub use collect_polymarket::{run_collect_polymarket, CollectPolymarketArgs};
pub use collect_signals::{run_collect_signals, CollectSignalsArgs};
pub use data_status::{run_data_status, DataStatusArgs};
pub use entry_strategy_sim::{run_entry_strategy_sim, EntryStrategySimArgs};
pub use polymarket_paper_trade::{run_polymarket_paper_trade, PolymarketPaperTradeArgs};
pub use validate_signals::{run_validate_signals, ValidateSignalsArgs};
