//! Database repositories for the statistical trading engine.
//!
//! Each repository provides typed access to a specific table with
//! batch insert capabilities and time-range queries.

pub mod funding_repo;
pub mod liquidation_repo;
pub mod news_repo;
pub mod orderbook_repo;
pub mod polymarket_repo;
pub mod signal_snapshot_repo;
pub mod trade_repo;

pub use funding_repo::FundingRateRepository;
pub use liquidation_repo::LiquidationRepository;
pub use news_repo::NewsEventRepository;
pub use orderbook_repo::OrderBookRepository;
pub use polymarket_repo::PolymarketOddsRepository;
pub use signal_snapshot_repo::{SignalSnapshotRepository, ValidationStats};
pub use trade_repo::BinaryTradeRepository;

use sqlx::PgPool;

/// Creates all repositories from a single database pool.
pub struct Repositories {
    pub orderbook: OrderBookRepository,
    pub funding: FundingRateRepository,
    pub liquidation: LiquidationRepository,
    pub polymarket: PolymarketOddsRepository,
    pub news: NewsEventRepository,
    pub trades: BinaryTradeRepository,
    pub signal_snapshots: SignalSnapshotRepository,
}

impl Repositories {
    /// Creates a new set of repositories from a database pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            orderbook: OrderBookRepository::new(pool.clone()),
            funding: FundingRateRepository::new(pool.clone()),
            liquidation: LiquidationRepository::new(pool.clone()),
            polymarket: PolymarketOddsRepository::new(pool.clone()),
            news: NewsEventRepository::new(pool.clone()),
            trades: BinaryTradeRepository::new(pool.clone()),
            signal_snapshots: SignalSnapshotRepository::new(pool),
        }
    }
}

#[cfg(test)]
mod tests {
    // Integration tests would go here, requiring a test database.
    // For unit tests, see individual repository modules.
}
