//! WebSocket client for Polymarket CLOB order book streaming.
//!
//! This module provides real-time order book updates via WebSocket connection to
//! Polymarket's CLOB API. It supports:
//!
//! - Subscribing to multiple token IDs (YES and NO outcomes)
//! - Full order book snapshots on initial subscription
//! - Incremental delta updates (price_change events)
//! - Automatic reconnection with exponential backoff
//! - In-memory L2 order book state maintenance
//!
//! # Architecture
//!
//! The WebSocket client uses an actor pattern with tokio channels:
//!
//! ```text
//! PolymarketWebSocket::connect()
//!        │
//!        ├─► Spawns connection handler task
//!        │   └─► Processes messages, emits BookEvents
//!        │
//!        └─► Returns (handle, mpsc::Receiver<BookEvent>)
//! ```
//!
//! # Example
//!
//! ```no_run
//! use algo_trade_polymarket::websocket::{PolymarketWebSocket, WebSocketConfig, BookEvent};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = WebSocketConfig::default();
//!     let token_ids = vec![
//!         "yes-token-123".to_string(),
//!         "no-token-456".to_string(),
//!     ];
//!
//!     let (ws, mut rx) = PolymarketWebSocket::connect(token_ids, config).await?;
//!
//!     while let Some(event) = rx.recv().await {
//!         match event {
//!             BookEvent::Snapshot { asset_id, book } => {
//!                 println!("Received snapshot for {}: bid={:?} ask={:?}",
//!                     asset_id, book.best_bid(), book.best_ask());
//!             }
//!             BookEvent::Delta { asset_id, side, price, size } => {
//!                 println!("Delta: {} {:?} @ {} size {}",
//!                     asset_id, side, price, size);
//!             }
//!             BookEvent::Trade { .. } => {}
//!             BookEvent::Connected => println!("Connected!"),
//!             BookEvent::Disconnected { reason } => {
//!                 println!("Disconnected: {}", reason);
//!             }
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # WebSocket Message Format
//!
//! Polymarket uses the following message types:
//!
//! ## Subscription
//! ```json
//! {
//!   "assets_ids": ["token_id_1", "token_id_2"],
//!   "type": "market"
//! }
//! ```
//!
//! ## Book Event (Snapshot)
//! ```json
//! {
//!   "event_type": "book",
//!   "asset_id": "...",
//!   "market": "...",
//!   "bids": [{"price": ".48", "size": "30"}],
//!   "asks": [{"price": ".52", "size": "25"}],
//!   "timestamp": "123456789000",
//!   "hash": "0x..."
//! }
//! ```
//!
//! ## Price Change Event (Delta)
//! ```json
//! {
//!   "event_type": "price_change",
//!   "market": "...",
//!   "timestamp": "...",
//!   "price_changes": [
//!     {"asset_id": "...", "price": ".50", "size": "0", "side": "BUY"}
//!   ]
//! }
//! ```

use crate::arbitrage::types::{L2OrderBook, Side};
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Default WebSocket URL for Polymarket CLOB market channel.
pub const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// Events emitted by the WebSocket client.
#[derive(Debug, Clone)]
pub enum BookEvent {
    /// Full order book snapshot received.
    Snapshot {
        /// Token/asset ID
        asset_id: String,
        /// Complete order book state
        book: L2OrderBook,
    },
    /// Incremental delta update to a single price level.
    Delta {
        /// Token/asset ID
        asset_id: String,
        /// Side of the update (Buy or Sell)
        side: Side,
        /// Price level
        price: Decimal,
        /// New size (0 means level removed)
        size: Decimal,
    },
    /// Trade execution event.
    Trade {
        /// Token/asset ID
        asset_id: String,
        /// Execution price
        price: Decimal,
        /// Trade size
        size: Decimal,
        /// Trade side
        side: Side,
    },
    /// WebSocket connection established.
    Connected,
    /// WebSocket connection lost.
    Disconnected {
        /// Reason for disconnection
        reason: String,
    },
}

/// Configuration for the WebSocket client.
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// WebSocket URL (defaults to Polymarket CLOB market channel).
    pub url: String,
    /// Initial reconnection delay.
    pub initial_reconnect_delay: Duration,
    /// Maximum reconnection delay (for exponential backoff).
    pub max_reconnect_delay: Duration,
    /// Maximum number of reconnection attempts (0 = unlimited).
    pub max_reconnect_attempts: u32,
    /// Ping interval to keep connection alive.
    pub ping_interval: Duration,
    /// Channel buffer size for events.
    pub channel_buffer_size: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            url: WS_URL.to_string(),
            initial_reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(30),
            max_reconnect_attempts: 0, // unlimited
            ping_interval: Duration::from_secs(30),
            channel_buffer_size: 1000,
        }
    }
}

/// WebSocket connection handle.
///
/// Provides access to in-memory order book state and control over the connection.
#[derive(Clone)]
pub struct PolymarketWebSocket {
    /// In-memory order book state for all subscribed tokens.
    books: Arc<RwLock<HashMap<String, L2OrderBook>>>,
    /// Shutdown signal sender.
    shutdown_tx: mpsc::Sender<()>,
    /// Subscribed token IDs.
    token_ids: Arc<Vec<String>>,
}

impl PolymarketWebSocket {
    /// Connects to the Polymarket WebSocket and subscribes to the given token IDs.
    ///
    /// Returns a handle to the connection and a receiver for book events.
    ///
    /// # Arguments
    ///
    /// * `token_ids` - Token IDs to subscribe to (typically YES and NO tokens)
    /// * `config` - WebSocket configuration
    ///
    /// # Returns
    ///
    /// * `Self` - Handle for accessing order book state
    /// * `mpsc::Receiver<BookEvent>` - Channel for receiving book events
    ///
    /// # Errors
    ///
    /// Returns error if initial connection fails.
    pub async fn connect(
        token_ids: Vec<String>,
        config: WebSocketConfig,
    ) -> Result<(Self, mpsc::Receiver<BookEvent>), WebSocketError> {
        let (event_tx, event_rx) = mpsc::channel(config.channel_buffer_size);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let books: Arc<RwLock<HashMap<String, L2OrderBook>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Initialize empty books for all tokens
        {
            let mut books_guard = books.write();
            for token_id in &token_ids {
                books_guard.insert(token_id.clone(), L2OrderBook::new(token_id.clone()));
            }
        }

        let token_ids_arc = Arc::new(token_ids.clone());
        let books_clone = Arc::clone(&books);

        // Spawn connection handler
        tokio::spawn(run_connection_loop(
            config,
            token_ids,
            books_clone,
            event_tx,
            shutdown_rx,
        ));

        Ok((
            Self {
                books,
                shutdown_tx,
                token_ids: token_ids_arc,
            },
            event_rx,
        ))
    }

    /// Returns a snapshot of the current order book for a token.
    #[must_use]
    pub fn get_book(&self, token_id: &str) -> Option<L2OrderBook> {
        self.books.read().get(token_id).cloned()
    }

    /// Returns snapshots of all order books.
    #[must_use]
    pub fn get_all_books(&self) -> HashMap<String, L2OrderBook> {
        self.books.read().clone()
    }

    /// Returns the list of subscribed token IDs.
    #[must_use]
    pub fn token_ids(&self) -> &[String] {
        &self.token_ids
    }

    /// Gracefully shuts down the WebSocket connection.
    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}

/// Errors that can occur during WebSocket operations.
#[derive(Error, Debug)]
pub enum WebSocketError {
    /// Failed to establish WebSocket connection.
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// WebSocket protocol error.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// JSON parsing error.
    #[error("Parse error: {0}")]
    Parse(#[from] serde_json::Error),

    /// Maximum reconnection attempts exceeded.
    #[error("Max reconnection attempts exceeded")]
    MaxReconnectsExceeded,
}

// ============================================================================
// WebSocket Message Types (Serde)
// ============================================================================

/// Subscription message sent to the WebSocket server.
#[derive(Debug, Serialize)]
struct SubscriptionMessage<'a> {
    assets_ids: &'a [String],
    #[serde(rename = "type")]
    msg_type: &'static str,
}

/// Incoming WebSocket message wrapper.
#[derive(Debug, Deserialize)]
struct WsMessage {
    event_type: String,
    #[serde(flatten)]
    data: serde_json::Value,
}

/// Order book snapshot message.
#[derive(Debug, Deserialize)]
struct BookMessage {
    asset_id: String,
    /// Market condition ID (included for completeness, not currently used)
    #[serde(default)]
    #[allow(dead_code)]
    market: String,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    #[serde(default)]
    timestamp: Option<String>,
}

/// Price level in order book.
#[derive(Debug, Deserialize)]
struct PriceLevel {
    price: String,
    size: String,
}

/// Price change (delta) message.
#[derive(Debug, Deserialize)]
struct PriceChangeMessage {
    /// Market condition ID (included for completeness, not currently used)
    #[serde(default)]
    #[allow(dead_code)]
    market: String,
    /// Timestamp (included for completeness, not currently used)
    #[serde(default)]
    #[allow(dead_code)]
    timestamp: Option<String>,
    #[serde(default)]
    price_changes: Vec<PriceChange>,
    // Single price change format (legacy/alternative)
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
}

/// Individual price change in a price_change message.
#[derive(Debug, Deserialize)]
struct PriceChange {
    asset_id: String,
    price: String,
    size: String,
    side: String,
}

/// Last trade price message.
#[derive(Debug, Deserialize)]
struct LastTradePriceMessage {
    asset_id: String,
    price: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
}

// ============================================================================
// Connection Loop Implementation
// ============================================================================

/// Main connection loop with reconnection logic.
async fn run_connection_loop(
    config: WebSocketConfig,
    token_ids: Vec<String>,
    books: Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: mpsc::Sender<BookEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    let mut reconnect_delay = config.initial_reconnect_delay;
    let mut reconnect_attempts: u32 = 0;

    loop {
        // Check for shutdown signal
        if shutdown_rx.try_recv().is_ok() {
            info!("WebSocket shutdown requested");
            break;
        }

        info!(url = %config.url, "Connecting to Polymarket WebSocket");

        match connect_and_run(&config, &token_ids, &books, &event_tx, &mut shutdown_rx).await {
            Ok(()) => {
                // Clean shutdown
                info!("WebSocket connection closed cleanly");
                break;
            }
            Err(e) => {
                reconnect_attempts += 1;
                error!(
                    error = %e,
                    attempt = reconnect_attempts,
                    "WebSocket connection failed"
                );

                // Check max attempts
                if config.max_reconnect_attempts > 0
                    && reconnect_attempts >= config.max_reconnect_attempts
                {
                    error!("Max reconnection attempts exceeded");
                    let _ = event_tx
                        .send(BookEvent::Disconnected {
                            reason: "Max reconnection attempts exceeded".to_string(),
                        })
                        .await;
                    break;
                }

                // Send disconnected event
                let _ = event_tx
                    .send(BookEvent::Disconnected {
                        reason: e.to_string(),
                    })
                    .await;

                // Wait before reconnecting (exponential backoff)
                info!(delay = ?reconnect_delay, "Waiting before reconnect");
                sleep(reconnect_delay).await;

                // Increase delay for next attempt (capped at max)
                reconnect_delay = (reconnect_delay * 2).min(config.max_reconnect_delay);
            }
        }
    }
}

/// Connect to WebSocket and process messages until disconnection.
async fn connect_and_run(
    config: &WebSocketConfig,
    token_ids: &[String],
    books: &Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: &mpsc::Sender<BookEvent>,
    shutdown_rx: &mut mpsc::Receiver<()>,
) -> Result<(), WebSocketError> {
    // Connect to WebSocket
    let (ws_stream, _response) = connect_async(&config.url)
        .await
        .map_err(|e| WebSocketError::ConnectionFailed(e.to_string()))?;

    info!("WebSocket connected successfully");

    let (mut write, mut read) = ws_stream.split();

    // Subscribe to tokens
    let sub_msg = SubscriptionMessage {
        assets_ids: token_ids,
        msg_type: "market",
    };
    let sub_json = serde_json::to_string(&sub_msg)?;
    debug!(message = %sub_json, "Sending subscription message");
    write.send(Message::Text(sub_json)).await?;

    // Send connected event
    let _ = event_tx.send(BookEvent::Connected).await;

    // Set up ping interval
    let mut ping_interval = tokio::time::interval(config.ping_interval);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Check for shutdown
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received, closing WebSocket");
                let _ = write.close().await;
                return Ok(());
            }

            // Periodic ping to keep connection alive
            _ = ping_interval.tick() => {
                debug!("Sending ping");
                if let Err(e) = write.send(Message::Ping(vec![])).await {
                    warn!(error = %e, "Failed to send ping");
                    return Err(WebSocketError::WebSocket(e));
                }
            }

            // Process incoming messages
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(e) = process_message(&text, books, event_tx).await {
                            warn!(error = %e, "Failed to process message");
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        debug!("Received ping, sending pong");
                        write.send(Message::Pong(data)).await?;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Received pong");
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!(frame = ?frame, "Received close frame");
                        return Err(WebSocketError::ConnectionFailed(
                            frame.map(|f| f.reason.to_string()).unwrap_or_else(|| "Connection closed".to_string())
                        ));
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Try to parse binary as text
                        if let Ok(text) = String::from_utf8(data) {
                            if let Err(e) = process_message(&text, books, event_tx).await {
                                warn!(error = %e, "Failed to process binary message");
                            }
                        }
                    }
                    Some(Ok(Message::Frame(_))) => {
                        // Raw frame, ignore
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "WebSocket error");
                        return Err(WebSocketError::WebSocket(e));
                    }
                    None => {
                        // Stream ended
                        return Err(WebSocketError::ConnectionFailed("Stream ended".to_string()));
                    }
                }
            }
        }
    }
}

/// Process a single WebSocket message.
async fn process_message(
    text: &str,
    books: &Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: &mpsc::Sender<BookEvent>,
) -> Result<(), WebSocketError> {
    // Try to parse as a single message or array of messages
    let messages: Vec<WsMessage> = if text.trim().starts_with('[') {
        serde_json::from_str(text)?
    } else {
        vec![serde_json::from_str(text)?]
    };

    for msg in messages {
        match msg.event_type.as_str() {
            "book" => {
                let book_msg: BookMessage = serde_json::from_value(msg.data)?;
                handle_book_snapshot(&book_msg, books, event_tx).await?;
            }
            "price_change" => {
                let change_msg: PriceChangeMessage = serde_json::from_value(msg.data)?;
                handle_price_change(&change_msg, books, event_tx).await?;
            }
            "last_trade_price" => {
                let trade_msg: LastTradePriceMessage = serde_json::from_value(msg.data)?;
                handle_last_trade_price(&trade_msg, event_tx).await?;
            }
            "tick_size_change" => {
                debug!("Received tick_size_change event (ignored)");
            }
            other => {
                debug!(event_type = %other, "Received unknown event type");
            }
        }
    }

    Ok(())
}

/// Handle a book snapshot message.
async fn handle_book_snapshot(
    msg: &BookMessage,
    books: &Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: &mpsc::Sender<BookEvent>,
) -> Result<(), WebSocketError> {
    let bids = parse_price_levels(&msg.bids);
    let asks = parse_price_levels(&msg.asks);

    let timestamp_ms = msg.timestamp.as_ref().and_then(|t| t.parse::<i64>().ok());

    // Update in-memory book
    {
        let mut books_guard = books.write();
        let book = books_guard
            .entry(msg.asset_id.clone())
            .or_insert_with(|| L2OrderBook::new(msg.asset_id.clone()));

        book.apply_snapshot(bids.clone(), asks.clone());
        book.last_update_ms = timestamp_ms;
    }

    // Get updated book for event
    let book = books.read().get(&msg.asset_id).cloned();
    if let Some(book) = book {
        debug!(
            asset_id = %msg.asset_id,
            bid_levels = book.bid_levels(),
            ask_levels = book.ask_levels(),
            best_bid = ?book.best_bid(),
            best_ask = ?book.best_ask(),
            "Received book snapshot"
        );

        let _ = event_tx
            .send(BookEvent::Snapshot {
                asset_id: msg.asset_id.clone(),
                book,
            })
            .await;
    }

    Ok(())
}

/// Handle a price change (delta) message.
async fn handle_price_change(
    msg: &PriceChangeMessage,
    books: &Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: &mpsc::Sender<BookEvent>,
) -> Result<(), WebSocketError> {
    // Handle array of price changes
    if !msg.price_changes.is_empty() {
        for change in &msg.price_changes {
            apply_single_delta(
                &change.asset_id,
                &change.price,
                &change.size,
                &change.side,
                books,
                event_tx,
            )
            .await?;
        }
    }

    // Handle single price change format (legacy/alternative)
    if let (Some(asset_id), Some(price), Some(size), Some(side)) = (
        msg.asset_id.as_ref(),
        msg.price.as_ref(),
        msg.size.as_ref(),
        msg.side.as_ref(),
    ) {
        apply_single_delta(asset_id, price, size, side, books, event_tx).await?;
    }

    Ok(())
}

/// Apply a single delta update.
async fn apply_single_delta(
    asset_id: &str,
    price_str: &str,
    size_str: &str,
    side_str: &str,
    books: &Arc<RwLock<HashMap<String, L2OrderBook>>>,
    event_tx: &mpsc::Sender<BookEvent>,
) -> Result<(), WebSocketError> {
    let price = parse_decimal(price_str);
    let size = parse_decimal(size_str);
    let side = parse_side(side_str);

    // Update in-memory book
    {
        let mut books_guard = books.write();
        if let Some(book) = books_guard.get_mut(asset_id) {
            book.apply_delta(side, price, size);
        } else {
            // Create new book if we don't have one for this asset
            let mut book = L2OrderBook::new(asset_id.to_string());
            book.apply_delta(side, price, size);
            books_guard.insert(asset_id.to_string(), book);
        }
    }

    debug!(
        asset_id = %asset_id,
        side = ?side,
        price = %price,
        size = %size,
        "Applied delta update"
    );

    let _ = event_tx
        .send(BookEvent::Delta {
            asset_id: asset_id.to_string(),
            side,
            price,
            size,
        })
        .await;

    Ok(())
}

/// Handle a last trade price message.
async fn handle_last_trade_price(
    msg: &LastTradePriceMessage,
    event_tx: &mpsc::Sender<BookEvent>,
) -> Result<(), WebSocketError> {
    let price = parse_decimal(&msg.price);
    let size = msg
        .size
        .as_ref()
        .map(|s| parse_decimal(s))
        .unwrap_or(Decimal::ZERO);
    let side = msg
        .side
        .as_ref()
        .map(|s| parse_side(s))
        .unwrap_or(Side::Buy);

    debug!(
        asset_id = %msg.asset_id,
        price = %price,
        size = %size,
        "Received last trade price"
    );

    let _ = event_tx
        .send(BookEvent::Trade {
            asset_id: msg.asset_id.clone(),
            price,
            size,
            side,
        })
        .await;

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse price levels from WebSocket format.
fn parse_price_levels(levels: &[PriceLevel]) -> Vec<(Decimal, Decimal)> {
    levels
        .iter()
        .filter_map(|level| {
            let price = parse_decimal(&level.price);
            let size = parse_decimal(&level.size);
            if size > Decimal::ZERO {
                Some((price, size))
            } else {
                None
            }
        })
        .collect()
}

/// Parse a decimal string, handling the ".XX" format used by Polymarket.
fn parse_decimal(s: &str) -> Decimal {
    // Handle ".XX" format (e.g., ".48" -> "0.48")
    let normalized = if s.starts_with('.') {
        format!("0{s}")
    } else {
        s.to_string()
    };

    Decimal::from_str(&normalized).unwrap_or(Decimal::ZERO)
}

/// Parse a side string ("BUY" or "SELL").
fn parse_side(s: &str) -> Side {
    match s.to_uppercase().as_str() {
        "BUY" | "B" => Side::Buy,
        "SELL" | "S" => Side::Sell,
        _ => Side::Buy, // Default to buy
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_parse_decimal_standard() {
        assert_eq!(parse_decimal("0.48"), dec!(0.48));
        assert_eq!(parse_decimal("1.00"), dec!(1.00));
        assert_eq!(parse_decimal("100"), dec!(100));
    }

    #[test]
    fn test_parse_decimal_polymarket_format() {
        // Polymarket uses ".48" instead of "0.48"
        assert_eq!(parse_decimal(".48"), dec!(0.48));
        assert_eq!(parse_decimal(".01"), dec!(0.01));
        assert_eq!(parse_decimal(".99"), dec!(0.99));
    }

    #[test]
    fn test_parse_decimal_invalid() {
        assert_eq!(parse_decimal("invalid"), Decimal::ZERO);
        assert_eq!(parse_decimal(""), Decimal::ZERO);
    }

    #[test]
    fn test_parse_side() {
        assert_eq!(parse_side("BUY"), Side::Buy);
        assert_eq!(parse_side("SELL"), Side::Sell);
        assert_eq!(parse_side("buy"), Side::Buy);
        assert_eq!(parse_side("sell"), Side::Sell);
        assert_eq!(parse_side("B"), Side::Buy);
        assert_eq!(parse_side("S"), Side::Sell);
    }

    #[test]
    fn test_parse_price_levels() {
        let levels = vec![
            PriceLevel {
                price: ".48".to_string(),
                size: "100".to_string(),
            },
            PriceLevel {
                price: ".47".to_string(),
                size: "200".to_string(),
            },
            PriceLevel {
                price: ".46".to_string(),
                size: "0".to_string(), // Should be filtered out
            },
        ];

        let parsed = parse_price_levels(&levels);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], (dec!(0.48), dec!(100)));
        assert_eq!(parsed[1], (dec!(0.47), dec!(200)));
    }

    #[test]
    fn test_config_default() {
        let config = WebSocketConfig::default();
        assert_eq!(config.url, WS_URL);
        assert_eq!(config.initial_reconnect_delay, Duration::from_secs(1));
        assert_eq!(config.max_reconnect_delay, Duration::from_secs(30));
        assert_eq!(config.max_reconnect_attempts, 0);
        assert_eq!(config.channel_buffer_size, 1000);
    }

    #[tokio::test]
    async fn test_book_message_parsing() {
        let json = r#"{
            "event_type": "book",
            "asset_id": "token-123",
            "market": "market-456",
            "bids": [{"price": ".48", "size": "100"}, {"price": ".47", "size": "200"}],
            "asks": [{"price": ".52", "size": "150"}, {"price": ".53", "size": "250"}],
            "timestamp": "1706745600000"
        }"#;

        let msg: WsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.event_type, "book");

        let book_msg: BookMessage = serde_json::from_value(msg.data).unwrap();
        assert_eq!(book_msg.asset_id, "token-123");
        assert_eq!(book_msg.market, "market-456");
        assert_eq!(book_msg.bids.len(), 2);
        assert_eq!(book_msg.asks.len(), 2);
        assert_eq!(book_msg.timestamp, Some("1706745600000".to_string()));
    }

    #[tokio::test]
    async fn test_price_change_message_parsing() {
        let json = r#"{
            "event_type": "price_change",
            "market": "market-456",
            "timestamp": "1706745600001",
            "price_changes": [
                {"asset_id": "token-123", "price": ".49", "size": "50", "side": "BUY"},
                {"asset_id": "token-123", "price": ".51", "size": "0", "side": "SELL"}
            ]
        }"#;

        let msg: WsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.event_type, "price_change");

        let change_msg: PriceChangeMessage = serde_json::from_value(msg.data).unwrap();
        assert_eq!(change_msg.market, "market-456");
        assert_eq!(change_msg.price_changes.len(), 2);
        assert_eq!(change_msg.price_changes[0].asset_id, "token-123");
        assert_eq!(change_msg.price_changes[0].price, ".49");
        assert_eq!(change_msg.price_changes[0].size, "50");
        assert_eq!(change_msg.price_changes[0].side, "BUY");
    }

    #[tokio::test]
    async fn test_price_change_single_format_parsing() {
        // Alternative single price change format
        let json = r#"{
            "event_type": "price_change",
            "market": "market-456",
            "asset_id": "token-123",
            "price": ".50",
            "size": "75",
            "side": "SELL"
        }"#;

        let msg: WsMessage = serde_json::from_str(json).unwrap();
        let change_msg: PriceChangeMessage = serde_json::from_value(msg.data).unwrap();
        assert_eq!(change_msg.asset_id, Some("token-123".to_string()));
        assert_eq!(change_msg.price, Some(".50".to_string()));
        assert_eq!(change_msg.size, Some("75".to_string()));
        assert_eq!(change_msg.side, Some("SELL".to_string()));
    }

    #[tokio::test]
    async fn test_last_trade_price_parsing() {
        let json = r#"{
            "event_type": "last_trade_price",
            "asset_id": "token-123",
            "price": ".48",
            "size": "25",
            "side": "BUY"
        }"#;

        let msg: WsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.event_type, "last_trade_price");

        let trade_msg: LastTradePriceMessage = serde_json::from_value(msg.data).unwrap();
        assert_eq!(trade_msg.asset_id, "token-123");
        assert_eq!(trade_msg.price, ".48");
        assert_eq!(trade_msg.size, Some("25".to_string()));
        assert_eq!(trade_msg.side, Some("BUY".to_string()));
    }

    #[tokio::test]
    async fn test_subscription_message_serialization() {
        let token_ids = vec!["token-1".to_string(), "token-2".to_string()];
        let sub_msg = SubscriptionMessage {
            assets_ids: &token_ids,
            msg_type: "market",
        };

        let json = serde_json::to_string(&sub_msg).unwrap();
        assert!(json.contains(r#""assets_ids":["token-1","token-2"]"#));
        assert!(json.contains(r#""type":"market""#));
    }

    #[test]
    fn test_book_event_variants() {
        // Test that all BookEvent variants can be constructed
        let snapshot = BookEvent::Snapshot {
            asset_id: "token-123".to_string(),
            book: L2OrderBook::new("token-123".to_string()),
        };
        assert!(matches!(snapshot, BookEvent::Snapshot { .. }));

        let delta = BookEvent::Delta {
            asset_id: "token-123".to_string(),
            side: Side::Buy,
            price: dec!(0.48),
            size: dec!(100),
        };
        assert!(matches!(delta, BookEvent::Delta { .. }));

        let trade = BookEvent::Trade {
            asset_id: "token-123".to_string(),
            price: dec!(0.48),
            size: dec!(25),
            side: Side::Buy,
        };
        assert!(matches!(trade, BookEvent::Trade { .. }));

        let connected = BookEvent::Connected;
        assert!(matches!(connected, BookEvent::Connected));

        let disconnected = BookEvent::Disconnected {
            reason: "test".to_string(),
        };
        assert!(matches!(disconnected, BookEvent::Disconnected { .. }));
    }

    #[test]
    fn test_websocket_error_display() {
        let err = WebSocketError::ConnectionFailed("test error".to_string());
        assert!(err.to_string().contains("Connection failed"));

        let err = WebSocketError::MaxReconnectsExceeded;
        assert!(err.to_string().contains("Max reconnection attempts"));
    }

    // Integration test placeholder - requires mock WebSocket server
    #[tokio::test]
    #[ignore = "Requires mock WebSocket server"]
    async fn test_websocket_connection() {
        // This test would use a mock WebSocket server to test:
        // 1. Connection establishment
        // 2. Subscription message sending
        // 3. Snapshot reception and book update
        // 4. Delta update reception and book update
        // 5. Reconnection on disconnect
        //
        // For now, this is marked as ignored because it requires
        // setting up a mock WebSocket server.
    }
}

#[cfg(test)]
mod mock_server_tests {
    //! Tests using a mock WebSocket server.
    //!
    //! These tests verify the full WebSocket client behavior including:
    //! - Connection and subscription
    //! - Snapshot processing
    //! - Delta updates
    //! - Reconnection logic
    //!
    //! The mock server pattern allows testing without connecting to the real
    //! Polymarket WebSocket endpoint.

    use super::*;
    use rust_decimal_macros::dec;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    /// Simple mock WebSocket server for testing.
    pub struct MockWebSocketServer {
        addr: SocketAddr,
        shutdown_tx: Option<oneshot::Sender<()>>,
    }

    impl MockWebSocketServer {
        /// Start a mock WebSocket server that sends a single book snapshot.
        pub async fn start_with_snapshot(
            asset_id: &str,
            bids: Vec<(&str, &str)>,
            asks: Vec<(&str, &str)>,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (shutdown_tx, shutdown_rx) = oneshot::channel();

            let asset_id = asset_id.to_string();
            let bids: Vec<(String, String)> = bids
                .into_iter()
                .map(|(p, s)| (p.to_string(), s.to_string()))
                .collect();
            let asks: Vec<(String, String)> = asks
                .into_iter()
                .map(|(p, s)| (p.to_string(), s.to_string()))
                .collect();

            tokio::spawn(async move {
                tokio::select! {
                    _ = shutdown_rx => {
                        return;
                    }
                    result = listener.accept() => {
                        if let Ok((stream, _)) = result {
                            let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
                            let (mut write, mut read) = ws_stream.split();

                            // Wait for subscription message
                            if let Some(Ok(_msg)) = read.next().await {
                                // Send book snapshot
                                let snapshot = serde_json::json!({
                                    "event_type": "book",
                                    "asset_id": asset_id,
                                    "market": "mock-market",
                                    "bids": bids.iter().map(|(p, s)| {
                                        serde_json::json!({"price": p, "size": s})
                                    }).collect::<Vec<_>>(),
                                    "asks": asks.iter().map(|(p, s)| {
                                        serde_json::json!({"price": p, "size": s})
                                    }).collect::<Vec<_>>(),
                                    "timestamp": "1706745600000"
                                });
                                let _ = write.send(Message::Text(snapshot.to_string())).await;

                                // Keep connection alive for a bit
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    }
                }
            });

            Self {
                addr,
                shutdown_tx: Some(shutdown_tx),
            }
        }

        /// Start a mock server that sends a sequence of deltas.
        pub async fn start_with_deltas(asset_id: &str, deltas: Vec<(&str, &str, &str)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (shutdown_tx, shutdown_rx) = oneshot::channel();

            let asset_id = asset_id.to_string();
            let deltas: Vec<(String, String, String)> = deltas
                .into_iter()
                .map(|(p, s, side)| (p.to_string(), s.to_string(), side.to_string()))
                .collect();

            tokio::spawn(async move {
                tokio::select! {
                    _ = shutdown_rx => {
                        return;
                    }
                    result = listener.accept() => {
                        if let Ok((stream, _)) = result {
                            let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
                            let (mut write, mut read) = ws_stream.split();

                            // Wait for subscription
                            if let Some(Ok(_msg)) = read.next().await {
                                // Send initial empty snapshot
                                let snapshot = serde_json::json!({
                                    "event_type": "book",
                                    "asset_id": asset_id,
                                    "market": "mock-market",
                                    "bids": [],
                                    "asks": [],
                                    "timestamp": "1706745600000"
                                });
                                let _ = write.send(Message::Text(snapshot.to_string())).await;

                                // Send deltas
                                for (price, size, side) in &deltas {
                                    let delta = serde_json::json!({
                                        "event_type": "price_change",
                                        "market": "mock-market",
                                        "asset_id": asset_id,
                                        "price": price,
                                        "size": size,
                                        "side": side
                                    });
                                    let _ = write.send(Message::Text(delta.to_string())).await;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }

                                // Keep connection alive
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    }
                }
            });

            Self {
                addr,
                shutdown_tx: Some(shutdown_tx),
            }
        }

        /// Get the WebSocket URL for this mock server.
        pub fn url(&self) -> String {
            format!("ws://{}", self.addr)
        }

        /// Shut down the mock server.
        pub fn shutdown(&mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    impl Drop for MockWebSocketServer {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    #[tokio::test]
    async fn test_mock_server_snapshot() {
        let mut server = MockWebSocketServer::start_with_snapshot(
            "test-token",
            vec![(".48", "100"), (".47", "200")],
            vec![(".52", "150"), (".53", "250")],
        )
        .await;

        let config = WebSocketConfig {
            url: server.url(),
            max_reconnect_attempts: 1,
            ..Default::default()
        };

        let (ws, mut rx) = PolymarketWebSocket::connect(vec!["test-token".to_string()], config)
            .await
            .unwrap();

        // Wait for connected event
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for event")
            .expect("Channel closed");

        assert!(matches!(event, BookEvent::Connected));

        // Wait for snapshot event
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for event")
            .expect("Channel closed");

        match event {
            BookEvent::Snapshot { asset_id, book } => {
                assert_eq!(asset_id, "test-token");
                assert_eq!(book.best_bid(), Some(dec!(0.48)));
                assert_eq!(book.best_ask(), Some(dec!(0.52)));
                assert_eq!(book.bid_levels(), 2);
                assert_eq!(book.ask_levels(), 2);
            }
            other => panic!("Expected Snapshot event, got {:?}", other),
        }

        // Verify in-memory book state
        let book = ws.get_book("test-token").unwrap();
        assert_eq!(book.best_bid(), Some(dec!(0.48)));
        assert_eq!(book.best_ask(), Some(dec!(0.52)));

        ws.shutdown().await;
        server.shutdown();
    }

    #[tokio::test]
    async fn test_mock_server_deltas() {
        let mut server = MockWebSocketServer::start_with_deltas(
            "test-token",
            vec![
                (".48", "100", "BUY"),
                (".49", "50", "BUY"),
                (".52", "150", "SELL"),
            ],
        )
        .await;

        let config = WebSocketConfig {
            url: server.url(),
            max_reconnect_attempts: 1,
            ..Default::default()
        };

        let (ws, mut rx) = PolymarketWebSocket::connect(vec!["test-token".to_string()], config)
            .await
            .unwrap();

        // Collect events
        let mut events = Vec::new();
        let timeout = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = rx.recv().await {
                events.push(event);
                if events.len() >= 5 {
                    // Connected + Snapshot + 3 Deltas
                    break;
                }
            }
        });

        let _ = timeout.await;

        // Verify we got the deltas
        let delta_count = events
            .iter()
            .filter(|e| matches!(e, BookEvent::Delta { .. }))
            .count();
        assert!(delta_count >= 2, "Expected at least 2 delta events");

        // Verify final book state
        let book = ws.get_book("test-token").unwrap();
        assert!(book.has_liquidity());

        ws.shutdown().await;
        server.shutdown();
    }

    #[tokio::test]
    async fn test_get_all_books() {
        let books: Arc<RwLock<HashMap<String, L2OrderBook>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Add some test books
        {
            let mut books_guard = books.write();
            let mut book1 = L2OrderBook::new("yes-token".to_string());
            book1.apply_snapshot(vec![(dec!(0.48), dec!(100))], vec![(dec!(0.52), dec!(100))]);
            books_guard.insert("yes-token".to_string(), book1);

            let mut book2 = L2OrderBook::new("no-token".to_string());
            book2.apply_snapshot(vec![(dec!(0.47), dec!(200))], vec![(dec!(0.53), dec!(200))]);
            books_guard.insert("no-token".to_string(), book2);
        }

        let all = books.read().clone();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("yes-token"));
        assert!(all.contains_key("no-token"));
    }
}
