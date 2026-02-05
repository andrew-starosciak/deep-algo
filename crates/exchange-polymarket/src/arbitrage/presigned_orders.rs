//! Pre-signed order pool for low-latency execution.
//!
//! This module provides a pool of pre-signed orders at various price levels,
//! eliminating the ~5-10ms signing overhead on the hot path.
//!
//! # How It Works
//!
//! 1. On startup, sign orders at common price levels (0.30, 0.31, ... 0.50)
//! 2. When a signal arrives, select the matching pre-signed order
//! 3. Submit immediately (skip signing step)
//! 4. Refresh orders before they expire
//!
//! # Latency Improvement
//!
//! | Step | Without Pre-signing | With Pre-signing |
//! |------|---------------------|------------------|
//! | Sign order | ~5-10ms | 0ms (pre-done) |
//! | Submit HTTP | ~50-200ms | ~50-200ms |
//! | **Total** | ~55-210ms | ~50-200ms |
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::presigned_orders::PreSignedOrderPool;
//!
//! let pool = PreSignedOrderPool::new(
//!     "yes-token-123",
//!     Side::Buy,
//!     100, // shares per order
//!     signer,
//! );
//!
//! // Pre-sign orders at price levels
//! pool.refresh().await?;
//!
//! // Get pre-signed order for target price
//! if let Some(order) = pool.get_order_at_price(dec!(0.35)) {
//!     // Submit immediately - no signing needed!
//!     client.submit_presigned(order).await?;
//! }
//! ```

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

use super::execution::Side;

// =============================================================================
// Constants
// =============================================================================

/// Default price step for pre-signed orders (1 cent).
pub const DEFAULT_PRICE_STEP: Decimal = dec!(0.01);

/// Default minimum price for pre-signed orders.
pub const DEFAULT_MIN_PRICE: Decimal = dec!(0.25);

/// Default maximum price for pre-signed orders.
pub const DEFAULT_MAX_PRICE: Decimal = dec!(0.50);

/// Default order expiration (5 minutes).
pub const DEFAULT_EXPIRATION_SECS: i64 = 300;

/// Refresh orders when this close to expiration (1 minute).
pub const REFRESH_THRESHOLD_SECS: i64 = 60;

// =============================================================================
// Errors
// =============================================================================

/// Errors from the pre-signed order pool.
#[derive(Error, Debug)]
pub enum PreSignedError {
    /// No order available at the requested price.
    #[error("No pre-signed order available at price {0}")]
    NoPriceLevel(Decimal),

    /// Order at price level has expired.
    #[error("Pre-signed order at price {0} has expired")]
    Expired(Decimal),

    /// Order at price level was already used.
    #[error("Pre-signed order at price {0} was already used")]
    AlreadyUsed(Decimal),

    /// Signing failed.
    #[error("Failed to sign order: {0}")]
    SigningFailed(String),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

// =============================================================================
// Pre-Signed Order
// =============================================================================

/// A pre-signed order ready for immediate submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreSignedOrder {
    /// Token ID.
    pub token_id: String,

    /// Order side.
    pub side: Side,

    /// Price level (0.01 to 0.99).
    pub price: Decimal,

    /// Order size in shares.
    pub size: Decimal,

    /// Unique nonce for this order.
    pub nonce: String,

    /// Expiration timestamp (Unix seconds, 0 for no expiration).
    pub expiration: u64,

    /// EIP-712 signature.
    pub signature: String,

    /// When this order was signed.
    pub signed_at: DateTime<Utc>,

    /// Whether this order has been used.
    pub used: bool,
}

impl PreSignedOrder {
    /// Returns true if this order has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        if self.expiration == 0 {
            return false; // No expiration
        }
        let now = Utc::now().timestamp() as u64;
        now >= self.expiration
    }

    /// Returns true if this order is still valid (not expired, not used).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.used && !self.is_expired()
    }

    /// Returns seconds until expiration.
    #[must_use]
    pub fn seconds_until_expiration(&self) -> i64 {
        if self.expiration == 0 {
            return i64::MAX; // No expiration
        }
        let now = Utc::now().timestamp() as u64;
        self.expiration.saturating_sub(now) as i64
    }

    /// Returns true if this order should be refreshed (close to expiration).
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        self.seconds_until_expiration() < REFRESH_THRESHOLD_SECS
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the pre-signed order pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreSignedPoolConfig {
    /// Minimum price to pre-sign (e.g., 0.25).
    pub min_price: Decimal,

    /// Maximum price to pre-sign (e.g., 0.50).
    pub max_price: Decimal,

    /// Price step between levels (e.g., 0.01).
    pub price_step: Decimal,

    /// Order size in shares.
    pub order_size: Decimal,

    /// Expiration time in seconds (0 for no expiration).
    pub expiration_secs: i64,

    /// Whether to use neg-risk mode.
    pub neg_risk: bool,
}

impl Default for PreSignedPoolConfig {
    fn default() -> Self {
        Self {
            min_price: DEFAULT_MIN_PRICE,
            max_price: DEFAULT_MAX_PRICE,
            price_step: DEFAULT_PRICE_STEP,
            order_size: dec!(100),
            expiration_secs: DEFAULT_EXPIRATION_SECS,
            neg_risk: true,
        }
    }
}

impl PreSignedPoolConfig {
    /// Creates a config for the "cheap" entry range (0.25 to 0.45).
    #[must_use]
    pub fn entry_range() -> Self {
        Self {
            min_price: dec!(0.25),
            max_price: dec!(0.45),
            ..Default::default()
        }
    }

    /// Creates a config for hedge range (0.50 to 0.70).
    #[must_use]
    pub fn hedge_range() -> Self {
        Self {
            min_price: dec!(0.50),
            max_price: dec!(0.70),
            ..Default::default()
        }
    }

    /// Creates a config for full price range (0.01 to 0.99).
    #[must_use]
    pub fn full_range() -> Self {
        Self {
            min_price: dec!(0.01),
            max_price: dec!(0.99),
            ..Default::default()
        }
    }

    /// Sets the order size.
    #[must_use]
    pub fn with_size(mut self, size: Decimal) -> Self {
        self.order_size = size;
        self
    }

    /// Sets the expiration time.
    #[must_use]
    pub fn with_expiration_secs(mut self, secs: i64) -> Self {
        self.expiration_secs = secs;
        self
    }

    /// Sets the price step.
    #[must_use]
    pub fn with_price_step(mut self, step: Decimal) -> Self {
        self.price_step = step;
        self
    }

    /// Returns the number of price levels this config will generate.
    #[must_use]
    pub fn price_level_count(&self) -> usize {
        if self.price_step <= Decimal::ZERO {
            return 0;
        }
        let range = self.max_price - self.min_price;
        ((range / self.price_step).to_string().parse::<f64>().unwrap_or(0.0) as usize) + 1
    }

    /// Generates all price levels.
    pub fn price_levels(&self) -> Vec<Decimal> {
        let mut levels = Vec::new();
        let mut price = self.min_price;
        while price <= self.max_price {
            levels.push(price);
            price += self.price_step;
        }
        levels
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), PreSignedError> {
        if self.min_price <= Decimal::ZERO || self.min_price >= Decimal::ONE {
            return Err(PreSignedError::InvalidConfig(
                "min_price must be between 0 and 1".into(),
            ));
        }
        if self.max_price <= Decimal::ZERO || self.max_price >= Decimal::ONE {
            return Err(PreSignedError::InvalidConfig(
                "max_price must be between 0 and 1".into(),
            ));
        }
        if self.min_price >= self.max_price {
            return Err(PreSignedError::InvalidConfig(
                "min_price must be less than max_price".into(),
            ));
        }
        if self.price_step <= Decimal::ZERO {
            return Err(PreSignedError::InvalidConfig(
                "price_step must be positive".into(),
            ));
        }
        if self.order_size <= Decimal::ZERO {
            return Err(PreSignedError::InvalidConfig(
                "order_size must be positive".into(),
            ));
        }
        Ok(())
    }
}

// =============================================================================
// Order Signer Trait
// =============================================================================

/// Trait for signing orders.
///
/// This abstraction allows for both real signing (via wallet) and mock signing (for tests).
#[async_trait::async_trait]
pub trait OrderSigner: Send + Sync {
    /// Signs an order and returns the signature.
    async fn sign_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
        nonce: &str,
        expiration: u64,
    ) -> Result<String, PreSignedError>;

    /// Returns the signer's address.
    fn address(&self) -> &str;
}

/// Mock signer for testing.
#[derive(Debug, Clone)]
pub struct MockSigner {
    address: String,
}

impl MockSigner {
    /// Creates a new mock signer.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }
}

#[async_trait::async_trait]
impl OrderSigner for MockSigner {
    async fn sign_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
        nonce: &str,
        expiration: u64,
    ) -> Result<String, PreSignedError> {
        // Generate a deterministic mock signature
        Ok(format!(
            "mock_sig_{}_{:?}_{}_{}_{}_{}",
            token_id, side, price, size, nonce, expiration
        ))
    }

    fn address(&self) -> &str {
        &self.address
    }
}

// =============================================================================
// Pre-Signed Order Pool
// =============================================================================

/// A pool of pre-signed orders at various price levels.
///
/// This pool maintains a set of ready-to-submit orders, eliminating the
/// signing overhead on the critical execution path.
pub struct PreSignedOrderPool {
    /// Token ID for this pool.
    token_id: String,

    /// Order side (Buy or Sell).
    side: Side,

    /// Pool configuration.
    config: PreSignedPoolConfig,

    /// The order signer.
    signer: Arc<dyn OrderSigner>,

    /// Pre-signed orders indexed by price.
    orders: RwLock<HashMap<String, PreSignedOrder>>,

    /// Nonce counter for unique order IDs.
    nonce_counter: AtomicU64,

    /// Statistics: orders signed.
    orders_signed: AtomicU64,

    /// Statistics: orders used.
    orders_used: AtomicU64,
}

impl PreSignedOrderPool {
    /// Creates a new pre-signed order pool.
    pub fn new(
        token_id: impl Into<String>,
        side: Side,
        config: PreSignedPoolConfig,
        signer: Arc<dyn OrderSigner>,
    ) -> Result<Self, PreSignedError> {
        config.validate()?;

        Ok(Self {
            token_id: token_id.into(),
            side,
            config,
            signer,
            orders: RwLock::new(HashMap::new()),
            nonce_counter: AtomicU64::new(Utc::now().timestamp_millis() as u64),
            orders_signed: AtomicU64::new(0),
            orders_used: AtomicU64::new(0),
        })
    }

    /// Returns the token ID.
    #[must_use]
    pub fn token_id(&self) -> &str {
        &self.token_id
    }

    /// Returns the order side.
    #[must_use]
    pub fn side(&self) -> Side {
        self.side
    }

    /// Returns the pool configuration.
    #[must_use]
    pub fn config(&self) -> &PreSignedPoolConfig {
        &self.config
    }

    /// Generates a unique nonce.
    fn next_nonce(&self) -> String {
        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        format!("{:016x}", nonce)
    }

    /// Converts a price to a map key.
    fn price_key(price: Decimal) -> String {
        format!("{:.2}", price)
    }

    /// Signs and stores an order at a specific price level.
    async fn sign_order_at_price(&self, price: Decimal) -> Result<PreSignedOrder, PreSignedError> {
        let nonce = self.next_nonce();
        let expiration = if self.config.expiration_secs > 0 {
            (Utc::now().timestamp() + self.config.expiration_secs) as u64
        } else {
            0
        };

        let signature = self
            .signer
            .sign_order(
                &self.token_id,
                self.side,
                price,
                self.config.order_size,
                &nonce,
                expiration,
            )
            .await?;

        let order = PreSignedOrder {
            token_id: self.token_id.clone(),
            side: self.side,
            price,
            size: self.config.order_size,
            nonce,
            expiration,
            signature,
            signed_at: Utc::now(),
            used: false,
        };

        self.orders_signed.fetch_add(1, Ordering::Relaxed);

        Ok(order)
    }

    /// Refreshes all pre-signed orders.
    ///
    /// This signs orders at all configured price levels, replacing any
    /// expired or used orders.
    pub async fn refresh_all(&self) -> Result<usize, PreSignedError> {
        let levels = self.config.price_levels();
        let mut signed = 0;

        info!(
            token_id = %self.token_id,
            side = ?self.side,
            levels = levels.len(),
            "Refreshing pre-signed orders"
        );

        for price in levels {
            let order = self.sign_order_at_price(price).await?;
            let key = Self::price_key(price);
            self.orders.write().insert(key, order);
            signed += 1;
        }

        debug!(signed = signed, "Pre-signed orders refreshed");
        Ok(signed)
    }

    /// Refreshes orders that need it (expired or close to expiring).
    pub async fn refresh_needed(&self) -> Result<usize, PreSignedError> {
        let levels = self.config.price_levels();
        let mut refreshed = 0;

        for price in levels {
            let key = Self::price_key(price);
            let needs_refresh = {
                let orders = self.orders.read();
                orders
                    .get(&key)
                    .map(|o| o.needs_refresh() || !o.is_valid())
                    .unwrap_or(true)
            };

            if needs_refresh {
                let order = self.sign_order_at_price(price).await?;
                self.orders.write().insert(key, order);
                refreshed += 1;
            }
        }

        if refreshed > 0 {
            debug!(refreshed = refreshed, "Refreshed expiring orders");
        }

        Ok(refreshed)
    }

    /// Gets a pre-signed order at the exact price level.
    ///
    /// Returns None if no order exists at this price or if the order is invalid.
    #[must_use]
    pub fn get_order_at_price(&self, price: Decimal) -> Option<PreSignedOrder> {
        let key = Self::price_key(price);
        let orders = self.orders.read();
        orders.get(&key).filter(|o| o.is_valid()).cloned()
    }

    /// Gets a pre-signed order at or near the target price.
    ///
    /// Returns the order at the nearest valid price level at or below the target.
    /// This is useful for buy orders where you want the best available price.
    #[must_use]
    pub fn get_best_order_at_or_below(&self, max_price: Decimal) -> Option<PreSignedOrder> {
        let orders = self.orders.read();

        orders
            .values()
            .filter(|o| o.is_valid() && o.price <= max_price)
            .max_by(|a, b| a.price.cmp(&b.price))
            .cloned()
    }

    /// Gets a pre-signed order at or near the target price.
    ///
    /// Returns the order at the nearest valid price level at or above the target.
    /// This is useful for sell orders where you want the best available price.
    #[must_use]
    pub fn get_best_order_at_or_above(&self, min_price: Decimal) -> Option<PreSignedOrder> {
        let orders = self.orders.read();

        orders
            .values()
            .filter(|o| o.is_valid() && o.price >= min_price)
            .min_by(|a, b| a.price.cmp(&b.price))
            .cloned()
    }

    /// Marks an order at a price level as used.
    ///
    /// Used orders will be refreshed on the next refresh cycle.
    pub fn mark_used(&self, price: Decimal) -> bool {
        let key = Self::price_key(price);
        let mut orders = self.orders.write();
        if let Some(order) = orders.get_mut(&key) {
            order.used = true;
            self.orders_used.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Takes an order at a price level, removing it from the pool.
    ///
    /// The order is marked as used and a new one should be signed.
    pub fn take_order_at_price(&self, price: Decimal) -> Option<PreSignedOrder> {
        let key = Self::price_key(price);
        let mut orders = self.orders.write();
        if let Some(order) = orders.get_mut(&key) {
            if order.is_valid() {
                order.used = true;
                self.orders_used.fetch_add(1, Ordering::Relaxed);
                return Some(order.clone());
            }
        }
        None
    }

    /// Returns the number of valid orders in the pool.
    #[must_use]
    pub fn valid_count(&self) -> usize {
        self.orders.read().values().filter(|o| o.is_valid()).count()
    }

    /// Returns the total number of orders in the pool (including invalid).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.orders.read().len()
    }

    /// Returns pool statistics.
    #[must_use]
    pub fn stats(&self) -> PreSignedPoolStats {
        let orders = self.orders.read();
        let valid = orders.values().filter(|o| o.is_valid()).count();
        let expired = orders.values().filter(|o| o.is_expired()).count();
        let used = orders.values().filter(|o| o.used).count();

        PreSignedPoolStats {
            token_id: self.token_id.clone(),
            side: self.side,
            total_orders: orders.len(),
            valid_orders: valid,
            expired_orders: expired,
            used_orders: used,
            total_signed: self.orders_signed.load(Ordering::Relaxed),
            total_used: self.orders_used.load(Ordering::Relaxed),
        }
    }

    /// Clears all orders from the pool.
    pub fn clear(&self) {
        self.orders.write().clear();
    }
}

/// Statistics for a pre-signed order pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreSignedPoolStats {
    /// Token ID.
    pub token_id: String,
    /// Order side.
    pub side: Side,
    /// Total orders in pool.
    pub total_orders: usize,
    /// Valid (usable) orders.
    pub valid_orders: usize,
    /// Expired orders.
    pub expired_orders: usize,
    /// Used orders.
    pub used_orders: usize,
    /// Total orders signed since creation.
    pub total_signed: u64,
    /// Total orders used since creation.
    pub total_used: u64,
}

// =============================================================================
// Multi-Pool Manager
// =============================================================================

/// Manages pre-signed order pools for multiple tokens (YES and NO).
pub struct PreSignedPoolManager {
    /// YES token buy orders pool.
    yes_buy: Option<PreSignedOrderPool>,
    /// YES token sell orders pool.
    yes_sell: Option<PreSignedOrderPool>,
    /// NO token buy orders pool.
    no_buy: Option<PreSignedOrderPool>,
    /// NO token sell orders pool.
    no_sell: Option<PreSignedOrderPool>,
}

impl PreSignedPoolManager {
    /// Creates a new manager with pools for both tokens.
    pub fn new(
        yes_token_id: &str,
        no_token_id: &str,
        buy_config: PreSignedPoolConfig,
        sell_config: PreSignedPoolConfig,
        signer: Arc<dyn OrderSigner>,
    ) -> Result<Self, PreSignedError> {
        Ok(Self {
            yes_buy: Some(PreSignedOrderPool::new(
                yes_token_id,
                Side::Buy,
                buy_config.clone(),
                signer.clone(),
            )?),
            yes_sell: Some(PreSignedOrderPool::new(
                yes_token_id,
                Side::Sell,
                sell_config.clone(),
                signer.clone(),
            )?),
            no_buy: Some(PreSignedOrderPool::new(
                no_token_id,
                Side::Buy,
                buy_config,
                signer.clone(),
            )?),
            no_sell: Some(PreSignedOrderPool::new(
                no_token_id,
                Side::Sell,
                sell_config,
                signer,
            )?),
        })
    }

    /// Creates a manager for buy orders only (most common for gabagool).
    pub fn buy_only(
        yes_token_id: &str,
        no_token_id: &str,
        config: PreSignedPoolConfig,
        signer: Arc<dyn OrderSigner>,
    ) -> Result<Self, PreSignedError> {
        Ok(Self {
            yes_buy: Some(PreSignedOrderPool::new(
                yes_token_id,
                Side::Buy,
                config.clone(),
                signer.clone(),
            )?),
            yes_sell: None,
            no_buy: Some(PreSignedOrderPool::new(
                no_token_id,
                Side::Buy,
                config,
                signer,
            )?),
            no_sell: None,
        })
    }

    /// Refreshes all pools.
    pub async fn refresh_all(&self) -> Result<usize, PreSignedError> {
        let mut total = 0;

        if let Some(ref pool) = self.yes_buy {
            total += pool.refresh_all().await?;
        }
        if let Some(ref pool) = self.yes_sell {
            total += pool.refresh_all().await?;
        }
        if let Some(ref pool) = self.no_buy {
            total += pool.refresh_all().await?;
        }
        if let Some(ref pool) = self.no_sell {
            total += pool.refresh_all().await?;
        }

        info!(total = total, "Refreshed all pre-signed order pools");
        Ok(total)
    }

    /// Refreshes orders that need it across all pools.
    pub async fn refresh_needed(&self) -> Result<usize, PreSignedError> {
        let mut total = 0;

        if let Some(ref pool) = self.yes_buy {
            total += pool.refresh_needed().await?;
        }
        if let Some(ref pool) = self.yes_sell {
            total += pool.refresh_needed().await?;
        }
        if let Some(ref pool) = self.no_buy {
            total += pool.refresh_needed().await?;
        }
        if let Some(ref pool) = self.no_sell {
            total += pool.refresh_needed().await?;
        }

        Ok(total)
    }

    /// Gets the YES buy pool.
    #[must_use]
    pub fn yes_buy(&self) -> Option<&PreSignedOrderPool> {
        self.yes_buy.as_ref()
    }

    /// Gets the NO buy pool.
    #[must_use]
    pub fn no_buy(&self) -> Option<&PreSignedOrderPool> {
        self.no_buy.as_ref()
    }

    /// Gets the YES sell pool.
    #[must_use]
    pub fn yes_sell(&self) -> Option<&PreSignedOrderPool> {
        self.yes_sell.as_ref()
    }

    /// Gets the NO sell pool.
    #[must_use]
    pub fn no_sell(&self) -> Option<&PreSignedOrderPool> {
        self.no_sell.as_ref()
    }

    /// Gets a pre-signed buy order for YES at the given price.
    #[must_use]
    pub fn get_yes_buy(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.yes_buy.as_ref()?.get_order_at_price(price)
    }

    /// Gets a pre-signed buy order for NO at the given price.
    #[must_use]
    pub fn get_no_buy(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.no_buy.as_ref()?.get_order_at_price(price)
    }

    /// Takes a YES buy order at the given price.
    pub fn take_yes_buy(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.yes_buy.as_ref()?.take_order_at_price(price)
    }

    /// Takes a NO buy order at the given price.
    pub fn take_no_buy(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.no_buy.as_ref()?.take_order_at_price(price)
    }

    /// Takes a YES sell order at the given price.
    pub fn take_yes_sell(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.yes_sell.as_ref()?.take_order_at_price(price)
    }

    /// Takes a NO sell order at the given price.
    pub fn take_no_sell(&self, price: Decimal) -> Option<PreSignedOrder> {
        self.no_sell.as_ref()?.take_order_at_price(price)
    }

    /// Returns statistics for all pools.
    #[must_use]
    pub fn stats(&self) -> Vec<PreSignedPoolStats> {
        let mut stats = Vec::new();
        if let Some(ref pool) = self.yes_buy {
            stats.push(pool.stats());
        }
        if let Some(ref pool) = self.yes_sell {
            stats.push(pool.stats());
        }
        if let Some(ref pool) = self.no_buy {
            stats.push(pool.stats());
        }
        if let Some(ref pool) = self.no_sell {
            stats.push(pool.stats());
        }
        stats
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = PreSignedPoolConfig::default();
        assert_eq!(config.min_price, dec!(0.25));
        assert_eq!(config.max_price, dec!(0.50));
        assert_eq!(config.price_step, dec!(0.01));
        assert_eq!(config.order_size, dec!(100));
    }

    #[test]
    fn test_config_entry_range() {
        let config = PreSignedPoolConfig::entry_range();
        assert_eq!(config.min_price, dec!(0.25));
        assert_eq!(config.max_price, dec!(0.45));
    }

    #[test]
    fn test_config_hedge_range() {
        let config = PreSignedPoolConfig::hedge_range();
        assert_eq!(config.min_price, dec!(0.50));
        assert_eq!(config.max_price, dec!(0.70));
    }

    #[test]
    fn test_config_price_level_count() {
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.40),
            price_step: dec!(0.01),
            ..Default::default()
        };
        // 0.30, 0.31, ..., 0.40 = 11 levels
        assert_eq!(config.price_level_count(), 11);
    }

    #[test]
    fn test_config_price_levels() {
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.33),
            price_step: dec!(0.01),
            ..Default::default()
        };
        let levels = config.price_levels();
        assert_eq!(levels, vec![dec!(0.30), dec!(0.31), dec!(0.32), dec!(0.33)]);
    }

    #[test]
    fn test_config_validate_success() {
        let config = PreSignedPoolConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_invalid_min_price() {
        let config = PreSignedPoolConfig {
            min_price: dec!(0),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_min_greater_than_max() {
        let config = PreSignedPoolConfig {
            min_price: dec!(0.50),
            max_price: dec!(0.30),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_zero_step() {
        let config = PreSignedPoolConfig {
            price_step: dec!(0),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    // =========================================================================
    // PreSignedOrder Tests
    // =========================================================================

    #[test]
    fn test_order_not_expired() {
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: (Utc::now().timestamp() + 300) as u64,
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: false,
        };
        assert!(!order.is_expired());
        assert!(order.is_valid());
    }

    #[test]
    fn test_order_expired() {
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: (Utc::now().timestamp() - 10) as u64, // Expired 10 seconds ago
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: false,
        };
        assert!(order.is_expired());
        assert!(!order.is_valid());
    }

    #[test]
    fn test_order_no_expiration() {
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: 0, // No expiration
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: false,
        };
        assert!(!order.is_expired());
        assert!(order.is_valid());
    }

    #[test]
    fn test_order_used() {
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: (Utc::now().timestamp() + 300) as u64,
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: true, // Already used
        };
        assert!(!order.is_expired());
        assert!(!order.is_valid()); // Used orders are not valid
    }

    #[test]
    fn test_order_needs_refresh() {
        // Order expiring in 30 seconds (below 60 second threshold)
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: (Utc::now().timestamp() + 30) as u64,
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: false,
        };
        assert!(order.needs_refresh());
    }

    #[test]
    fn test_order_does_not_need_refresh() {
        // Order expiring in 120 seconds (above 60 second threshold)
        let order = PreSignedOrder {
            token_id: "test".into(),
            side: Side::Buy,
            price: dec!(0.35),
            size: dec!(100),
            nonce: "123".into(),
            expiration: (Utc::now().timestamp() + 120) as u64,
            signature: "sig".into(),
            signed_at: Utc::now(),
            used: false,
        };
        assert!(!order.needs_refresh());
    }

    // =========================================================================
    // Pool Tests
    // =========================================================================

    #[tokio::test]
    async fn test_pool_creation() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();

        assert_eq!(pool.token_id(), "token123");
        assert_eq!(pool.side(), Side::Buy);
        assert_eq!(pool.valid_count(), 0); // Not refreshed yet
    }

    #[tokio::test]
    async fn test_pool_refresh_all() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            order_size: dec!(50),
            expiration_secs: 300,
            neg_risk: true,
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();

        let signed = pool.refresh_all().await.unwrap();
        assert_eq!(signed, 6); // 0.30, 0.31, 0.32, 0.33, 0.34, 0.35
        assert_eq!(pool.valid_count(), 6);
    }

    #[tokio::test]
    async fn test_pool_get_order_at_price() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();
        pool.refresh_all().await.unwrap();

        // Get existing price
        let order = pool.get_order_at_price(dec!(0.32));
        assert!(order.is_some());
        let order = order.unwrap();
        assert_eq!(order.price, dec!(0.32));
        assert_eq!(order.token_id, "token123");

        // Non-existent price
        let order = pool.get_order_at_price(dec!(0.29));
        assert!(order.is_none());
    }

    #[tokio::test]
    async fn test_pool_take_order() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();
        pool.refresh_all().await.unwrap();

        // Take order
        let order = pool.take_order_at_price(dec!(0.32));
        assert!(order.is_some());
        assert_eq!(pool.valid_count(), 5); // One less valid

        // Try to take same order again - should fail
        let order = pool.take_order_at_price(dec!(0.32));
        assert!(order.is_none());
    }

    #[tokio::test]
    async fn test_pool_get_best_order_at_or_below() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.40),
            price_step: dec!(0.02),
            ..Default::default()
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();
        pool.refresh_all().await.unwrap();

        // Should get 0.34 (highest at or below 0.35)
        let order = pool.get_best_order_at_or_below(dec!(0.35));
        assert!(order.is_some());
        assert_eq!(order.unwrap().price, dec!(0.34));

        // Should get 0.40 (exact match)
        let order = pool.get_best_order_at_or_below(dec!(0.40));
        assert!(order.is_some());
        assert_eq!(order.unwrap().price, dec!(0.40));

        // Should get nothing (below range)
        let order = pool.get_best_order_at_or_below(dec!(0.29));
        assert!(order.is_none());
    }

    #[tokio::test]
    async fn test_pool_stats() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.32),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let pool = PreSignedOrderPool::new("token123", Side::Buy, config, signer).unwrap();
        pool.refresh_all().await.unwrap();

        // Take one
        pool.take_order_at_price(dec!(0.31));

        let stats = pool.stats();
        assert_eq!(stats.total_orders, 3);
        assert_eq!(stats.valid_orders, 2);
        assert_eq!(stats.used_orders, 1);
        assert_eq!(stats.total_signed, 3);
        assert_eq!(stats.total_used, 1);
    }

    // =========================================================================
    // Manager Tests
    // =========================================================================

    #[tokio::test]
    async fn test_manager_buy_only() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let manager =
            PreSignedPoolManager::buy_only("yes-token", "no-token", config, signer).unwrap();

        // Should have buy pools
        assert!(manager.yes_buy().is_some());
        assert!(manager.no_buy().is_some());

        // Should not have sell pools
        assert!(manager.yes_sell().is_none());
        assert!(manager.no_sell().is_none());
    }

    #[tokio::test]
    async fn test_manager_refresh_all() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.32),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let manager =
            PreSignedPoolManager::buy_only("yes-token", "no-token", config, signer).unwrap();

        let total = manager.refresh_all().await.unwrap();
        assert_eq!(total, 6); // 3 levels * 2 pools

        assert_eq!(manager.yes_buy().unwrap().valid_count(), 3);
        assert_eq!(manager.no_buy().unwrap().valid_count(), 3);
    }

    #[tokio::test]
    async fn test_manager_get_and_take_orders() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.35),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let manager =
            PreSignedPoolManager::buy_only("yes-token", "no-token", config, signer).unwrap();
        manager.refresh_all().await.unwrap();

        // Get YES buy
        let order = manager.get_yes_buy(dec!(0.32));
        assert!(order.is_some());
        assert_eq!(order.unwrap().token_id, "yes-token");

        // Get NO buy
        let order = manager.get_no_buy(dec!(0.33));
        assert!(order.is_some());
        assert_eq!(order.unwrap().token_id, "no-token");

        // Take YES buy
        let order = manager.take_yes_buy(dec!(0.32));
        assert!(order.is_some());

        // Can't get it again
        let order = manager.get_yes_buy(dec!(0.32));
        assert!(order.is_none());
    }

    #[tokio::test]
    async fn test_manager_stats() {
        let signer = Arc::new(MockSigner::new("0x1234"));
        let config = PreSignedPoolConfig {
            min_price: dec!(0.30),
            max_price: dec!(0.32),
            price_step: dec!(0.01),
            ..Default::default()
        };

        let manager =
            PreSignedPoolManager::buy_only("yes-token", "no-token", config, signer).unwrap();
        manager.refresh_all().await.unwrap();

        let stats = manager.stats();
        assert_eq!(stats.len(), 2); // 2 pools (YES buy, NO buy)

        let total_valid: usize = stats.iter().map(|s| s.valid_orders).sum();
        assert_eq!(total_valid, 6); // 3 levels * 2 pools
    }

    // =========================================================================
    // Mock Signer Tests
    // =========================================================================

    #[tokio::test]
    async fn test_mock_signer() {
        let signer = MockSigner::new("0xAbCdEf1234");
        assert_eq!(signer.address(), "0xAbCdEf1234");

        let sig = signer
            .sign_order("token", Side::Buy, dec!(0.35), dec!(100), "nonce1", 12345)
            .await
            .unwrap();

        assert!(sig.starts_with("mock_sig_"));
        assert!(sig.contains("token"));
        assert!(sig.contains("0.35"));
    }
}
