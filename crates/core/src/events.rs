use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    Quote {
        symbol: String,
        bid: Decimal,
        ask: Decimal,
        timestamp: DateTime<Utc>,
    },
    Trade {
        symbol: String,
        price: Decimal,
        size: Decimal,
        timestamp: DateTime<Utc>,
    },
    Bar {
        symbol: String,
        open: Decimal,
        high: Decimal,
        low: Decimal,
        close: Decimal,
        volume: Decimal,
        timestamp: DateTime<Utc>,
    },
}

impl MarketEvent {
    #[must_use]
    pub const fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::Quote { timestamp, .. }
            | Self::Trade { timestamp, .. }
            | Self::Bar { timestamp, .. } => *timestamp,
        }
    }

    #[must_use]
    pub const fn close_price(&self) -> Option<Decimal> {
        match self {
            Self::Bar { close, .. } => Some(*close),
            Self::Trade { price, .. } => Some(*price),
            Self::Quote { .. } => None,
        }
    }

    #[must_use]
    pub fn symbol(&self) -> &str {
        match self {
            Self::Quote { symbol, .. } | Self::Trade { symbol, .. } | Self::Bar { symbol, .. } => {
                symbol
            }
        }
    }

    #[must_use]
    pub const fn volume(&self) -> Option<Decimal> {
        match self {
            Self::Bar { volume, .. } => Some(*volume),
            Self::Trade { size, .. } => Some(*size),
            Self::Quote { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    pub symbol: String,
    pub direction: SignalDirection,
    pub strength: f64,
    pub price: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SignalDirection {
    Long,
    Short,
    Exit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderEvent {
    pub symbol: String,
    pub order_type: OrderType,
    pub direction: OrderDirection,
    pub quantity: Decimal,
    pub price: Option<Decimal>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderDirection {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub order_id: String,
    pub symbol: String,
    pub direction: OrderDirection,
    pub quantity: Decimal,
    pub price: Decimal,
    pub commission: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// Represents a closed trade (complete round trip: entry + exit)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosedTrade {
    pub symbol: String,
    pub entry_time: DateTime<Utc>,
    pub exit_time: DateTime<Utc>,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub quantity: Decimal,
    pub direction: TradeDirection, // Long or Short
    pub pnl: Decimal,
    pub pnl_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TradeDirection {
    Long,  // Buy then Sell
    Short, // Sell then Buy
}
