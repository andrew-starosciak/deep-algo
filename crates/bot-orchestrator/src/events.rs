use algo_trade_core::events::{FillEvent, OrderEvent, SignalEvent};
use crate::commands::BotState;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BotEvent {
    /// Market event received (candle close)
    MarketUpdate {
        symbol: String,
        price: Decimal,
        volume: Decimal,
        timestamp: DateTime<Utc>,
    },

    /// Strategy generated signal
    SignalGenerated(SignalEvent),

    /// Order submitted to exchange
    OrderPlaced(OrderEvent),

    /// Order filled by exchange
    OrderFilled(FillEvent),

    /// Position opened/modified/closed
    PositionUpdate {
        symbol: String,
        quantity: Decimal,
        avg_price: Decimal,
        unrealized_pnl: Decimal,
    },

    /// Trade closed (realized `PnL`)
    TradeClosed {
        symbol: String,
        pnl: Decimal,
        win: bool,
    },

    /// Error occurred
    Error {
        message: String,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedBotStatus {
    pub bot_id: String,
    pub state: BotState,
    pub last_heartbeat: DateTime<Utc>,

    // Performance metrics
    pub current_equity: Decimal,
    pub initial_capital: Decimal,
    pub total_return_pct: f64,
    pub sharpe_ratio: f64,
    pub max_drawdown: f64,
    pub win_rate: f64,
    pub num_trades: usize,

    // Open positions
    pub open_positions: Vec<PositionInfo>,

    // Recent events (last 10)
    pub recent_events: Vec<BotEvent>,

    // Error if any
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionInfo {
    pub symbol: String,
    pub quantity: Decimal,
    pub avg_price: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub unrealized_pnl_pct: f64,
}
