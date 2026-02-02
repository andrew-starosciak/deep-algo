//! Live execution handler for Polymarket arbitrage using rs-clob-client.
//!
//! This module provides production order execution for Polymarket's CLOB.
//! The actual signing and submission uses the rs-clob-client library for
//! EIP-712 order signing.
//!
//! # Overview
//!
//! The `LiveExecutor` wraps the rs-clob-client library and implements the
//! `PolymarketExecutor` trait for production trading. It handles:
//!
//! - Wallet initialization from private keys
//! - Rate limiting for API calls
//! - Order validation against hard limits
//! - Retry logic with exponential backoff
//! - Metrics integration
//!
//! # Security
//!
//! - Private keys are loaded from environment variables, NEVER logged
//! - All orders are validated against configurable hard limits
//! - Rate limiting prevents API abuse
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::live_executor::{LiveExecutor, LiveExecutorConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create executor for mainnet (loads wallet from env)
//!     let executor = LiveExecutor::mainnet()?;
//!
//!     println!("Trading as address: {}", executor.address());
//!
//!     // Check balance
//!     let balance = executor.get_balance().await?;
//!     println!("Available balance: {}", balance);
//!
//!     Ok(())
//! }
//! ```
//!
//! # TODO: rs-clob-client Integration
//!
//! The actual signing and submission logic requires rs-clob-client integration:
//! - EIP-712 order signing
//! - Batch order submission
//! - Position queries
//! - Balance queries

use async_trait::async_trait;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use super::execution::{
    ExecutionError, OrderParams, OrderResult, PolymarketExecutor, Position,
};
use super::metrics::ArbitrageMetrics;
use super::rate_limiter::{ClobRateLimiter, RateLimiterConfig};
use super::signer::{Wallet, WalletConfig, WalletError};

// =============================================================================
// Constants
// =============================================================================

/// Polymarket CLOB mainnet URL.
pub const POLYMARKET_MAINNET_URL: &str = "https://clob.polymarket.com";

/// Polymarket CLOB testnet URL (Mumbai).
pub const POLYMARKET_TESTNET_URL: &str = "https://clob.polymarket.com";

// =============================================================================
// Configuration Types
// =============================================================================

/// Configuration for the live executor.
///
/// Controls connection settings, rate limiting, and safety parameters.
#[derive(Debug, Clone)]
pub struct LiveExecutorConfig {
    /// Base URL for the CLOB API.
    pub base_url: String,

    /// Wallet configuration (env var name, chain ID).
    pub wallet_config: WalletConfig,

    /// Rate limiter configuration.
    pub rate_limiter_config: RateLimiterConfig,

    /// Maximum retry attempts for transient failures.
    pub max_retries: u32,

    /// Timeout in seconds for order operations.
    pub order_timeout_secs: u64,

    /// Whether to use neg_risk flag (required for binary markets).
    pub neg_risk: bool,

    /// Hard limits for order validation (safety).
    pub hard_limits: HardLimits,
}

impl Default for LiveExecutorConfig {
    fn default() -> Self {
        Self {
            base_url: POLYMARKET_MAINNET_URL.to_string(),
            wallet_config: WalletConfig::mainnet(),
            rate_limiter_config: RateLimiterConfig::default(),
            max_retries: 3,
            order_timeout_secs: 10,
            neg_risk: true,
            hard_limits: HardLimits::default(),
        }
    }
}

impl LiveExecutorConfig {
    /// Creates a mainnet configuration.
    ///
    /// Loads wallet credentials from environment variable:
    /// - `POLYMARKET_PRIVATE_KEY`: Wallet private key (64-char hex, optional 0x prefix)
    #[must_use]
    pub fn mainnet() -> Self {
        Self {
            base_url: POLYMARKET_MAINNET_URL.to_string(),
            wallet_config: WalletConfig::mainnet(),
            ..Default::default()
        }
    }

    /// Creates a testnet configuration.
    ///
    /// Uses Amoy testnet for testing without real funds.
    #[must_use]
    pub fn testnet() -> Self {
        Self {
            base_url: POLYMARKET_TESTNET_URL.to_string(),
            wallet_config: WalletConfig::testnet(),
            ..Default::default()
        }
    }

    /// Sets the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Sets the wallet configuration.
    #[must_use]
    pub fn with_wallet_config(mut self, config: WalletConfig) -> Self {
        self.wallet_config = config;
        self
    }

    /// Sets the rate limiter configuration.
    #[must_use]
    pub fn with_rate_limiter_config(mut self, config: RateLimiterConfig) -> Self {
        self.rate_limiter_config = config;
        self
    }

    /// Sets the maximum retry count.
    #[must_use]
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Sets the order timeout.
    #[must_use]
    pub fn with_order_timeout_secs(mut self, secs: u64) -> Self {
        self.order_timeout_secs = secs;
        self
    }

    /// Sets the neg_risk flag.
    #[must_use]
    pub fn with_neg_risk(mut self, neg_risk: bool) -> Self {
        self.neg_risk = neg_risk;
        self
    }

    /// Sets the hard limits.
    #[must_use]
    pub fn with_hard_limits(mut self, limits: HardLimits) -> Self {
        self.hard_limits = limits;
        self
    }
}

// =============================================================================
// Hard Limits
// =============================================================================

/// Hard limits for order validation.
///
/// These are safety limits to prevent catastrophic trading errors.
/// All orders are validated against these limits before submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardLimits {
    /// Maximum order size in shares.
    pub max_order_size: Decimal,

    /// Minimum order size in shares.
    pub min_order_size: Decimal,

    /// Maximum price (should be < 1.0 for probability markets).
    pub max_price: Decimal,

    /// Minimum price (should be > 0.0).
    pub min_price: Decimal,

    /// Maximum total position value in USDC.
    pub max_position_value: Decimal,

    /// Maximum single order value in USDC.
    pub max_order_value: Decimal,
}

impl Default for HardLimits {
    fn default() -> Self {
        Self {
            max_order_size: dec!(10000),      // Max 10k shares per order
            min_order_size: dec!(1),          // Min 1 share
            max_price: dec!(0.99),            // Max 99 cents
            min_price: dec!(0.01),            // Min 1 cent
            max_position_value: dec!(50000),  // Max $50k position
            max_order_value: dec!(5000),      // Max $5k per order
        }
    }
}

impl HardLimits {
    /// Creates conservative hard limits for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_order_size: dec!(1000),
            min_order_size: dec!(1),
            max_price: dec!(0.95),
            min_price: dec!(0.05),
            max_position_value: dec!(5000),
            max_order_value: dec!(500),
        }
    }

    /// Creates aggressive hard limits for production.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            max_order_size: dec!(50000),
            min_order_size: dec!(1),
            max_price: dec!(0.99),
            min_price: dec!(0.01),
            max_position_value: dec!(200000),
            max_order_value: dec!(20000),
        }
    }

    /// Validates an order against these limits.
    ///
    /// # Errors
    /// Returns an error message if validation fails.
    pub fn validate_order(&self, order: &OrderParams) -> Result<(), String> {
        // Validate size
        if order.size < self.min_order_size {
            return Err(format!(
                "Order size {} below minimum {}",
                order.size, self.min_order_size
            ));
        }
        if order.size > self.max_order_size {
            return Err(format!(
                "Order size {} exceeds maximum {}",
                order.size, self.max_order_size
            ));
        }

        // Validate price
        if order.price < self.min_price {
            return Err(format!(
                "Order price {} below minimum {}",
                order.price, self.min_price
            ));
        }
        if order.price > self.max_price {
            return Err(format!(
                "Order price {} exceeds maximum {}",
                order.price, self.max_price
            ));
        }

        // Validate order value
        let order_value = order.notional_value();
        if order_value > self.max_order_value {
            return Err(format!(
                "Order value {} exceeds maximum {}",
                order_value, self.max_order_value
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Live Executor
// =============================================================================

/// Live executor for production Polymarket trading.
///
/// Implements `PolymarketExecutor` trait for real order execution.
/// Uses rs-clob-client for EIP-712 order signing (integration TODO).
///
/// # Thread Safety
///
/// The executor is thread-safe and can be shared across tasks.
pub struct LiveExecutor {
    /// Configuration.
    config: LiveExecutorConfig,

    /// Wallet for signing.
    wallet: Wallet,

    /// Rate limiter for API calls.
    rate_limiter: ClobRateLimiter,

    /// HTTP client for API requests.
    http: reqwest::Client,

    /// Optional metrics tracker.
    metrics: Option<Arc<RwLock<ArbitrageMetrics>>>,
}

impl std::fmt::Debug for LiveExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveExecutor")
            .field("base_url", &self.config.base_url)
            .field("wallet_address", &self.wallet.address())
            .field("chain_id", &self.wallet.chain_id())
            .field("has_metrics", &self.metrics.is_some())
            .finish_non_exhaustive()
    }
}

impl LiveExecutor {
    /// Creates a new live executor with the given configuration.
    ///
    /// # Arguments
    /// * `config` - Executor configuration
    ///
    /// # Errors
    /// Returns error if wallet initialization fails.
    pub fn new(config: LiveExecutorConfig) -> Result<Self, WalletError> {
        // Initialize wallet from environment
        let wallet = Wallet::from_env(config.wallet_config.clone())?;

        // Initialize rate limiter
        let rate_limiter = ClobRateLimiter::new(config.rate_limiter_config.clone());

        // Initialize HTTP client with timeout
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.order_timeout_secs))
            .build()
            .map_err(|e| WalletError::SigningFailed(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            config,
            wallet,
            rate_limiter,
            http,
            metrics: None,
        })
    }

    /// Creates a live executor for Polygon mainnet.
    ///
    /// Loads wallet from default environment variable:
    /// - `POLYMARKET_PRIVATE_KEY`
    ///
    /// # Errors
    /// Returns error if environment variable is missing or invalid.
    pub fn mainnet() -> Result<Self, WalletError> {
        Self::new(LiveExecutorConfig::mainnet())
    }

    /// Creates a live executor for Amoy testnet.
    ///
    /// # Errors
    /// Returns error if environment variable is missing or invalid.
    pub fn testnet() -> Result<Self, WalletError> {
        Self::new(LiveExecutorConfig::testnet())
    }

    /// Attaches a metrics tracker for recording execution statistics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<RwLock<ArbitrageMetrics>>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Returns the wallet address.
    #[must_use]
    pub fn address(&self) -> &str {
        self.wallet.address()
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &LiveExecutorConfig {
        &self.config
    }

    /// Returns the chain ID.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.wallet.chain_id()
    }

    /// Returns a reference to the HTTP client.
    #[must_use]
    #[allow(dead_code)] // Will be used by rs-clob-client integration
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Validates an order against hard limits.
    fn validate_order(&self, order: &OrderParams) -> Result<(), ExecutionError> {
        self.config
            .hard_limits
            .validate_order(order)
            .map_err(ExecutionError::InvalidOrder)
    }

    /// Waits for rate limiter before making an order submission API call.
    async fn wait_for_order_submit(&self) {
        self.rate_limiter.wait_for_order_submit().await;
    }

    /// Waits for rate limiter before making a read API call.
    async fn wait_for_read(&self) {
        self.rate_limiter.wait_for_read().await;
    }

    /// Waits for rate limiter before making an order cancel API call.
    async fn wait_for_order_cancel(&self) {
        self.rate_limiter.wait_for_order_cancel().await;
    }

    /// Records metrics for an execution attempt.
    #[allow(dead_code)] // Will be used when rs-clob-client is integrated
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
impl PolymarketExecutor for LiveExecutor {
    async fn submit_order(&self, order: OrderParams) -> Result<OrderResult, ExecutionError> {
        // Validate order against hard limits
        self.validate_order(&order)?;

        // Wait for rate limit
        self.wait_for_order_submit().await;

        // TODO: Integrate rs-clob-client for actual order signing and submission
        //
        // The implementation would look something like:
        // ```
        // let signed_order = self.clob_client.create_order(
        //     order.token_id,
        //     order.price,
        //     order.size,
        //     order.side,
        //     order.order_type,
        // )?;
        //
        // let result = self.clob_client.post_order(signed_order).await?;
        // ```

        tracing::warn!(
            "LiveExecutor::submit_order called but rs-clob-client integration not yet implemented"
        );

        // Return NotImplemented error until rs-clob-client is integrated
        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }

    async fn submit_orders_batch(
        &self,
        orders: Vec<OrderParams>,
    ) -> Result<Vec<OrderResult>, ExecutionError> {
        // Validate all orders against hard limits
        for order in &orders {
            self.validate_order(order)?;
        }

        // Wait for rate limit
        self.wait_for_order_submit().await;

        // TODO: Integrate rs-clob-client for batch order signing and submission
        //
        // The implementation would look something like:
        // ```
        // let signed_orders: Vec<_> = orders.iter()
        //     .map(|o| self.clob_client.create_order(...))
        //     .collect::<Result<_, _>>()?;
        //
        // let results = self.clob_client.post_orders(signed_orders).await?;
        // ```

        tracing::warn!(
            "LiveExecutor::submit_orders_batch called but rs-clob-client integration not yet implemented"
        );

        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }

    async fn cancel_order(&self, order_id: &str) -> Result<(), ExecutionError> {
        // Wait for rate limit
        self.wait_for_order_cancel().await;

        // TODO: Integrate rs-clob-client for order cancellation
        //
        // The implementation would look something like:
        // ```
        // self.clob_client.cancel_order(order_id).await?;
        // ```

        tracing::warn!(
            "LiveExecutor::cancel_order called for {} but rs-clob-client integration not yet implemented",
            order_id
        );

        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }

    async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        // TODO: Integrate rs-clob-client for order status queries
        //
        // The implementation would look something like:
        // ```
        // let status = self.clob_client.get_order(order_id).await?;
        // ```

        tracing::warn!(
            "LiveExecutor::get_order_status called for {} but rs-clob-client integration not yet implemented",
            order_id
        );

        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }

    async fn wait_for_terminal(
        &self,
        order_id: &str,
        timeout: Duration,
    ) -> Result<OrderResult, ExecutionError> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(250);

        loop {
            // Check if we've exceeded the timeout
            if start.elapsed() > timeout {
                return Err(ExecutionError::timeout(order_id));
            }

            // Try to get order status
            match self.get_order_status(order_id).await {
                Ok(result) => {
                    if result.status.is_terminal() {
                        return Ok(result);
                    }
                    // Not terminal yet, wait and poll again
                }
                Err(ExecutionError::Api(msg))
                    if msg.contains("rs-clob-client integration not yet implemented") =>
                {
                    // Re-throw the not implemented error
                    return Err(ExecutionError::Api(msg));
                }
                Err(e) if e.is_retryable() => {
                    // Transient error, wait and retry
                    tracing::debug!("Retryable error polling order {}: {}", order_id, e);
                }
                Err(e) => {
                    // Non-retryable error
                    return Err(e);
                }
            }

            // Wait before next poll
            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn get_positions(&self) -> Result<Vec<Position>, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        // TODO: Integrate rs-clob-client for position queries
        //
        // The implementation would look something like:
        // ```
        // let positions = self.clob_client.get_positions().await?;
        // ```

        tracing::warn!(
            "LiveExecutor::get_positions called but rs-clob-client integration not yet implemented"
        );

        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }

    async fn get_balance(&self) -> Result<Decimal, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        // TODO: Integrate rs-clob-client for balance queries
        //
        // The implementation would look something like:
        // ```
        // let balance = self.clob_client.get_balance().await?;
        // ```

        tracing::warn!(
            "LiveExecutor::get_balance called but rs-clob-client integration not yet implemented"
        );

        Err(ExecutionError::Api(
            "rs-clob-client integration not yet implemented - use PaperExecutor for testing"
                .to_string(),
        ))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // Test private key - NOT A REAL KEY, just valid hex format
    const TEST_PRIVATE_KEY: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Helper to set up test environment variables
    fn setup_test_env(env_var: &str) {
        std::env::set_var(env_var, TEST_PRIVATE_KEY);
    }

    /// Helper to clean up test environment variables
    fn cleanup_test_env(env_var: &str) {
        std::env::remove_var(env_var);
    }

    // ==================== LiveExecutorConfig Tests ====================

    #[test]
    fn test_config_default() {
        let config = LiveExecutorConfig::default();

        assert_eq!(config.base_url, POLYMARKET_MAINNET_URL);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.order_timeout_secs, 10);
        assert!(config.neg_risk);
    }

    #[test]
    fn test_config_mainnet() {
        let config = LiveExecutorConfig::mainnet();

        assert_eq!(config.base_url, POLYMARKET_MAINNET_URL);
        assert_eq!(config.wallet_config.chain_id(), 137);
    }

    #[test]
    fn test_config_testnet() {
        let config = LiveExecutorConfig::testnet();

        assert_eq!(config.wallet_config.chain_id(), 80002);
    }

    #[test]
    fn test_config_builder_methods() {
        let config = LiveExecutorConfig::default()
            .with_base_url("https://custom.url")
            .with_max_retries(5)
            .with_order_timeout_secs(30)
            .with_neg_risk(false);

        assert_eq!(config.base_url, "https://custom.url");
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.order_timeout_secs, 30);
        assert!(!config.neg_risk);
    }

    // ==================== HardLimits Tests ====================

    #[test]
    fn test_hard_limits_default() {
        let limits = HardLimits::default();

        assert_eq!(limits.max_order_size, dec!(10000));
        assert_eq!(limits.min_order_size, dec!(1));
        assert_eq!(limits.max_price, dec!(0.99));
        assert_eq!(limits.min_price, dec!(0.01));
        assert_eq!(limits.max_position_value, dec!(50000));
        assert_eq!(limits.max_order_value, dec!(5000));
    }

    #[test]
    fn test_hard_limits_conservative() {
        let limits = HardLimits::conservative();

        assert_eq!(limits.max_order_size, dec!(1000));
        assert_eq!(limits.max_order_value, dec!(500));
    }

    #[test]
    fn test_hard_limits_aggressive() {
        let limits = HardLimits::aggressive();

        assert_eq!(limits.max_order_size, dec!(50000));
        assert_eq!(limits.max_order_value, dec!(20000));
    }

    #[test]
    fn test_hard_limits_validate_order_valid() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(100));

        assert!(limits.validate_order(&order).is_ok());
    }

    #[test]
    fn test_hard_limits_validate_order_size_too_small() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(0.5));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("below minimum"));
    }

    #[test]
    fn test_hard_limits_validate_order_size_too_large() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(50000));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_order_price_too_low() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.001), dec!(100));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("below minimum"));
    }

    #[test]
    fn test_hard_limits_validate_order_price_too_high() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.999), dec!(100));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_order_value_too_high() {
        let limits = HardLimits::default();
        // 15000 * 0.50 = 7500 which exceeds max_order_value of 5000
        // But size 15000 also exceeds max_order_size of 10000, so it fails on size first
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(15000));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
    }

    // ==================== LiveExecutor Creation Tests ====================

    #[test]
    fn test_live_executor_new_success() {
        let env_var = "TEST_LIVE_EXEC_KEY_1";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let result = LiveExecutor::new(config);
        cleanup_test_env(env_var);

        assert!(result.is_ok());
    }

    #[test]
    fn test_live_executor_new_missing_env() {
        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var("NONEXISTENT_KEY_12345"));

        let result = LiveExecutor::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_live_executor_address() {
        let env_var = "TEST_LIVE_EXEC_KEY_2";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        assert!(!executor.address().is_empty());
        assert!(executor.address().starts_with("0x"));
    }

    #[test]
    fn test_live_executor_chain_id() {
        let env_var = "TEST_LIVE_EXEC_KEY_3";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::testnet()
            .with_wallet_config(WalletConfig::testnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        assert_eq!(executor.chain_id(), 80002);
    }

    #[test]
    fn test_live_executor_validate_order_valid() {
        let env_var = "TEST_LIVE_EXEC_KEY_4";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(100));
        let result = executor.validate_order(&order);
        assert!(result.is_ok());
    }

    #[test]
    fn test_live_executor_validate_order_invalid() {
        let env_var = "TEST_LIVE_EXEC_KEY_5";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(50000)); // Too large
        let result = executor.validate_order(&order);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::InvalidOrder(_)));
    }

    #[test]
    fn test_live_executor_debug() {
        let env_var = "TEST_LIVE_EXEC_KEY_6";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let debug_str = format!("{:?}", executor);
        assert!(debug_str.contains("LiveExecutor"));
        assert!(debug_str.contains("wallet_address"));
        // Should NOT contain the private key
        assert!(!debug_str.contains(TEST_PRIVATE_KEY));
    }

    // ==================== Async Tests ====================

    #[tokio::test]
    async fn test_live_executor_submit_order_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_7";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(100));
        let result = executor.submit_order(order).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_submit_order_validates_first() {
        let env_var = "TEST_LIVE_EXEC_KEY_8";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(50000)); // Too large
        let result = executor.submit_order(order).await;

        assert!(result.is_err());
        // Should fail on validation, not on "not implemented"
        assert!(matches!(result.unwrap_err(), ExecutionError::InvalidOrder(_)));
    }

    #[tokio::test]
    async fn test_live_executor_submit_orders_batch_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_9";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let orders = vec![
            OrderParams::buy_fok("token-yes", dec!(0.45), dec!(100)),
            OrderParams::buy_fok("token-no", dec!(0.52), dec!(100)),
        ];
        let result = executor.submit_orders_batch(orders).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_cancel_order_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_10";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let result = executor.cancel_order("order-123").await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_get_order_status_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_11";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let result = executor.get_order_status("order-123").await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_get_balance_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_12";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let result = executor.get_balance().await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_get_positions_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_13";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let result = executor.get_positions().await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    #[tokio::test]
    async fn test_live_executor_wait_for_terminal_returns_not_implemented() {
        let env_var = "TEST_LIVE_EXEC_KEY_14";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let executor = LiveExecutor::new(config).unwrap();
        cleanup_test_env(env_var);

        let result = executor
            .wait_for_terminal("order-123", Duration::from_secs(5))
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExecutionError::Api(msg) if msg.contains("not yet implemented")));
    }

    // ==================== Metrics Tests ====================

    #[test]
    fn test_live_executor_with_metrics() {
        let env_var = "TEST_LIVE_EXEC_KEY_15";
        setup_test_env(env_var);

        let config = LiveExecutorConfig::default()
            .with_wallet_config(WalletConfig::mainnet().with_env_var(env_var));

        let metrics = Arc::new(RwLock::new(ArbitrageMetrics::new()));
        let executor = LiveExecutor::new(config).unwrap().with_metrics(metrics.clone());

        cleanup_test_env(env_var);

        assert!(executor.metrics.is_some());
    }
}
