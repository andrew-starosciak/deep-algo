//! Core types for IB options trading.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Options contract right (call or put).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionRight {
    Call,
    Put,
}

impl std::fmt::Display for OptionRight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Call => write!(f, "C"),
            Self::Put => write!(f, "P"),
        }
    }
}

/// An options contract specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsContract {
    pub symbol: String,
    pub expiry: NaiveDate,
    pub strike: Decimal,
    pub right: OptionRight,
    /// Exchange routing (e.g., "SMART", "CBOE").
    pub exchange: String,
    /// Contract multiplier (100 for standard US equity options).
    pub multiplier: Decimal,
}

impl OptionsContract {
    /// Create a new standard US equity options contract.
    pub fn new(symbol: &str, expiry: NaiveDate, strike: Decimal, right: OptionRight) -> Self {
        Self {
            symbol: symbol.to_uppercase(),
            expiry,
            strike,
            right,
            exchange: "SMART".to_string(),
            multiplier: Decimal::from(100),
        }
    }

    /// Human-readable contract description (e.g., "NVDA 140C 2026-03-20").
    pub fn display_name(&self) -> String {
        format!("{} {}{} {}", self.symbol, self.strike, self.right, self.expiry)
    }
}

/// Real-time option quote with greeks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionQuote {
    pub contract: OptionsContract,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last: Decimal,
    pub mid: Decimal,
    pub volume: u64,
    pub open_interest: u64,
    pub iv: f64,
    pub greeks: OptionGreeks,
}

/// Option greeks snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptionGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub theta: f64,
    pub vega: f64,
    pub rho: f64,
}

/// IB account summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSummary {
    pub account_id: String,
    pub net_liquidation: Decimal,
    pub buying_power: Decimal,
    pub available_funds: Decimal,
    pub maintenance_margin: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
}

/// Order side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Order type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit { price: Decimal },
}

/// An order to place via IB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsOrder {
    pub contract: OptionsContract,
    pub side: OrderSide,
    pub quantity: i32,
    pub order_type: OrderType,
}

/// A confirmed fill from IB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsFill {
    pub order_id: String,
    pub contract: OptionsContract,
    pub side: OrderSide,
    pub quantity: i32,
    pub avg_fill_price: Decimal,
    pub commission: Decimal,
    pub filled_at: chrono::DateTime<chrono::Utc>,
}
