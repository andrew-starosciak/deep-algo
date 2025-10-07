use algo_trade_backtest::SimulatedExecutionHandler;
use algo_trade_core::events::{FillEvent, OrderEvent};
use algo_trade_core::traits::ExecutionHandler;
use anyhow::Result;
use async_trait::async_trait;

/// Paper trading execution handler for testing with live data.
///
/// This handler wraps `SimulatedExecutionHandler` to provide paper trading functionality
/// when connected to live market data. It simulates order fills without making real API calls,
/// allowing safe testing of strategies with real-time market conditions.
///
/// # Safety
///
/// This handler makes **zero API calls** to the exchange. All fills are simulated locally.
/// It is impossible to execute real trades through this handler.
///
/// # Use Cases
///
/// - Test strategies with live market data before going live
/// - Validate bot configuration and setup
/// - Monitor strategy behavior in real-time without risk
/// - Debug signal generation and order logic
///
/// # Example
///
/// ```rust,no_run
/// use algo_trade_exchange_hyperliquid::PaperTradingExecutionHandler;
///
/// let handler = PaperTradingExecutionHandler::new(0.001, 10.0);
/// // Use with LiveDataProvider for paper trading with live data
/// ```
pub struct PaperTradingExecutionHandler {
    inner: SimulatedExecutionHandler,
}

impl PaperTradingExecutionHandler {
    /// Creates a new paper trading execution handler.
    ///
    /// # Arguments
    ///
    /// * `commission_rate` - Commission as a decimal (e.g., 0.001 = 0.1%)
    /// * `slippage_bps` - Slippage in basis points (e.g., 10.0 = 10 bps = 0.1%)
    ///
    /// # Recommended Values
    ///
    /// - Hyperliquid commission: 0.00025 (0.025% taker fee)
    /// - Conservative slippage: 10.0 bps (0.1%)
    ///
    /// # Example
    ///
    /// ```rust
    /// use algo_trade_exchange_hyperliquid::PaperTradingExecutionHandler;
    ///
    /// // Conservative defaults
    /// let handler = PaperTradingExecutionHandler::new(0.00025, 10.0);
    /// ```
    #[must_use]
    pub fn new(commission_rate: f64, slippage_bps: f64) -> Self {
        Self {
            inner: SimulatedExecutionHandler::new(commission_rate, slippage_bps),
        }
    }
}

#[async_trait]
impl ExecutionHandler for PaperTradingExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // Delegate to SimulatedExecutionHandler - zero API calls
        self.inner.execute_order(order).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_core::events::{OrderDirection, OrderType};
    use chrono::Utc;
    use rust_decimal::Decimal;

    #[tokio::test]
    async fn test_paper_trading_handler_buy_order() {
        let mut handler = PaperTradingExecutionHandler::new(0.001, 5.0);

        let order = OrderEvent {
            symbol: "BTC".to_string(),
            order_type: OrderType::Market,
            direction: OrderDirection::Buy,
            quantity: Decimal::from(1),
            price: Some(Decimal::from(50000)),
            timestamp: Utc::now(),
        };

        let fill = handler.execute_order(order).await.unwrap();

        assert_eq!(fill.symbol, "BTC");
        assert_eq!(fill.quantity, Decimal::from(1));
        assert!(fill.price > Decimal::from(50000)); // Slippage applied
    }

    #[tokio::test]
    async fn test_paper_trading_handler_sell_order() {
        let mut handler = PaperTradingExecutionHandler::new(0.001, 5.0);

        let order = OrderEvent {
            symbol: "ETH".to_string(),
            order_type: OrderType::Market,
            direction: OrderDirection::Sell,
            quantity: Decimal::from(10),
            price: Some(Decimal::from(3000)),
            timestamp: Utc::now(),
        };

        let fill = handler.execute_order(order).await.unwrap();

        assert_eq!(fill.symbol, "ETH");
        assert_eq!(fill.quantity, Decimal::from(10));
        assert!(fill.price < Decimal::from(3000)); // Slippage applied (negative for sell)
    }

    #[tokio::test]
    async fn test_commission_applied() {
        let mut handler = PaperTradingExecutionHandler::new(0.001, 0.0); // 0.1% commission, 0 slippage

        let order = OrderEvent {
            symbol: "BTC".to_string(),
            order_type: OrderType::Market,
            direction: OrderDirection::Buy,
            quantity: Decimal::from(1),
            price: Some(Decimal::from(50000)),
            timestamp: Utc::now(),
        };

        let fill = handler.execute_order(order).await.unwrap();

        // Commission = 50000 * 1 * 0.001 = 50
        assert_eq!(fill.commission, Decimal::from(50));
    }
}
