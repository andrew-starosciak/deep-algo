//! Automated execution bridge for gabagool signals.
//!
//! This module connects the signal detection pipeline (`GabagoolRunner`) to the
//! order execution layer (`PolymarketExecutor`), enabling fully automated trading.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────┐         ┌──────────────────────┐
//! │   GabagoolRunner     │ signals │   AutoExecutor       │
//! │   (produces signals) │─────────▶ - Kelly sizing       │
//! └──────────────────────┘  mpsc   │ - Position tracking  │
//!                                  │ - P&L tracking       │
//!                                  │         ↓            │
//!                                  ┌──────────────────────┐
//!                                  │  PolymarketExecutor  │
//!                                  │  submit_order()      │
//!                                  └──────────────────────┘
//! ```
//!
//! # Safety Features
//!
//! - Kelly criterion position sizing with configurable fraction
//! - Position limits per window
//! - Daily loss limits (via circuit breaker)
//! - Graceful shutdown handling
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::auto_executor::{AutoExecutor, AutoExecutorConfig};
//! use algo_trade_polymarket::arbitrage::{GabagoolRunner, GabagoolRunnerConfig, PaperExecutor};
//!
//! // Create components
//! let runner_config = GabagoolRunnerConfig { ... };
//! let (runner, signal_rx) = GabagoolRunner::new(runner_config);
//!
//! let executor = PaperExecutor::default();
//! let auto_config = AutoExecutorConfig::default();
//! let auto_executor = AutoExecutor::new(executor, auto_config);
//!
//! // Run both concurrently
//! tokio::spawn(runner.run());
//! auto_executor.run(signal_rx).await;
//! ```

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::execution::{
    ExecutionError, OrderParams, OrderResult, OrderStatus, PolymarketExecutor, PresignedData, Side,
};
use super::gabagool_detector::{
    GabagoolDirection, GabagoolSignal, GabagoolSignalType, OpenPosition, SignalConfidence,
};
use super::position_persistence::PositionPersistence;
use super::presigned_orders::PreSignedPoolManager;

// =============================================================================
// Errors
// =============================================================================

/// Errors from the auto executor.
#[derive(Error, Debug)]
pub enum AutoExecutorError {
    /// Execution error from underlying executor.
    #[error("Execution error: {0}")]
    Execution(#[from] ExecutionError),

    /// Position limit exceeded.
    #[error("Position limit exceeded: current {current}, limit {limit}")]
    PositionLimit { current: Decimal, limit: Decimal },

    /// Insufficient balance.
    #[error("Insufficient balance: need {required}, have {available}")]
    InsufficientBalance {
        required: Decimal,
        available: Decimal,
    },

    /// Signal channel closed.
    #[error("Signal channel closed")]
    ChannelClosed,

    /// IO error (for persistence).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the auto executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoExecutorConfig {
    /// Kelly fraction (0.0 to 1.0). Default: 0.25 (quarter Kelly).
    pub kelly_fraction: f64,

    /// Fixed bet size in USDC (overrides Kelly if set).
    pub fixed_bet_size: Option<Decimal>,

    /// Minimum bet size in USDC.
    pub min_bet_size: Decimal,

    /// Maximum bet size in USDC.
    pub max_bet_size: Decimal,

    /// Minimum edge required to execute (0.0 to 1.0).
    pub min_edge: f64,

    /// Maximum position value per window.
    pub max_position_per_window: Decimal,

    /// Whether to execute Entry signals.
    pub execute_entries: bool,

    /// Whether to execute Hedge signals.
    pub execute_hedges: bool,

    /// Whether to execute Scratch signals.
    pub execute_scratches: bool,

    /// Path to persist trade history (optional).
    pub history_path: Option<PathBuf>,

    /// Path to persist positions for restart recovery (optional).
    pub persistence_path: Option<PathBuf>,

    /// Maximum trade history to keep in memory.
    pub max_history: usize,

    /// YES token ID for the market.
    pub yes_token_id: String,

    /// NO token ID for the market.
    pub no_token_id: String,
}

impl Default for AutoExecutorConfig {
    fn default() -> Self {
        Self {
            kelly_fraction: 0.25,
            fixed_bet_size: None,
            min_bet_size: dec!(5),
            max_bet_size: dec!(100),
            min_edge: 0.02, // 2% minimum edge
            max_position_per_window: dec!(200),
            execute_entries: true,
            execute_hedges: true,
            execute_scratches: true,
            history_path: None,
            persistence_path: None,
            max_history: 1000,
            yes_token_id: String::new(),
            no_token_id: String::new(),
        }
    }
}

impl AutoExecutorConfig {
    /// Creates a micro testing configuration with tight limits.
    #[must_use]
    pub fn micro_testing() -> Self {
        Self {
            kelly_fraction: 0.10,
            fixed_bet_size: Some(dec!(10)),
            min_bet_size: dec!(5),
            max_bet_size: dec!(25),
            min_edge: 0.01,
            max_position_per_window: dec!(50),
            execute_entries: true,
            execute_hedges: true,
            execute_scratches: true,
            history_path: None,
            persistence_path: None,
            max_history: 100,
            yes_token_id: String::new(),
            no_token_id: String::new(),
        }
    }

    /// Creates a conservative configuration.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            kelly_fraction: 0.125, // Eighth Kelly
            fixed_bet_size: None,
            min_bet_size: dec!(10),
            max_bet_size: dec!(50),
            min_edge: 0.05, // 5% minimum edge
            max_position_per_window: dec!(100),
            execute_entries: true,
            execute_hedges: true,
            execute_scratches: true,
            history_path: None,
            persistence_path: None,
            max_history: 500,
            yes_token_id: String::new(),
            no_token_id: String::new(),
        }
    }

    /// Sets the YES token ID.
    #[must_use]
    pub fn with_yes_token(mut self, token_id: impl Into<String>) -> Self {
        self.yes_token_id = token_id.into();
        self
    }

    /// Sets the NO token ID.
    #[must_use]
    pub fn with_no_token(mut self, token_id: impl Into<String>) -> Self {
        self.no_token_id = token_id.into();
        self
    }

    /// Sets the fixed bet size.
    #[must_use]
    pub fn with_fixed_bet(mut self, size: Decimal) -> Self {
        self.fixed_bet_size = Some(size);
        self
    }

    /// Sets the Kelly fraction.
    #[must_use]
    pub fn with_kelly_fraction(mut self, fraction: f64) -> Self {
        self.kelly_fraction = fraction;
        self
    }

    /// Sets the history export path.
    #[must_use]
    pub fn with_history_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.history_path = Some(path.into());
        self
    }

    /// Sets the position persistence path for restart recovery.
    #[must_use]
    pub fn with_persistence_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.persistence_path = Some(path.into());
        self
    }
}

// =============================================================================
// Trade Record
// =============================================================================

/// A record of an executed trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Unique trade ID.
    pub trade_id: String,

    /// The signal that triggered this trade.
    pub signal_type: GabagoolSignalType,

    /// Direction (YES or NO).
    pub direction: GabagoolDirection,

    /// Order side (BUY or SELL).
    pub side: Side,

    /// Requested price.
    pub price: Decimal,

    /// Requested size in shares.
    pub size: Decimal,

    /// Filled size.
    pub filled_size: Decimal,

    /// Average fill price.
    pub avg_fill_price: Option<Decimal>,

    /// Order status.
    pub status: OrderStatus,

    /// Timestamp of signal.
    pub signal_timestamp: DateTime<Utc>,

    /// Timestamp of execution.
    pub execution_timestamp: DateTime<Utc>,

    /// Window this trade belongs to.
    pub window_start_ms: i64,

    /// Estimated edge at time of signal.
    pub estimated_edge: f64,

    /// Confidence level.
    pub confidence: SignalConfidence,
}

// =============================================================================
// Statistics
// =============================================================================

/// Statistics for the auto executor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoExecutorStats {
    /// Total signals received.
    pub signals_received: u64,

    /// Signals skipped (below threshold, etc.).
    pub signals_skipped: u64,

    /// Orders attempted.
    pub orders_attempted: u64,

    /// Orders filled.
    pub orders_filled: u64,

    /// Orders partially filled.
    pub orders_partial: u64,

    /// Orders rejected/failed.
    pub orders_failed: u64,

    /// Entry trades executed.
    pub entry_trades: u64,

    /// Hedge trades executed.
    pub hedge_trades: u64,

    /// Scratch trades executed.
    pub scratch_trades: u64,

    /// Total volume traded (USDC).
    pub total_volume: Decimal,

    /// Current position value.
    pub current_position_value: Decimal,

    /// Realized P&L (from scratches and settlements).
    pub realized_pnl: Decimal,

    /// Start time.
    pub started_at: Option<DateTime<Utc>>,

    /// Last trade time.
    pub last_trade_time: Option<DateTime<Utc>>,
}

// =============================================================================
// Kelly Sizer
// =============================================================================

/// Calculates position size using Kelly criterion.
pub struct KellySizer {
    /// Kelly fraction to use (0.0 to 1.0).
    fraction: f64,
    /// Minimum bet size.
    min_size: Decimal,
    /// Maximum bet size.
    max_size: Decimal,
}

impl KellySizer {
    /// Creates a new Kelly sizer.
    #[must_use]
    pub fn new(fraction: f64, min_size: Decimal, max_size: Decimal) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
            min_size,
            max_size,
        }
    }

    /// Calculates the optimal bet size.
    ///
    /// Uses the formula: f* = (p(b+1) - 1) / b
    /// Where:
    /// - p = win probability (estimated from edge)
    /// - b = net odds = (1 - price) / price
    ///
    /// # Arguments
    /// * `edge` - Estimated edge (p - price)
    /// * `price` - Entry price (cost per share)
    /// * `bankroll` - Available bankroll
    ///
    /// # Returns
    /// Recommended bet size in USDC, or None if no bet recommended.
    #[must_use]
    pub fn size(&self, edge: f64, price: Decimal, bankroll: Decimal) -> Option<Decimal> {
        // Convert to f64 for calculation
        let price_f64 = price.to_string().parse::<f64>().unwrap_or(0.5);

        // Estimated win probability
        let p = price_f64 + edge;

        // Net odds: b = (1 - price) / price
        if price_f64 <= 0.0 || price_f64 >= 1.0 {
            return None;
        }
        let b = (1.0 - price_f64) / price_f64;

        // Full Kelly: f* = (p(b+1) - 1) / b
        let full_kelly = (p * (b + 1.0) - 1.0) / b;

        // No bet if Kelly is negative (no edge)
        if full_kelly <= 0.0 {
            return None;
        }

        // Apply fraction
        let kelly_fraction = full_kelly * self.fraction;

        // Convert bankroll to f64
        let bankroll_f64 = bankroll.to_string().parse::<f64>().unwrap_or(0.0);

        // Calculate bet size
        let bet_f64 = bankroll_f64 * kelly_fraction;

        // Convert back to Decimal
        let bet = Decimal::from_f64_retain(bet_f64)?;

        // If calculated bet is below minimum, no bet
        if bet < self.min_size {
            return None;
        }

        // Apply maximum and bankroll limits
        let bet = bet.min(self.max_size).min(bankroll);

        Some(bet)
    }
}

// =============================================================================
// Position Tracker
// =============================================================================

/// Tracks open positions for the current window.
#[derive(Debug, Clone, Default)]
pub struct WindowPositionTracker {
    /// Current window start timestamp.
    pub window_start_ms: i64,

    /// YES position for this window.
    pub yes_position: Option<OpenPosition>,

    /// NO position for this window.
    pub no_position: Option<OpenPosition>,

    /// Total cost invested this window.
    pub total_cost: Decimal,
}

impl WindowPositionTracker {
    /// Creates a new tracker for the given window.
    #[must_use]
    pub fn new(window_start_ms: i64) -> Self {
        Self {
            window_start_ms,
            yes_position: None,
            no_position: None,
            total_cost: Decimal::ZERO,
        }
    }

    /// Records an entry position.
    pub fn record_entry(&mut self, position: OpenPosition) {
        let cost = position.entry_price * position.quantity;
        match position.direction {
            GabagoolDirection::Yes => {
                self.yes_position = Some(position);
            }
            GabagoolDirection::No => {
                self.no_position = Some(position);
            }
        }
        self.total_cost += cost;
    }

    /// Records a hedge (second leg).
    pub fn record_hedge(&mut self, position: OpenPosition) {
        let cost = position.entry_price * position.quantity;
        match position.direction {
            GabagoolDirection::Yes => {
                self.yes_position = Some(position);
            }
            GabagoolDirection::No => {
                self.no_position = Some(position);
            }
        }
        self.total_cost += cost;
    }

    /// Clears positions (on scratch or window end).
    pub fn clear(&mut self) {
        self.yes_position = None;
        self.no_position = None;
        self.total_cost = Decimal::ZERO;
    }

    /// Returns true if there's an open position.
    #[must_use]
    pub fn has_position(&self) -> bool {
        self.yes_position.is_some() || self.no_position.is_some()
    }

    /// Returns true if hedged (both sides have positions).
    #[must_use]
    pub fn is_hedged(&self) -> bool {
        self.yes_position.is_some() && self.no_position.is_some()
    }

    /// Returns the open position direction if unhedged.
    #[must_use]
    pub fn open_direction(&self) -> Option<GabagoolDirection> {
        match (&self.yes_position, &self.no_position) {
            (Some(_), None) => Some(GabagoolDirection::Yes),
            (None, Some(_)) => Some(GabagoolDirection::No),
            _ => None,
        }
    }
}

// =============================================================================
// Auto Executor
// =============================================================================

/// Automated execution bridge for gabagool signals.
///
/// Consumes signals from `GabagoolRunner` and executes them via `PolymarketExecutor`.
pub struct AutoExecutor<E: PolymarketExecutor> {
    /// The underlying executor.
    executor: E,

    /// Configuration.
    config: AutoExecutorConfig,

    /// Kelly position sizer.
    sizer: KellySizer,

    /// Current window position tracker.
    position: Arc<RwLock<WindowPositionTracker>>,

    /// Execution statistics.
    stats: Arc<RwLock<AutoExecutorStats>>,

    /// Trade history.
    history: Arc<RwLock<VecDeque<TradeRecord>>>,

    /// Stop flag.
    should_stop: Arc<AtomicBool>,

    /// Position persistence handler (optional).
    persistence: Option<PositionPersistence>,

    /// Pre-signed order pool for low-latency execution (optional).
    presigned_pool: Option<Arc<RwLock<PreSignedPoolManager>>>,
}

impl<E: PolymarketExecutor> AutoExecutor<E> {
    /// Creates a new auto executor.
    ///
    /// If `persistence_path` is configured, attempts to load existing positions.
    /// Note: Positions will be validated against the current window on first signal.
    pub fn new(executor: E, config: AutoExecutorConfig) -> Self {
        let sizer = KellySizer::new(
            config.kelly_fraction,
            config.min_bet_size,
            config.max_bet_size,
        );

        // Set up persistence if configured
        let persistence = config
            .persistence_path
            .as_ref()
            .map(|p| PositionPersistence::new(p.clone()));

        // Try to load existing positions if persistence is configured
        // Use window 0 initially - will be validated on first signal
        let initial_position = if let Some(ref p) = persistence {
            match p.load_raw() {
                Ok(persisted) => {
                    if persisted.yes_position.is_some() || persisted.no_position.is_some() {
                        info!(
                            window_ms = persisted.window_start_ms,
                            has_yes = persisted.yes_position.is_some(),
                            has_no = persisted.no_position.is_some(),
                            total_cost = %persisted.total_cost,
                            "Loaded persisted positions on startup (will validate on first signal)"
                        );
                    }
                    persisted.into_tracker()
                }
                Err(e) => {
                    debug!(error = %e, "No persisted positions found, starting fresh");
                    WindowPositionTracker::default()
                }
            }
        } else {
            WindowPositionTracker::default()
        };

        Self {
            executor,
            config,
            sizer,
            position: Arc::new(RwLock::new(initial_position)),
            stats: Arc::new(RwLock::new(AutoExecutorStats::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            should_stop: Arc::new(AtomicBool::new(false)),
            persistence,
            presigned_pool: None,
        }
    }

    /// Creates a new auto executor with a specific initial window.
    ///
    /// This loads persisted positions for the given window, clearing stale positions.
    pub fn new_with_window(
        executor: E,
        config: AutoExecutorConfig,
        current_window_ms: i64,
    ) -> Self {
        let sizer = KellySizer::new(
            config.kelly_fraction,
            config.min_bet_size,
            config.max_bet_size,
        );

        // Set up persistence if configured
        let persistence = config
            .persistence_path
            .as_ref()
            .map(|p| PositionPersistence::new(p.clone()));

        // Load positions with window validation
        let initial_position = if let Some(ref p) = persistence {
            match p.load(current_window_ms) {
                Ok(tracker) => {
                    if tracker.has_position() {
                        info!(
                            window_ms = tracker.window_start_ms,
                            has_yes = tracker.yes_position.is_some(),
                            has_no = tracker.no_position.is_some(),
                            total_cost = %tracker.total_cost,
                            "Loaded persisted positions for current window"
                        );
                    }
                    tracker
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load persisted positions, starting fresh");
                    WindowPositionTracker::new(current_window_ms)
                }
            }
        } else {
            WindowPositionTracker::new(current_window_ms)
        };

        Self {
            executor,
            config,
            sizer,
            position: Arc::new(RwLock::new(initial_position)),
            stats: Arc::new(RwLock::new(AutoExecutorStats::default())),
            history: Arc::new(RwLock::new(VecDeque::new())),
            should_stop: Arc::new(AtomicBool::new(false)),
            persistence,
            presigned_pool: None,
        }
    }

    /// Returns a handle to stop the executor.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.should_stop.clone()
    }

    /// Returns the shared stats.
    #[must_use]
    pub fn stats(&self) -> Arc<RwLock<AutoExecutorStats>> {
        self.stats.clone()
    }

    /// Returns the trade history.
    #[must_use]
    pub fn history(&self) -> Arc<RwLock<VecDeque<TradeRecord>>> {
        self.history.clone()
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &AutoExecutorConfig {
        &self.config
    }

    /// Sets the pre-signed order pool for low-latency execution.
    ///
    /// When set, the executor will attempt to use pre-signed orders first,
    /// falling back to regular signing if no suitable pre-signed order is available.
    pub fn set_presigned_pool(&mut self, pool: PreSignedPoolManager) {
        self.presigned_pool = Some(Arc::new(RwLock::new(pool)));
    }

    /// Returns whether a pre-signed order pool is configured.
    #[must_use]
    pub fn has_presigned_pool(&self) -> bool {
        self.presigned_pool.is_some()
    }

    /// Tries to get a pre-signed order at the given price for the given token and side.
    ///
    /// Returns `Some(PresignedData)` if a suitable pre-signed order is available and valid,
    /// `None` otherwise (will fall back to regular signing).
    async fn try_get_presigned(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
    ) -> Option<PresignedData> {
        let pool = self.presigned_pool.as_ref()?;
        let pool_guard = pool.write().await;

        // Determine which pool to use based on token ID and side
        let is_yes_token = token_id == self.config.yes_token_id;
        let is_no_token = token_id == self.config.no_token_id;

        if !is_yes_token && !is_no_token {
            debug!(token_id, "Token ID not recognized for pre-signed pool");
            return None;
        }

        // Try to take a pre-signed order at or near the target price
        let order = if is_yes_token {
            if side == Side::Buy {
                pool_guard.take_yes_buy(price)
            } else {
                pool_guard.take_yes_sell(price)
            }
        } else if side == Side::Buy {
            pool_guard.take_no_buy(price)
        } else {
            pool_guard.take_no_sell(price)
        };

        match order {
            Some(presigned) => {
                info!(
                    token_id,
                    side = %side,
                    price = %price,
                    nonce = %presigned.nonce,
                    "Using pre-signed order (saved ~5-10ms signing)"
                );
                Some(PresignedData {
                    nonce: presigned.nonce,
                    expiration: presigned.expiration,
                    signature: presigned.signature,
                })
            }
            None => {
                debug!(
                    token_id,
                    side = %side,
                    price = %price,
                    "No pre-signed order available, will sign fresh"
                );
                None
            }
        }
    }

    /// Persists the current position state to disk if persistence is configured.
    async fn persist_position(&self) {
        if let Some(ref persistence) = self.persistence {
            let tracker = self.position.read().await;
            if let Err(e) = persistence.save(&tracker) {
                warn!(error = %e, "Failed to persist position state");
            } else {
                debug!(
                    window_ms = tracker.window_start_ms,
                    has_yes = tracker.yes_position.is_some(),
                    has_no = tracker.no_position.is_some(),
                    "Persisted position state"
                );
            }
        }
    }

    /// Runs the auto executor, consuming signals and executing trades.
    pub async fn run(
        &mut self,
        mut signal_rx: mpsc::Receiver<GabagoolSignal>,
    ) -> Result<(), AutoExecutorError> {
        info!(
            kelly = self.config.kelly_fraction,
            min_edge = self.config.min_edge,
            max_position = %self.config.max_position_per_window,
            "AutoExecutor starting"
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.started_at = Some(Utc::now());
        }

        loop {
            if self.should_stop.load(Ordering::SeqCst) {
                info!("AutoExecutor stopping");
                break;
            }

            tokio::select! {
                signal = signal_rx.recv() => {
                    match signal {
                        Some(s) => {
                            if let Err(e) = self.handle_signal(s).await {
                                error!(error = %e, "Error handling signal");
                            }
                        }
                        None => {
                            info!("Signal channel closed");
                            return Err(AutoExecutorError::ChannelClosed);
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    // Periodic check for stop
                }
            }
        }

        // Export history if configured
        if let Some(ref path) = self.config.history_path {
            self.export_history(path).await?;
        }

        Ok(())
    }

    /// Handles a single signal.
    async fn handle_signal(&mut self, signal: GabagoolSignal) -> Result<(), AutoExecutorError> {
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.signals_received += 1;
        }

        // Check if we should execute this signal type
        let should_execute = match signal.signal_type {
            GabagoolSignalType::Entry => self.config.execute_entries,
            GabagoolSignalType::Hedge => self.config.execute_hedges,
            GabagoolSignalType::Scratch => self.config.execute_scratches,
        };

        if !should_execute {
            debug!(signal_type = ?signal.signal_type, "Skipping signal (disabled)");
            self.stats.write().await.signals_skipped += 1;
            return Ok(());
        }

        // Check minimum edge for entries
        if signal.signal_type == GabagoolSignalType::Entry
            && signal.estimated_edge < self.config.min_edge
        {
            debug!(
                edge = signal.estimated_edge,
                min = self.config.min_edge,
                "Skipping entry (edge too low)"
            );
            self.stats.write().await.signals_skipped += 1;
            return Ok(());
        }

        // Handle window transitions
        let window_changed = {
            let mut pos = self.position.write().await;
            // Simple window tracking by signal timestamp
            let signal_window_ms = (signal.timestamp.timestamp_millis() / 900_000) * 900_000; // 15-min windows

            if signal_window_ms != pos.window_start_ms {
                // New window - clear old positions
                info!(
                    old_window = pos.window_start_ms,
                    new_window = signal_window_ms,
                    "Window transition - clearing positions"
                );
                pos.window_start_ms = signal_window_ms;
                pos.clear();
                true
            } else {
                false
            }
        };

        // Persist cleared state on window change
        if window_changed {
            self.persist_position().await;
        }

        // Execute based on signal type
        match signal.signal_type {
            GabagoolSignalType::Entry => self.execute_entry(signal).await,
            GabagoolSignalType::Hedge => self.execute_hedge(signal).await,
            GabagoolSignalType::Scratch => self.execute_scratch(signal).await,
        }
    }

    /// Executes an entry signal.
    async fn execute_entry(&mut self, signal: GabagoolSignal) -> Result<(), AutoExecutorError> {
        // Check if we already have a position
        {
            let pos = self.position.read().await;
            if pos.has_position() {
                debug!("Skipping entry - already have position");
                self.stats.write().await.signals_skipped += 1;
                return Ok(());
            }

            // Check position limit
            if pos.total_cost >= self.config.max_position_per_window {
                debug!(
                    current = %pos.total_cost,
                    limit = %self.config.max_position_per_window,
                    "Skipping entry - position limit reached"
                );
                self.stats.write().await.signals_skipped += 1;
                return Ok(());
            }
        }

        // Get balance
        let balance = self.executor.get_balance().await?;

        // Calculate position size
        let bet_size = if let Some(fixed) = self.config.fixed_bet_size {
            fixed
        } else {
            match self
                .sizer
                .size(signal.estimated_edge, signal.entry_price, balance)
            {
                Some(size) => size,
                None => {
                    debug!("Kelly recommends no bet");
                    self.stats.write().await.signals_skipped += 1;
                    return Ok(());
                }
            }
        };

        // Calculate shares from bet size
        let shares = bet_size / signal.entry_price;

        // Determine token ID
        let token_id = match signal.direction {
            GabagoolDirection::Yes => &self.config.yes_token_id,
            GabagoolDirection::No => &self.config.no_token_id,
        };

        if token_id.is_empty() {
            warn!(
                "Token ID not configured for direction {:?}",
                signal.direction
            );
            return Ok(());
        }

        // Try to get a pre-signed order for lower latency
        let presigned = self
            .try_get_presigned(token_id, Side::Buy, signal.entry_price)
            .await;

        // Create order (with pre-signed data if available)
        let order = if let Some(data) = presigned {
            OrderParams::buy_fok(token_id, signal.entry_price, shares).with_presigned(data)
        } else {
            OrderParams::buy_fok(token_id, signal.entry_price, shares)
        };

        info!(
            direction = ?signal.direction,
            price = %signal.entry_price,
            shares = %shares,
            bet_size = %bet_size,
            edge = signal.estimated_edge,
            presigned = order.is_presigned(),
            "Executing ENTRY"
        );

        // Submit order
        let result = self.executor.submit_order(order).await?;

        // Record trade
        self.record_trade(&signal, &result, Side::Buy, signal.entry_price, shares)
            .await;

        // Update position if filled
        if result.status == OrderStatus::Filled {
            let pos = OpenPosition {
                direction: signal.direction,
                entry_price: result.avg_fill_price.unwrap_or(signal.entry_price),
                quantity: result.filled_size,
                entry_time_ms: signal.timestamp.timestamp_millis(),
                window_start_ms: self.position.read().await.window_start_ms,
            };

            self.position.write().await.record_entry(pos);

            // Persist position after entry
            self.persist_position().await;

            let mut stats = self.stats.write().await;
            stats.entry_trades += 1;
            stats.orders_filled += 1;
            stats.total_volume += result.fill_notional();
            stats.current_position_value += result.fill_notional();
            stats.last_trade_time = Some(Utc::now());
        } else {
            self.stats.write().await.orders_failed += 1;
        }

        self.stats.write().await.orders_attempted += 1;

        Ok(())
    }

    /// Executes a hedge signal.
    async fn execute_hedge(&mut self, signal: GabagoolSignal) -> Result<(), AutoExecutorError> {
        // Check if we have a position to hedge
        let existing_position = {
            let pos = self.position.read().await;
            if !pos.has_position() || pos.is_hedged() {
                debug!("Skipping hedge - no position or already hedged");
                self.stats.write().await.signals_skipped += 1;
                return Ok(());
            }

            // Get the existing position to match size
            match signal.direction {
                GabagoolDirection::Yes => pos.no_position.clone(),
                GabagoolDirection::No => pos.yes_position.clone(),
            }
        };

        let existing = match existing_position {
            Some(p) => p,
            None => {
                debug!("No existing position to hedge");
                return Ok(());
            }
        };

        // Hedge with same quantity as existing position
        let shares = existing.quantity;

        // Determine token ID
        let token_id = match signal.direction {
            GabagoolDirection::Yes => &self.config.yes_token_id,
            GabagoolDirection::No => &self.config.no_token_id,
        };

        if token_id.is_empty() {
            warn!(
                "Token ID not configured for direction {:?}",
                signal.direction
            );
            return Ok(());
        }

        // Try to get a pre-signed order for lower latency
        let presigned = self
            .try_get_presigned(token_id, Side::Buy, signal.entry_price)
            .await;

        // Create order (with pre-signed data if available)
        let order = if let Some(data) = presigned {
            OrderParams::buy_fok(token_id, signal.entry_price, shares).with_presigned(data)
        } else {
            OrderParams::buy_fok(token_id, signal.entry_price, shares)
        };

        info!(
            direction = ?signal.direction,
            price = %signal.entry_price,
            shares = %shares,
            existing_price = %existing.entry_price,
            total_cost = %(existing.entry_price + signal.entry_price),
            presigned = order.is_presigned(),
            "Executing HEDGE"
        );

        // Submit order
        let result = self.executor.submit_order(order).await?;

        // Record trade
        self.record_trade(&signal, &result, Side::Buy, signal.entry_price, shares)
            .await;

        // Update position if filled
        if result.status == OrderStatus::Filled {
            let pos = OpenPosition {
                direction: signal.direction,
                entry_price: result.avg_fill_price.unwrap_or(signal.entry_price),
                quantity: result.filled_size,
                entry_time_ms: signal.timestamp.timestamp_millis(),
                window_start_ms: self.position.read().await.window_start_ms,
            };

            self.position.write().await.record_hedge(pos);

            // Persist position after hedge
            self.persist_position().await;

            let mut stats = self.stats.write().await;
            stats.hedge_trades += 1;
            stats.orders_filled += 1;
            stats.total_volume += result.fill_notional();
            stats.current_position_value += result.fill_notional();
            stats.last_trade_time = Some(Utc::now());
        } else {
            self.stats.write().await.orders_failed += 1;
        }

        self.stats.write().await.orders_attempted += 1;

        Ok(())
    }

    /// Executes a scratch signal.
    async fn execute_scratch(&mut self, signal: GabagoolSignal) -> Result<(), AutoExecutorError> {
        // Check if we have a position to scratch
        let existing_position = {
            let pos = self.position.read().await;
            if !pos.has_position() {
                debug!("Skipping scratch - no position");
                self.stats.write().await.signals_skipped += 1;
                return Ok(());
            }

            match signal.direction {
                GabagoolDirection::Yes => pos.yes_position.clone(),
                GabagoolDirection::No => pos.no_position.clone(),
            }
        };

        let existing = match existing_position {
            Some(p) => p,
            None => {
                debug!("No position to scratch in this direction");
                return Ok(());
            }
        };

        // Sell at current bid (signal.entry_price is the exit price for scratches)
        let shares = existing.quantity;

        // Determine token ID
        let token_id = match signal.direction {
            GabagoolDirection::Yes => &self.config.yes_token_id,
            GabagoolDirection::No => &self.config.no_token_id,
        };

        if token_id.is_empty() {
            warn!(
                "Token ID not configured for direction {:?}",
                signal.direction
            );
            return Ok(());
        }

        // Try to get a pre-signed order for lower latency
        let presigned = self
            .try_get_presigned(token_id, Side::Sell, signal.entry_price)
            .await;

        // Create sell order (FAK to take whatever liquidity is available)
        let order = if let Some(data) = presigned {
            OrderParams::sell_fak(token_id, signal.entry_price, shares).with_presigned(data)
        } else {
            OrderParams::sell_fak(token_id, signal.entry_price, shares)
        };

        let loss = existing.entry_price - signal.entry_price;
        info!(
            direction = ?signal.direction,
            exit_price = %signal.entry_price,
            entry_price = %existing.entry_price,
            shares = %shares,
            loss = %loss,
            presigned = order.is_presigned(),
            "Executing SCRATCH"
        );

        // Submit order
        let result = self.executor.submit_order(order).await?;

        // Record trade
        self.record_trade(&signal, &result, Side::Sell, signal.entry_price, shares)
            .await;

        // Update position if filled
        if result.status == OrderStatus::Filled || result.status == OrderStatus::PartiallyFilled {
            // Calculate realized P&L
            let exit_value = result.fill_notional();
            let entry_value = existing.entry_price * result.filled_size;
            let pnl = exit_value - entry_value;

            // Clear position
            self.position.write().await.clear();

            // Persist position after scratch (now empty)
            self.persist_position().await;

            let mut stats = self.stats.write().await;
            stats.scratch_trades += 1;
            stats.orders_filled += 1;
            stats.total_volume += exit_value;
            stats.realized_pnl += pnl;
            stats.current_position_value = Decimal::ZERO;
            stats.last_trade_time = Some(Utc::now());
        } else {
            self.stats.write().await.orders_failed += 1;
        }

        self.stats.write().await.orders_attempted += 1;

        Ok(())
    }

    /// Records a trade in history.
    async fn record_trade(
        &self,
        signal: &GabagoolSignal,
        result: &OrderResult,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) {
        let record = TradeRecord {
            trade_id: result.order_id.clone(),
            signal_type: signal.signal_type,
            direction: signal.direction,
            side,
            price,
            size,
            filled_size: result.filled_size,
            avg_fill_price: result.avg_fill_price,
            status: result.status,
            signal_timestamp: signal.timestamp,
            execution_timestamp: Utc::now(),
            window_start_ms: self.position.read().await.window_start_ms,
            estimated_edge: signal.estimated_edge,
            confidence: signal.confidence,
        };

        let mut history = self.history.write().await;
        history.push_back(record);
        while history.len() > self.config.max_history {
            history.pop_front();
        }
    }

    /// Exports trade history to a file.
    async fn export_history(&self, path: &PathBuf) -> Result<(), AutoExecutorError> {
        use std::fs::File;
        use std::io::{BufWriter, Write};

        let history = self.history.read().await;
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        for record in history.iter() {
            serde_json::to_writer(&mut writer, record)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            writeln!(writer)?;
        }

        writer.flush()?;
        info!(path = %path.display(), count = history.len(), "Exported trade history");

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::paper_executor::{PaperExecutor, PaperExecutorConfig};

    // =========================================================================
    // Kelly Sizer Tests
    // =========================================================================

    #[test]
    fn test_kelly_sizer_positive_edge() {
        let sizer = KellySizer::new(0.25, dec!(5), dec!(100));

        // 10% edge at 40 cents = good bet
        let size = sizer.size(0.10, dec!(0.40), dec!(1000));

        assert!(size.is_some());
        let size = size.unwrap();
        assert!(size >= dec!(5)); // Above minimum
        assert!(size <= dec!(100)); // Below maximum
    }

    #[test]
    fn test_kelly_sizer_no_edge() {
        let sizer = KellySizer::new(0.25, dec!(5), dec!(100));

        // Zero edge = no bet
        let size = sizer.size(0.0, dec!(0.50), dec!(1000));

        assert!(size.is_none());
    }

    #[test]
    fn test_kelly_sizer_negative_edge() {
        let sizer = KellySizer::new(0.25, dec!(5), dec!(100));

        // Negative edge = definitely no bet
        let size = sizer.size(-0.05, dec!(0.50), dec!(1000));

        assert!(size.is_none());
    }

    #[test]
    fn test_kelly_sizer_respects_max() {
        let sizer = KellySizer::new(1.0, dec!(5), dec!(50)); // Full Kelly, max $50

        // Huge edge would suggest large bet, but capped
        let size = sizer.size(0.30, dec!(0.30), dec!(10000));

        assert!(size.is_some());
        assert!(size.unwrap() <= dec!(50));
    }

    #[test]
    fn test_kelly_sizer_respects_min() {
        let sizer = KellySizer::new(0.001, dec!(10), dec!(100)); // Very tiny fraction

        // Very small Kelly fraction with small bankroll
        // With 0.001 fraction, 5% edge at 50c on $100 bankroll = ~$0.10 bet (way below $10 min)
        let size = sizer.size(0.05, dec!(0.50), dec!(100));

        // Should return None because calculated bet is below minimum
        assert!(size.is_none());
    }

    #[test]
    fn test_kelly_sizer_bankroll_cap() {
        let sizer = KellySizer::new(0.5, dec!(5), dec!(1000)); // Half Kelly, high max

        // Small bankroll should cap the bet
        let size = sizer.size(0.20, dec!(0.40), dec!(50));

        assert!(size.is_some());
        assert!(size.unwrap() <= dec!(50)); // Can't bet more than bankroll
    }

    // =========================================================================
    // Position Tracker Tests
    // =========================================================================

    #[test]
    fn test_position_tracker_empty() {
        let tracker = WindowPositionTracker::new(0);

        assert!(!tracker.has_position());
        assert!(!tracker.is_hedged());
        assert!(tracker.open_direction().is_none());
    }

    #[test]
    fn test_position_tracker_single_entry() {
        let mut tracker = WindowPositionTracker::new(0);

        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            quantity: dec!(100),
            entry_time_ms: 0,
            window_start_ms: 0,
        });

        assert!(tracker.has_position());
        assert!(!tracker.is_hedged());
        assert_eq!(tracker.open_direction(), Some(GabagoolDirection::Yes));
        assert_eq!(tracker.total_cost, dec!(35)); // 0.35 * 100
    }

    #[test]
    fn test_position_tracker_hedged() {
        let mut tracker = WindowPositionTracker::new(0);

        // Entry
        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            quantity: dec!(100),
            entry_time_ms: 0,
            window_start_ms: 0,
        });

        // Hedge
        tracker.record_hedge(OpenPosition {
            direction: GabagoolDirection::No,
            entry_price: dec!(0.60),
            quantity: dec!(100),
            entry_time_ms: 0,
            window_start_ms: 0,
        });

        assert!(tracker.has_position());
        assert!(tracker.is_hedged());
        assert!(tracker.open_direction().is_none()); // Hedged = no open direction
        assert_eq!(tracker.total_cost, dec!(95)); // 35 + 60
    }

    #[test]
    fn test_position_tracker_clear() {
        let mut tracker = WindowPositionTracker::new(0);

        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::No,
            entry_price: dec!(0.40),
            quantity: dec!(50),
            entry_time_ms: 0,
            window_start_ms: 0,
        });

        assert!(tracker.has_position());

        tracker.clear();

        assert!(!tracker.has_position());
        assert_eq!(tracker.total_cost, Decimal::ZERO);
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = AutoExecutorConfig::default();

        assert!((config.kelly_fraction - 0.25).abs() < 0.001);
        assert!(config.fixed_bet_size.is_none());
        assert_eq!(config.min_bet_size, dec!(5));
        assert_eq!(config.max_bet_size, dec!(100));
        assert!((config.min_edge - 0.02).abs() < 0.001);
    }

    #[test]
    fn test_config_micro_testing() {
        let config = AutoExecutorConfig::micro_testing();

        assert_eq!(config.fixed_bet_size, Some(dec!(10)));
        assert_eq!(config.max_bet_size, dec!(25));
    }

    #[test]
    fn test_config_conservative() {
        let config = AutoExecutorConfig::conservative();

        assert!((config.kelly_fraction - 0.125).abs() < 0.001);
        assert!((config.min_edge - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_config_builder() {
        let config = AutoExecutorConfig::default()
            .with_yes_token("yes-123")
            .with_no_token("no-456")
            .with_fixed_bet(dec!(50))
            .with_kelly_fraction(0.5);

        assert_eq!(config.yes_token_id, "yes-123");
        assert_eq!(config.no_token_id, "no-456");
        assert_eq!(config.fixed_bet_size, Some(dec!(50)));
        assert!((config.kelly_fraction - 0.5).abs() < 0.001);
    }

    // =========================================================================
    // Stats Tests
    // =========================================================================

    #[test]
    fn test_stats_default() {
        let stats = AutoExecutorStats::default();

        assert_eq!(stats.signals_received, 0);
        assert_eq!(stats.orders_attempted, 0);
        assert_eq!(stats.total_volume, Decimal::ZERO);
        assert!(stats.started_at.is_none());
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_auto_executor_creation() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0, // Always fill
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token");

        let auto = AutoExecutor::new(executor, auto_config);

        assert_eq!(auto.config().yes_token_id, "yes-token");
        assert_eq!(auto.config().no_token_id, "no-token");
    }

    #[tokio::test]
    async fn test_auto_executor_stop_handle() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        let auto = AutoExecutor::new(executor, AutoExecutorConfig::default());

        let stop = auto.stop_handle();
        assert!(!stop.load(Ordering::SeqCst));

        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_auto_executor_signal_handling() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42), // Reproducible
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token")
            .with_fixed_bet(dec!(50));

        let mut auto = AutoExecutor::new(executor, auto_config);

        // Create a test signal
        let signal = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: Utc::now(),
            estimated_edge: 0.10,
            existing_entry_price: None,
            confidence: SignalConfidence::High,
        };

        // Handle the signal directly
        let result = auto.handle_signal(signal).await;
        assert!(result.is_ok());

        // Check stats
        let stats = auto.stats.read().await;
        assert_eq!(stats.signals_received, 1);
        assert_eq!(stats.orders_attempted, 1);
        assert_eq!(stats.entry_trades, 1);
        assert!(stats.total_volume > Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_auto_executor_skips_low_edge() {
        let executor = PaperExecutor::new(PaperExecutorConfig::default());

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token");

        let mut auto = AutoExecutor::new(executor, auto_config);

        // Signal with edge below threshold
        let signal = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.48),
            spot_price: 78010.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00013,
            current_pair_cost: dec!(0.98),
            time_remaining_secs: 600,
            timestamp: Utc::now(),
            estimated_edge: 0.01, // 1% edge, below 2% threshold
            existing_entry_price: None,
            confidence: SignalConfidence::Low,
        };

        let result = auto.handle_signal(signal).await;
        assert!(result.is_ok());

        let stats = auto.stats.read().await;
        assert_eq!(stats.signals_received, 1);
        assert_eq!(stats.signals_skipped, 1);
        assert_eq!(stats.orders_attempted, 0);
    }

    #[tokio::test]
    async fn test_auto_executor_full_cycle() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token")
            .with_fixed_bet(dec!(35));

        let mut auto = AutoExecutor::new(executor, auto_config);

        let now = Utc::now();

        // 1. Entry signal
        let entry = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: now,
            estimated_edge: 0.15,
            existing_entry_price: None,
            confidence: SignalConfidence::High,
        };

        auto.handle_signal(entry).await.unwrap();

        // Check position
        {
            let pos = auto.position.read().await;
            assert!(pos.has_position());
            assert!(!pos.is_hedged());
            assert_eq!(pos.open_direction(), Some(GabagoolDirection::Yes));
        }

        // 2. Hedge signal
        let hedge = GabagoolSignal {
            signal_type: GabagoolSignalType::Hedge,
            direction: GabagoolDirection::No,
            entry_price: dec!(0.60),
            spot_price: 78050.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00064,
            current_pair_cost: dec!(0.95),
            time_remaining_secs: 400,
            timestamp: now,
            estimated_edge: 0.05,
            existing_entry_price: Some(dec!(0.35)),
            confidence: SignalConfidence::High,
        };

        auto.handle_signal(hedge).await.unwrap();

        // Check hedged
        {
            let pos = auto.position.read().await;
            assert!(pos.is_hedged());
        }

        // Check final stats
        let stats = auto.stats.read().await;
        assert_eq!(stats.entry_trades, 1);
        assert_eq!(stats.hedge_trades, 1);
        assert!(stats.total_volume > Decimal::ZERO);
    }

    // =========================================================================
    // Persistence Integration Tests
    // =========================================================================

    #[test]
    fn test_config_with_persistence_path() {
        let config =
            AutoExecutorConfig::default().with_persistence_path("/tmp/test_positions.json");

        assert!(config.persistence_path.is_some());
        assert_eq!(
            config.persistence_path.unwrap().to_str().unwrap(),
            "/tmp/test_positions.json"
        );
    }

    #[tokio::test]
    async fn test_auto_executor_persists_on_entry() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");

        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token")
            .with_fixed_bet(dec!(35))
            .with_persistence_path(&path);

        let mut auto = AutoExecutor::new(executor, auto_config);

        // Entry signal
        let signal = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: Utc::now(),
            estimated_edge: 0.15,
            existing_entry_price: None,
            confidence: SignalConfidence::High,
        };

        auto.handle_signal(signal).await.unwrap();

        // Verify file was created
        assert!(path.exists());

        // Load and verify contents
        let content = std::fs::read_to_string(&path).unwrap();
        let persisted: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(persisted.get("yes_position").is_some());
        assert!(persisted["yes_position"].is_object());
    }

    #[tokio::test]
    async fn test_auto_executor_loads_position_on_startup() {
        use crate::arbitrage::position_persistence::PositionPersistence;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");

        // Pre-populate position file
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            quantity: dec!(100),
            entry_time_ms: 1000,
            window_start_ms: 900_000,
        });

        let persistence = PositionPersistence::new(path.clone());
        persistence.save(&tracker).unwrap();

        // Create executor - should load position
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        let auto_config = AutoExecutorConfig::default().with_persistence_path(&path);

        let auto = AutoExecutor::new(executor, auto_config);

        // Check position was loaded
        let pos = auto.position.read().await;
        assert!(pos.has_position());
        assert!(pos.yes_position.is_some());
        assert_eq!(pos.window_start_ms, 900_000);
    }

    #[tokio::test]
    async fn test_auto_executor_new_with_window_validates_staleness() {
        use crate::arbitrage::position_persistence::PositionPersistence;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");

        // Pre-populate with OLD window position
        let mut tracker = WindowPositionTracker::new(900_000); // Old window
        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            quantity: dec!(100),
            entry_time_ms: 1000,
            window_start_ms: 900_000,
        });

        let persistence = PositionPersistence::new(path.clone());
        persistence.save(&tracker).unwrap();

        // Create executor with DIFFERENT window - should clear stale position
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        let auto_config = AutoExecutorConfig::default().with_persistence_path(&path);

        let auto = AutoExecutor::new_with_window(executor, auto_config, 1_800_000); // New window

        // Position should be cleared (stale)
        let pos = auto.position.read().await;
        assert!(!pos.has_position());
        assert_eq!(pos.window_start_ms, 1_800_000);
    }

    #[tokio::test]
    async fn test_auto_executor_new_with_window_preserves_current() {
        use crate::arbitrage::position_persistence::PositionPersistence;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");

        // Pre-populate with SAME window position
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(OpenPosition {
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            quantity: dec!(100),
            entry_time_ms: 1000,
            window_start_ms: 900_000,
        });

        let persistence = PositionPersistence::new(path.clone());
        persistence.save(&tracker).unwrap();

        // Create executor with SAME window - should preserve position
        let executor = PaperExecutor::new(PaperExecutorConfig::default());
        let auto_config = AutoExecutorConfig::default().with_persistence_path(&path);

        let auto = AutoExecutor::new_with_window(executor, auto_config, 900_000);

        // Position should be preserved
        let pos = auto.position.read().await;
        assert!(pos.has_position());
        assert!(pos.yes_position.is_some());
        assert_eq!(pos.window_start_ms, 900_000);
    }

    #[tokio::test]
    async fn test_auto_executor_persists_on_scratch() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");

        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token")
            .with_fixed_bet(dec!(35))
            .with_persistence_path(&path);

        let mut auto = AutoExecutor::new(executor, auto_config);

        let now = Utc::now();

        // 1. Entry
        let entry = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: now,
            estimated_edge: 0.15,
            existing_entry_price: None,
            confidence: SignalConfidence::High,
        };
        auto.handle_signal(entry).await.unwrap();

        // Verify position persisted
        {
            let content = std::fs::read_to_string(&path).unwrap();
            let persisted: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert!(persisted["yes_position"].is_object());
        }

        // 2. Scratch
        let scratch = GabagoolSignal {
            signal_type: GabagoolSignalType::Scratch,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.33), // Exit at lower price
            spot_price: 78000.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.0,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 60,
            timestamp: now,
            estimated_edge: -0.02, // Loss
            existing_entry_price: Some(dec!(0.35)),
            confidence: SignalConfidence::High,
        };
        auto.handle_signal(scratch).await.unwrap();

        // Verify position cleared in persistence
        let content = std::fs::read_to_string(&path).unwrap();
        let persisted: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(persisted["yes_position"].is_null());
        assert!(persisted["no_position"].is_null());
    }

    #[tokio::test]
    async fn test_auto_executor_no_persistence_when_path_not_set() {
        let paper_config = PaperExecutorConfig {
            initial_balance: dec!(1000),
            fill_rate: 1.0,
            random_seed: Some(42),
            ..Default::default()
        };
        let executor = PaperExecutor::new(paper_config);

        // No persistence path set
        let auto_config = AutoExecutorConfig::default()
            .with_yes_token("yes-token")
            .with_no_token("no-token")
            .with_fixed_bet(dec!(35));

        let mut auto = AutoExecutor::new(executor, auto_config);

        // Entry signal
        let signal = GabagoolSignal {
            signal_type: GabagoolSignalType::Entry,
            direction: GabagoolDirection::Yes,
            entry_price: dec!(0.35),
            spot_price: 78100.0,
            reference_price: 78000.0,
            spot_delta_pct: 0.00128,
            current_pair_cost: dec!(1.02),
            time_remaining_secs: 600,
            timestamp: Utc::now(),
            estimated_edge: 0.15,
            existing_entry_price: None,
            confidence: SignalConfidence::High,
        };

        // Should work without persistence (no errors)
        auto.handle_signal(signal).await.unwrap();

        // Position should be tracked in memory
        let pos = auto.position.read().await;
        assert!(pos.has_position());
    }
}
