use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    Quote { symbol: String, bid: Decimal, ask: Decimal, timestamp: DateTime<Utc> },
    Trade { symbol: String, price: Decimal, size: Decimal, timestamp: DateTime<Utc> },
    Bar { symbol: String, open: Decimal, high: Decimal, low: Decimal, close: Decimal, volume: Decimal, timestamp: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    pub symbol: String,
    pub direction: SignalDirection,
    pub strength: f64,
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
