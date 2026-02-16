//! Types for options position management.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// An open options position tracked by the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsPosition {
    pub id: i64,
    pub recommendation_id: i64,
    pub ticker: String,
    pub right: String,
    pub strike: Decimal,
    pub expiry: NaiveDate,
    pub quantity: i32,
    pub avg_fill_price: Decimal,
    pub current_price: Decimal,
    pub cost_basis: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub status: String,
    pub opened_at: DateTime<Utc>,
}

impl OptionsPosition {
    /// Current P&L as a percentage of cost basis.
    pub fn pnl_pct(&self) -> Decimal {
        if self.cost_basis.is_zero() {
            return Decimal::ZERO;
        }
        (self.unrealized_pnl / self.cost_basis) * Decimal::from(100)
    }

    /// Days until expiration.
    pub fn days_to_expiry(&self) -> i64 {
        let today = Utc::now().date_naive();
        (self.expiry - today).num_days()
    }
}

/// Action the manager can take on a position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopAction {
    /// Close the entire position.
    CloseAll { reason: CloseReason },
    /// Close a portion of the position.
    ClosePartial { quantity: i32, reason: CloseReason },
}

/// Reason for closing a position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloseReason {
    HardStop,
    ProfitTarget,
    TimeStop,
    AllocationExceeded,
    Manual,
    ThesisInvalidated,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HardStop => write!(f, "stop_loss"),
            Self::ProfitTarget => write!(f, "profit_target"),
            Self::TimeStop => write!(f, "time_stop"),
            Self::AllocationExceeded => write!(f, "allocation_exceeded"),
            Self::Manual => write!(f, "manual"),
            Self::ThesisInvalidated => write!(f, "thesis_invalid"),
        }
    }
}

/// Approved trade recommendation read from DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedRecommendation {
    pub id: i64,
    pub thesis_id: i64,
    pub ticker: String,
    pub right: String,
    pub strike: Decimal,
    pub expiry: NaiveDate,
    pub entry_price_low: Decimal,
    pub entry_price_high: Decimal,
    pub position_size_usd: Decimal,
}

/// Manager configuration.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// How often to poll (seconds).
    pub poll_interval_secs: u64,
    /// Hard stop loss percentage (e.g., 50.0 = close at -50%).
    pub hard_stop_pct: Decimal,
    /// First profit target percentage (e.g., 50.0 = sell half at +50%).
    pub profit_target_1_pct: Decimal,
    /// Second profit target percentage (e.g., 100.0 = sell rest at +100%).
    pub profit_target_2_pct: Decimal,
    /// Close losing positions within this many DTE.
    pub time_stop_dte: i64,
    /// Maximum percentage of account in swing options.
    pub max_allocation_pct: Decimal,
    /// Maximum correlated positions in same sector.
    pub max_correlated: usize,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            hard_stop_pct: Decimal::from(50),
            profit_target_1_pct: Decimal::from(50),
            profit_target_2_pct: Decimal::from(100),
            time_stop_dte: 7,
            max_allocation_pct: Decimal::from(10),
            max_correlated: 3,
        }
    }
}
