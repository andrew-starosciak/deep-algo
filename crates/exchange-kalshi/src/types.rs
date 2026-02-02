//! Data models for Kalshi exchange integration.
//!
//! All financial values use `rust_decimal::Decimal` for precision.
//! Kalshi uses cents (1-99) for prices.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// =============================================================================
// Market Types
// =============================================================================

/// A Kalshi market (event contract).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// Market ticker (e.g., "KXBTC-26FEB02-B100000").
    pub ticker: String,

    /// Event ticker this market belongs to.
    pub event_ticker: String,

    /// Market title/question.
    pub title: String,

    /// Market subtitle (often describes the condition).
    pub subtitle: Option<String>,

    /// Whether the market is currently open for trading.
    pub status: MarketStatus,

    /// Yes bid price in cents (1-99).
    pub yes_bid: Option<Decimal>,

    /// Yes ask price in cents (1-99).
    pub yes_ask: Option<Decimal>,

    /// No bid price in cents (1-99).
    pub no_bid: Option<Decimal>,

    /// No ask price in cents (1-99).
    pub no_ask: Option<Decimal>,

    /// Last trade price in cents.
    pub last_price: Option<Decimal>,

    /// 24h volume in contracts.
    pub volume_24h: Option<i64>,

    /// Open interest (total outstanding contracts).
    pub open_interest: Option<i64>,

    /// Market close time.
    pub close_time: Option<DateTime<Utc>>,

    /// Expiration/settlement time.
    pub expiration_time: Option<DateTime<Utc>>,

    /// Strike value for the market (e.g., 100000 for "$100,000").
    pub strike_value: Option<Decimal>,

    /// Category (e.g., "Crypto").
    pub category: Option<String>,
}

impl Market {
    /// Returns the yes mid price in cents.
    #[must_use]
    pub fn yes_mid(&self) -> Option<Decimal> {
        match (self.yes_bid, self.yes_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            (Some(bid), None) => Some(bid),
            (None, Some(ask)) => Some(ask),
            (None, None) => self.last_price,
        }
    }

    /// Returns the no mid price in cents.
    #[must_use]
    pub fn no_mid(&self) -> Option<Decimal> {
        match (self.no_bid, self.no_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            (Some(bid), None) => Some(bid),
            (None, Some(ask)) => Some(ask),
            (None, None) => self.last_price.map(|p| Decimal::from(100) - p),
        }
    }

    /// Returns true if the market is currently tradeable.
    #[must_use]
    pub fn is_tradeable(&self) -> bool {
        self.status == MarketStatus::Open
    }

    /// Returns true if this is a BTC-related market.
    #[must_use]
    pub fn is_btc_market(&self) -> bool {
        let ticker_lower = self.ticker.to_lowercase();
        let title_lower = self.title.to_lowercase();

        ticker_lower.contains("btc")
            || ticker_lower.contains("bitcoin")
            || title_lower.contains("bitcoin")
            || title_lower.contains("btc")
    }

    /// Returns the yes-no spread in cents.
    #[must_use]
    pub fn spread(&self) -> Option<Decimal> {
        match (self.yes_bid, self.yes_ask) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }
}

/// Market status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketStatus {
    /// Market is open for trading.
    Open,
    /// Market is closed (no trading).
    Closed,
    /// Market has settled.
    Settled,
    /// Market is paused.
    Paused,
}

// =============================================================================
// Order Types
// =============================================================================

/// Side of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Buy YES contracts.
    Yes,
    /// Buy NO contracts.
    No,
}

impl Side {
    /// Returns the opposite side.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Yes => Self::No,
            Self::No => Self::Yes,
        }
    }

    /// Returns the API string representation.
    #[must_use]
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

/// Order action (buy or sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Buy contracts.
    Buy,
    /// Sell contracts.
    Sell,
}

impl Action {
    /// Returns the API string representation.
    #[must_use]
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// Order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    /// Market order (fill at best available).
    Market,
    /// Limit order (fill at specified price or better).
    Limit,
}

/// Request to submit an order.
#[derive(Debug, Clone, Serialize)]
pub struct OrderRequest {
    /// Market ticker.
    pub ticker: String,

    /// Side (yes/no).
    pub side: Side,

    /// Action (buy/sell).
    pub action: Action,

    /// Order type.
    pub order_type: OrderType,

    /// Number of contracts.
    pub count: u32,

    /// Price in cents (1-99) for limit orders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_price: Option<u32>,

    /// Price in cents (1-99) for limit orders (alternative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_price: Option<u32>,

    /// Client-specified order ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,

    /// Time-in-force expiration time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_ts: Option<i64>,
}

impl OrderRequest {
    /// Creates a new limit buy order for YES contracts.
    pub fn buy_yes(ticker: impl Into<String>, price_cents: u32, count: u32) -> Self {
        Self {
            ticker: ticker.into(),
            side: Side::Yes,
            action: Action::Buy,
            order_type: OrderType::Limit,
            count,
            yes_price: Some(price_cents),
            no_price: None,
            client_order_id: None,
            expiration_ts: None,
        }
    }

    /// Creates a new limit buy order for NO contracts.
    pub fn buy_no(ticker: impl Into<String>, price_cents: u32, count: u32) -> Self {
        Self {
            ticker: ticker.into(),
            side: Side::No,
            action: Action::Buy,
            order_type: OrderType::Limit,
            count,
            yes_price: None,
            no_price: Some(price_cents),
            client_order_id: None,
            expiration_ts: None,
        }
    }

    /// Creates a new market buy order.
    pub fn market_buy(ticker: impl Into<String>, side: Side, count: u32) -> Self {
        Self {
            ticker: ticker.into(),
            side,
            action: Action::Buy,
            order_type: OrderType::Market,
            count,
            yes_price: None,
            no_price: None,
            client_order_id: None,
            expiration_ts: None,
        }
    }

    /// Sets a client order ID.
    #[must_use]
    pub fn with_client_order_id(mut self, id: impl Into<String>) -> Self {
        self.client_order_id = Some(id.into());
        self
    }

    /// Sets an expiration timestamp.
    #[must_use]
    pub fn with_expiration(mut self, ts: i64) -> Self {
        self.expiration_ts = Some(ts);
        self
    }

    /// Returns the order value in cents.
    #[must_use]
    pub fn order_value_cents(&self) -> u64 {
        let price = self.yes_price.or(self.no_price).unwrap_or(50) as u64;
        price * self.count as u64
    }
}

/// Status of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Order is pending (not yet on book).
    Pending,
    /// Order is resting on the book.
    Resting,
    /// Order was fully filled.
    Filled,
    /// Order was cancelled.
    Cancelled,
    /// Order was partially filled then cancelled.
    PartialFilled,
    /// Order was rejected.
    Rejected,
}

impl OrderStatus {
    /// Returns true if the order is in a terminal state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Cancelled | Self::PartialFilled | Self::Rejected
        )
    }

    /// Returns true if the order has any fills.
    #[must_use]
    pub fn has_fills(self) -> bool {
        matches!(self, Self::Filled | Self::PartialFilled)
    }
}

/// Response from order submission or status query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Order ID assigned by Kalshi.
    pub order_id: String,

    /// Client order ID if provided.
    pub client_order_id: Option<String>,

    /// Market ticker.
    pub ticker: String,

    /// Side (yes/no).
    pub side: Side,

    /// Action (buy/sell).
    pub action: Action,

    /// Order type.
    pub order_type: OrderType,

    /// Order status.
    pub status: OrderStatus,

    /// Original order quantity.
    pub count: u32,

    /// Filled quantity.
    pub filled_count: u32,

    /// Remaining quantity.
    pub remaining_count: u32,

    /// Limit price in cents (if limit order).
    pub price: Option<u32>,

    /// Average fill price in cents.
    pub avg_fill_price: Option<Decimal>,

    /// Order creation time.
    pub created_time: Option<DateTime<Utc>>,

    /// Last update time.
    pub updated_time: Option<DateTime<Utc>>,
}

impl Order {
    /// Returns true if the order is fully filled.
    #[must_use]
    pub fn is_filled(&self) -> bool {
        self.status == OrderStatus::Filled
    }

    /// Returns true if the order is partially filled.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        self.status == OrderStatus::PartialFilled
            || (self.filled_count > 0 && self.remaining_count > 0)
    }

    /// Returns the fill rate (0.0 to 1.0).
    #[must_use]
    pub fn fill_rate(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.filled_count as f64 / self.count as f64
    }

    /// Returns the filled value in cents.
    #[must_use]
    pub fn filled_value_cents(&self) -> Decimal {
        match self.avg_fill_price {
            Some(price) => price * Decimal::from(self.filled_count),
            None => Decimal::ZERO,
        }
    }
}

// =============================================================================
// Position Types
// =============================================================================

/// A position in a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Market ticker.
    pub ticker: String,

    /// Side (yes/no contracts held).
    pub side: Side,

    /// Number of contracts held.
    pub count: u32,

    /// Average entry price in cents.
    pub avg_price: Decimal,

    /// Current market price in cents.
    pub market_price: Option<Decimal>,

    /// Unrealized P&L in cents.
    pub unrealized_pnl: Option<Decimal>,

    /// Realized P&L in cents.
    pub realized_pnl: Option<Decimal>,
}

impl Position {
    /// Returns the position value at entry in cents.
    #[must_use]
    pub fn entry_value_cents(&self) -> Decimal {
        self.avg_price * Decimal::from(self.count)
    }

    /// Returns the current position value in cents (if market price known).
    #[must_use]
    pub fn current_value_cents(&self) -> Option<Decimal> {
        self.market_price.map(|p| p * Decimal::from(self.count))
    }
}

// =============================================================================
// Orderbook Types
// =============================================================================

/// A price level in the orderbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    /// Price in cents.
    pub price: u32,

    /// Total contracts at this level.
    pub count: u32,
}

/// Orderbook for a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Orderbook {
    /// Market ticker.
    pub ticker: String,

    /// Yes bid levels (sorted by price descending).
    pub yes_bids: Vec<PriceLevel>,

    /// Yes ask levels (sorted by price ascending).
    pub yes_asks: Vec<PriceLevel>,

    /// Timestamp of the snapshot.
    pub timestamp: DateTime<Utc>,
}

impl Orderbook {
    /// Returns the best yes bid price.
    #[must_use]
    pub fn best_yes_bid(&self) -> Option<u32> {
        self.yes_bids.first().map(|l| l.price)
    }

    /// Returns the best yes ask price.
    #[must_use]
    pub fn best_yes_ask(&self) -> Option<u32> {
        self.yes_asks.first().map(|l| l.price)
    }

    /// Returns the best no bid price (100 - yes ask).
    #[must_use]
    pub fn best_no_bid(&self) -> Option<u32> {
        self.best_yes_ask().map(|p| 100 - p)
    }

    /// Returns the best no ask price (100 - yes bid).
    #[must_use]
    pub fn best_no_ask(&self) -> Option<u32> {
        self.best_yes_bid().map(|p| 100 - p)
    }

    /// Returns the yes bid-ask spread in cents.
    #[must_use]
    pub fn yes_spread(&self) -> Option<u32> {
        match (self.best_yes_bid(), self.best_yes_ask()) {
            (Some(bid), Some(ask)) => Some(ask.saturating_sub(bid)),
            _ => None,
        }
    }

    /// Returns total liquidity on yes bid side.
    #[must_use]
    pub fn yes_bid_depth(&self) -> u32 {
        self.yes_bids.iter().map(|l| l.count).sum()
    }

    /// Returns total liquidity on yes ask side.
    #[must_use]
    pub fn yes_ask_depth(&self) -> u32 {
        self.yes_asks.iter().map(|l| l.count).sum()
    }
}

// =============================================================================
// Balance Types
// =============================================================================

/// Account balance information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    /// Total balance in cents.
    pub balance: i64,

    /// Available balance for trading in cents.
    pub available_balance: i64,

    /// Balance reserved for open orders in cents.
    pub reserved_balance: i64,
}

impl Balance {
    /// Returns available balance as Decimal.
    #[must_use]
    pub fn available_decimal(&self) -> Decimal {
        Decimal::from(self.available_balance)
    }

    /// Returns total balance as Decimal.
    #[must_use]
    pub fn total_decimal(&self) -> Decimal {
        Decimal::from(self.balance)
    }
}

// =============================================================================
// API Response Types
// =============================================================================

/// Wrapper for paginated API responses.
#[derive(Debug, Clone, Deserialize)]
pub struct PaginatedResponse<T> {
    /// The data items.
    pub data: Vec<T>,

    /// Cursor for next page.
    pub cursor: Option<String>,
}

/// API response for market data.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketsResponse {
    /// List of markets.
    pub markets: Vec<Market>,

    /// Cursor for pagination.
    pub cursor: Option<String>,
}

/// API response for order submission.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    /// The submitted order.
    pub order: Order,
}

/// API response for balance query.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    /// Account balance.
    pub balance: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ==================== Market Tests ====================

    fn sample_market() -> Market {
        Market {
            ticker: "KXBTC-26FEB02-B100000".to_string(),
            event_ticker: "KXBTC-26FEB02".to_string(),
            title: "Bitcoin above $100,000?".to_string(),
            subtitle: Some("Will BTC exceed $100k at 3pm EST?".to_string()),
            status: MarketStatus::Open,
            yes_bid: Some(dec!(45)),
            yes_ask: Some(dec!(47)),
            no_bid: Some(dec!(53)),
            no_ask: Some(dec!(55)),
            last_price: Some(dec!(46)),
            volume_24h: Some(50000),
            open_interest: Some(100000),
            close_time: None,
            expiration_time: None,
            strike_value: Some(dec!(100000)),
            category: Some("Crypto".to_string()),
        }
    }

    #[test]
    fn test_market_yes_mid() {
        let market = sample_market();
        assert_eq!(market.yes_mid(), Some(dec!(46)));
    }

    #[test]
    fn test_market_no_mid() {
        let market = sample_market();
        assert_eq!(market.no_mid(), Some(dec!(54)));
    }

    #[test]
    fn test_market_yes_mid_bid_only() {
        let mut market = sample_market();
        market.yes_ask = None;
        assert_eq!(market.yes_mid(), Some(dec!(45)));
    }

    #[test]
    fn test_market_yes_mid_fallback_to_last() {
        let mut market = sample_market();
        market.yes_bid = None;
        market.yes_ask = None;
        assert_eq!(market.yes_mid(), Some(dec!(46)));
    }

    #[test]
    fn test_market_is_tradeable() {
        let market = sample_market();
        assert!(market.is_tradeable());
    }

    #[test]
    fn test_market_not_tradeable_when_closed() {
        let mut market = sample_market();
        market.status = MarketStatus::Closed;
        assert!(!market.is_tradeable());
    }

    #[test]
    fn test_market_is_btc_market() {
        let market = sample_market();
        assert!(market.is_btc_market());
    }

    #[test]
    fn test_market_spread() {
        let market = sample_market();
        assert_eq!(market.spread(), Some(dec!(2)));
    }

    // ==================== Side Tests ====================

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Yes.opposite(), Side::No);
        assert_eq!(Side::No.opposite(), Side::Yes);
    }

    #[test]
    fn test_side_api_str() {
        assert_eq!(Side::Yes.as_api_str(), "yes");
        assert_eq!(Side::No.as_api_str(), "no");
    }

    // ==================== OrderRequest Tests ====================

    #[test]
    fn test_order_request_buy_yes() {
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);

        assert_eq!(order.ticker, "KXBTC-TEST");
        assert_eq!(order.side, Side::Yes);
        assert_eq!(order.action, Action::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.count, 100);
        assert_eq!(order.yes_price, Some(45));
        assert!(order.no_price.is_none());
    }

    #[test]
    fn test_order_request_buy_no() {
        let order = OrderRequest::buy_no("KXBTC-TEST", 55, 50);

        assert_eq!(order.side, Side::No);
        assert!(order.yes_price.is_none());
        assert_eq!(order.no_price, Some(55));
    }

    #[test]
    fn test_order_request_market() {
        let order = OrderRequest::market_buy("KXBTC-TEST", Side::Yes, 25);

        assert_eq!(order.order_type, OrderType::Market);
        assert!(order.yes_price.is_none());
    }

    #[test]
    fn test_order_request_with_client_id() {
        let order =
            OrderRequest::buy_yes("KXBTC-TEST", 45, 100).with_client_order_id("my-order-123");

        assert_eq!(order.client_order_id, Some("my-order-123".to_string()));
    }

    #[test]
    fn test_order_request_value_cents() {
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);
        assert_eq!(order.order_value_cents(), 4500);
    }

    // ==================== OrderStatus Tests ====================

    #[test]
    fn test_order_status_terminal() {
        assert!(OrderStatus::Filled.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(OrderStatus::PartialFilled.is_terminal());
        assert!(OrderStatus::Rejected.is_terminal());
        assert!(!OrderStatus::Pending.is_terminal());
        assert!(!OrderStatus::Resting.is_terminal());
    }

    #[test]
    fn test_order_status_has_fills() {
        assert!(OrderStatus::Filled.has_fills());
        assert!(OrderStatus::PartialFilled.has_fills());
        assert!(!OrderStatus::Cancelled.has_fills());
        assert!(!OrderStatus::Pending.has_fills());
    }

    // ==================== Order Tests ====================

    fn sample_order() -> Order {
        Order {
            order_id: "order-123".to_string(),
            client_order_id: Some("client-456".to_string()),
            ticker: "KXBTC-TEST".to_string(),
            side: Side::Yes,
            action: Action::Buy,
            order_type: OrderType::Limit,
            status: OrderStatus::Filled,
            count: 100,
            filled_count: 100,
            remaining_count: 0,
            price: Some(45),
            avg_fill_price: Some(dec!(44.5)),
            created_time: None,
            updated_time: None,
        }
    }

    #[test]
    fn test_order_is_filled() {
        let order = sample_order();
        assert!(order.is_filled());
    }

    #[test]
    fn test_order_is_partial() {
        let mut order = sample_order();
        order.status = OrderStatus::PartialFilled;
        order.filled_count = 50;
        order.remaining_count = 50;
        assert!(order.is_partial());
    }

    #[test]
    fn test_order_fill_rate() {
        let order = sample_order();
        assert!((order.fill_rate() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_order_fill_rate_partial() {
        let mut order = sample_order();
        order.filled_count = 25;
        assert!((order.fill_rate() - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_order_filled_value() {
        let order = sample_order();
        assert_eq!(order.filled_value_cents(), dec!(4450));
    }

    // ==================== Position Tests ====================

    #[test]
    fn test_position_entry_value() {
        let position = Position {
            ticker: "KXBTC-TEST".to_string(),
            side: Side::Yes,
            count: 100,
            avg_price: dec!(45),
            market_price: Some(dec!(50)),
            unrealized_pnl: Some(dec!(500)),
            realized_pnl: None,
        };

        assert_eq!(position.entry_value_cents(), dec!(4500));
    }

    #[test]
    fn test_position_current_value() {
        let position = Position {
            ticker: "KXBTC-TEST".to_string(),
            side: Side::Yes,
            count: 100,
            avg_price: dec!(45),
            market_price: Some(dec!(50)),
            unrealized_pnl: None,
            realized_pnl: None,
        };

        assert_eq!(position.current_value_cents(), Some(dec!(5000)));
    }

    // ==================== Orderbook Tests ====================

    fn sample_orderbook() -> Orderbook {
        Orderbook {
            ticker: "KXBTC-TEST".to_string(),
            yes_bids: vec![
                PriceLevel {
                    price: 45,
                    count: 100,
                },
                PriceLevel {
                    price: 44,
                    count: 200,
                },
            ],
            yes_asks: vec![
                PriceLevel {
                    price: 47,
                    count: 150,
                },
                PriceLevel {
                    price: 48,
                    count: 100,
                },
            ],
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_orderbook_best_yes_bid() {
        let book = sample_orderbook();
        assert_eq!(book.best_yes_bid(), Some(45));
    }

    #[test]
    fn test_orderbook_best_yes_ask() {
        let book = sample_orderbook();
        assert_eq!(book.best_yes_ask(), Some(47));
    }

    #[test]
    fn test_orderbook_best_no_bid() {
        let book = sample_orderbook();
        // NO bid = 100 - YES ask
        assert_eq!(book.best_no_bid(), Some(53));
    }

    #[test]
    fn test_orderbook_best_no_ask() {
        let book = sample_orderbook();
        // NO ask = 100 - YES bid
        assert_eq!(book.best_no_ask(), Some(55));
    }

    #[test]
    fn test_orderbook_spread() {
        let book = sample_orderbook();
        assert_eq!(book.yes_spread(), Some(2));
    }

    #[test]
    fn test_orderbook_bid_depth() {
        let book = sample_orderbook();
        assert_eq!(book.yes_bid_depth(), 300);
    }

    #[test]
    fn test_orderbook_ask_depth() {
        let book = sample_orderbook();
        assert_eq!(book.yes_ask_depth(), 250);
    }

    // ==================== Balance Tests ====================

    #[test]
    fn test_balance_decimal() {
        let balance = Balance {
            balance: 10000,
            available_balance: 7500,
            reserved_balance: 2500,
        };

        assert_eq!(balance.available_decimal(), dec!(7500));
        assert_eq!(balance.total_decimal(), dec!(10000));
    }

    // ==================== MarketStatus Tests ====================

    #[test]
    fn test_market_status_variants() {
        assert_eq!(MarketStatus::Open, MarketStatus::Open);
        assert_ne!(MarketStatus::Open, MarketStatus::Closed);
    }

    // ==================== Action Tests ====================

    #[test]
    fn test_action_api_str() {
        assert_eq!(Action::Buy.as_api_str(), "buy");
        assert_eq!(Action::Sell.as_api_str(), "sell");
    }
}
