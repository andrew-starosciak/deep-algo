//! Live execution handler for Polymarket arbitrage using polymarket-client-sdk.
//!
//! This module provides production order execution for Polymarket's CLOB.
//! The actual signing and submission uses the polymarket-client-sdk library for
//! EIP-712 order signing.
//!
//! # Overview
//!
//! The `LiveExecutor` wraps the polymarket-client-sdk library and implements the
//! `PolymarketExecutor` trait for production trading. It handles:
//!
//! - Wallet initialization from private keys
//! - EIP-712 order signing via polymarket-client-sdk
//! - Rate limiting for API calls
//! - Order validation against hard limits
//! - Circuit breaker integration for safety
//! - Daily volume tracking
//! - Retry logic with exponential backoff
//! - Metrics integration
//!
//! # Security
//!
//! - Private keys are loaded from environment variables, NEVER logged
//! - All orders are validated against configurable hard limits
//! - Circuit breaker halts trading on excessive failures or losses
//! - Daily volume limits prevent runaway trading
//! - Minimum balance reserve ensures funds for unwinding
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::live_executor::{LiveExecutor, LiveExecutorConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create executor for mainnet (loads wallet from env)
//!     let executor = LiveExecutor::mainnet().await?;
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

use async_trait::async_trait;
use parking_lot::RwLock;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError};
use super::execution::{
    ExecutionError, OrderParams, OrderResult, OrderType, PolymarketExecutor, Position, Side,
};
use super::metrics::ArbitrageMetrics;
use super::rate_limiter::{ClobRateLimiter, RateLimiterConfig};
use super::sdk_client::{ClobClient, ClobClientConfig, ClobError};
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

    /// CLOB client configuration.
    pub clob_config: ClobClientConfig,

    /// Rate limiter configuration.
    pub rate_limiter_config: RateLimiterConfig,

    /// Circuit breaker configuration.
    pub circuit_breaker_config: CircuitBreakerConfig,

    /// Hard limits for order validation (safety).
    pub hard_limits: HardLimits,

    /// Maximum retry attempts for transient failures.
    pub max_retries: u32,

    /// Timeout in seconds for order operations.
    pub order_timeout_secs: u64,

    /// Whether to use neg_risk flag (check market's neg_risk field via CLOB API).
    pub neg_risk: bool,
}

impl Default for LiveExecutorConfig {
    fn default() -> Self {
        Self {
            base_url: POLYMARKET_MAINNET_URL.to_string(),
            wallet_config: WalletConfig::mainnet(),
            clob_config: ClobClientConfig::mainnet(),
            rate_limiter_config: RateLimiterConfig::default(),
            circuit_breaker_config: CircuitBreakerConfig::default(),
            hard_limits: HardLimits::default(),
            max_retries: 3,
            order_timeout_secs: 10,
            neg_risk: false,
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
            clob_config: ClobClientConfig::mainnet(),
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
            clob_config: ClobClientConfig::testnet(),
            ..Default::default()
        }
    }

    /// Creates a micro testing configuration with tight limits.
    ///
    /// Suitable for initial validation with small amounts.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            base_url: POLYMARKET_MAINNET_URL.to_string(),
            wallet_config: WalletConfig::mainnet(),
            clob_config: ClobClientConfig::mainnet(),
            rate_limiter_config: RateLimiterConfig::conservative(),
            circuit_breaker_config: CircuitBreakerConfig::micro_testing(),
            hard_limits: HardLimits::micro_testing(),
            max_retries: 2,
            order_timeout_secs: 15,
            neg_risk: false,
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

    /// Sets the CLOB client configuration.
    #[must_use]
    pub fn with_clob_config(mut self, config: ClobClientConfig) -> Self {
        self.clob_config = config;
        self
    }

    /// Sets the rate limiter configuration.
    #[must_use]
    pub fn with_rate_limiter_config(mut self, config: RateLimiterConfig) -> Self {
        self.rate_limiter_config = config;
        self
    }

    /// Sets the circuit breaker configuration.
    #[must_use]
    pub fn with_circuit_breaker_config(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_config = config;
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

    /// Maximum single order value in USDC.
    pub max_order_value: Decimal,

    /// Maximum daily volume in USDC.
    pub max_daily_volume: Decimal,

    /// Minimum balance reserve to keep (prevents draining account).
    pub min_balance_reserve: Decimal,
}

impl Default for HardLimits {
    fn default() -> Self {
        Self {
            max_order_size: dec!(10000),    // Max 10k shares per order
            min_order_size: dec!(5),        // Min 1 share
            max_price: dec!(0.99),          // Max 99 cents
            min_price: dec!(0.01),          // Min 1 cent
            max_order_value: dec!(5000),    // Max $5k per order
            max_daily_volume: dec!(50000),  // Max $50k daily volume
            min_balance_reserve: dec!(100), // Keep $100 minimum
        }
    }
}

impl HardLimits {
    /// Creates conservative hard limits for initial testing.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_order_size: dec!(1000),
            min_order_size: dec!(5),
            max_price: dec!(0.95),
            min_price: dec!(0.05),
            max_order_value: dec!(500),
            max_daily_volume: dec!(5000),
            min_balance_reserve: dec!(200),
        }
    }

    /// Creates micro testing hard limits for very small amounts.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            max_order_size: dec!(200),
            min_order_size: dec!(5),
            max_price: dec!(0.95),
            min_price: dec!(0.05),
            max_order_value: dec!(10),
            max_daily_volume: dec!(100),
            min_balance_reserve: dec!(5),
        }
    }

    /// Creates aggressive hard limits for production.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            max_order_size: dec!(50000),
            min_order_size: dec!(5),
            max_price: dec!(0.99),
            min_price: dec!(0.01),
            max_order_value: dec!(20000),
            max_daily_volume: dec!(200000),
            min_balance_reserve: dec!(500),
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

    /// Validates that an order doesn't exceed remaining daily volume.
    ///
    /// # Arguments
    /// * `order` - The order to validate
    /// * `current_daily_volume` - Volume already traded today
    ///
    /// # Errors
    /// Returns an error message if daily volume would be exceeded.
    pub fn validate_daily_volume(
        &self,
        order: &OrderParams,
        current_daily_volume: Decimal,
    ) -> Result<(), String> {
        let order_value = order.notional_value();
        let new_total = current_daily_volume + order_value;

        if new_total > self.max_daily_volume {
            return Err(format!(
                "Order would exceed daily volume limit: {} + {} = {} > {}",
                current_daily_volume, order_value, new_total, self.max_daily_volume
            ));
        }

        Ok(())
    }

    /// Validates that balance after order would meet minimum reserve.
    ///
    /// # Arguments
    /// * `order` - The order to validate
    /// * `current_balance` - Current account balance
    ///
    /// # Errors
    /// Returns an error message if balance would fall below reserve.
    pub fn validate_balance_reserve(
        &self,
        order: &OrderParams,
        current_balance: Decimal,
    ) -> Result<(), String> {
        let order_value = order.notional_value();
        let remaining = current_balance - order_value;

        if remaining < self.min_balance_reserve {
            return Err(format!(
                "Order would leave balance {} below minimum reserve {}",
                remaining, self.min_balance_reserve
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Daily Volume Tracker
// =============================================================================

/// Tracks daily trading volume for limit enforcement.
#[derive(Debug)]
struct DailyVolumeTracker {
    /// Current day's total volume.
    volume: Decimal,

    /// Date when volume was last reset (days since epoch).
    last_reset_day: u64,
}

impl DailyVolumeTracker {
    /// Creates a new tracker with zero volume.
    fn new() -> Self {
        Self {
            volume: Decimal::ZERO,
            last_reset_day: Self::current_day(),
        }
    }

    /// Returns current day as days since Unix epoch.
    fn current_day() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / 86400)
            .unwrap_or(0)
    }

    /// Resets volume if day has changed.
    fn maybe_reset(&mut self) {
        let current = Self::current_day();
        if current != self.last_reset_day {
            self.volume = Decimal::ZERO;
            self.last_reset_day = current;
        }
    }

    /// Gets current daily volume, resetting if needed.
    fn get(&mut self) -> Decimal {
        self.maybe_reset();
        self.volume
    }

    /// Adds to daily volume.
    fn add(&mut self, amount: Decimal) {
        self.maybe_reset();
        self.volume += amount;
    }
}

// =============================================================================
// Live Executor
// =============================================================================

/// Live executor for production Polymarket trading.
///
/// Implements `PolymarketExecutor` trait for real order execution.
/// Uses polymarket-client-sdk for EIP-712 order signing.
///
/// # Safety Features
///
/// - Circuit breaker halts trading on excessive failures or losses
/// - Hard limits prevent oversized orders
/// - Daily volume tracking prevents runaway trading
/// - Minimum balance reserve ensures funds for unwinding
/// - Rate limiting prevents API abuse
///
/// # Thread Safety
///
/// The executor is thread-safe and can be shared across tasks.
pub struct LiveExecutor {
    /// Configuration.
    config: LiveExecutorConfig,

    /// Wallet address (cached for display, actual wallet owned by client).
    wallet_address: String,

    /// Chain ID.
    chain_id: u64,

    /// CLOB API client (owns the wallet).
    client: ClobClient,

    /// Rate limiter for API calls.
    rate_limiter: ClobRateLimiter,

    /// Circuit breaker for safety.
    circuit_breaker: CircuitBreaker,

    /// Daily volume tracker (protected by RwLock for interior mutability).
    daily_volume: RwLock<DailyVolumeTracker>,

    /// Cached balance (updated on each get_balance call).
    cached_balance: RwLock<Option<Decimal>>,

    /// Optional metrics tracker.
    metrics: Option<Arc<RwLock<ArbitrageMetrics>>>,
}

impl std::fmt::Debug for LiveExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveExecutor")
            .field("base_url", &self.config.base_url)
            .field("wallet_address", &self.wallet_address)
            .field("chain_id", &self.chain_id)
            .field("has_metrics", &self.metrics.is_some())
            .field("daily_volume", &self.daily_volume.read().volume)
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
    pub async fn new(config: LiveExecutorConfig) -> Result<Self, WalletError> {
        // Initialize wallet from environment
        let wallet = Wallet::from_env(config.wallet_config.clone())?;

        // Cache wallet info before moving wallet to client
        let wallet_address = wallet.address().to_string();
        let chain_id = wallet.chain_id();

        // Initialize CLOB client (takes ownership of wallet)
        let client = ClobClient::new(wallet, config.clob_config.clone()).map_err(|e| {
            WalletError::SigningFailed(format!("Failed to create CLOB client: {e}"))
        })?;

        // Initialize rate limiter
        let rate_limiter = ClobRateLimiter::new(config.rate_limiter_config.clone());

        // Initialize circuit breaker
        let circuit_breaker = CircuitBreaker::new(config.circuit_breaker_config.clone());

        tracing::info!(
            address = %wallet_address,
            chain_id = chain_id,
            max_order_value = %config.hard_limits.max_order_value,
            max_daily_volume = %config.hard_limits.max_daily_volume,
            "LiveExecutor initialized"
        );

        Ok(Self {
            config,
            wallet_address,
            chain_id,
            client,
            rate_limiter,
            circuit_breaker,
            daily_volume: RwLock::new(DailyVolumeTracker::new()),
            cached_balance: RwLock::new(None),
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
    pub async fn mainnet() -> Result<Self, WalletError> {
        Self::new(LiveExecutorConfig::mainnet()).await
    }

    /// Creates a live executor for Amoy testnet.
    ///
    /// # Errors
    /// Returns error if environment variable is missing or invalid.
    pub async fn testnet() -> Result<Self, WalletError> {
        Self::new(LiveExecutorConfig::testnet()).await
    }

    /// Creates a live executor with micro testing configuration.
    ///
    /// # Errors
    /// Returns error if environment variable is missing or invalid.
    pub async fn micro_testing() -> Result<Self, WalletError> {
        Self::new(LiveExecutorConfig::micro_testing()).await
    }

    /// Attaches a metrics tracker for recording execution statistics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<RwLock<ArbitrageMetrics>>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Authenticates with the CLOB API.
    ///
    /// Must be called before submitting orders.
    ///
    /// # Errors
    /// Returns error if authentication fails.
    pub async fn authenticate(&mut self) -> Result<(), ExecutionError> {
        self.client
            .authenticate()
            .await
            .map_err(|e| e.into_execution_error())
    }

    /// Queries and sets the taker fee rate for a given token.
    ///
    /// Call this once at startup with any token from the markets you'll trade.
    /// Fee-enabled markets (15-min crypto) return 1000 bps (10%).
    ///
    /// If the API returns 0 but a non-zero fee was already configured,
    /// the pre-configured value is kept (the API endpoint may not exist).
    pub async fn configure_fee_rate(&mut self, token_id: &str) -> Result<(), ExecutionError> {
        let bps = self
            .client
            .get_fee_rate_bps(token_id)
            .await
            .map_err(|e| e.into_execution_error())?;
        let current = self.client.config().taker_fee_bps;
        if bps > 0 {
            self.client.set_taker_fee_bps(bps);
            tracing::info!(fee_rate_bps = bps, token_id = %token_id, "Configured taker fee rate from API");
        } else if current > 0 {
            tracing::info!(fee_rate_bps = current, token_id = %token_id, "API returned 0, keeping pre-configured fee rate");
        } else {
            tracing::warn!(token_id = %token_id, "API returned fee_rate_bps=0 and no default configured");
        }
        Ok(())
    }

    /// Returns true if the client is authenticated.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.client.is_authenticated()
    }

    /// Returns the wallet address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.wallet_address
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &LiveExecutorConfig {
        &self.config
    }

    /// Returns the chain ID.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Returns a reference to the circuit breaker.
    #[must_use]
    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    /// Returns the current daily volume.
    #[must_use]
    pub fn daily_volume(&self) -> Decimal {
        self.daily_volume.write().get()
    }

    /// Manually trips the circuit breaker for emergency stops.
    pub fn trip_circuit_breaker(&self) {
        self.circuit_breaker.trip();
    }


    /// Resets the circuit breaker to allow trading again.
    pub fn reset_circuit_breaker(&self) {
        self.circuit_breaker.reset();
    }

    /// Records a profit/loss for circuit breaker tracking.
    pub fn record_pnl(&self, pnl: Decimal) {
        self.circuit_breaker.record_success(pnl);
    }

    /// Checks if trading is allowed by the circuit breaker.
    ///
    /// # Errors
    /// Returns the circuit breaker error if trading is blocked.
    pub fn check_circuit_breaker(&self) -> Result<(), CircuitBreakerError> {
        self.circuit_breaker.can_trade()
    }

    /// Validates an order against all safety checks.
    ///
    /// Checks:
    /// 1. Hard limits (size, price, order value)
    /// 2. Daily volume limit
    /// 3. Balance reserve (if balance is cached)
    /// 4. Circuit breaker status
    fn validate_order(&self, order: &OrderParams) -> Result<(), ExecutionError> {
        // Check circuit breaker first
        self.circuit_breaker
            .can_trade()
            .map_err(|e| ExecutionError::Api(format!("Circuit breaker: {e}")))?;

        // Validate against hard limits
        self.config
            .hard_limits
            .validate_order(order)
            .map_err(ExecutionError::InvalidOrder)?;

        // Validate daily volume
        let current_volume = self.daily_volume.write().get();
        self.config
            .hard_limits
            .validate_daily_volume(order, current_volume)
            .map_err(ExecutionError::InvalidOrder)?;

        // Validate balance reserve if we have cached balance
        if let Some(balance) = *self.cached_balance.read() {
            self.config
                .hard_limits
                .validate_balance_reserve(order, balance)
                .map_err(|_| ExecutionError::InsufficientBalance {
                    required: order.notional_value() + self.config.hard_limits.min_balance_reserve,
                    available: balance,
                })?;
        }

        Ok(())
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
    fn record_metrics(&self, success: bool, partial: bool) {
        if let Some(ref metrics) = self.metrics {
            let mut m = metrics.write();
            m.record_execution(success, partial);
        }
    }

    /// Converts ClobError to ExecutionError.
    fn handle_clob_error(&self, error: ClobError, record_failure: bool) -> ExecutionError {
        if record_failure {
            self.circuit_breaker.record_failure();
        }
        error.into_execution_error()
    }

    /// Converts our Side type to SDK Side type string.
    #[allow(dead_code)]
    fn side_to_string(side: Side) -> &'static str {
        match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }

    /// Converts our OrderType to SDK OrderType string.
    #[allow(dead_code)]
    fn order_type_to_string(order_type: OrderType) -> &'static str {
        match order_type {
            OrderType::Fok => "FOK",
            OrderType::Fak => "FAK",
            OrderType::Gtc => "GTC",
        }
    }
}

// =============================================================================
// Trait Implementation
// =============================================================================

#[async_trait]
impl PolymarketExecutor for LiveExecutor {
    async fn submit_order(&self, order: OrderParams) -> Result<OrderResult, ExecutionError> {
        // Validate order against all safety checks
        self.validate_order(&order)?;

        // Wait for rate limit
        self.wait_for_order_submit().await;

        tracing::info!(
            token_id = %order.token_id,
            side = ?order.side,
            price = %order.price,
            size = %order.size,
            order_type = ?order.order_type,
            "Submitting order"
        );

        // Submit via CLOB client with retry
        let result = self
            .client
            .submit_order_with_retry(&order)
            .await
            .map_err(|e| self.handle_clob_error(e, true))?;

        // Update daily volume on success
        if result.status.has_fills() {
            let fill_value = result.fill_notional();
            self.daily_volume.write().add(fill_value);

            // Record success with circuit breaker
            // Note: P&L is recorded separately via record_pnl()
            self.circuit_breaker.record_success(Decimal::ZERO);
        }

        // Record metrics
        self.record_metrics(
            result.is_filled(),
            result.status.has_fills() && !result.is_filled(),
        );

        tracing::info!(
            order_id = %result.order_id,
            status = ?result.status,
            filled_size = %result.filled_size,
            "Order submitted"
        );

        Ok(result)
    }

    async fn submit_orders_batch(
        &self,
        orders: Vec<OrderParams>,
    ) -> Result<Vec<OrderResult>, ExecutionError> {
        // Validate all orders against safety checks
        for order in &orders {
            self.validate_order(order)?;
        }

        // Wait for rate limit
        self.wait_for_order_submit().await;

        tracing::info!(count = orders.len(), "Submitting order batch");

        // Submit orders sequentially (Polymarket doesn't have batch endpoint)
        let mut results = Vec::with_capacity(orders.len());
        for order in orders {
            match self.submit_order(order).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    // Record failure and create rejected result
                    results.push(OrderResult::rejected(
                        uuid::Uuid::new_v4().to_string(),
                        e.to_string(),
                    ));
                }
            }
        }

        Ok(results)
    }

    async fn cancel_order(&self, order_id: &str) -> Result<(), ExecutionError> {
        // Wait for rate limit
        self.wait_for_order_cancel().await;

        tracing::info!(order_id = %order_id, "Cancelling order");

        self.client
            .cancel_order(order_id)
            .await
            .map_err(|e| self.handle_clob_error(e, false))?;

        Ok(())
    }

    async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        self.client
            .get_order_status(order_id)
            .await
            .map_err(|e| self.handle_clob_error(e, false))
    }

    async fn wait_for_terminal(
        &self,
        order_id: &str,
        timeout: Duration,
    ) -> Result<OrderResult, ExecutionError> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(250);

        loop {
            if start.elapsed() > timeout {
                return Err(ExecutionError::timeout(order_id));
            }

            match self.get_order_status(order_id).await {
                Ok(result) => {
                    if result.status.is_terminal() {
                        return Ok(result);
                    }
                }
                Err(e) if e.is_retryable() => {
                    tracing::debug!(order_id = %order_id, error = %e, "Retryable error, will retry");
                }
                Err(e) => {
                    return Err(e);
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn get_positions(&self) -> Result<Vec<Position>, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        // Query wallet positions from Polymarket Data API
        let wallet_positions = self
            .client
            .get_positions()
            .await
            .map_err(|e| self.handle_clob_error(e, false))?;

        // Convert WalletPosition to Position
        let positions: Vec<Position> = wallet_positions
            .into_iter()
            .map(|wp| {
                let size = Decimal::from_f64_retain(wp.size).unwrap_or_default();
                let avg_price = Decimal::from_f64_retain(wp.avg_price).unwrap_or_default();
                let current_price = Some(Decimal::from_f64_retain(wp.cur_price).unwrap_or_default());
                let unrealized_pnl = current_price.map(|cp| (cp - avg_price) * size);

                Position {
                    token_id: wp.asset,
                    size,
                    avg_price,
                    current_price,
                    unrealized_pnl,
                }
            })
            .collect();

        tracing::debug!(count = positions.len(), "Fetched wallet positions");

        Ok(positions)
    }

    async fn get_balance(&self) -> Result<Decimal, ExecutionError> {
        // Wait for rate limit
        self.wait_for_read().await;

        let balance = self
            .client
            .get_balance()
            .await
            .map_err(|e| self.handle_clob_error(e, false))?;

        // Cache the balance for validation
        *self.cached_balance.write() = Some(balance);

        // Check if balance is below warning threshold
        if self.circuit_breaker.is_balance_warning(balance) {
            tracing::warn!(
                balance = %balance,
                threshold = %self.config.circuit_breaker_config.min_balance_warning,
                "Balance below warning threshold"
            );
        }

        Ok(balance)
    }

    async fn get_effective_balance(&self) -> Result<Decimal, ExecutionError> {
        // Get USDC balance
        let usdc_balance = self.get_balance().await?;

        // Query positions for redeemable value (non-fatal on failure)
        let redeemable_value = match self.get_positions().await {
            Ok(positions) => positions
                .iter()
                .filter(|p| {
                    // Positions with cur_price >= 0.95 are winners ready to redeem
                    p.current_price.map_or(false, |price| price >= dec!(0.95))
                })
                .fold(Decimal::ZERO, |acc, p| acc + p.size),
            Err(e) => {
                tracing::warn!("Failed to fetch positions for effective balance, using USDC only: {e}");
                Decimal::ZERO
            }
        };

        let effective = usdc_balance + redeemable_value;
        if redeemable_value > Decimal::ZERO {
            tracing::info!(
                usdc = %usdc_balance,
                redeemable = %redeemable_value,
                effective = %effective,
                "Effective balance includes redeemable positions"
            );
        }

        Ok(effective)
    }

    async fn redeem_resolved_positions(&self) -> Result<u64, ExecutionError> {
        let rpc_url = std::env::var("POLYGON_RPC_URL")
            .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

        self.client
            .redeem_resolved_positions(&rpc_url)
            .await
            .map_err(|e| self.handle_clob_error(e, false))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ==================== LiveExecutorConfig Tests ====================

    #[test]
    fn test_config_default() {
        let config = LiveExecutorConfig::default();

        assert_eq!(config.base_url, POLYMARKET_MAINNET_URL);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.order_timeout_secs, 10);
        assert!(!config.neg_risk);
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
    fn test_config_micro_testing() {
        let config = LiveExecutorConfig::micro_testing();

        assert_eq!(config.hard_limits.max_order_value, dec!(10));
        assert_eq!(config.hard_limits.max_daily_volume, dec!(100));
        assert_eq!(config.hard_limits.min_balance_reserve, dec!(5));
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
        assert_eq!(limits.min_order_size, dec!(5));
        assert_eq!(limits.max_price, dec!(0.99));
        assert_eq!(limits.min_price, dec!(0.01));
        assert_eq!(limits.max_order_value, dec!(5000));
        assert_eq!(limits.max_daily_volume, dec!(50000));
        assert_eq!(limits.min_balance_reserve, dec!(100));
    }

    #[test]
    fn test_hard_limits_conservative() {
        let limits = HardLimits::conservative();

        assert_eq!(limits.max_order_size, dec!(1000));
        assert_eq!(limits.max_order_value, dec!(500));
        assert_eq!(limits.max_daily_volume, dec!(5000));
        assert_eq!(limits.min_balance_reserve, dec!(200));
    }

    #[test]
    fn test_hard_limits_micro_testing() {
        let limits = HardLimits::micro_testing();

        assert_eq!(limits.max_order_size, dec!(200));
        assert_eq!(limits.max_order_value, dec!(10));
        assert_eq!(limits.max_daily_volume, dec!(100));
        assert_eq!(limits.min_balance_reserve, dec!(5));
    }

    #[test]
    fn test_hard_limits_aggressive() {
        let limits = HardLimits::aggressive();

        assert_eq!(limits.max_order_size, dec!(50000));
        assert_eq!(limits.max_order_value, dec!(20000));
        assert_eq!(limits.max_daily_volume, dec!(200000));
        assert_eq!(limits.min_balance_reserve, dec!(500));
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
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(15000));

        let result = limits.validate_order(&order);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));
    }

    #[test]
    fn test_hard_limits_validate_daily_volume_within_limit() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(100)); // $50 order

        let result = limits.validate_daily_volume(&order, dec!(1000)); // $1000 already traded
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_validate_daily_volume_exceeds_limit() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(100)); // $50 order

        let result = limits.validate_daily_volume(&order, dec!(49990)); // Near limit
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("daily volume limit"));
    }

    #[test]
    fn test_hard_limits_validate_balance_reserve_ok() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(100)); // $50 order

        let result = limits.validate_balance_reserve(&order, dec!(500)); // $500 balance
        assert!(result.is_ok());
    }

    #[test]
    fn test_hard_limits_validate_balance_reserve_too_low() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.50), dec!(100)); // $50 order

        let result = limits.validate_balance_reserve(&order, dec!(120)); // Only $120 balance
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("minimum reserve"));
    }

    // ==================== DailyVolumeTracker Tests ====================

    #[test]
    fn test_daily_volume_tracker_new() {
        let tracker = DailyVolumeTracker::new();
        assert_eq!(tracker.volume, Decimal::ZERO);
    }

    #[test]
    fn test_daily_volume_tracker_add() {
        let mut tracker = DailyVolumeTracker::new();

        tracker.add(dec!(100));
        assert_eq!(tracker.get(), dec!(100));

        tracker.add(dec!(50));
        assert_eq!(tracker.get(), dec!(150));
    }

    #[test]
    fn test_daily_volume_tracker_get_without_add() {
        let mut tracker = DailyVolumeTracker::new();
        assert_eq!(tracker.get(), Decimal::ZERO);
    }

    // ==================== Type Conversion Tests ====================

    #[test]
    fn test_side_to_string() {
        assert_eq!(LiveExecutor::side_to_string(Side::Buy), "BUY");
        assert_eq!(LiveExecutor::side_to_string(Side::Sell), "SELL");
    }

    #[test]
    fn test_order_type_to_string() {
        assert_eq!(LiveExecutor::order_type_to_string(OrderType::Fok), "FOK");
        assert_eq!(LiveExecutor::order_type_to_string(OrderType::Fak), "FAK");
        assert_eq!(LiveExecutor::order_type_to_string(OrderType::Gtc), "GTC");
    }

    // ==================== Metrics Tests ====================

    #[test]
    fn test_metrics_can_be_attached() {
        let metrics = Arc::new(RwLock::new(ArbitrageMetrics::new()));
        let _summary = metrics.read().validation_summary();
    }

    // ==================== Circuit Breaker Integration Tests ====================

    #[test]
    fn test_circuit_breaker_blocks_after_failures() {
        let config = CircuitBreakerConfig::default().with_max_consecutive_failures(2);
        let breaker = CircuitBreaker::new(config);

        // Record failures to trigger breaker
        breaker.record_failure();
        breaker.record_failure();

        // Should now be blocked
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_circuit_breaker_resets_after_success() {
        let config = CircuitBreakerConfig::default()
            .with_max_consecutive_failures(3)
            .with_pause_duration(Duration::from_millis(10));
        let breaker = CircuitBreaker::new(config);

        // Record some failures (but not enough to trigger)
        breaker.record_failure();
        breaker.record_failure();

        // Record success
        breaker.record_success(Decimal::ZERO);

        // Should be allowed (failures reset)
        assert!(breaker.can_trade().is_ok());
    }

    #[test]
    fn test_circuit_breaker_blocks_on_daily_loss() {
        let config = CircuitBreakerConfig::default().with_max_daily_loss(dec!(50));
        let breaker = CircuitBreaker::new(config);

        // Record losses
        breaker.record_success(dec!(-60)); // -$60 loss

        // Should be blocked
        assert!(breaker.can_trade().is_err());
    }

    #[test]
    fn test_circuit_breaker_manual_trip() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert!(breaker.can_trade().is_ok());

        breaker.trip();
        assert!(breaker.can_trade().is_err());

        breaker.reset();
        assert!(breaker.can_trade().is_ok());
    }

    // ==================== Order Validation Integration Tests ====================

    #[test]
    fn test_order_validation_all_checks_pass() {
        let limits = HardLimits::default();
        let order = OrderParams::buy_fok("token-123", dec!(0.45), dec!(100));

        // All individual checks should pass
        assert!(limits.validate_order(&order).is_ok());
        assert!(limits.validate_daily_volume(&order, dec!(0)).is_ok());
        assert!(limits.validate_balance_reserve(&order, dec!(1000)).is_ok());
    }

    #[test]
    fn test_order_validation_boundary_values() {
        let limits = HardLimits::default();

        // At minimum size (Polymarket requires 5 shares minimum)
        let order_min = OrderParams::buy_fok("token", dec!(0.50), dec!(5));
        assert!(limits.validate_order(&order_min).is_ok());

        // At maximum size (but within value limit)
        let order_max_size = OrderParams::buy_fok("token", dec!(0.10), dec!(10000));
        assert!(limits.validate_order(&order_max_size).is_ok());

        // At minimum price
        let order_min_price = OrderParams::buy_fok("token", dec!(0.01), dec!(100));
        assert!(limits.validate_order(&order_min_price).is_ok());

        // At maximum price
        let order_max_price = OrderParams::buy_fok("token", dec!(0.99), dec!(100));
        assert!(limits.validate_order(&order_max_price).is_ok());
    }

    #[test]
    fn test_order_notional_value_calculation() {
        let order = OrderParams::buy_fok("token", dec!(0.45), dec!(100));
        assert_eq!(order.notional_value(), dec!(45));

        let order2 = OrderParams::buy_fok("token", dec!(0.50), dec!(200));
        assert_eq!(order2.notional_value(), dec!(100));
    }

    // ==================== Config Builder Chain Tests ====================

    #[test]
    fn test_config_full_builder_chain() {
        let config = LiveExecutorConfig::default()
            .with_base_url("https://test.url")
            .with_wallet_config(WalletConfig::testnet())
            .with_clob_config(ClobClientConfig::testnet())
            .with_rate_limiter_config(RateLimiterConfig::conservative())
            .with_circuit_breaker_config(CircuitBreakerConfig::micro_testing())
            .with_hard_limits(HardLimits::micro_testing())
            .with_max_retries(5)
            .with_order_timeout_secs(30)
            .with_neg_risk(false);

        assert_eq!(config.base_url, "https://test.url");
        assert_eq!(config.wallet_config.chain_id(), 80002);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.order_timeout_secs, 30);
        assert!(!config.neg_risk);
        assert_eq!(config.hard_limits.max_order_value, dec!(10));
    }

    // ==================== HardLimits Serialization Tests ====================

    #[test]
    fn test_hard_limits_serialization() {
        let limits = HardLimits::micro_testing();
        let json = serde_json::to_string(&limits).unwrap();
        let deserialized: HardLimits = serde_json::from_str(&json).unwrap();

        assert_eq!(limits.max_order_size, deserialized.max_order_size);
        assert_eq!(limits.max_daily_volume, deserialized.max_daily_volume);
        assert_eq!(limits.min_balance_reserve, deserialized.min_balance_reserve);
    }

    // ==================== Edge Case Tests ====================

    #[test]
    fn test_validate_order_exactly_at_limits() {
        let limits = HardLimits {
            max_order_size: dec!(100),
            min_order_size: dec!(10),
            max_price: dec!(0.90),
            min_price: dec!(0.10),
            max_order_value: dec!(50),
            max_daily_volume: dec!(100),
            min_balance_reserve: dec!(50),
        };

        // Exactly at max size but within value limit
        let order_at_max_size = OrderParams::buy_fok("token", dec!(0.10), dec!(100)); // 100 * 0.10 = $10
        assert!(limits.validate_order(&order_at_max_size).is_ok());

        // Exactly at max value
        let order_at_max_value = OrderParams::buy_fok("token", dec!(0.50), dec!(100)); // 100 * 0.50 = $50
        assert!(limits.validate_order(&order_at_max_value).is_ok());
    }

    #[test]
    fn test_validate_order_just_over_limits() {
        let limits = HardLimits {
            max_order_size: dec!(100),
            min_order_size: dec!(10),
            max_price: dec!(0.90),
            min_price: dec!(0.10),
            max_order_value: dec!(50),
            max_daily_volume: dec!(100),
            min_balance_reserve: dec!(50),
        };

        // Just over max size
        let order_over_size = OrderParams::buy_fok("token", dec!(0.10), dec!(101));
        assert!(limits.validate_order(&order_over_size).is_err());

        // Just over max value
        let order_over_value = OrderParams::buy_fok("token", dec!(0.51), dec!(100)); // 100 * 0.51 = $51
        assert!(limits.validate_order(&order_over_value).is_err());
    }

    #[test]
    fn test_daily_volume_exactly_at_limit() {
        let limits = HardLimits {
            max_daily_volume: dec!(100),
            ..HardLimits::default()
        };

        // At limit should pass
        let order = OrderParams::buy_fok("token", dec!(0.50), dec!(20)); // $10 order
        assert!(limits.validate_daily_volume(&order, dec!(90)).is_ok()); // 90 + 10 = 100

        // Just over limit should fail
        assert!(limits.validate_daily_volume(&order, dec!(91)).is_err()); // 91 + 10 = 101
    }

    #[test]
    fn test_balance_reserve_exactly_at_limit() {
        let limits = HardLimits {
            min_balance_reserve: dec!(50),
            ..HardLimits::default()
        };

        // At limit should pass
        let order = OrderParams::buy_fok("token", dec!(0.50), dec!(100)); // $50 order
        assert!(limits.validate_balance_reserve(&order, dec!(100)).is_ok()); // 100 - 50 = 50 reserve

        // Just under should fail
        assert!(limits.validate_balance_reserve(&order, dec!(99)).is_err()); // 99 - 50 = 49 reserve
    }
}
