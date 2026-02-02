//! Kalshi exchange integration for statistical trading engine.
//!
//! This crate provides:
//! - REST client with rate limiting for Kalshi trading API
//! - RSA-PSS authentication for API requests
//! - Order execution with hard limits and circuit breaker
//! - Data models for markets, orders, and positions
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_kalshi::{KalshiClient, KalshiClientConfig, KalshiExecutor, KalshiExecutorConfig};
//! use algo_trade_kalshi::types::OrderRequest;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create client for demo environment
//!     let client = KalshiClient::demo()?;
//!
//!     // Discover BTC markets
//!     let markets = client.get_tradeable_btc_markets().await?;
//!     println!("Found {} tradeable BTC markets", markets.len());
//!
//!     // Create executor with safety limits
//!     let executor = KalshiExecutor::demo()?;
//!
//!     // Submit an order (validates against hard limits)
//!     let order = OrderRequest::buy_yes("KXBTC-26FEB02-B100000", 45, 10);
//!     let result = executor.execute_order(&order).await?;
//!     println!("Order {:?}: {:?}", result.order_id, result.status);
//!
//!     Ok(())
//! }
//! ```
//!
//! # Authentication
//!
//! Kalshi uses RSA-PSS (SHA-256) signatures for API authentication.
//! Set the following environment variables:
//!
//! - `KALSHI_API_KEY`: Your API key ID
//! - `KALSHI_PRIVATE_KEY`: Your RSA private key in PEM format
//!
//! For demo environment, use `KALSHI_DEMO_API_KEY` and `KALSHI_DEMO_PRIVATE_KEY`.
//!
//! # Safety Features
//!
//! The executor provides multiple layers of protection:
//!
//! - **Hard limits**: Maximum order size, price range, and order value
//! - **Daily volume tracking**: Prevents excessive trading
//! - **Balance reserve**: Keeps minimum balance for unwinding
//! - **Circuit breaker**: Halts trading after consecutive failures or losses
//!
//! # API Endpoints
//!
//! The client supports the following Kalshi API endpoints:
//!
//! - `GET /markets` - List markets
//! - `GET /markets/{ticker}` - Get specific market
//! - `GET /markets/{ticker}/orderbook` - Get orderbook
//! - `GET /portfolio/balance` - Get account balance
//! - `GET /portfolio/positions` - Get positions
//! - `POST /portfolio/orders` - Submit order
//! - `DELETE /portfolio/orders/{order_id}` - Cancel order
//! - `GET /portfolio/orders/{order_id}` - Get order status

pub mod auth;
pub mod client;
pub mod error;
pub mod executor;
pub mod types;

// Re-export main types for convenience
pub use auth::{KalshiAuth, KalshiAuthConfig, SignedHeaders};
pub use client::{KalshiClient, KalshiClientConfig, KALSHI_DEMO_URL, KALSHI_PROD_URL};
pub use error::{KalshiError, Result};
pub use executor::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerState, HardLimits, KalshiExecutor,
    KalshiExecutorConfig,
};
pub use types::{
    Action, Balance, Market, MarketStatus, Order, OrderRequest, OrderStatus, OrderType, Orderbook,
    Position, PriceLevel, Side,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_api_exports() {
        // Verify that all public types are accessible
        let _ = KalshiAuthConfig::default();
        let _ = KalshiClientConfig::default();
        let _ = KalshiExecutorConfig::default();
        let _ = HardLimits::default();
        let _ = CircuitBreakerConfig::default();
    }

    #[test]
    fn test_error_types_accessible() {
        let err = KalshiError::api(400, "bad request");
        assert!(err.to_string().contains("400"));
    }

    #[test]
    fn test_types_accessible() {
        let order = OrderRequest::buy_yes("KXBTC-TEST", 45, 100);
        assert_eq!(order.ticker, "KXBTC-TEST");
        assert_eq!(order.side, Side::Yes);
        assert_eq!(order.action, Action::Buy);
    }

    #[test]
    fn test_constants_accessible() {
        assert!(KALSHI_PROD_URL.starts_with("https://"));
        assert!(KALSHI_DEMO_URL.starts_with("https://"));
    }
}
