//! Paper trading execution handler for arbitrage testing.
//!
//! This module provides a simulated execution environment for testing the full
//! arbitrage pipeline without risking real funds. The `PaperExecutor` implements
//! the `PolymarketExecutor` trait with configurable fill simulation.
//!
//! # Features
//!
//! - Simulated balance tracking with realistic deductions
//! - Configurable fill rate for testing failure scenarios
//! - Partial fill simulation for FAK orders
//! - Complete order history for analysis
//! - Thread-safe position tracking
//! - Metrics integration for statistical validation
//!
//! # Example
//!
//! ```
//! use algo_trade_polymarket::arbitrage::paper_executor::{PaperExecutor, PaperExecutorConfig};
//! use algo_trade_polymarket::arbitrage::PolymarketExecutor;
//! use rust_decimal_macros::dec;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create executor with default config ($1000 balance, 85% fill rate)
//!     let executor = PaperExecutor::new(PaperExecutorConfig::default());
//!     let balance = executor.get_balance().await.unwrap();
//!     println!("Paper trading with {} balance", balance);
//! }
//! ```

use async_trait::async_trait;
use parking_lot::RwLock;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use super::execution::{
    ExecutionError, OrderParams, OrderResult, OrderStatus, PolymarketExecutor, Position,
};
use super::metrics::ArbitrageMetrics;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for paper trading executor.
///
/// Controls the simulation behavior including fill rates, latency, and initial conditions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperExecutorConfig {
    /// Initial USDC balance for paper trading.
    pub initial_balance: Decimal,

    /// Probability that an order will fill (0.0 to 1.0).
    /// Default: 0.85 (85% fill rate)
    pub fill_rate: f64,

    /// Probability of partial fills for FAK orders (0.0 to 1.0).
    /// When a fill occurs, this is the chance it will be partial.
    /// Default: 0.10 (10% partial fill rate)
    pub partial_fill_rate: f64,

    /// Simulated latency in milliseconds.
    /// When > 0, orders will sleep for this duration before returning.
    /// Default: 0 (no latency)
    pub simulate_latency_ms: u64,

    /// Minimum partial fill percentage when partial fills occur.
    /// Default: 0.25 (at least 25% fills)
    pub min_partial_fill_pct: f64,

    /// Maximum partial fill percentage when partial fills occur.
    /// Default: 0.95 (at most 95% fills)
    pub max_partial_fill_pct: f64,

    /// Optional random seed for reproducible testing.
    /// Default: None (uses system entropy)
    pub random_seed: Option<u64>,
}

impl Default for PaperExecutorConfig {
    fn default() -> Self {
        Self {
            initial_balance: dec!(1000),
            fill_rate: 0.85,
            partial_fill_rate: 0.10,
            simulate_latency_ms: 0,
            min_partial_fill_pct: 0.25,
            max_partial_fill_pct: 0.95,
            random_seed: None,
        }
    }
}

impl PaperExecutorConfig {
    /// Creates a new config with the specified initial balance.
    #[must_use]
    pub fn with_balance(initial_balance: Decimal) -> Self {
        Self {
            initial_balance,
            ..Default::default()
        }
    }

    /// Creates a config that always fills orders (for deterministic testing).
    #[must_use]
    pub fn always_fill() -> Self {
        Self {
            fill_rate: 1.0,
            partial_fill_rate: 0.0,
            ..Default::default()
        }
    }

    /// Creates a config that never fills orders (for failure testing).
    #[must_use]
    pub fn never_fill() -> Self {
        Self {
            fill_rate: 0.0,
            ..Default::default()
        }
    }

    /// Creates a config with a specific random seed for reproducible tests.
    #[must_use]
    pub fn with_seed(seed: u64) -> Self {
        Self {
            random_seed: Some(seed),
            ..Default::default()
        }
    }

    /// Sets the fill rate.
    #[must_use]
    pub fn fill_rate(mut self, rate: f64) -> Self {
        self.fill_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Sets the partial fill rate.
    #[must_use]
    pub fn partial_fill_rate(mut self, rate: f64) -> Self {
        self.partial_fill_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Sets the simulated latency.
    #[must_use]
    pub fn latency_ms(mut self, ms: u64) -> Self {
        self.simulate_latency_ms = ms;
        self
    }
}

// =============================================================================
// Internal State
// =============================================================================

/// Internal state for the paper executor.
#[derive(Debug)]
struct PaperState {
    /// Current USDC balance.
    balance: Decimal,

    /// Simulated positions by token ID.
    positions: HashMap<String, Position>,

    /// Order history by order ID.
    orders: HashMap<String, OrderResult>,

    /// Order parameters by order ID (for reference).
    order_params: HashMap<String, OrderParams>,

    /// Total invested amount (for metrics).
    total_invested: Decimal,

    /// Total number of orders submitted.
    orders_submitted: u32,

    /// Total number of successful fills.
    successful_fills: u32,

    /// Total number of rejected orders.
    rejected_orders: u32,
}

impl PaperState {
    fn new(initial_balance: Decimal) -> Self {
        Self {
            balance: initial_balance,
            positions: HashMap::new(),
            orders: HashMap::new(),
            order_params: HashMap::new(),
            total_invested: Decimal::ZERO,
            orders_submitted: 0,
            successful_fills: 0,
            rejected_orders: 0,
        }
    }
}

// =============================================================================
// Paper Executor
// =============================================================================

/// Paper trading executor for testing arbitrage strategies.
///
/// Implements `PolymarketExecutor` with simulated fills based on configurable
/// probabilities. Tracks all orders, positions, and balances for analysis.
///
/// # Thread Safety
///
/// The executor is thread-safe and can be shared across multiple tasks.
/// Internal state is protected by a read-write lock.
pub struct PaperExecutor {
    /// Configuration.
    config: PaperExecutorConfig,

    /// Internal state.
    state: Arc<RwLock<PaperState>>,

    /// Random number generator (protected for thread safety).
    rng: Arc<RwLock<StdRng>>,

    /// Optional metrics reference for tracking.
    metrics: Option<Arc<RwLock<ArbitrageMetrics>>>,
}

impl std::fmt::Debug for PaperExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaperExecutor")
            .field("config", &self.config)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PaperExecutor {
    /// Creates a new paper executor with the given configuration.
    #[must_use]
    pub fn new(config: PaperExecutorConfig) -> Self {
        let rng = match config.random_seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_entropy(),
        };

        Self {
            state: Arc::new(RwLock::new(PaperState::new(config.initial_balance))),
            rng: Arc::new(RwLock::new(rng)),
            config,
            metrics: None,
        }
    }

    /// Creates a new paper executor with default configuration.
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(PaperExecutorConfig::default())
    }

    /// Attaches a metrics tracker for recording execution statistics.
    pub fn with_metrics(mut self, metrics: Arc<RwLock<ArbitrageMetrics>>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Returns the current configuration.
    #[must_use]
    pub fn config(&self) -> &PaperExecutorConfig {
        &self.config
    }

    /// Returns the total number of orders submitted.
    #[must_use]
    pub fn orders_submitted(&self) -> u32 {
        self.state.read().orders_submitted
    }

    /// Returns the total number of successful fills.
    #[must_use]
    pub fn successful_fills(&self) -> u32 {
        self.state.read().successful_fills
    }

    /// Returns the total number of rejected orders.
    #[must_use]
    pub fn rejected_orders(&self) -> u32 {
        self.state.read().rejected_orders
    }

    /// Returns the actual fill rate (successful / submitted).
    #[must_use]
    pub fn actual_fill_rate(&self) -> f64 {
        let state = self.state.read();
        if state.orders_submitted == 0 {
            return 0.0;
        }
        state.successful_fills as f64 / state.orders_submitted as f64
    }

    /// Returns the total invested amount.
    #[must_use]
    pub fn total_invested(&self) -> Decimal {
        self.state.read().total_invested
    }

    /// Returns a copy of all order results.
    #[must_use]
    pub fn order_history(&self) -> Vec<OrderResult> {
        self.state.read().orders.values().cloned().collect()
    }

    /// Resets the executor to initial state.
    pub fn reset(&self) {
        let mut state = self.state.write();
        *state = PaperState::new(self.config.initial_balance);
    }

    /// Resets with a new balance.
    pub fn reset_with_balance(&self, balance: Decimal) {
        let mut state = self.state.write();
        *state = PaperState::new(balance);
    }

    /// Simulates whether an order should fill based on configured probability.
    fn should_fill(&self) -> bool {
        let mut rng = self.rng.write();
        rng.gen::<f64>() < self.config.fill_rate
    }

    /// Simulates whether a fill should be partial.
    fn should_partial_fill(&self) -> bool {
        let mut rng = self.rng.write();
        rng.gen::<f64>() < self.config.partial_fill_rate
    }

    /// Generates a partial fill percentage.
    fn partial_fill_percentage(&self) -> f64 {
        let mut rng = self.rng.write();
        rng.gen_range(self.config.min_partial_fill_pct..=self.config.max_partial_fill_pct)
    }

    /// Generates a new order ID.
    fn generate_order_id(&self) -> String {
        format!("paper-{}", Uuid::new_v4())
    }

    /// Simulates latency if configured.
    async fn simulate_latency(&self) {
        if self.config.simulate_latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.simulate_latency_ms)).await;
        }
    }

    /// Processes a single order and returns the result.
    fn process_order(&self, order: &OrderParams) -> OrderResult {
        let order_id = self.generate_order_id();
        let mut state = self.state.write();

        state.orders_submitted += 1;

        // Check balance for buy orders
        if order.side == super::execution::Side::Buy {
            let required = order.notional_value();
            if required > state.balance {
                state.rejected_orders += 1;
                let result = OrderResult::rejected(&order_id, "Insufficient balance");
                state.orders.insert(order_id.clone(), result.clone());
                state.order_params.insert(order_id, order.clone());
                return result;
            }
        }

        // Simulate fill decision
        if !self.should_fill() {
            state.rejected_orders += 1;
            let result = OrderResult {
                order_id: order_id.clone(),
                status: OrderStatus::Expired,
                filled_size: Decimal::ZERO,
                avg_fill_price: None,
                error: Some("No fill - simulated market conditions".to_string()),
                latency_ms: None,
            };
            state.orders.insert(order_id.clone(), result.clone());
            state.order_params.insert(order_id, order.clone());
            return result;
        }

        // Determine fill size
        let fill_size = match order.order_type {
            super::execution::OrderType::Fok => {
                // FOK: all or nothing
                order.size
            }
            super::execution::OrderType::Fak | super::execution::OrderType::Gtc => {
                // FAK/GTC: potentially partial
                if self.should_partial_fill() {
                    let pct = self.partial_fill_percentage();
                    let pct_decimal = Decimal::from_f64_retain(pct).unwrap_or(dec!(0.5));
                    (order.size * pct_decimal).round_dp(2)
                } else {
                    order.size
                }
            }
        };

        // Calculate cost and update state
        let fill_cost = fill_size * order.price;

        if order.side == super::execution::Side::Buy {
            state.balance -= fill_cost;
            state.total_invested += fill_cost;

            // Update position
            let position = state
                .positions
                .entry(order.token_id.clone())
                .or_insert_with(|| Position::new(&order.token_id, Decimal::ZERO, Decimal::ZERO));

            // Calculate new average price
            let old_cost = position.size * position.avg_price;
            let new_total_size = position.size + fill_size;
            let new_avg_price = if new_total_size > Decimal::ZERO {
                (old_cost + fill_cost) / new_total_size
            } else {
                order.price
            };

            position.size = new_total_size;
            position.avg_price = new_avg_price;
        } else {
            // Sell - add proceeds to balance
            state.balance += fill_cost;

            // Update position
            if let Some(position) = state.positions.get_mut(&order.token_id) {
                position.size -= fill_size;
                if position.size <= Decimal::ZERO {
                    state.positions.remove(&order.token_id);
                }
            }
        }

        state.successful_fills += 1;

        let status = if fill_size == order.size {
            OrderStatus::Filled
        } else {
            OrderStatus::PartiallyFilled
        };

        let result = OrderResult {
            order_id: order_id.clone(),
            status,
            filled_size: fill_size,
            avg_fill_price: Some(order.price),
            error: None,
            latency_ms: None,
        };

        state.orders.insert(order_id.clone(), result.clone());
        state.order_params.insert(order_id, order.clone());

        result
    }

    /// Records execution metrics if a metrics tracker is attached.
    fn record_metrics(&self, success: bool, partial: bool) {
        if let Some(ref metrics) = self.metrics {
            let mut m = metrics.write();
            m.record_execution(success, partial);
        }
    }
}

// =============================================================================
// Trait Implementation
// =============================================================================

#[async_trait]
impl PolymarketExecutor for PaperExecutor {
    async fn submit_order(&self, order: OrderParams) -> Result<OrderResult, ExecutionError> {
        self.simulate_latency().await;

        let result = self.process_order(&order);

        // Record metrics
        let success = result.status == OrderStatus::Filled;
        let partial = result.status == OrderStatus::PartiallyFilled;
        self.record_metrics(success, partial);

        Ok(result)
    }

    async fn submit_orders_batch(
        &self,
        orders: Vec<OrderParams>,
    ) -> Result<Vec<OrderResult>, ExecutionError> {
        self.simulate_latency().await;

        let results: Vec<OrderResult> = orders
            .iter()
            .map(|order| {
                let result = self.process_order(order);

                // Record metrics for each order
                let success = result.status == OrderStatus::Filled;
                let partial = result.status == OrderStatus::PartiallyFilled;
                self.record_metrics(success, partial);

                result
            })
            .collect();

        Ok(results)
    }

    async fn cancel_all_orders(&self) -> Result<u32, ExecutionError> {
        Ok(0) // Paper executor has no resting orders
    }

    async fn cancel_order(&self, order_id: &str) -> Result<(), ExecutionError> {
        let mut state = self.state.write();

        if let Some(order) = state.orders.get_mut(order_id) {
            if order.status.is_terminal() {
                return Err(ExecutionError::rejected(format!(
                    "Cannot cancel order in terminal state: {}",
                    order.status
                )));
            }

            order.status = OrderStatus::Cancelled;
            Ok(())
        } else {
            Err(ExecutionError::Api(format!("Order not found: {order_id}")))
        }
    }

    async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ExecutionError> {
        let state = self.state.read();

        state
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| ExecutionError::Api(format!("Order not found: {order_id}")))
    }

    async fn wait_for_terminal(
        &self,
        order_id: &str,
        _timeout: Duration,
    ) -> Result<OrderResult, ExecutionError> {
        // In paper trading, orders are immediately terminal
        self.get_order_status(order_id).await
    }

    async fn get_positions(&self) -> Result<Vec<Position>, ExecutionError> {
        let state = self.state.read();
        Ok(state.positions.values().cloned().collect())
    }

    async fn get_balance(&self) -> Result<Decimal, ExecutionError> {
        let state = self.state.read();
        Ok(state.balance)
    }

    async fn credit_balance(&self, amount: Decimal) -> Result<(), ExecutionError> {
        let mut state = self.state.write();
        state.balance += amount;
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ==================== Configuration Tests ====================

    #[test]
    fn test_config_default() {
        let config = PaperExecutorConfig::default();

        assert_eq!(config.initial_balance, dec!(1000));
        assert!((config.fill_rate - 0.85).abs() < f64::EPSILON);
        assert!((config.partial_fill_rate - 0.10).abs() < f64::EPSILON);
        assert_eq!(config.simulate_latency_ms, 0);
        assert!(config.random_seed.is_none());
    }

    #[test]
    fn test_config_with_balance() {
        let config = PaperExecutorConfig::with_balance(dec!(5000));
        assert_eq!(config.initial_balance, dec!(5000));
    }

    #[test]
    fn test_config_always_fill() {
        let config = PaperExecutorConfig::always_fill();
        assert!((config.fill_rate - 1.0).abs() < f64::EPSILON);
        assert!((config.partial_fill_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_config_never_fill() {
        let config = PaperExecutorConfig::never_fill();
        assert!((config.fill_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_config_with_seed() {
        let config = PaperExecutorConfig::with_seed(12345);
        assert_eq!(config.random_seed, Some(12345));
    }

    #[test]
    fn test_config_builder_methods() {
        let config = PaperExecutorConfig::default()
            .fill_rate(0.90)
            .partial_fill_rate(0.15)
            .latency_ms(100);

        assert!((config.fill_rate - 0.90).abs() < f64::EPSILON);
        assert!((config.partial_fill_rate - 0.15).abs() < f64::EPSILON);
        assert_eq!(config.simulate_latency_ms, 100);
    }

    #[test]
    fn test_config_fill_rate_clamps() {
        let config = PaperExecutorConfig::default().fill_rate(1.5);
        assert!((config.fill_rate - 1.0).abs() < f64::EPSILON);

        let config2 = PaperExecutorConfig::default().fill_rate(-0.5);
        assert!((config2.fill_rate - 0.0).abs() < f64::EPSILON);
    }

    // ==================== Executor Creation Tests ====================

    #[test]
    fn test_executor_new() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        assert_eq!(executor.orders_submitted(), 0);
        assert_eq!(executor.successful_fills(), 0);
        assert_eq!(executor.rejected_orders(), 0);
    }

    #[test]
    fn test_executor_default_config() {
        let executor = PaperExecutor::default_config();
        assert_eq!(executor.config().initial_balance, dec!(1000));
    }

    // ==================== Order Submission Tests ====================

    #[tokio::test]
    async fn test_submit_order_always_fill() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_size, dec!(100));
        assert_eq!(result.avg_fill_price, Some(dec!(0.45)));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_submit_order_never_fill() {
        let executor = PaperExecutor::new(PaperExecutorConfig::never_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        assert_eq!(result.status, OrderStatus::Expired);
        assert_eq!(result.filled_size, Decimal::ZERO);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_submit_order_insufficient_balance() {
        let config = PaperExecutorConfig::always_fill();
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(10),
            ..config
        });

        // Try to buy 100 shares at 0.45 = $45 cost, but only have $10
        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        assert_eq!(result.status, OrderStatus::Rejected);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Insufficient balance"));
    }

    #[tokio::test]
    async fn test_submit_order_updates_balance() {
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(1000),
            ..PaperExecutorConfig::always_fill()
        });

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();

        // Balance should be 1000 - (100 * 0.45) = 1000 - 45 = 955
        let balance = executor.get_balance().await.unwrap();
        assert_eq!(balance, dec!(955));
    }

    #[tokio::test]
    async fn test_submit_order_updates_position() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();

        let positions = executor.get_positions().await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].token_id, "token-yes");
        assert_eq!(positions[0].size, dec!(100));
        assert_eq!(positions[0].avg_price, dec!(0.45));
    }

    #[tokio::test]
    async fn test_submit_order_increments_counters() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();

        assert_eq!(executor.orders_submitted(), 1);
        assert_eq!(executor.successful_fills(), 1);
        assert_eq!(executor.rejected_orders(), 0);
    }

    // ==================== Batch Order Tests ====================

    #[tokio::test]
    async fn test_submit_orders_batch() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let orders = vec![
            OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100)),
            OrderParams::buy_fok("token-no", dec!(0.52), dec!(100)),
        ];

        let results = executor.submit_orders_batch(orders).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, OrderStatus::Filled);
        assert_eq!(results[1].status, OrderStatus::Filled);
        assert_eq!(executor.orders_submitted(), 2);
        assert_eq!(executor.successful_fills(), 2);
    }

    #[tokio::test]
    async fn test_submit_orders_batch_partial_failure() {
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(50), // Only enough for one order
            ..PaperExecutorConfig::always_fill()
        });

        let orders = vec![
            OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100)), // $45 - will fill
            OrderParams::buy_fok("token-no", dec!(0.52), dec!(100)),  // $52 - will reject
        ];

        let results = executor.submit_orders_batch(orders).await.unwrap();

        assert_eq!(results[0].status, OrderStatus::Filled);
        assert_eq!(results[1].status, OrderStatus::Rejected);
    }

    // ==================== Cancel Order Tests ====================

    #[tokio::test]
    async fn test_cancel_order_not_found() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        let result = executor.cancel_order("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_order_already_filled() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        let cancel_result = executor.cancel_order(&result.order_id).await;
        assert!(cancel_result.is_err());
    }

    // ==================== Get Order Status Tests ====================

    #[tokio::test]
    async fn test_get_order_status_exists() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let submitted = executor.submit_order(order).await.unwrap();

        let status = executor
            .get_order_status(&submitted.order_id)
            .await
            .unwrap();
        assert_eq!(status.order_id, submitted.order_id);
        assert_eq!(status.status, OrderStatus::Filled);
    }

    #[tokio::test]
    async fn test_get_order_status_not_found() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        let result = executor.get_order_status("nonexistent").await;
        assert!(result.is_err());
    }

    // ==================== Wait for Terminal Tests ====================

    #[tokio::test]
    async fn test_wait_for_terminal() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let submitted = executor.submit_order(order).await.unwrap();

        let result = executor
            .wait_for_terminal(&submitted.order_id, Duration::from_secs(5))
            .await
            .unwrap();

        assert!(result.is_terminal());
    }

    // ==================== Position Tracking Tests ====================

    #[tokio::test]
    async fn test_position_accumulation() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        // Buy 100 at 0.45
        let order1 = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order1).await.unwrap();

        // Buy 100 more at 0.50
        let order2 = OrderParams::buy_fok("token-yes", dec!(0.50), dec!(100));
        executor.submit_order(order2).await.unwrap();

        let positions = executor.get_positions().await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].size, dec!(200));

        // Average price should be (100*0.45 + 100*0.50) / 200 = 95 / 200 = 0.475
        assert_eq!(positions[0].avg_price, dec!(0.475));
    }

    #[tokio::test]
    async fn test_position_reduction_on_sell() {
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(1000),
            ..PaperExecutorConfig::always_fill()
        });

        // Buy 100
        let buy = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(buy).await.unwrap();

        // Sell 50
        let sell = OrderParams::sell_fak("token-yes", dec!(0.50), dec!(50));
        executor.submit_order(sell).await.unwrap();

        let positions = executor.get_positions().await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].size, dec!(50));

        // Balance should be: 1000 - 45 (buy cost) + 25 (sell proceeds) = 980
        let balance = executor.get_balance().await.unwrap();
        assert_eq!(balance, dec!(980));
    }

    #[tokio::test]
    async fn test_position_removal_on_full_sell() {
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(1000),
            ..PaperExecutorConfig::always_fill()
        });

        // Buy 100
        let buy = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(buy).await.unwrap();

        // Sell all 100
        let sell = OrderParams::sell_fak("token-yes", dec!(0.50), dec!(100));
        executor.submit_order(sell).await.unwrap();

        let positions = executor.get_positions().await.unwrap();
        assert!(positions.is_empty());
    }

    // ==================== Metrics Integration Tests ====================

    #[tokio::test]
    async fn test_metrics_integration() {
        let metrics = Arc::new(RwLock::new(ArbitrageMetrics::new()));
        let executor =
            PaperExecutor::new(PaperExecutorConfig::always_fill()).with_metrics(metrics.clone());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();

        let m = metrics.read();
        assert_eq!(m.attempts, 1);
        assert_eq!(m.successful_pairs, 1);
    }

    // ==================== Reproducibility Tests ====================

    #[tokio::test]
    async fn test_seeded_reproducibility() {
        let config = PaperExecutorConfig {
            initial_balance: dec!(10000),
            fill_rate: 0.5,
            random_seed: Some(42),
            ..Default::default()
        };

        let executor1 = PaperExecutor::new(config.clone());
        let executor2 = PaperExecutor::new(config);

        // Submit same orders to both
        for i in 0..10 {
            let order = OrderParams::buy_fok(format!("token-{i}"), dec!(0.45), dec!(10));
            executor1.submit_order(order.clone()).await.unwrap();
            executor2.submit_order(order).await.unwrap();
        }

        // Should have same results
        assert_eq!(executor1.successful_fills(), executor2.successful_fills());
        assert_eq!(executor1.rejected_orders(), executor2.rejected_orders());
    }

    // ==================== Reset Tests ====================

    #[tokio::test]
    async fn test_reset() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();

        assert_eq!(executor.orders_submitted(), 1);
        assert!(executor.get_balance().await.unwrap() < dec!(1000));

        executor.reset();

        assert_eq!(executor.orders_submitted(), 0);
        assert_eq!(executor.get_balance().await.unwrap(), dec!(1000));
        assert!(executor.get_positions().await.unwrap().is_empty());
        assert!(executor.order_history().is_empty());
    }

    #[tokio::test]
    async fn test_reset_with_balance() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        executor.reset_with_balance(dec!(5000));

        assert_eq!(executor.get_balance().await.unwrap(), dec!(5000));
    }

    // ==================== Fill Rate Tests ====================

    #[tokio::test]
    async fn test_actual_fill_rate() {
        // Use a seeded RNG with a specific fill rate
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(10000),
            fill_rate: 0.5,
            random_seed: Some(12345),
            ..Default::default()
        });

        for i in 0..100 {
            let order = OrderParams::buy_fok(format!("token-{i}"), dec!(0.01), dec!(1));
            let _ = executor.submit_order(order).await;
        }

        // With 100 samples and 50% fill rate, actual should be close to 0.5
        let actual = executor.actual_fill_rate();
        assert!(
            actual > 0.3 && actual < 0.7,
            "Actual fill rate {} should be near 0.5",
            actual
        );
    }

    // ==================== Order History Tests ====================

    #[tokio::test]
    async fn test_order_history() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        for i in 0..5 {
            let order = OrderParams::buy_fok(format!("token-{i}"), dec!(0.10), dec!(10));
            executor.submit_order(order).await.unwrap();
        }

        let history = executor.order_history();
        assert_eq!(history.len(), 5);
    }

    // ==================== Total Invested Tests ====================

    #[tokio::test]
    async fn test_total_invested() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        // Buy 100 at 0.45 = $45
        let order1 = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order1).await.unwrap();

        // Buy 100 at 0.50 = $50
        let order2 = OrderParams::buy_fok("token-no", dec!(0.50), dec!(100));
        executor.submit_order(order2).await.unwrap();

        assert_eq!(executor.total_invested(), dec!(95));
    }

    // ==================== Partial Fill Tests ====================

    #[tokio::test]
    async fn test_partial_fill_fak_order() {
        // Configure for guaranteed partial fills on FAK orders
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            partial_fill_rate: 1.0, // Always partial fill
            min_partial_fill_pct: 0.50,
            max_partial_fill_pct: 0.50, // Always 50%
            random_seed: Some(42),
            ..Default::default()
        });

        let order = OrderParams::sell_fak("token-yes", dec!(0.50), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        assert_eq!(result.status, OrderStatus::PartiallyFilled);
        assert_eq!(result.filled_size, dec!(50));
    }

    #[tokio::test]
    async fn test_fok_no_partial_fill() {
        // FOK orders should never partially fill
        let executor = PaperExecutor::new(PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            partial_fill_rate: 1.0, // Would cause partial fill
            random_seed: Some(42),
            ..Default::default()
        });

        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await.unwrap();

        // FOK should be fully filled, not partial
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_size, dec!(100));
    }

    // ==================== Latency Simulation Tests ====================

    #[tokio::test]
    async fn test_latency_simulation() {
        let executor = PaperExecutor::new(PaperExecutorConfig {
            simulate_latency_ms: 50,
            ..PaperExecutorConfig::always_fill()
        });

        let start = std::time::Instant::now();
        let order = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        executor.submit_order(order).await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(50),
            "Expected at least 50ms latency, got {:?}",
            elapsed
        );
    }

    // ==================== Multiple Token Tests ====================

    #[tokio::test]
    async fn test_multiple_token_positions() {
        let executor = PaperExecutor::new(PaperExecutorConfig::always_fill());

        let order1 = OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100));
        let order2 = OrderParams::buy_fok("token-no", dec!(0.52), dec!(100));

        executor.submit_order(order1).await.unwrap();
        executor.submit_order(order2).await.unwrap();

        let positions = executor.get_positions().await.unwrap();
        assert_eq!(positions.len(), 2);

        let yes_pos = positions
            .iter()
            .find(|p| p.token_id == "token-yes")
            .unwrap();
        let no_pos = positions.iter().find(|p| p.token_id == "token-no").unwrap();

        assert_eq!(yes_pos.size, dec!(100));
        assert_eq!(no_pos.size, dec!(100));
    }
}
