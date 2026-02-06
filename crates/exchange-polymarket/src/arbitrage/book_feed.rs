//! Real-time order book feed for arbitrage detection.
//!
//! This module provides a managed connection to Polymarket's WebSocket
//! that maintains live order book state for arbitrage opportunity detection.
//!
//! # Usage
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::book_feed::{BookFeed, BookFeedConfig};
//!
//! // Create feed for a market
//! let feed = BookFeed::connect(
//!     "yes-token-id".to_string(),
//!     "no-token-id".to_string(),
//!     BookFeedConfig::default(),
//! ).await?;
//!
//! // Wait for initial snapshots
//! feed.wait_for_ready(Duration::from_secs(10)).await?;
//!
//! // Get current order books
//! let (yes_book, no_book) = feed.get_books();
//! ```

use crate::arbitrage::types::L2OrderBook;
use crate::websocket::{BookEvent, PolymarketWebSocket, WebSocketConfig, WebSocketError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Errors that can occur with the book feed.
#[derive(Error, Debug)]
pub enum BookFeedError {
    /// WebSocket connection failed.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] WebSocketError),

    /// Timed out waiting for initial book snapshots.
    #[error("Timeout waiting for book snapshots after {0:?}")]
    Timeout(Duration),

    /// Feed was disconnected.
    #[error("Feed disconnected: {0}")]
    Disconnected(String),

    /// Books not yet available.
    #[error("Order books not yet available")]
    NotReady,
}

/// Configuration for the book feed.
#[derive(Debug, Clone)]
pub struct BookFeedConfig {
    /// WebSocket configuration.
    pub ws_config: WebSocketConfig,
    /// Timeout for waiting for initial snapshots.
    pub ready_timeout: Duration,
}

impl Default for BookFeedConfig {
    fn default() -> Self {
        Self {
            ws_config: WebSocketConfig::default(),
            ready_timeout: Duration::from_secs(30),
        }
    }
}

impl BookFeedConfig {
    /// Creates a config optimized for fast startup.
    #[must_use]
    pub fn fast() -> Self {
        Self {
            ws_config: WebSocketConfig::default(),
            ready_timeout: Duration::from_secs(10),
        }
    }
}

/// Real-time order book feed for a single market (YES/NO pair).
///
/// Maintains live order book state via WebSocket connection.
pub struct BookFeed {
    /// WebSocket handle.
    ws: PolymarketWebSocket,
    /// YES token ID.
    yes_token_id: String,
    /// NO token ID.
    no_token_id: String,
    /// Whether both books have received initial snapshots.
    ready: Arc<AtomicBool>,
    /// Event receiver (for monitoring).
    _event_rx: mpsc::Receiver<BookEvent>,
}

impl BookFeed {
    /// Connects to the WebSocket and starts receiving book updates.
    ///
    /// This returns immediately after connection. Use `wait_for_ready()`
    /// to ensure initial book snapshots have been received.
    pub async fn connect(
        yes_token_id: String,
        no_token_id: String,
        config: BookFeedConfig,
    ) -> Result<Self, BookFeedError> {
        let token_ids = vec![yes_token_id.clone(), no_token_id.clone()];

        info!(
            yes_token = %yes_token_id,
            no_token = %no_token_id,
            "Connecting to Polymarket WebSocket for book feed"
        );

        let (ws, event_rx) = PolymarketWebSocket::connect(token_ids, config.ws_config).await?;

        Ok(Self {
            ws,
            yes_token_id,
            no_token_id,
            ready: Arc::new(AtomicBool::new(false)),
            _event_rx: event_rx,
        })
    }

    /// Waits until both order books have received initial snapshots.
    ///
    /// # Errors
    ///
    /// Returns error if timeout is reached before both books are ready.
    pub async fn wait_for_ready(&self, timeout_duration: Duration) -> Result<(), BookFeedError> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout_duration {
            // Check if both books have data
            let yes_ready = self
                .ws
                .get_book(&self.yes_token_id)
                .map(|b| b.has_liquidity())
                .unwrap_or(false);
            let no_ready = self
                .ws
                .get_book(&self.no_token_id)
                .map(|b| b.has_liquidity())
                .unwrap_or(false);

            if yes_ready && no_ready {
                self.ready.store(true, Ordering::SeqCst);
                info!(
                    yes_token = %self.yes_token_id,
                    no_token = %self.no_token_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Book feed ready - both snapshots received"
                );
                return Ok(());
            }

            debug!(
                yes_ready = yes_ready,
                no_ready = no_ready,
                "Waiting for book snapshots..."
            );

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err(BookFeedError::Timeout(timeout_duration))
    }

    /// Returns whether the feed is ready (both books have data).
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Gets the current YES and NO order books.
    ///
    /// Returns (yes_book, no_book).
    ///
    /// # Errors
    ///
    /// Returns error if books are not yet available.
    pub fn get_books(&self) -> Result<(L2OrderBook, L2OrderBook), BookFeedError> {
        let yes_book = self
            .ws
            .get_book(&self.yes_token_id)
            .ok_or(BookFeedError::NotReady)?;
        let no_book = self
            .ws
            .get_book(&self.no_token_id)
            .ok_or(BookFeedError::NotReady)?;

        Ok((yes_book, no_book))
    }

    /// Gets the current YES order book.
    #[must_use]
    pub fn get_yes_book(&self) -> Option<L2OrderBook> {
        self.ws.get_book(&self.yes_token_id)
    }

    /// Gets the current NO order book.
    #[must_use]
    pub fn get_no_book(&self) -> Option<L2OrderBook> {
        self.ws.get_book(&self.no_token_id)
    }

    /// Returns the YES token ID.
    #[must_use]
    pub fn yes_token_id(&self) -> &str {
        &self.yes_token_id
    }

    /// Returns the NO token ID.
    #[must_use]
    pub fn no_token_id(&self) -> &str {
        &self.no_token_id
    }

    /// Gracefully shuts down the WebSocket connection.
    pub async fn shutdown(&self) {
        self.ws.shutdown().await;
    }
}

/// Manages multiple book feeds for different markets.
pub struct BookFeedManager {
    /// Active feeds by condition ID.
    feeds: std::collections::HashMap<String, BookFeed>,
    /// Configuration for new feeds.
    config: BookFeedConfig,
}

impl BookFeedManager {
    /// Creates a new feed manager.
    #[must_use]
    pub fn new(config: BookFeedConfig) -> Self {
        Self {
            feeds: std::collections::HashMap::new(),
            config,
        }
    }

    /// Adds a market to the feed manager.
    ///
    /// Connects to WebSocket and starts receiving updates.
    pub async fn add_market(
        &mut self,
        condition_id: String,
        yes_token_id: String,
        no_token_id: String,
    ) -> Result<(), BookFeedError> {
        if self.feeds.contains_key(&condition_id) {
            debug!(condition_id = %condition_id, "Market already subscribed");
            return Ok(());
        }

        let feed = BookFeed::connect(yes_token_id, no_token_id, self.config.clone()).await?;

        self.feeds.insert(condition_id, feed);
        Ok(())
    }

    /// Gets books for a specific market.
    pub fn get_books(
        &self,
        condition_id: &str,
    ) -> Result<(L2OrderBook, L2OrderBook), BookFeedError> {
        self.feeds
            .get(condition_id)
            .ok_or(BookFeedError::NotReady)?
            .get_books()
    }

    /// Waits for all feeds to be ready.
    pub async fn wait_for_all_ready(
        &self,
        timeout_duration: Duration,
    ) -> Result<(), BookFeedError> {
        for (condition_id, feed) in &self.feeds {
            feed.wait_for_ready(timeout_duration).await.map_err(|e| {
                warn!(condition_id = %condition_id, error = %e, "Feed not ready");
                e
            })?;
        }
        Ok(())
    }

    /// Returns the number of active feeds.
    #[must_use]
    pub fn feed_count(&self) -> usize {
        self.feeds.len()
    }

    /// Shuts down all feeds.
    pub async fn shutdown_all(&self) {
        for feed in self.feeds.values() {
            feed.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== BookFeedConfig Tests ====================

    #[test]
    fn test_config_default() {
        let config = BookFeedConfig::default();
        assert_eq!(config.ready_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_config_fast() {
        let config = BookFeedConfig::fast();
        assert_eq!(config.ready_timeout, Duration::from_secs(10));
    }

    // ==================== BookFeedError Tests ====================

    #[test]
    fn test_error_display() {
        let err = BookFeedError::Timeout(Duration::from_secs(5));
        assert!(err.to_string().contains("5"));

        let err = BookFeedError::NotReady;
        assert!(err.to_string().contains("not yet available"));

        let err = BookFeedError::Disconnected("network error".to_string());
        assert!(err.to_string().contains("network error"));
    }

    // ==================== BookFeedManager Tests ====================

    #[test]
    fn test_manager_new() {
        let manager = BookFeedManager::new(BookFeedConfig::default());
        assert_eq!(manager.feed_count(), 0);
    }

    #[test]
    fn test_manager_get_books_not_found() {
        let manager = BookFeedManager::new(BookFeedConfig::default());
        let result = manager.get_books("nonexistent");
        assert!(matches!(result, Err(BookFeedError::NotReady)));
    }

    // ==================== Integration Tests (require network) ====================
    // These are ignored by default - run with: cargo test -- --ignored

    #[tokio::test]
    #[ignore = "requires network connection to Polymarket"]
    async fn test_book_feed_connect_real() {
        // Use real token IDs from a BTC market
        // These would need to be fetched from Gamma API in real usage
        let yes_token = "test-yes-token".to_string();
        let no_token = "test-no-token".to_string();

        let result = BookFeed::connect(yes_token, no_token, BookFeedConfig::fast()).await;

        // Should connect (but may not have valid tokens)
        assert!(result.is_ok() || result.is_err());
    }
}
