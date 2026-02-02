//! polymarket-paper-trade CLI command for paper trading on Polymarket.
//!
//! Simulates trading on Polymarket BTC markets using signal-based decision making
//! with Kelly criterion position sizing.
//!
//! ## Signal Modes
//!
//! - **Simulated**: Uses price-based heuristics (default for testing)
//! - **Real**: Uses `LiquidationCascadeSignal` from database aggregates
//!
//! ## Real Signal Configuration
//!
//! When `--use-simulated-signals=false`, the system fetches liquidation aggregates
//! from the database and computes signals using the configured mode:
//!
//! - **cascade**: Follow liquidation momentum
//! - **exhaustion**: Bet on reversals after spikes
//! - **combined**: Weight both factors

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use chrono::{DateTime, Timelike, Utc};
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::commands::collect_signals::parse_duration;
use crate::commands::window_timing::WindowTimer;
use algo_trade_backtest::{
    create_entry_strategy, BetDirection, EntryContext, EntryDecision, EntryStrategy,
    EntryStrategyConfig, EntryStrategyType, FeeModel, FeeTier, PolymarketFees,
};
use algo_trade_core::{
    Direction, LiquidationAggregate, SignalContext, SignalGenerator, SignalValue,
};
use algo_trade_data::{
    KellyCriterion, LiquidationAggregateRecord, LiquidationRepository, PaperTradeDirection,
    PaperTradeRecord, PaperTradeRepository, PolymarketOddsRecord, PolymarketOddsRepository,
    SettlementService,
};
use algo_trade_signals::{
    CascadeConfig, CompositeSignal, ExhaustionConfig, FundingPercentileConfig, FundingRateSignal,
    FundingSignalMode, LiquidationCascadeSignal, LiquidationRatioConfig, LiquidationRatioSignal,
    LiquidationSignalMode, OrderBookImbalanceSignal,
};

/// Arguments for the polymarket-paper-trade command.
#[derive(Args, Debug, Clone)]
pub struct PolymarketPaperTradeArgs {
    /// Duration to run paper trading (e.g., "1h", "24h", "7d", "2w")
    #[arg(long, default_value = "24h")]
    pub duration: String,

    /// Signal type to use (composite, imbalance, funding, liquidation, news)
    #[arg(long, default_value = "composite")]
    pub signal: String,

    /// Minimum signal strength required to place a bet (0.0 to 1.0)
    #[arg(long, default_value = "0.6")]
    pub min_signal_strength: f64,

    /// Fixed stake per bet in USD (ignored if using Kelly sizing)
    #[arg(long, default_value = "100")]
    pub stake: f64,

    /// Kelly fraction to use (0.25 for quarter Kelly, 0.5 for half Kelly)
    #[arg(long, default_value = "0.25")]
    pub kelly_fraction: f64,

    /// Minimum edge required to place a bet (0.0 to 1.0)
    #[arg(long, default_value = "0.02")]
    pub min_edge: f64,

    /// Maximum price to buy shares at (0.0 to 1.0)
    /// Per research: only buy when price <= 0.55 for decent odds (1.82x+ payout)
    /// At 0.80, payout is only 1.25x which offers poor risk/reward
    #[arg(long, default_value = "0.55")]
    pub max_price: f64,

    /// Initial bankroll in USD
    #[arg(long, default_value = "10000")]
    pub bankroll: f64,

    /// Maximum bet size as fraction of bankroll
    #[arg(long, default_value = "0.05")]
    pub max_bet_fraction: f64,

    /// Polymarket fee tier (0, 1, 2, 3, or maker)
    #[arg(long, default_value = "0")]
    pub fee_tier: String,

    /// Poll interval in seconds for checking new opportunities
    #[arg(long, default_value = "60")]
    pub poll_interval_secs: u64,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    /// Minimum time between trades on same market in seconds
    #[arg(long, default_value = "900")]
    pub cooldown_secs: u64,

    /// Whether to use fixed stake instead of Kelly sizing
    #[arg(long, default_value = "false")]
    pub use_fixed_stake: bool,

    // =========================================================================
    // Entry Strategy Arguments
    // =========================================================================
    /// Entry strategy: immediate, fixed_time, or edge_threshold
    #[arg(long, default_value = "immediate")]
    pub entry_strategy: String,

    /// Minimum edge threshold for edge_threshold strategy (0.0 to 1.0)
    #[arg(long, default_value = "0.03")]
    pub entry_threshold: f64,

    /// Fixed entry offset as percentage of window (0.0 to 1.0) for fixed_time strategy
    #[arg(long, default_value = "0.25")]
    pub entry_offset_pct: f64,

    /// Fallback entry time in minutes before window close (0 to disable)
    #[arg(long, default_value = "2")]
    pub entry_fallback_mins: i64,

    /// Window duration in minutes (Polymarket BTC settles every 15 mins at :00/:15/:30/:45)
    #[arg(long, default_value = "15")]
    pub window_minutes: i64,

    /// Minimum time remaining in window to enter a trade (minutes)
    /// Don't enter trades with less than this time before settlement
    #[arg(long, default_value = "2")]
    pub entry_cutoff_mins: i64,

    /// Entry poll interval in seconds for monitoring entry conditions
    #[arg(long, default_value = "10")]
    pub entry_poll_secs: u64,

    /// Maximum signal age in minutes before rejecting entry (0 to disable)
    /// Cascade signals become stale as the initial move may have already played out.
    /// Recommended: 3-4 minutes for 15-minute windows.
    #[arg(long, default_value = "4")]
    pub max_signal_age_mins: i64,

    /// Maximum liquidation aggregate age in minutes before ignoring (0 to disable)
    /// Prevents signals from firing based on data from the previous window.
    /// Should be <= liquidation_window_mins to ensure data is from current window.
    /// Recommended: 5 minutes (same as default liquidation window).
    #[arg(long, default_value = "5")]
    pub max_aggregate_age_mins: i64,

    // =========================================================================
    // Real Signal Arguments
    // =========================================================================
    /// Use simulated signals instead of real liquidation data.
    /// Pass --use-simulated-signals to enable, omit for real signals.
    #[arg(long, default_value_t = false)]
    pub use_simulated_signals: bool,

    /// Signal mode: cascade, exhaustion, or combined
    #[arg(long, default_value = "cascade")]
    pub signal_mode: String,

    /// Minimum total volume in USD to consider a cascade
    #[arg(long, default_value = "100000")]
    pub min_volume_usd: f64,

    /// Minimum imbalance ratio to trigger cascade signal (0.0 to 1.0)
    #[arg(long, default_value = "0.6")]
    pub imbalance_threshold: f64,

    /// Liquidation aggregation window in minutes
    #[arg(long, default_value = "5")]
    pub liquidation_window_mins: i32,

    /// Symbol for liquidation data (e.g., BTCUSDT)
    #[arg(long, default_value = "BTCUSDT")]
    pub liquidation_symbol: String,

    /// Exchange for liquidation data (e.g., binance)
    #[arg(long, default_value = "binance")]
    pub liquidation_exchange: String,

    // =========================================================================
    // Composite Signal Arguments
    // =========================================================================
    /// Enable composite voting mode (requires multiple signals to agree)
    #[arg(long, default_value_t = false)]
    pub enable_composite: bool,

    /// Minimum number of signals that must agree for composite mode (default: 2)
    #[arg(long, default_value = "2")]
    pub min_signals_agree: usize,

    /// Include order book imbalance signal in composite
    #[arg(long, default_value_t = false)]
    pub enable_orderbook_signal: bool,

    /// Include funding rate percentile signal in composite
    #[arg(long, default_value_t = false)]
    pub enable_funding_signal: bool,

    /// Include 24h liquidation ratio signal in composite
    #[arg(long, default_value_t = false)]
    pub enable_liq_ratio_signal: bool,

    // =========================================================================
    // Settlement Arguments
    // =========================================================================
    /// Polygon RPC URL for Chainlink price feed settlement.
    /// Uses public endpoint if not provided (rate-limited).
    #[arg(long, env = "POLYGON_RPC_URL")]
    pub polygon_rpc_url: Option<String>,

    /// Settlement fee rate (0.0 to 1.0, e.g., 0.02 for 2%)
    #[arg(long, default_value = "0.02")]
    pub settlement_fee_rate: f64,
}

// =============================================================================
// Signal Configuration
// =============================================================================

/// Parses signal mode string to enum.
///
/// # Arguments
/// * `s` - Signal mode string (cascade, exhaustion, combined)
///
/// # Returns
/// The corresponding `LiquidationSignalMode`, defaulting to `Cascade` for invalid inputs.
#[must_use]
pub fn parse_signal_mode(s: &str) -> LiquidationSignalMode {
    match s.to_lowercase().as_str() {
        "cascade" => LiquidationSignalMode::Cascade,
        "exhaustion" => LiquidationSignalMode::Exhaustion,
        "combined" => LiquidationSignalMode::Combined,
        _ => LiquidationSignalMode::Cascade,
    }
}

/// Configuration for real signal generation.
#[derive(Debug, Clone)]
pub struct SignalConfig {
    /// Whether to use simulated signals instead of real data.
    pub use_simulated: bool,
    /// Signal mode (cascade, exhaustion, combined).
    pub signal_mode: LiquidationSignalMode,
    /// Minimum volume in USD for cascade detection.
    pub min_volume_usd: Decimal,
    /// Imbalance threshold for cascade detection.
    pub imbalance_threshold: f64,
    /// Liquidation window size in minutes.
    pub liquidation_window_mins: i32,
    /// Symbol for liquidation data.
    pub liquidation_symbol: String,
    /// Exchange for liquidation data.
    pub liquidation_exchange: String,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            use_simulated: true,
            signal_mode: LiquidationSignalMode::Cascade,
            min_volume_usd: dec!(100000),
            imbalance_threshold: 0.6,
            liquidation_window_mins: 5,
            liquidation_symbol: "BTCUSDT".to_string(),
            liquidation_exchange: "binance".to_string(),
        }
    }
}

/// Configuration for composite signal mode.
#[derive(Debug, Clone)]
pub struct CompositeSignalConfig {
    /// Whether composite mode is enabled.
    pub enabled: bool,
    /// Minimum number of signals that must agree.
    pub min_signals_agree: usize,
    /// Enable order book imbalance signal.
    pub enable_orderbook: bool,
    /// Enable funding rate percentile signal.
    pub enable_funding: bool,
    /// Enable 24h liquidation ratio signal.
    pub enable_liq_ratio: bool,
}

impl Default for CompositeSignalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_signals_agree: 2,
            enable_orderbook: false,
            enable_funding: false,
            enable_liq_ratio: false,
        }
    }
}

impl CompositeSignalConfig {
    /// Creates a CompositeSignalConfig from CLI arguments.
    #[must_use]
    pub fn from_args(args: &PolymarketPaperTradeArgs) -> Self {
        Self {
            enabled: args.enable_composite,
            min_signals_agree: args.min_signals_agree.max(1),
            enable_orderbook: args.enable_orderbook_signal,
            enable_funding: args.enable_funding_signal,
            enable_liq_ratio: args.enable_liq_ratio_signal,
        }
    }

    /// Returns the number of enabled signals.
    #[must_use]
    pub fn enabled_signal_count(&self) -> usize {
        let mut count = 0;
        if self.enable_orderbook {
            count += 1;
        }
        if self.enable_funding {
            count += 1;
        }
        if self.enable_liq_ratio {
            count += 1;
        }
        count
    }
}

impl SignalConfig {
    /// Creates a SignalConfig from CLI arguments.
    #[must_use]
    pub fn from_args(args: &PolymarketPaperTradeArgs) -> Self {
        Self {
            use_simulated: args.use_simulated_signals,
            signal_mode: parse_signal_mode(&args.signal_mode),
            min_volume_usd: Decimal::try_from(args.min_volume_usd).unwrap_or(dec!(100000)),
            imbalance_threshold: args.imbalance_threshold.clamp(0.0, 1.0),
            liquidation_window_mins: args.liquidation_window_mins,
            liquidation_symbol: args.liquidation_symbol.clone(),
            liquidation_exchange: args.liquidation_exchange.clone(),
        }
    }

    /// Creates a `CascadeConfig` from this signal config.
    #[must_use]
    pub fn to_cascade_config(&self) -> CascadeConfig {
        CascadeConfig::new(self.min_volume_usd, self.imbalance_threshold)
    }

    /// Creates an `ExhaustionConfig` with default values.
    #[must_use]
    pub fn to_exhaustion_config(&self) -> ExhaustionConfig {
        ExhaustionConfig::default()
    }
}

/// Converts a `LiquidationAggregateRecord` from data layer to core `LiquidationAggregate`.
#[must_use]
pub fn convert_aggregate_record_to_core(
    record: &LiquidationAggregateRecord,
) -> LiquidationAggregate {
    LiquidationAggregate {
        timestamp: record.timestamp,
        window_minutes: record.window_minutes,
        long_volume_usd: record.long_volume,
        short_volume_usd: record.short_volume,
        net_delta_usd: record.net_delta,
        count_long: record.count_long,
        count_short: record.count_short,
    }
}

/// Creates a `LiquidationCascadeSignal` configured according to `SignalConfig`.
///
/// # Arguments
/// * `config` - Signal configuration from CLI args
///
/// # Returns
/// A configured `LiquidationCascadeSignal` ready for computation.
#[must_use]
pub fn create_liquidation_signal(config: &SignalConfig) -> LiquidationCascadeSignal {
    let cascade_config = config.to_cascade_config();

    let mut signal = LiquidationCascadeSignal::default()
        .with_mode(config.signal_mode)
        .with_cascade_config(cascade_config)
        .with_min_volume(config.min_volume_usd);

    // Add exhaustion config for exhaustion and combined modes
    if matches!(
        config.signal_mode,
        LiquidationSignalMode::Exhaustion | LiquidationSignalMode::Combined
    ) {
        signal = signal.with_exhaustion_config(config.to_exhaustion_config());
    }

    signal
}

/// Creates a `CompositeSignal` with the configured generators.
///
/// # Arguments
/// * `composite_config` - Composite signal configuration from CLI args
///
/// # Returns
/// A configured `CompositeSignal` using require_n combination method.
#[must_use]
pub fn create_composite_signal(composite_config: &CompositeSignalConfig) -> CompositeSignal {
    let mut composite =
        CompositeSignal::require_n("composite_multi_signal", composite_config.min_signals_agree);

    // Add order book imbalance signal
    if composite_config.enable_orderbook {
        let orderbook_signal = OrderBookImbalanceSignal::default();
        composite.add_generator(Box::new(orderbook_signal));
        tracing::info!("Added order book imbalance signal to composite");
    }

    // Add funding rate percentile signal
    if composite_config.enable_funding {
        let funding_signal = FundingRateSignal::default()
            .with_percentile_config(FundingPercentileConfig {
                lookback_periods: 90, // 30 days * 3 periods/day
                high_threshold: 0.80, // Top 20%
                low_threshold: 0.20,  // Bottom 20%
                min_data_points: 30,
            })
            .with_signal_mode(FundingSignalMode::Percentile);
        composite.add_generator(Box::new(funding_signal));
        tracing::info!("Added funding rate percentile signal to composite");
    }

    // Add 24h liquidation ratio signal
    if composite_config.enable_liq_ratio {
        let liq_ratio_signal = LiquidationRatioSignal::new(LiquidationRatioConfig::default());
        composite.add_generator(Box::new(liq_ratio_signal));
        tracing::info!("Added 24h liquidation ratio signal to composite");
    }

    tracing::info!(
        "Composite signal created with {} generators, require {} to agree",
        composite.generator_count(),
        composite_config.min_signals_agree
    );

    composite
}

/// Converts a `Direction` to a (signal_direction, is_directional) tuple.
///
/// # Returns
/// - `signal_direction`: true for Up, false for Down, true for Neutral (default)
/// - `is_directional`: true for Up/Down, false for Neutral
#[must_use]
pub fn direction_to_signal_bool(direction: Direction) -> (bool, bool) {
    match direction {
        Direction::Up => (true, true),
        Direction::Down => (false, true),
        Direction::Neutral => (true, false), // Default to true but mark as non-directional
    }
}

/// Result of computing a real signal.
#[derive(Debug, Clone)]
pub struct SignalResult {
    /// Whether the signal has a directional bias.
    pub is_directional: bool,
    /// The signal direction (true = Up/Yes, false = Down/No).
    pub signal_direction: bool,
    /// Signal strength (0.0 to 1.0).
    pub strength: f64,
    /// Total liquidation volume.
    pub total_volume: f64,
    /// Net delta (-1.0 to 1.0).
    pub net_delta: f64,
    /// Long liquidation volume.
    pub long_volume: f64,
    /// Short liquidation volume.
    pub short_volume: f64,
}

impl SignalResult {
    /// Creates a `SignalResult` from a `SignalValue`.
    #[must_use]
    pub fn from_signal_value(value: &SignalValue) -> Self {
        let (signal_direction, is_directional) = direction_to_signal_bool(value.direction);

        Self {
            is_directional,
            signal_direction,
            strength: value.strength,
            total_volume: *value.metadata.get("total_volume").unwrap_or(&0.0),
            net_delta: *value.metadata.get("net_delta").unwrap_or(&0.0),
            long_volume: *value.metadata.get("long_volume").unwrap_or(&0.0),
            short_volume: *value.metadata.get("short_volume").unwrap_or(&0.0),
        }
    }
}

/// Formats a log message for a signal result.
#[must_use]
pub fn format_signal_log(result: &SignalResult, market_id: &str) -> String {
    if result.is_directional {
        let direction_str = if result.signal_direction {
            "Up"
        } else {
            "Down"
        };
        format!(
            "Signal for {}: direction={}, strength={:.2}, volume=${:.0}, delta={:.2}, long=${:.0}, short=${:.0}",
            market_id,
            direction_str,
            result.strength,
            result.total_volume,
            result.net_delta,
            result.long_volume,
            result.short_volume
        )
    } else {
        format!(
            "No signal for {}: Neutral (volume=${:.0}, delta={:.2})",
            market_id, result.total_volume, result.net_delta
        )
    }
}

// =============================================================================
// Pending Entry Tracking
// =============================================================================

/// Tracks a pending entry opportunity waiting for optimal timing.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    /// Market data snapshot when signal fired.
    pub market: PolymarketOddsRecord,
    /// Signal strength when opportunity was detected.
    pub signal_strength: f64,
    /// Signal direction (true = Yes/Up, false = No/Down).
    pub signal_direction: bool,
    /// Estimated probability of the predicted outcome.
    pub estimated_prob: Decimal,
    /// Window start time.
    pub window_start: DateTime<Utc>,
    /// Window end time.
    pub window_end: DateTime<Utc>,
    /// Whether we've already placed a trade for this window.
    pub traded: bool,
    /// BTC price when the signal was detected (approximates window start price).
    pub btc_price_at_signal: Option<Decimal>,
    /// When the signal was detected (for staleness checking).
    pub signal_detected_at: DateTime<Utc>,
}

impl PendingEntry {
    /// Creates a new pending entry.
    #[must_use]
    pub fn new(
        market: PolymarketOddsRecord,
        signal_strength: f64,
        signal_direction: bool,
        estimated_prob: Decimal,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
        signal_detected_at: DateTime<Utc>,
    ) -> Self {
        Self {
            market,
            signal_strength,
            signal_direction,
            estimated_prob,
            window_start,
            window_end,
            traded: false,
            btc_price_at_signal: None,
            signal_detected_at,
        }
    }

    /// Sets the BTC price captured when the signal was detected.
    #[must_use]
    pub fn with_btc_price(mut self, price: Decimal) -> Self {
        self.btc_price_at_signal = Some(price);
        self
    }

    /// Returns true if this entry opportunity has expired (window ended).
    #[must_use]
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.window_end
    }

    /// Returns the current offset from window start.
    #[must_use]
    pub fn current_offset(&self, now: DateTime<Utc>) -> chrono::Duration {
        now - self.window_start
    }

    /// Returns the age of the signal (time since detection).
    #[must_use]
    pub fn signal_age(&self, now: DateTime<Utc>) -> chrono::Duration {
        now - self.signal_detected_at
    }

    /// Returns true if the signal is too old based on the max age.
    /// A stale signal indicates the initial move may have already played out.
    #[must_use]
    pub fn is_signal_stale(&self, now: DateTime<Utc>, max_age_mins: i64) -> bool {
        if max_age_mins <= 0 {
            return false; // Disabled
        }
        self.signal_age(now).num_minutes() >= max_age_mins
    }

    /// Creates an EntryContext for strategy evaluation.
    #[must_use]
    pub fn to_entry_context(
        &self,
        current_price: Decimal,
        now: DateTime<Utc>,
        window_duration: chrono::Duration,
        fee_rate: Decimal,
    ) -> EntryContext {
        let direction = if self.signal_direction {
            BetDirection::Yes
        } else {
            BetDirection::No
        };

        EntryContext::new(
            current_price,
            self.estimated_prob,
            self.current_offset(now),
            window_duration,
            direction,
            fee_rate,
        )
    }
}

// =============================================================================
// Decision Engine Configuration
// =============================================================================

/// Configuration for the paper trading decision engine.
#[derive(Debug, Clone)]
pub struct DecisionEngineConfig {
    /// Signal type to use.
    pub signal_type: String,
    /// Minimum signal strength to place bet.
    pub min_signal_strength: f64,
    /// Minimum edge required.
    pub min_edge: Decimal,
    /// Maximum price to buy at (for decent odds).
    pub max_price: Decimal,
    /// Kelly criterion calculator.
    pub kelly: KellyCriterion,
    /// Fee model for fee calculations.
    pub fee_tier: FeeTier,
    /// Use fixed stake instead of Kelly.
    pub use_fixed_stake: bool,
    /// Fixed stake amount.
    pub fixed_stake: Decimal,
    /// Cooldown between trades on same market.
    pub cooldown: Duration,

    // Entry strategy configuration
    /// Entry strategy configuration.
    pub entry_config: EntryStrategyConfig,
    /// Entry poll interval in seconds.
    pub entry_poll_secs: u64,

    // Real signal configuration
    /// Configuration for real signal generation.
    pub signal_config: SignalConfig,
}

impl DecisionEngineConfig {
    /// Creates a new config from CLI arguments.
    #[must_use]
    pub fn from_args(args: &PolymarketPaperTradeArgs) -> Self {
        let fee_tier = parse_fee_tier(&args.fee_tier).unwrap_or(FeeTier::Tier0);
        let kelly = KellyCriterion::new(
            Decimal::try_from(args.kelly_fraction).unwrap_or(dec!(0.25)),
            Decimal::try_from(args.max_bet_fraction).unwrap_or(dec!(0.05)),
            Decimal::try_from(args.min_edge).unwrap_or(dec!(0.02)),
        );

        // Parse entry strategy configuration
        let strategy_type =
            EntryStrategyType::parse(&args.entry_strategy).unwrap_or(EntryStrategyType::Immediate);

        let entry_config = EntryStrategyConfig {
            strategy_type,
            edge_threshold: Decimal::try_from(args.entry_threshold).unwrap_or(dec!(0.03)),
            offset_pct: args.entry_offset_pct,
            fallback_mins: args.entry_fallback_mins,
            window_mins: args.window_minutes,
            cutoff_mins: args.window_minutes - 1, // Default cutoff 1 minute before window close
        };

        // Parse signal configuration
        let signal_config = SignalConfig::from_args(args);

        Self {
            signal_type: args.signal.clone(),
            min_signal_strength: args.min_signal_strength,
            min_edge: Decimal::try_from(args.min_edge).unwrap_or(dec!(0.02)),
            max_price: Decimal::try_from(args.max_price).unwrap_or(dec!(0.55)),
            kelly,
            fee_tier,
            use_fixed_stake: args.use_fixed_stake,
            fixed_stake: Decimal::try_from(args.stake).unwrap_or(dec!(100)),
            cooldown: Duration::from_secs(args.cooldown_secs),
            entry_config,
            entry_poll_secs: args.entry_poll_secs,
            signal_config,
        }
    }

    /// Creates the entry strategy from this configuration.
    #[must_use]
    pub fn create_entry_strategy(&self) -> Box<dyn EntryStrategy> {
        create_entry_strategy(&self.entry_config)
    }
}

/// Parse fee tier string into FeeTier enum.
pub fn parse_fee_tier(s: &str) -> Result<FeeTier> {
    match s.to_lowercase().as_str() {
        "0" | "tier0" => Ok(FeeTier::Tier0),
        "1" | "tier1" => Ok(FeeTier::Tier1),
        "2" | "tier2" => Ok(FeeTier::Tier2),
        "3" | "tier3" => Ok(FeeTier::Tier3),
        "maker" => Ok(FeeTier::Maker),
        _ => Err(anyhow!("Invalid fee tier: {}. Use 0, 1, 2, 3, or maker", s)),
    }
}

/// Decision engine for determining whether to place trades.
#[derive(Debug)]
pub struct DecisionEngine {
    config: DecisionEngineConfig,
    current_bankroll: Decimal,
    last_trade_times: std::collections::HashMap<String, DateTime<Utc>>,
}

impl DecisionEngine {
    /// Creates a new decision engine.
    #[must_use]
    pub fn new(config: DecisionEngineConfig, initial_bankroll: Decimal) -> Self {
        Self {
            config,
            current_bankroll: initial_bankroll,
            last_trade_times: std::collections::HashMap::new(),
        }
    }

    /// Returns the current bankroll.
    #[must_use]
    pub fn bankroll(&self) -> Decimal {
        self.current_bankroll
    }

    /// Updates the bankroll after a trade settlement.
    pub fn update_bankroll(&mut self, pnl: Decimal) {
        self.current_bankroll += pnl;
    }

    /// Checks if cooldown has passed for a market.
    #[must_use]
    pub fn is_cooldown_active(&self, market_id: &str, now: DateTime<Utc>) -> bool {
        if let Some(last_trade) = self.last_trade_times.get(market_id) {
            let elapsed = now.signed_duration_since(*last_trade);
            elapsed.num_seconds() < self.config.cooldown.as_secs() as i64
        } else {
            false
        }
    }

    /// Records a trade for cooldown tracking.
    pub fn record_trade(&mut self, market_id: &str, timestamp: DateTime<Utc>) {
        self.last_trade_times
            .insert(market_id.to_string(), timestamp);
    }

    /// Evaluates a market opportunity and returns a trade decision.
    ///
    /// # Arguments
    /// * `market` - The Polymarket odds record
    /// * `signal_strength` - The signal strength (0.0 to 1.0)
    /// * `signal_direction` - The signal direction (true = up/yes, false = down/no)
    /// * `now` - Current timestamp
    #[must_use]
    pub fn evaluate(
        &self,
        market: &PolymarketOddsRecord,
        signal_strength: f64,
        signal_direction: bool,
        now: DateTime<Utc>,
    ) -> TradeDecision {
        // Check signal strength threshold
        if signal_strength < self.config.min_signal_strength {
            return TradeDecision::no_trade(&format!(
                "Signal strength {:.2} below threshold {:.2}",
                signal_strength, self.config.min_signal_strength
            ));
        }

        // Check cooldown
        if self.is_cooldown_active(&market.market_id, now) {
            return TradeDecision::no_trade("Cooldown active for this market");
        }

        // Determine direction and price
        let (direction, price) = if signal_direction {
            (PaperTradeDirection::Yes, market.outcome_yes_price)
        } else {
            (PaperTradeDirection::No, market.outcome_no_price)
        };

        // Check max price (poor odds filter)
        // Per research: only buy when price <= 0.55 for decent odds (1.82x+ payout)
        // At price 0.80, payout is only 1.25x which offers poor risk/reward
        if price > self.config.max_price {
            let payout = Decimal::ONE / price;
            return TradeDecision::no_trade(&format!(
                "Price {:.2} exceeds max {:.2} (payout only {:.2}x, need better odds)",
                price, self.config.max_price, payout
            ));
        }

        // Convert signal strength to estimated probability
        // Signal strength of 0.6 means 60% confidence in the direction
        // Map to estimated probability: base 0.5 + (strength - 0.5) * confidence_factor
        let estimated_prob = Self::signal_to_probability(signal_strength, price);

        // Check edge
        let edge = estimated_prob - price;
        if edge < self.config.min_edge {
            return TradeDecision::no_trade(&format!(
                "Edge {:.4} below minimum {:.4}",
                edge, self.config.min_edge
            ));
        }

        // Calculate bet size
        let (stake, kelly_fraction) = if self.config.use_fixed_stake {
            (self.config.fixed_stake, dec!(0))
        } else {
            match self
                .config
                .kelly
                .calculate_bet_size(estimated_prob, price, self.current_bankroll)
            {
                Some(bet_size) => (bet_size, self.config.kelly.fraction),
                None => {
                    return TradeDecision::no_trade("Kelly sizing returned no bet");
                }
            }
        };

        // Calculate shares and expected value
        let shares = stake / price;
        let expected_value = PaperTradeRecord::calculate_ev(estimated_prob, price, stake);

        TradeDecision::trade(direction, shares, stake, kelly_fraction, expected_value)
    }

    /// Converts signal strength to estimated probability.
    ///
    /// Uses a simple mapping: prob = price + (strength - 0.5) * adjustment_factor
    /// This means a 0.5 strength signal agrees with market price.
    #[must_use]
    pub fn signal_to_probability(signal_strength: f64, market_price: Decimal) -> Decimal {
        let strength_decimal = Decimal::try_from(signal_strength).unwrap_or(dec!(0.5));
        let adjustment = (strength_decimal - dec!(0.5)) * dec!(0.4); // Scale adjustment

        // Clamp between 0.01 and 0.99
        let prob = market_price + adjustment;
        prob.max(dec!(0.01)).min(dec!(0.99))
    }
}

/// Result of evaluating a trade opportunity.
#[derive(Debug, Clone)]
pub struct TradeDecision {
    /// Whether to place a trade.
    pub should_trade: bool,
    /// Direction of the trade.
    pub direction: Option<PaperTradeDirection>,
    /// Number of shares to buy.
    pub shares: Decimal,
    /// Stake amount.
    pub stake: Decimal,
    /// Kelly fraction used.
    pub kelly_fraction: Decimal,
    /// Expected value.
    pub expected_value: Decimal,
    /// Reason for the decision.
    pub reason: String,
}

impl TradeDecision {
    /// Creates a "no trade" decision.
    #[must_use]
    pub fn no_trade(reason: &str) -> Self {
        Self {
            should_trade: false,
            direction: None,
            shares: Decimal::ZERO,
            stake: Decimal::ZERO,
            kelly_fraction: Decimal::ZERO,
            expected_value: Decimal::ZERO,
            reason: reason.to_string(),
        }
    }

    /// Creates a "trade" decision.
    #[must_use]
    pub fn trade(
        direction: PaperTradeDirection,
        shares: Decimal,
        stake: Decimal,
        kelly_fraction: Decimal,
        expected_value: Decimal,
    ) -> Self {
        Self {
            should_trade: true,
            direction: Some(direction),
            shares,
            stake,
            kelly_fraction,
            expected_value,
            reason: "Signal meets criteria".to_string(),
        }
    }
}

/// Entry timing statistics for tracking entry strategy performance.
#[derive(Debug, Clone, Default)]
pub struct EntryTimingStats {
    /// Number of trades that used the primary entry strategy.
    pub primary_entries: u32,
    /// Number of trades that used the fallback entry.
    pub fallback_entries: u32,
    /// Sum of entry offset seconds for calculating average.
    pub total_entry_offset_secs: i64,
    /// Sum of edge at entry for calculating average.
    pub total_edge_at_entry: Decimal,
    /// Number of entries with edge data (for average calculation).
    pub entries_with_edge: u32,
}

impl EntryTimingStats {
    /// Records an entry with timing information.
    pub fn record_entry(&mut self, offset_secs: i64, edge: Decimal, used_fallback: bool) {
        if used_fallback {
            self.fallback_entries += 1;
        } else {
            self.primary_entries += 1;
        }
        self.total_entry_offset_secs += offset_secs;
        self.total_edge_at_entry += edge;
        self.entries_with_edge += 1;
    }

    /// Returns the total number of entries.
    #[must_use]
    pub fn total_entries(&self) -> u32 {
        self.primary_entries + self.fallback_entries
    }

    /// Returns the fallback rate (0.0 to 1.0).
    #[must_use]
    pub fn fallback_rate(&self) -> f64 {
        let total = self.total_entries();
        if total == 0 {
            0.0
        } else {
            self.fallback_entries as f64 / total as f64
        }
    }

    /// Returns the average entry offset in seconds.
    #[must_use]
    pub fn avg_entry_offset_secs(&self) -> Option<f64> {
        let total = self.total_entries();
        if total == 0 {
            None
        } else {
            Some(self.total_entry_offset_secs as f64 / total as f64)
        }
    }

    /// Returns the average edge at entry.
    #[must_use]
    pub fn avg_edge_at_entry(&self) -> Option<Decimal> {
        if self.entries_with_edge == 0 {
            None
        } else {
            Some(self.total_edge_at_entry / Decimal::from(self.entries_with_edge))
        }
    }
}

/// Paper trading executor that manages trades and tracks positions.
pub struct PaperTradeExecutor {
    engine: DecisionEngine,
    fee_model: PolymarketFees,
    session_id: String,
    trades_count: u32,
    wins: u32,
    losses: u32,
    /// Entry timing statistics.
    entry_stats: EntryTimingStats,
    /// Entry strategy name for reporting.
    entry_strategy_name: String,
}

impl PaperTradeExecutor {
    /// Creates a new paper trade executor.
    #[must_use]
    pub fn new(config: DecisionEngineConfig, initial_bankroll: Decimal) -> Self {
        let fee_model = PolymarketFees::new(config.fee_tier);
        let entry_strategy_name = config.entry_config.strategy_type.as_str().to_string();
        let engine = DecisionEngine::new(config, initial_bankroll);
        let session_id = Uuid::new_v4().to_string();

        Self {
            engine,
            fee_model,
            session_id,
            trades_count: 0,
            wins: 0,
            losses: 0,
            entry_stats: EntryTimingStats::default(),
            entry_strategy_name,
        }
    }

    /// Returns the session ID.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the current bankroll.
    #[must_use]
    pub fn bankroll(&self) -> Decimal {
        self.engine.bankroll()
    }

    /// Returns trade statistics.
    #[must_use]
    pub fn stats(&self) -> (u32, u32, u32) {
        (self.trades_count, self.wins, self.losses)
    }

    /// Returns entry timing statistics.
    #[must_use]
    pub fn entry_stats(&self) -> &EntryTimingStats {
        &self.entry_stats
    }

    /// Records entry timing for a trade.
    pub fn record_entry_timing(&mut self, offset_secs: i64, edge: Decimal, used_fallback: bool) {
        self.entry_stats
            .record_entry(offset_secs, edge, used_fallback);
    }

    /// Calculates fees for a trade.
    #[must_use]
    pub fn calculate_fees(&self, stake: Decimal, price: Decimal) -> Decimal {
        self.fee_model.calculate_fee(stake, price)
    }

    /// Executes a paper trade if decision is positive.
    ///
    /// Returns the paper trade record if a trade was placed.
    #[must_use]
    pub fn execute(
        &mut self,
        market: &PolymarketOddsRecord,
        signal_strength: f64,
        signal_direction: bool,
        now: DateTime<Utc>,
    ) -> Option<PaperTradeRecord> {
        let decision = self
            .engine
            .evaluate(market, signal_strength, signal_direction, now);

        if !decision.should_trade {
            tracing::debug!(
                market_id = %market.market_id,
                reason = %decision.reason,
                "Trade not placed"
            );
            return None;
        }

        let direction = decision.direction.unwrap();
        let price = match direction {
            PaperTradeDirection::Yes => market.outcome_yes_price,
            PaperTradeDirection::No => market.outcome_no_price,
        };

        // Create paper trade record
        let estimated_prob = DecisionEngine::signal_to_probability(signal_strength, price);
        let mut trade = PaperTradeRecord::new(
            now,
            market.market_id.clone(),
            market.question.clone(),
            direction,
            decision.shares,
            price,
            estimated_prob,
            decision.kelly_fraction,
            Decimal::try_from(signal_strength).unwrap_or(dec!(0)),
            self.session_id.clone(),
        )
        .with_signals(json!({
            "strength": signal_strength,
            "direction": if signal_direction { "up" } else { "down" },
            "market_yes_price": market.outcome_yes_price.to_string(),
            "market_no_price": market.outcome_no_price.to_string(),
        }));

        // Store market end_date for proper settlement timing
        if let Some(end_date) = market.end_date {
            trade = trade.with_market_end_date(end_date);
        }

        self.engine.record_trade(&market.market_id, now);
        self.trades_count += 1;

        tracing::info!(
            market_id = %market.market_id,
            direction = %trade.direction,
            shares = %trade.shares,
            stake = %trade.stake,
            expected_value = %trade.expected_value,
            "Paper trade placed"
        );

        Some(trade)
    }

    /// Settles a paper trade based on simulated outcome.
    ///
    /// For now, this uses a simple random simulation.
    /// In a real implementation, this would check actual market resolution.
    pub fn settle(&mut self, trade: &mut PaperTradeRecord, won: bool, settled_at: DateTime<Utc>) {
        let fees = self.calculate_fees(trade.stake, trade.entry_price);
        trade.settle(won, fees, settled_at);

        if let Some(pnl) = trade.pnl {
            self.engine.update_bankroll(pnl);
            if won {
                self.wins += 1;
            } else {
                self.losses += 1;
            }
        }
    }

    /// Applies settlement results from the settlement service.
    ///
    /// Updates bankroll and win/loss counts based on external settlement.
    pub fn apply_settlement(&mut self, pnl: Decimal, won: bool) {
        self.engine.update_bankroll(pnl);
        if won {
            self.wins += 1;
        } else {
            self.losses += 1;
        }
    }

    /// Formats a summary of the trading session.
    #[must_use]
    pub fn format_summary(&self) -> String {
        let win_rate = if self.trades_count > 0 {
            self.wins as f64 / self.trades_count as f64 * 100.0
        } else {
            0.0
        };

        // Entry timing summary
        let entry_summary = if self.entry_stats.total_entries() > 0 {
            let avg_offset = self.entry_stats.avg_entry_offset_secs().unwrap_or(0.0);
            let avg_edge = self
                .entry_stats
                .avg_edge_at_entry()
                .unwrap_or(Decimal::ZERO);
            let fallback_rate = self.entry_stats.fallback_rate() * 100.0;

            format!(
                "\n\
                 Entry Strategy Stats:\n\
                 - Strategy: {}\n\
                 - Primary entries: {}\n\
                 - Fallback entries: {} ({:.1}%)\n\
                 - Avg entry offset: {:.1}s\n\
                 - Avg edge at entry: {:.4}",
                self.entry_strategy_name,
                self.entry_stats.primary_entries,
                self.entry_stats.fallback_entries,
                fallback_rate,
                avg_offset,
                avg_edge
            )
        } else {
            format!("\nEntry Strategy: {}", self.entry_strategy_name)
        };

        format!(
            "Paper Trading Session Summary:\n\
             - Session ID: {}\n\
             - Total trades: {}\n\
             - Wins: {} | Losses: {}\n\
             - Win rate: {:.1}%\n\
             - Final bankroll: ${:.2}\
             {}",
            self.session_id,
            self.trades_count,
            self.wins,
            self.losses,
            win_rate,
            self.bankroll(),
            entry_summary
        )
    }
}

/// Runs the polymarket-paper-trade command.
///
/// # Errors
/// Returns an error if database connection fails or execution cannot continue.
pub async fn run_polymarket_paper_trade(args: PolymarketPaperTradeArgs) -> Result<()> {
    use sqlx::postgres::PgPoolOptions;

    // Parse duration
    let duration = parse_duration(&args.duration)?;

    tracing::info!(
        "Starting Polymarket paper trading for {:?} with {} signal",
        duration,
        args.signal
    );

    // Get database URL
    let db_url = args
        .db_url
        .clone()
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    // Create database pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Create repositories
    let odds_repo = PolymarketOddsRepository::new(pool.clone());
    let paper_repo = PaperTradeRepository::new(pool.clone());
    let liq_repo = LiquidationRepository::new(pool);

    // Create executor
    let config = DecisionEngineConfig::from_args(&args);
    let signal_config = config.signal_config.clone();
    let entry_config = config.entry_config.clone();
    let initial_bankroll = Decimal::try_from(args.bankroll).unwrap_or(dec!(10000));
    let executor = Arc::new(Mutex::new(PaperTradeExecutor::new(
        config,
        initial_bankroll,
    )));

    // Log signal mode
    let signal_mode_str = if signal_config.use_simulated {
        "simulated".to_string()
    } else {
        format!(
            "real ({:?}) from {}/{} window={}m",
            signal_config.signal_mode,
            signal_config.liquidation_symbol,
            signal_config.liquidation_exchange,
            signal_config.liquidation_window_mins
        )
    };

    tracing::info!(
        "Paper trading config: signal={}, mode={}, min_strength={:.2}, kelly_fraction={:.2}, bankroll=${}",
        args.signal,
        signal_mode_str,
        args.min_signal_strength,
        args.kelly_fraction,
        initial_bankroll
    );

    // Shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));

    // Create window timer for Polymarket 15-minute clock alignment
    let window_timer = WindowTimer::new(args.window_minutes, args.entry_cutoff_mins);

    tracing::info!(
        "Window timing: {}m windows, {}m entry cutoff, {}m max signal age, {}m max aggregate age (settle at :00/:15/:30/:45)",
        args.window_minutes,
        args.entry_cutoff_mins,
        args.max_signal_age_mins,
        args.max_aggregate_age_mins
    );

    // Create settlement service
    let settlement_service = if let Some(rpc_url) = args.polygon_rpc_url.clone() {
        SettlementService::new(rpc_url, args.window_minutes)
    } else {
        SettlementService::default_polygon(args.window_minutes)
    };
    let settlement_fee_rate = Decimal::try_from(args.settlement_fee_rate).unwrap_or(dec!(0.02));
    let settlement_service = Arc::new(Mutex::new(settlement_service));

    tracing::info!(
        "Settlement: Chainlink BTC/USD on Polygon, fee_rate={:.2}%",
        settlement_fee_rate * dec!(100)
    );

    // Create entry strategy
    let entry_strategy = create_entry_strategy(&entry_config);
    let entry_poll_interval = Duration::from_secs(args.entry_poll_secs);
    let window_duration = chrono::Duration::minutes(args.window_minutes);
    let entry_fee_rate = Decimal::try_from(args.settlement_fee_rate).unwrap_or(dec!(0.02));

    tracing::info!(
        "Entry strategy: {} (poll every {}s)",
        entry_config.strategy_type.as_str(),
        args.entry_poll_secs
    );

    // Create composite signal configuration
    let composite_config = CompositeSignalConfig::from_args(&args);
    if composite_config.enabled {
        tracing::info!(
            "Composite mode: require {} signals to agree (orderbook={}, funding={}, liq_ratio={})",
            composite_config.min_signals_agree,
            composite_config.enable_orderbook,
            composite_config.enable_funding,
            composite_config.enable_liq_ratio
        );
    }

    // Main trading loop
    let poll_interval = Duration::from_secs(args.poll_interval_secs);
    let shutdown_clone = shutdown.clone();
    let executor_clone = executor.clone();
    let signal_type = args.signal.clone();

    let max_signal_age_mins = args.max_signal_age_mins;
    let max_aggregate_age_mins = args.max_aggregate_age_mins;
    let trading_handle = tokio::spawn(async move {
        let ctx = TradingLoopContext {
            executor: executor_clone,
            odds_repo,
            paper_repo,
            liq_repo,
            poll_interval,
            shutdown: shutdown_clone,
            signal_type,
            signal_config,
            window_timer,
            settlement_service,
            settlement_fee_rate,
            entry_strategy,
            entry_poll_interval,
            window_duration,
            entry_fee_rate,
            composite_config,
            max_signal_age_mins,
            max_aggregate_age_mins,
        };
        run_trading_loop(ctx).await
    });

    // Wait for duration or shutdown signal
    tracing::info!(
        "Paper trading running. Will stop in {:?} or on Ctrl+C",
        duration
    );

    let timeout = tokio::time::sleep(duration);
    tokio::pin!(timeout);

    tokio::select! {
        _ = &mut timeout => {
            tracing::info!("Duration elapsed, initiating shutdown");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, initiating shutdown");
        }
    }

    // Signal shutdown
    shutdown.store(true, Ordering::Relaxed);

    // Wait for trading loop to finish
    tokio::time::sleep(Duration::from_secs(2)).await;
    trading_handle.abort();

    // Print final summary
    let final_executor = executor.lock().await;
    tracing::info!("{}", final_executor.format_summary());

    tracing::info!("Paper trading session complete");
    Ok(())
}

/// Context for the trading loop containing all required dependencies.
struct TradingLoopContext {
    executor: Arc<Mutex<PaperTradeExecutor>>,
    odds_repo: PolymarketOddsRepository,
    paper_repo: PaperTradeRepository,
    liq_repo: LiquidationRepository,
    poll_interval: Duration,
    shutdown: Arc<AtomicBool>,
    signal_type: String,
    signal_config: SignalConfig,
    /// Window timer for Polymarket 15-minute clock alignment
    window_timer: WindowTimer,
    /// Settlement service for settling trades via Chainlink
    settlement_service: Arc<Mutex<SettlementService>>,
    /// Fee rate for settlement
    settlement_fee_rate: Decimal,
    /// Entry strategy for timing trade entries
    entry_strategy: Box<dyn EntryStrategy>,
    /// Entry poll interval (faster polling when waiting for entry conditions)
    entry_poll_interval: Duration,
    /// Window duration for entry context
    window_duration: chrono::Duration,
    /// Fee rate for entry context edge calculation
    entry_fee_rate: Decimal,
    /// Composite signal configuration for multi-signal voting
    composite_config: CompositeSignalConfig,
    /// Maximum signal age in minutes before rejecting entry (0 to disable)
    max_signal_age_mins: i64,
    /// Maximum liquidation aggregate age in minutes before ignoring (0 to disable)
    max_aggregate_age_mins: i64,
}

/// Main trading loop that polls for opportunities and executes trades.
async fn run_trading_loop(ctx: TradingLoopContext) {
    let TradingLoopContext {
        executor,
        odds_repo,
        paper_repo,
        liq_repo,
        poll_interval: _poll_interval,
        shutdown,
        signal_type,
        signal_config,
        window_timer,
        settlement_service,
        settlement_fee_rate,
        entry_strategy,
        entry_poll_interval,
        window_duration,
        entry_fee_rate,
        composite_config,
        max_signal_age_mins,
        max_aggregate_age_mins,
    } = ctx;

    // Use faster polling when we have pending entries, slower otherwise
    let mut interval = tokio::time::interval(entry_poll_interval);

    // Track pending entry opportunities (keyed by window start time)
    let mut pending_entries: std::collections::HashMap<DateTime<Utc>, PendingEntry> =
        std::collections::HashMap::new();

    // Initialize signal generator(s) based on configuration
    let mut liq_signal = if !signal_config.use_simulated {
        Some(create_liquidation_signal(&signal_config))
    } else {
        None
    };

    // Initialize composite signal if enabled
    let mut composite_signal = if composite_config.enabled {
        let composite = create_composite_signal(&composite_config);
        tracing::info!(
            "Initialized composite signal with {} generators",
            composite.generator_count()
        );
        Some(composite)
    } else {
        None
    };

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("Trading loop received shutdown signal");
            break;
        }

        let now = Utc::now();

        // =====================================================================
        // WINDOW TIMING CHECK
        // Polymarket BTC 15-min binaries settle at :00, :15, :30, :45
        // We need to ensure we have enough time before settlement to enter
        // =====================================================================
        let window_status = window_timer.status(now);

        // Log window status periodically (every minute at :00 seconds)
        if now.second() < 5 {
            tracing::info!(
                "Window: {} | {}",
                window_status,
                if window_status.can_trade {
                    "TRADING ENABLED"
                } else {
                    "WAITING FOR NEXT WINDOW"
                }
            );
        }

        if !window_status.can_trade {
            tracing::debug!(
                "Too close to settlement ({} remaining), waiting for next window at {:02}:{:02}",
                format!(
                    "{}m {}s",
                    window_status.time_remaining.num_minutes(),
                    window_status.time_remaining.num_seconds() % 60
                ),
                window_timer.next_window(now).start.hour(),
                window_timer.next_window(now).start.minute()
            );
            continue;
        }

        // Get latest market data for ACTIVE markets only (filters out expired)
        let markets = match odds_repo.get_active_markets().await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Failed to query market data: {}", e);
                continue;
            }
        };

        if markets.is_empty() {
            tracing::debug!("No market data available");
            continue;
        }

        // Compute signal (composite, real, or simulated)
        let (signal_strength, signal_direction) = if let Some(ref mut composite) = composite_signal
        {
            // Use composite multi-signal voting
            // Build a minimal SignalContext for composite signal computation
            let ctx = SignalContext::new(now, "BTCUSD");

            match composite.compute(&ctx).await {
                Ok(signal_value) => {
                    let (direction_bool, is_directional) =
                        direction_to_signal_bool(signal_value.direction);
                    if is_directional {
                        tracing::info!(
                            direction = if direction_bool { "Up" } else { "Down" },
                            strength = signal_value.strength,
                            confidence = signal_value.confidence,
                            signals_agreed = composite_config.min_signals_agree,
                            "Composite signal fired"
                        );
                    }
                    (signal_value.strength, direction_bool)
                }
                Err(e) => {
                    tracing::warn!("Failed to compute composite signal: {}", e);
                    (0.0, true) // Neutral fallback
                }
            }
        } else if let Some(ref mut signal) = liq_signal {
            // Use real liquidation signal
            match compute_real_signal(
                &liq_repo,
                signal,
                &signal_config,
                now,
                max_aggregate_age_mins,
            )
            .await
            {
                Ok(result) => {
                    // Log signal computation
                    if result.is_directional {
                        tracing::debug!(
                            direction = if result.signal_direction {
                                "Up"
                            } else {
                                "Down"
                            },
                            strength = result.strength,
                            volume = result.total_volume,
                            delta = result.net_delta,
                            "Real signal computed"
                        );
                    }
                    (result.strength, result.signal_direction)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to compute real signal, falling back to simulated: {}",
                        e
                    );
                    // Fallback to simulated for this iteration
                    (0.0, true)
                }
            }
        } else {
            // Use simulated signal (computed per-market below)
            (0.0, true) // Placeholder, will be computed per market
        };

        // =====================================================================
        // STEP 1: Check for new signal opportunities and create PendingEntries
        // =====================================================================
        let current_window = window_timer.current_window(now);

        for market in &markets {
            // Skip if we already have a pending entry for this window
            if pending_entries.contains_key(&current_window.start) {
                continue;
            }

            // Determine signal for this market
            let (strength, direction) = if liq_signal.is_some() {
                // Real signal already computed above (applies to all markets)
                (signal_strength, signal_direction)
            } else {
                // Simulate signal per-market
                simulate_signal(&signal_type, market)
            };

            // Check if signal meets minimum strength threshold
            let exec = executor.lock().await;
            let min_signal = exec.engine.config.min_signal_strength;
            let max_price = exec.engine.config.max_price;
            drop(exec);

            if strength < min_signal {
                tracing::debug!(
                    strength = strength,
                    min_signal = min_signal,
                    "Signal below minimum threshold"
                );
                continue;
            }

            // Check price constraint
            let current_price = if direction {
                market.outcome_yes_price
            } else {
                Decimal::ONE - market.outcome_yes_price // No price
            };

            if current_price > max_price {
                tracing::debug!(
                    price = %current_price,
                    max_price = %max_price,
                    "Price above maximum threshold"
                );
                continue;
            }

            // Create pending entry - entry strategy will determine when to trade
            // estimated_prob is the probability of YES winning
            // For Yes direction: high signal = high prob of Yes = 0.5 + strength * 0.3
            // For No direction: high signal = LOW prob of Yes = 0.5 - strength * 0.3
            let estimated_prob = if direction {
                // Yes/Up signal: higher strength = higher P(Yes)
                Decimal::from_f64_retain(0.5 + strength * 0.3).unwrap_or(dec!(0.55))
            } else {
                // No/Down signal: higher strength = LOWER P(Yes) = higher P(No)
                Decimal::from_f64_retain(0.5 - strength * 0.3).unwrap_or(dec!(0.45))
            };

            // Capture BTC price at signal detection (approximates window start price)
            let btc_price_at_signal = {
                let settlement = settlement_service.lock().await;
                settlement.get_current_price().await.ok()
            };

            let mut pending = PendingEntry::new(
                market.clone(),
                strength,
                direction,
                estimated_prob,
                current_window.start,
                current_window.end,
                now, // Track when signal was detected for staleness checking
            );
            if let Some(price) = btc_price_at_signal {
                pending = pending.with_btc_price(price);
            }

            tracing::info!(
                market_id = %market.market_id,
                signal_strength = strength,
                direction = if direction { "Yes" } else { "No" },
                window_start = %current_window.start.format("%H:%M"),
                window_end = %current_window.end.format("%H:%M"),
                btc_price = ?btc_price_at_signal,
                signal_detected_at = %now.format("%H:%M:%S"),
                "Created pending entry - waiting for entry strategy"
            );

            pending_entries.insert(current_window.start, pending);
        }

        // =====================================================================
        // STEP 2: Evaluate pending entries with entry strategy
        // =====================================================================
        let mut entries_to_remove = Vec::new();

        for (window_start, pending) in pending_entries.iter_mut() {
            // Skip if already traded
            if pending.traded {
                continue;
            }

            // Check if window expired
            if pending.is_expired(now) {
                tracing::info!(
                    window_start = %window_start.format("%H:%M"),
                    window_end = %pending.window_end.format("%H:%M"),
                    signal_strength = pending.signal_strength,
                    direction = if pending.signal_direction { "Yes" } else { "No" },
                    "Pending entry EXPIRED without trading - entry conditions not met"
                );
                entries_to_remove.push(*window_start);
                continue;
            }

            // Check if signal is too old (cascade move may have already played out)
            if pending.is_signal_stale(now, max_signal_age_mins) {
                let signal_age_mins = pending.signal_age(now).num_minutes();
                tracing::info!(
                    window_start = %window_start.format("%H:%M"),
                    signal_age_mins = signal_age_mins,
                    max_age_mins = max_signal_age_mins,
                    signal_strength = pending.signal_strength,
                    direction = if pending.signal_direction { "Yes" } else { "No" },
                    "Signal too stale - initial move likely played out, skipping entry"
                );
                entries_to_remove.push(*window_start);
                continue;
            }

            // Get current market price (fetch latest data)
            let current_price = if pending.signal_direction {
                pending.market.outcome_yes_price
            } else {
                Decimal::ONE - pending.market.outcome_yes_price
            };

            // Build entry context for strategy evaluation
            let entry_ctx =
                pending.to_entry_context(current_price, now, window_duration, entry_fee_rate);

            // Evaluate entry strategy
            let decision = entry_strategy.evaluate(&entry_ctx);

            match decision {
                EntryDecision::Enter {
                    offset,
                    direction: bet_dir,
                } => {
                    tracing::info!(
                        strategy = entry_strategy.name(),
                        offset_mins = offset.num_minutes(),
                        direction = ?bet_dir,
                        edge = %entry_ctx.calculate_edge(),
                        "Entry strategy triggered - executing trade"
                    );

                    // Get BTC price at entry time
                    let btc_price_at_entry = {
                        let settlement = settlement_service.lock().await;
                        settlement.get_current_price().await.ok()
                    };

                    // Execute the trade
                    let mut exec = executor.lock().await;
                    if let Some(mut trade) = exec.execute(
                        &pending.market,
                        pending.signal_strength,
                        pending.signal_direction,
                        now,
                    ) {
                        // Add BTC prices to trade for analysis
                        if let (Some(window_start_price), Some(entry_price)) =
                            (pending.btc_price_at_signal, btc_price_at_entry)
                        {
                            trade = trade.with_btc_prices(window_start_price, entry_price);
                        }

                        // Store the trade
                        match paper_repo.insert(&trade).await {
                            Ok(id) => {
                                tracing::info!(
                                    trade_id = id,
                                    market_id = %trade.market_id,
                                    signal_strength = pending.signal_strength,
                                    signal_direction = if pending.signal_direction { "yes" } else { "no" },
                                    entry_offset_mins = offset.num_minutes(),
                                    edge = %entry_ctx.calculate_edge(),
                                    btc_window_start = ?pending.btc_price_at_signal,
                                    btc_at_entry = ?btc_price_at_entry,
                                    "Paper trade stored via entry strategy"
                                );
                            }
                            Err(e) => {
                                tracing::error!("Failed to store paper trade: {}", e);
                            }
                        }
                        pending.traded = true;
                    }
                }
                EntryDecision::NoEntry { reason } => {
                    // Log at info level once per minute (every 6th iteration at 10s intervals)
                    let offset_secs = entry_ctx.current_offset.num_seconds();
                    if offset_secs % 60 < 10 {
                        tracing::info!(
                            strategy = entry_strategy.name(),
                            reason = reason,
                            offset_mins = entry_ctx.current_offset.num_minutes(),
                            edge = %entry_ctx.calculate_edge(),
                            signal_strength = pending.signal_strength,
                            direction = if pending.signal_direction { "Yes" } else { "No" },
                            "Entry strategy waiting - conditions not met"
                        );
                    }
                }
            }
        }

        // Cleanup only expired entries (NOT traded entries)
        // Keep traded entries until window expires to prevent re-evaluating the same window
        for window_start in entries_to_remove {
            pending_entries.remove(&window_start);
        }

        // =====================================================================
        // SETTLEMENT CHECK
        // Settle any pending trades whose window has ended
        // =====================================================================
        {
            let mut settlement = settlement_service.lock().await;
            match settlement
                .settle_pending_trades(&paper_repo, settlement_fee_rate)
                .await
            {
                Ok(results) if !results.is_empty() => {
                    let mut exec = executor.lock().await;
                    let current_session_id = exec.session_id().to_string();

                    // Only update stats for trades from the current session
                    let mut session_wins = 0u32;
                    let mut session_losses = 0u32;
                    for result in &results {
                        // Look up trade to check session
                        if let Ok(Some(trade)) = paper_repo.get_by_id(result.trade_id).await {
                            if trade.session_id == current_session_id {
                                exec.apply_settlement(result.pnl, result.won);
                                if result.won {
                                    session_wins += 1;
                                } else {
                                    session_losses += 1;
                                }
                            }
                        }
                    }

                    let wins_count = results.iter().filter(|r| r.won).count();
                    let losses_count = results.len() - wins_count;
                    tracing::info!(
                        settled = results.len(),
                        wins = wins_count,
                        losses = losses_count,
                        session_wins = session_wins,
                        session_losses = session_losses,
                        total_pnl = %results.iter().map(|r| r.pnl).sum::<Decimal>(),
                        "Settled pending trades"
                    );
                }
                Ok(_) => {
                    // No trades to settle
                }
                Err(e) => {
                    tracing::warn!("Failed to settle pending trades: {}", e);
                }
            }
        }

        // Log periodic stats
        let exec = executor.lock().await;
        let (total, wins, losses) = exec.stats();
        tracing::info!(
            total_trades = total,
            wins = wins,
            losses = losses,
            bankroll = %exec.bankroll(),
            "Trading loop status"
        );
    }
}

/// Computes a real signal from the liquidation repository.
///
/// Fetches the latest liquidation aggregate and computes the signal using
/// the configured `LiquidationCascadeSignal`.
///
/// # Arguments
/// * `liq_repo` - Liquidation repository for fetching data
/// * `signal` - The liquidation cascade signal generator
/// * `config` - Signal configuration
/// * `now` - Current timestamp
/// * `max_aggregate_age_mins` - Maximum age of aggregate in minutes (0 to disable)
///
/// # Errors
/// Returns an error if database query fails or signal computation fails.
async fn compute_real_signal(
    liq_repo: &LiquidationRepository,
    signal: &mut LiquidationCascadeSignal,
    config: &SignalConfig,
    now: DateTime<Utc>,
    max_aggregate_age_mins: i64,
) -> Result<SignalResult> {
    // Fetch latest aggregate
    let aggregate_record = liq_repo
        .get_latest_aggregate(
            &config.liquidation_symbol,
            &config.liquidation_exchange,
            config.liquidation_window_mins,
        )
        .await?;

    // Convert to core type and compute signal
    let signal_value = if let Some(record) = aggregate_record {
        // Check if aggregate is too old (likely from previous window)
        if max_aggregate_age_mins > 0 {
            let aggregate_age = now - record.timestamp;
            let max_age = chrono::Duration::minutes(max_aggregate_age_mins);

            if aggregate_age > max_age {
                tracing::debug!(
                    aggregate_timestamp = %record.timestamp.format("%H:%M:%S"),
                    aggregate_age_secs = aggregate_age.num_seconds(),
                    max_age_mins = max_aggregate_age_mins,
                    "Liquidation aggregate too old (likely from previous window), returning neutral"
                );
                return Ok(SignalResult::from_signal_value(&SignalValue::neutral()));
            }
        }

        let core_agg = convert_aggregate_record_to_core(&record);
        let ctx = SignalContext::new(now, &config.liquidation_symbol)
            .with_exchange(&config.liquidation_exchange)
            .with_liquidation_aggregates(core_agg);

        signal.compute(&ctx).await?
    } else {
        tracing::debug!(
            "No liquidation aggregate found for {}/{} window={}m",
            config.liquidation_symbol,
            config.liquidation_exchange,
            config.liquidation_window_mins
        );
        SignalValue::neutral()
    };

    Ok(SignalResult::from_signal_value(&signal_value))
}

/// Simulates signal generation for a market.
///
/// In production, this would use the actual signal registry and computed signals.
fn simulate_signal(signal_type: &str, market: &PolymarketOddsRecord) -> (f64, bool) {
    // Simple simulation based on price imbalance
    let yes_price: f64 = market.outcome_yes_price.try_into().unwrap_or(0.5);
    let no_price: f64 = market.outcome_no_price.try_into().unwrap_or(0.5);

    match signal_type {
        "composite" | "imbalance" => {
            // If yes price < 0.5, there might be an opportunity
            if yes_price < 0.45 {
                (0.7 + (0.5 - yes_price), true) // Bet yes
            } else if no_price < 0.45 {
                (0.7 + (0.5 - no_price), false) // Bet no
            } else {
                (0.4, true) // Low signal, unlikely to trade
            }
        }
        _ => (0.5, true), // Neutral signal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    // =========================================================================
    // Test Helpers
    // =========================================================================

    fn sample_timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 31, 12, 0, 0).unwrap()
    }

    fn sample_market() -> PolymarketOddsRecord {
        PolymarketOddsRecord {
            timestamp: sample_timestamp(),
            market_id: "btc-100k-feb".to_string(),
            question: "Will Bitcoin exceed $100k by Feb 2025?".to_string(),
            outcome_yes_price: dec!(0.50), // Under max_price of 0.55 for decent odds
            outcome_no_price: dec!(0.50),
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
            end_date: None,
        }
    }

    fn sample_config() -> DecisionEngineConfig {
        DecisionEngineConfig {
            signal_type: "composite".to_string(),
            min_signal_strength: 0.6,
            min_edge: dec!(0.02),
            max_price: dec!(0.55),
            kelly: KellyCriterion::quarter_kelly(),
            fee_tier: FeeTier::Tier0,
            use_fixed_stake: false,
            fixed_stake: dec!(100),
            cooldown: Duration::from_secs(900),
            entry_config: EntryStrategyConfig::immediate(),
            entry_poll_secs: 10,
            signal_config: SignalConfig::default(),
        }
    }

    // =========================================================================
    // parse_fee_tier Tests
    // =========================================================================

    #[test]
    fn test_parse_fee_tier_numeric() {
        assert_eq!(parse_fee_tier("0").unwrap(), FeeTier::Tier0);
        assert_eq!(parse_fee_tier("1").unwrap(), FeeTier::Tier1);
        assert_eq!(parse_fee_tier("2").unwrap(), FeeTier::Tier2);
        assert_eq!(parse_fee_tier("3").unwrap(), FeeTier::Tier3);
    }

    #[test]
    fn test_parse_fee_tier_named() {
        assert_eq!(parse_fee_tier("tier0").unwrap(), FeeTier::Tier0);
        assert_eq!(parse_fee_tier("TIER1").unwrap(), FeeTier::Tier1);
        assert_eq!(parse_fee_tier("maker").unwrap(), FeeTier::Maker);
    }

    #[test]
    fn test_parse_fee_tier_invalid() {
        assert!(parse_fee_tier("4").is_err());
        assert!(parse_fee_tier("invalid").is_err());
    }

    // =========================================================================
    // DecisionEngineConfig Tests
    // =========================================================================

    /// Creates default test args with all fields populated.
    fn sample_args() -> PolymarketPaperTradeArgs {
        PolymarketPaperTradeArgs {
            duration: "24h".to_string(),
            signal: "composite".to_string(),
            min_signal_strength: 0.6,
            stake: 100.0,
            kelly_fraction: 0.25,
            min_edge: 0.02,
            max_price: 0.55,
            bankroll: 10000.0,
            max_bet_fraction: 0.05,
            fee_tier: "0".to_string(),
            poll_interval_secs: 60,
            db_url: None,
            cooldown_secs: 900,
            use_fixed_stake: false,
            entry_strategy: "immediate".to_string(),
            entry_threshold: 0.03,
            entry_offset_pct: 0.25,
            entry_fallback_mins: 2,
            window_minutes: 15,
            entry_cutoff_mins: 2,
            entry_poll_secs: 10,
            max_signal_age_mins: 4,
            max_aggregate_age_mins: 5,
            // Signal config defaults
            use_simulated_signals: true,
            signal_mode: "cascade".to_string(),
            min_volume_usd: 100000.0,
            imbalance_threshold: 0.6,
            liquidation_window_mins: 5,
            liquidation_symbol: "BTCUSDT".to_string(),
            liquidation_exchange: "binance".to_string(),
            // Composite signal config defaults
            enable_composite: false,
            min_signals_agree: 2,
            enable_orderbook_signal: false,
            enable_funding_signal: false,
            enable_liq_ratio_signal: false,
            // Settlement config defaults
            polygon_rpc_url: None,
            settlement_fee_rate: 0.02,
        }
    }

    // =========================================================================
    // CompositeSignalConfig Tests
    // =========================================================================

    #[test]
    fn test_composite_signal_config_default() {
        let config = CompositeSignalConfig::default();

        assert!(!config.enabled);
        assert_eq!(config.min_signals_agree, 2);
        assert!(!config.enable_orderbook);
        assert!(!config.enable_funding);
        assert!(!config.enable_liq_ratio);
    }

    #[test]
    fn test_composite_signal_config_from_args() {
        let mut args = sample_args();
        args.enable_composite = true;
        args.min_signals_agree = 3;
        args.enable_orderbook_signal = true;
        args.enable_funding_signal = true;
        args.enable_liq_ratio_signal = false;

        let config = CompositeSignalConfig::from_args(&args);

        assert!(config.enabled);
        assert_eq!(config.min_signals_agree, 3);
        assert!(config.enable_orderbook);
        assert!(config.enable_funding);
        assert!(!config.enable_liq_ratio);
    }

    #[test]
    fn test_composite_signal_config_enabled_count() {
        let mut config = CompositeSignalConfig::default();

        // None enabled
        assert_eq!(config.enabled_signal_count(), 0);

        // One enabled
        config.enable_orderbook = true;
        assert_eq!(config.enabled_signal_count(), 1);

        // Two enabled
        config.enable_funding = true;
        assert_eq!(config.enabled_signal_count(), 2);

        // All enabled
        config.enable_liq_ratio = true;
        assert_eq!(config.enabled_signal_count(), 3);
    }

    #[test]
    fn test_composite_signal_config_min_signals_clamped() {
        let mut args = sample_args();
        args.min_signals_agree = 0; // Should be clamped to at least 1

        let config = CompositeSignalConfig::from_args(&args);

        assert_eq!(config.min_signals_agree, 1);
    }

    // =========================================================================
    // create_composite_signal Tests
    // =========================================================================

    #[test]
    fn test_create_composite_signal_empty() {
        let config = CompositeSignalConfig::default();

        let composite = create_composite_signal(&config);

        assert_eq!(composite.generator_count(), 0);
    }

    #[test]
    fn test_create_composite_signal_with_orderbook() {
        let config = CompositeSignalConfig {
            enabled: true,
            min_signals_agree: 2,
            enable_orderbook: true,
            enable_funding: false,
            enable_liq_ratio: false,
        };

        let composite = create_composite_signal(&config);

        assert_eq!(composite.generator_count(), 1);
    }

    #[test]
    fn test_create_composite_signal_with_all() {
        let config = CompositeSignalConfig {
            enabled: true,
            min_signals_agree: 2,
            enable_orderbook: true,
            enable_funding: true,
            enable_liq_ratio: true,
        };

        let composite = create_composite_signal(&config);

        assert_eq!(composite.generator_count(), 3);
    }

    #[test]
    fn test_create_composite_signal_name() {
        let config = CompositeSignalConfig {
            enabled: true,
            min_signals_agree: 2,
            enable_orderbook: true,
            enable_funding: true,
            enable_liq_ratio: false,
        };

        let composite = create_composite_signal(&config);

        assert_eq!(composite.name(), "composite_multi_signal");
    }

    #[test]
    fn test_decision_engine_config_from_args() {
        let mut args = sample_args();
        args.min_signal_strength = 0.7;
        args.stake = 200.0;
        args.kelly_fraction = 0.5;
        args.min_edge = 0.03;
        args.bankroll = 20000.0;
        args.max_bet_fraction = 0.1;
        args.fee_tier = "2".to_string();
        args.poll_interval_secs = 120;
        args.cooldown_secs = 1800;
        args.use_fixed_stake = true;
        args.entry_strategy = "edge_threshold".to_string();
        args.entry_threshold = 0.05;
        args.entry_offset_pct = 0.3;
        args.entry_fallback_mins = 3;
        args.entry_poll_secs = 15;

        let config = DecisionEngineConfig::from_args(&args);

        assert_eq!(config.signal_type, "composite");
        assert!((config.min_signal_strength - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.min_edge, dec!(0.03));
        assert_eq!(config.fee_tier, FeeTier::Tier2);
        assert!(config.use_fixed_stake);
        assert_eq!(
            config.entry_config.strategy_type,
            EntryStrategyType::EdgeThreshold
        );
        assert_eq!(config.entry_config.edge_threshold, dec!(0.05));
        assert_eq!(config.entry_poll_secs, 15);
        assert_eq!(config.fixed_stake, dec!(200));
        assert_eq!(config.cooldown, Duration::from_secs(1800));
    }

    // =========================================================================
    // DecisionEngine Tests
    // =========================================================================

    #[test]
    fn test_decision_engine_new() {
        let config = sample_config();
        let engine = DecisionEngine::new(config, dec!(10000));

        assert_eq!(engine.bankroll(), dec!(10000));
    }

    #[test]
    fn test_decision_engine_update_bankroll() {
        let config = sample_config();
        let mut engine = DecisionEngine::new(config, dec!(10000));

        engine.update_bankroll(dec!(500));
        assert_eq!(engine.bankroll(), dec!(10500));

        engine.update_bankroll(dec!(-200));
        assert_eq!(engine.bankroll(), dec!(10300));
    }

    #[test]
    fn test_decision_engine_cooldown_inactive() {
        let config = sample_config();
        let engine = DecisionEngine::new(config, dec!(10000));
        let now = sample_timestamp();

        // No trades recorded, cooldown should be inactive
        assert!(!engine.is_cooldown_active("market-1", now));
    }

    #[test]
    fn test_decision_engine_cooldown_active() {
        let config = sample_config();
        let mut engine = DecisionEngine::new(config, dec!(10000));
        let now = sample_timestamp();

        // Record a trade
        engine.record_trade("market-1", now);

        // Cooldown should be active immediately after
        let slightly_later = now + chrono::Duration::seconds(10);
        assert!(engine.is_cooldown_active("market-1", slightly_later));

        // Different market should not have cooldown
        assert!(!engine.is_cooldown_active("market-2", slightly_later));
    }

    #[test]
    fn test_decision_engine_cooldown_expired() {
        let mut config = sample_config();
        config.cooldown = Duration::from_secs(60);
        let mut engine = DecisionEngine::new(config, dec!(10000));
        let now = sample_timestamp();

        // Record a trade
        engine.record_trade("market-1", now);

        // Cooldown should expire after 60 seconds
        let later = now + chrono::Duration::seconds(61);
        assert!(!engine.is_cooldown_active("market-1", later));
    }

    #[test]
    fn test_decision_engine_evaluate_low_signal_strength() {
        let config = sample_config();
        let engine = DecisionEngine::new(config, dec!(10000));
        let market = sample_market();
        let now = sample_timestamp();

        // Signal strength below threshold
        let decision = engine.evaluate(&market, 0.5, true, now);

        assert!(!decision.should_trade);
        assert!(decision.reason.contains("below threshold"));
    }

    #[test]
    fn test_decision_engine_evaluate_cooldown_active() {
        let config = sample_config();
        let mut engine = DecisionEngine::new(config, dec!(10000));
        let market = sample_market();
        let now = sample_timestamp();

        // Record a trade to trigger cooldown
        engine.record_trade(&market.market_id, now);

        // Try to trade again immediately
        let decision = engine.evaluate(&market, 0.8, true, now);

        assert!(!decision.should_trade);
        assert!(decision.reason.contains("Cooldown"));
    }

    #[test]
    fn test_decision_engine_evaluate_low_edge() {
        let mut config = sample_config();
        config.min_edge = dec!(0.20); // Very high edge requirement
        let engine = DecisionEngine::new(config, dec!(10000));
        let market = sample_market();
        let now = sample_timestamp();

        // Signal meets strength but edge is too low
        let decision = engine.evaluate(&market, 0.65, true, now);

        assert!(!decision.should_trade);
        assert!(decision.reason.contains("Edge"));
    }

    #[test]
    fn test_decision_engine_evaluate_price_too_high() {
        let config = sample_config(); // max_price = 0.55
        let engine = DecisionEngine::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.80); // Price above max_price
        let now = sample_timestamp();

        // Even with strong signal, price is too high (poor odds)
        let decision = engine.evaluate(&market, 0.85, true, now);

        assert!(!decision.should_trade);
        assert!(decision.reason.contains("Price"));
        assert!(decision.reason.contains("exceeds max"));
    }

    #[test]
    fn test_decision_engine_evaluate_success() {
        let config = sample_config();
        let engine = DecisionEngine::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.45); // Lower price = higher potential edge
        let now = sample_timestamp();

        // Strong signal, good edge
        let decision = engine.evaluate(&market, 0.75, true, now);

        assert!(decision.should_trade);
        assert_eq!(decision.direction, Some(PaperTradeDirection::Yes));
        assert!(decision.shares > dec!(0));
        assert!(decision.stake > dec!(0));
        assert!(decision.expected_value > dec!(0));
    }

    #[test]
    fn test_decision_engine_evaluate_fixed_stake() {
        let mut config = sample_config();
        config.use_fixed_stake = true;
        config.fixed_stake = dec!(250);
        let engine = DecisionEngine::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.45);
        let now = sample_timestamp();

        let decision = engine.evaluate(&market, 0.75, true, now);

        assert!(decision.should_trade);
        assert_eq!(decision.stake, dec!(250));
        assert_eq!(decision.kelly_fraction, dec!(0)); // Kelly not used
    }

    #[test]
    fn test_decision_engine_signal_to_probability() {
        // At 0.5 strength, probability should equal market price
        let prob = DecisionEngine::signal_to_probability(0.5, dec!(0.60));
        assert_eq!(prob, dec!(0.60));

        // At 0.75 strength (high confidence), probability should be higher
        let prob_high = DecisionEngine::signal_to_probability(0.75, dec!(0.60));
        assert!(prob_high > dec!(0.60));

        // At 0.25 strength (low confidence), probability should be lower
        let prob_low = DecisionEngine::signal_to_probability(0.25, dec!(0.60));
        assert!(prob_low < dec!(0.60));
    }

    #[test]
    fn test_decision_engine_signal_to_probability_clamped() {
        // Very high strength should be clamped at 0.99
        let prob = DecisionEngine::signal_to_probability(1.0, dec!(0.90));
        assert_eq!(prob, dec!(0.99));

        // Very low strength should be clamped at 0.01
        let prob = DecisionEngine::signal_to_probability(0.0, dec!(0.10));
        assert_eq!(prob, dec!(0.01));
    }

    // =========================================================================
    // TradeDecision Tests
    // =========================================================================

    #[test]
    fn test_trade_decision_no_trade() {
        let decision = TradeDecision::no_trade("Not enough edge");

        assert!(!decision.should_trade);
        assert!(decision.direction.is_none());
        assert_eq!(decision.shares, dec!(0));
        assert_eq!(decision.stake, dec!(0));
        assert_eq!(decision.reason, "Not enough edge");
    }

    #[test]
    fn test_trade_decision_trade() {
        let decision = TradeDecision::trade(
            PaperTradeDirection::Yes,
            dec!(100),
            dec!(60),
            dec!(0.25),
            dec!(10),
        );

        assert!(decision.should_trade);
        assert_eq!(decision.direction, Some(PaperTradeDirection::Yes));
        assert_eq!(decision.shares, dec!(100));
        assert_eq!(decision.stake, dec!(60));
        assert_eq!(decision.kelly_fraction, dec!(0.25));
        assert_eq!(decision.expected_value, dec!(10));
    }

    // =========================================================================
    // PaperTradeExecutor Tests
    // =========================================================================

    #[test]
    fn test_executor_new() {
        let config = sample_config();
        let executor = PaperTradeExecutor::new(config, dec!(10000));

        assert_eq!(executor.bankroll(), dec!(10000));
        assert!(!executor.session_id().is_empty());
        assert_eq!(executor.stats(), (0, 0, 0));
    }

    #[test]
    fn test_executor_calculate_fees() {
        let config = sample_config();
        let executor = PaperTradeExecutor::new(config, dec!(10000));

        // Tier 0: 2% fee on potential profit
        // stake = 100, price = 0.50, potential_profit = 100
        // fee = 100 * 0.02 = 2
        let fee = executor.calculate_fees(dec!(100), dec!(0.50));
        assert_eq!(fee, dec!(2));
    }

    #[test]
    fn test_executor_execute_no_trade() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));
        let market = sample_market();
        let now = sample_timestamp();

        // Low signal strength should not trigger trade
        let trade = executor.execute(&market, 0.4, true, now);

        assert!(trade.is_none());
        assert_eq!(executor.stats(), (0, 0, 0));
    }

    #[test]
    fn test_executor_execute_trade() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.45);
        let now = sample_timestamp();

        // Strong signal should trigger trade
        let trade = executor.execute(&market, 0.75, true, now);

        assert!(trade.is_some());
        let trade = trade.unwrap();
        assert_eq!(trade.direction, "yes");
        assert!(trade.stake > dec!(0));
        assert_eq!(executor.stats(), (1, 0, 0));
    }

    #[test]
    fn test_executor_settle_win() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.45);
        let now = sample_timestamp();

        let mut trade = executor.execute(&market, 0.75, true, now).unwrap();
        let initial_bankroll = executor.bankroll();

        // Settle as win
        let settle_time = now + chrono::Duration::hours(1);
        executor.settle(&mut trade, true, settle_time);

        assert!(trade.is_win());
        assert!(executor.bankroll() > initial_bankroll);
        assert_eq!(executor.stats(), (1, 1, 0));
    }

    #[test]
    fn test_executor_settle_loss() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.45);
        let now = sample_timestamp();

        let mut trade = executor.execute(&market, 0.75, true, now).unwrap();
        let initial_bankroll = executor.bankroll();

        // Settle as loss
        let settle_time = now + chrono::Duration::hours(1);
        executor.settle(&mut trade, false, settle_time);

        assert!(trade.is_loss());
        assert!(executor.bankroll() < initial_bankroll);
        assert_eq!(executor.stats(), (1, 0, 1));
    }

    #[test]
    fn test_executor_format_summary() {
        let config = sample_config();
        let executor = PaperTradeExecutor::new(config, dec!(10000));

        let summary = executor.format_summary();

        assert!(summary.contains("Paper Trading Session Summary"));
        assert!(summary.contains("Session ID"));
        assert!(summary.contains("Total trades: 0"));
        assert!(summary.contains("$10000"));
    }

    // =========================================================================
    // simulate_signal Tests
    // =========================================================================

    #[test]
    fn test_simulate_signal_composite() {
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.40); // Low price = opportunity

        let (strength, direction) = simulate_signal("composite", &market);

        assert!(strength > 0.6); // Should have signal
        assert!(direction); // Should be yes
    }

    #[test]
    fn test_simulate_signal_no_opportunity() {
        let mut market = sample_market();
        market.outcome_yes_price = dec!(0.50);
        market.outcome_no_price = dec!(0.50);

        let (strength, _) = simulate_signal("composite", &market);

        assert!(strength < 0.5); // No clear signal
    }

    #[test]
    fn test_simulate_signal_unknown() {
        let market = sample_market();

        let (strength, _) = simulate_signal("unknown_signal", &market);

        assert!((strength - 0.5).abs() < f64::EPSILON); // Neutral
    }

    // =========================================================================
    // PolymarketPaperTradeArgs Tests
    // =========================================================================

    #[test]
    fn test_args_structure() {
        let mut args = sample_args();
        args.duration = "7d".to_string();
        args.signal = "imbalance".to_string();
        args.min_signal_strength = 0.7;
        args.stake = 200.0;
        args.kelly_fraction = 0.5;
        args.min_edge = 0.05;
        args.bankroll = 50000.0;
        args.max_bet_fraction = 0.1;
        args.fee_tier = "1".to_string();
        args.poll_interval_secs = 120;
        args.db_url = Some("postgres://localhost/test".to_string());
        args.cooldown_secs = 1800;

        assert_eq!(args.duration, "7d");
        assert_eq!(args.signal, "imbalance");
        assert!((args.min_signal_strength - 0.7).abs() < f64::EPSILON);
        assert!(args.db_url.is_some());
        assert_eq!(args.entry_strategy, "immediate");
        assert_eq!(args.entry_fallback_mins, 2);
        assert_eq!(args.window_minutes, 15);
    }

    // =========================================================================
    // Entry Strategy CLI Tests
    // =========================================================================

    #[test]
    fn test_entry_strategy_config_immediate() {
        let args = sample_args(); // Uses immediate by default

        let config = DecisionEngineConfig::from_args(&args);
        assert_eq!(
            config.entry_config.strategy_type,
            EntryStrategyType::Immediate
        );

        let strategy = config.create_entry_strategy();
        assert_eq!(strategy.name(), "ImmediateEntry");
    }

    #[test]
    fn test_entry_strategy_config_fixed_time_with_fallback() {
        let mut args = sample_args();
        args.entry_strategy = "fixed_time".to_string();
        args.entry_offset_pct = 0.5;
        args.entry_fallback_mins = 3;

        let config = DecisionEngineConfig::from_args(&args);
        assert_eq!(
            config.entry_config.strategy_type,
            EntryStrategyType::FixedTime
        );
        assert!((config.entry_config.offset_pct - 0.5).abs() < f64::EPSILON);

        let strategy = config.create_entry_strategy();
        // FallbackEntry returns inner strategy name (FixedTimeEntry)
        assert_eq!(strategy.name(), "FixedTimeEntry");
    }

    #[test]
    fn test_entry_strategy_config_edge_threshold_without_fallback() {
        let mut args = sample_args();
        args.entry_strategy = "edge_threshold".to_string();
        args.entry_threshold = 0.05;
        args.entry_fallback_mins = 0; // Disable fallback

        let config = DecisionEngineConfig::from_args(&args);
        assert_eq!(
            config.entry_config.strategy_type,
            EntryStrategyType::EdgeThreshold
        );
        assert_eq!(config.entry_config.edge_threshold, dec!(0.05));
        assert_eq!(config.entry_config.fallback_mins, 0);

        let strategy = config.create_entry_strategy();
        // Should be EdgeThresholdEntry directly (no fallback)
        assert_eq!(strategy.name(), "EdgeThresholdEntry");
    }

    // =========================================================================
    // Entry Timing Stats Tests
    // =========================================================================

    #[test]
    fn test_entry_timing_stats_default() {
        let stats = EntryTimingStats::default();

        assert_eq!(stats.primary_entries, 0);
        assert_eq!(stats.fallback_entries, 0);
        assert_eq!(stats.total_entries(), 0);
        assert!((stats.fallback_rate() - 0.0).abs() < f64::EPSILON);
        assert!(stats.avg_entry_offset_secs().is_none());
        assert!(stats.avg_edge_at_entry().is_none());
    }

    #[test]
    fn test_entry_timing_stats_record_primary_entry() {
        let mut stats = EntryTimingStats::default();

        stats.record_entry(120, dec!(0.08), false); // 2 minutes, 8% edge, not fallback

        assert_eq!(stats.primary_entries, 1);
        assert_eq!(stats.fallback_entries, 0);
        assert_eq!(stats.total_entries(), 1);
        assert!((stats.fallback_rate() - 0.0).abs() < f64::EPSILON);
        assert_eq!(stats.avg_entry_offset_secs(), Some(120.0));
        assert_eq!(stats.avg_edge_at_entry(), Some(dec!(0.08)));
    }

    #[test]
    fn test_entry_timing_stats_record_fallback_entry() {
        let mut stats = EntryTimingStats::default();

        stats.record_entry(720, dec!(0.02), true); // 12 minutes, 2% edge, fallback

        assert_eq!(stats.primary_entries, 0);
        assert_eq!(stats.fallback_entries, 1);
        assert_eq!(stats.total_entries(), 1);
        assert!((stats.fallback_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_entry_timing_stats_multiple_entries() {
        let mut stats = EntryTimingStats::default();

        stats.record_entry(60, dec!(0.10), false); // Primary
        stats.record_entry(120, dec!(0.08), false); // Primary
        stats.record_entry(720, dec!(0.02), true); // Fallback

        assert_eq!(stats.primary_entries, 2);
        assert_eq!(stats.fallback_entries, 1);
        assert_eq!(stats.total_entries(), 3);
        assert!((stats.fallback_rate() - (1.0 / 3.0)).abs() < 0.01);

        // Avg offset = (60 + 120 + 720) / 3 = 300
        assert_eq!(stats.avg_entry_offset_secs(), Some(300.0));

        // Avg edge = (0.10 + 0.08 + 0.02) / 3 = 0.0666...
        let avg_edge = stats.avg_edge_at_entry().unwrap();
        let expected_avg = (dec!(0.10) + dec!(0.08) + dec!(0.02)) / dec!(3);
        assert_eq!(avg_edge, expected_avg);
    }

    #[test]
    fn test_executor_entry_stats() {
        let config = sample_config();
        let executor = PaperTradeExecutor::new(config, dec!(10000));

        let stats = executor.entry_stats();
        assert_eq!(stats.total_entries(), 0);
    }

    #[test]
    fn test_executor_record_entry_timing() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));

        executor.record_entry_timing(180, dec!(0.05), false);
        executor.record_entry_timing(720, dec!(0.02), true);

        let stats = executor.entry_stats();
        assert_eq!(stats.primary_entries, 1);
        assert_eq!(stats.fallback_entries, 1);
        assert_eq!(stats.total_entries(), 2);
    }

    #[test]
    fn test_executor_format_summary_includes_entry_stats() {
        let config = sample_config();
        let mut executor = PaperTradeExecutor::new(config, dec!(10000));

        // Record some entry timing
        executor.record_entry_timing(60, dec!(0.08), false);
        executor.record_entry_timing(120, dec!(0.06), false);
        executor.record_entry_timing(720, dec!(0.02), true);

        let summary = executor.format_summary();

        // Should contain entry strategy info
        assert!(summary.contains("Entry Strategy Stats"));
        assert!(summary.contains("Strategy: immediate"));
        assert!(summary.contains("Primary entries: 2"));
        assert!(summary.contains("Fallback entries: 1"));
    }

    #[test]
    fn test_executor_format_summary_no_entry_stats() {
        let config = sample_config();
        let executor = PaperTradeExecutor::new(config, dec!(10000));

        let summary = executor.format_summary();

        // Should still contain entry strategy name
        assert!(summary.contains("Entry Strategy: immediate"));
        // But not the detailed stats
        assert!(!summary.contains("Primary entries"));
    }

    // =========================================================================
    // Real Signal Wiring Tests (TDD)
    // =========================================================================

    mod real_signal_tests {
        use super::*;
        use algo_trade_core::{Direction, LiquidationAggregate};

        // =====================================================================
        // parse_signal_mode Tests
        // =====================================================================

        #[test]
        fn test_parse_signal_mode_cascade() {
            let mode = parse_signal_mode("cascade");
            assert_eq!(mode, LiquidationSignalMode::Cascade);
        }

        #[test]
        fn test_parse_signal_mode_exhaustion() {
            let mode = parse_signal_mode("exhaustion");
            assert_eq!(mode, LiquidationSignalMode::Exhaustion);
        }

        #[test]
        fn test_parse_signal_mode_combined() {
            let mode = parse_signal_mode("combined");
            assert_eq!(mode, LiquidationSignalMode::Combined);
        }

        #[test]
        fn test_parse_signal_mode_case_insensitive() {
            assert_eq!(parse_signal_mode("CASCADE"), LiquidationSignalMode::Cascade);
            assert_eq!(
                parse_signal_mode("Exhaustion"),
                LiquidationSignalMode::Exhaustion
            );
            assert_eq!(
                parse_signal_mode("COMBINED"),
                LiquidationSignalMode::Combined
            );
        }

        #[test]
        fn test_parse_signal_mode_invalid_defaults_to_cascade() {
            let mode = parse_signal_mode("invalid");
            assert_eq!(mode, LiquidationSignalMode::Cascade);
        }

        // =====================================================================
        // SignalConfig Tests
        // =====================================================================

        #[test]
        fn test_signal_config_default() {
            let config = SignalConfig::default();

            assert_eq!(config.signal_mode, LiquidationSignalMode::Cascade);
            assert_eq!(config.min_volume_usd, dec!(100000));
            assert!((config.imbalance_threshold - 0.6).abs() < f64::EPSILON);
            assert_eq!(config.liquidation_window_mins, 5);
            assert_eq!(config.liquidation_symbol, "BTCUSDT");
            assert_eq!(config.liquidation_exchange, "binance");
        }

        #[test]
        fn test_signal_config_from_args_custom_values() {
            let args = PolymarketPaperTradeArgs {
                duration: "24h".to_string(),
                signal: "liquidation".to_string(),
                min_signal_strength: 0.6,
                stake: 100.0,
                kelly_fraction: 0.25,
                min_edge: 0.02,
                max_price: 0.55,
                bankroll: 10000.0,
                max_bet_fraction: 0.05,
                fee_tier: "0".to_string(),
                poll_interval_secs: 60,
                db_url: None,
                cooldown_secs: 900,
                use_fixed_stake: false,
                entry_strategy: "immediate".to_string(),
                entry_threshold: 0.03,
                entry_offset_pct: 0.25,
                entry_fallback_mins: 2,
                window_minutes: 15,
                entry_poll_secs: 10,
                // New signal args
                use_simulated_signals: false,
                signal_mode: "exhaustion".to_string(),
                min_volume_usd: 200000.0,
                imbalance_threshold: 0.7,
                liquidation_window_mins: 10,
                liquidation_symbol: "ETHUSDT".to_string(),
                liquidation_exchange: "bybit".to_string(),
                entry_cutoff_mins: 2,
                max_signal_age_mins: 4,
                max_aggregate_age_mins: 5,
                // Composite signal config
                enable_composite: false,
                min_signals_agree: 2,
                enable_orderbook_signal: false,
                enable_funding_signal: false,
                enable_liq_ratio_signal: false,
                // Settlement config
                polygon_rpc_url: None,
                settlement_fee_rate: 0.02,
            };

            let config = SignalConfig::from_args(&args);

            assert!(!config.use_simulated);
            assert_eq!(config.signal_mode, LiquidationSignalMode::Exhaustion);
            assert_eq!(config.min_volume_usd, dec!(200000));
            assert!((config.imbalance_threshold - 0.7).abs() < f64::EPSILON);
            assert_eq!(config.liquidation_window_mins, 10);
            assert_eq!(config.liquidation_symbol, "ETHUSDT");
            assert_eq!(config.liquidation_exchange, "bybit");
        }

        #[test]
        fn test_signal_config_simulated_mode() {
            let args = PolymarketPaperTradeArgs {
                duration: "24h".to_string(),
                signal: "composite".to_string(),
                min_signal_strength: 0.6,
                stake: 100.0,
                kelly_fraction: 0.25,
                min_edge: 0.02,
                max_price: 0.55,
                bankroll: 10000.0,
                max_bet_fraction: 0.05,
                fee_tier: "0".to_string(),
                poll_interval_secs: 60,
                db_url: None,
                cooldown_secs: 900,
                use_fixed_stake: false,
                entry_strategy: "immediate".to_string(),
                entry_threshold: 0.03,
                entry_offset_pct: 0.25,
                entry_fallback_mins: 2,
                window_minutes: 15,
                entry_poll_secs: 10,
                max_signal_age_mins: 4,
                max_aggregate_age_mins: 5,
                // Use simulated signals
                use_simulated_signals: true,
                signal_mode: "cascade".to_string(),
                min_volume_usd: 100000.0,
                imbalance_threshold: 0.6,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
                entry_cutoff_mins: 2,
                // Composite signal config
                enable_composite: false,
                min_signals_agree: 2,
                enable_orderbook_signal: false,
                enable_funding_signal: false,
                enable_liq_ratio_signal: false,
                // Settlement config
                polygon_rpc_url: None,
                settlement_fee_rate: 0.02,
            };

            let config = SignalConfig::from_args(&args);
            assert!(config.use_simulated);
        }

        // =====================================================================
        // convert_aggregate_record_to_core Tests
        // =====================================================================

        #[test]
        fn test_convert_aggregate_record_to_core() {
            use crate::commands::polymarket_paper_trade::convert_aggregate_record_to_core;
            use algo_trade_data::LiquidationAggregateRecord;

            let timestamp = sample_timestamp();
            let record = LiquidationAggregateRecord {
                timestamp,
                symbol: "BTCUSDT".to_string(),
                exchange: "binance".to_string(),
                window_minutes: 5,
                long_volume: dec!(100000),
                short_volume: dec!(50000),
                net_delta: dec!(50000),
                count_long: 10,
                count_short: 5,
            };

            let core_agg = convert_aggregate_record_to_core(&record);

            assert_eq!(core_agg.timestamp, timestamp);
            assert_eq!(core_agg.window_minutes, 5);
            assert_eq!(core_agg.long_volume_usd, dec!(100000));
            assert_eq!(core_agg.short_volume_usd, dec!(50000));
            assert_eq!(core_agg.net_delta_usd, dec!(50000));
            assert_eq!(core_agg.count_long, 10);
            assert_eq!(core_agg.count_short, 5);
        }

        // =====================================================================
        // direction_to_signal_bool Tests
        // =====================================================================

        #[test]
        fn test_direction_to_signal_bool_up_is_true() {
            let (direction, is_directional) = direction_to_signal_bool(Direction::Up);
            assert!(direction);
            assert!(is_directional);
        }

        #[test]
        fn test_direction_to_signal_bool_down_is_false() {
            let (direction, is_directional) = direction_to_signal_bool(Direction::Down);
            assert!(!direction);
            assert!(is_directional);
        }

        #[test]
        fn test_direction_to_signal_bool_neutral() {
            let (direction, is_directional) = direction_to_signal_bool(Direction::Neutral);
            // For neutral, direction defaults to true but is_directional is false
            assert!(direction);
            assert!(!is_directional);
        }

        // =====================================================================
        // SignalResult Tests
        // =====================================================================

        #[test]
        fn test_signal_result_from_signal_value_strong_up() {
            use algo_trade_core::SignalValue;

            let signal_value = SignalValue::new(Direction::Up, 0.8, 0.9)
                .unwrap()
                .with_metadata("total_volume", 150000.0)
                .with_metadata("net_delta", 0.65);

            let result = SignalResult::from_signal_value(&signal_value);

            assert!(result.is_directional);
            assert!(result.signal_direction);
            assert!((result.strength - 0.8).abs() < f64::EPSILON);
            assert!((result.total_volume - 150000.0).abs() < 0.01);
            assert!((result.net_delta - 0.65).abs() < 0.01);
        }

        #[test]
        fn test_signal_result_from_signal_value_down() {
            use algo_trade_core::SignalValue;

            let signal_value = SignalValue::new(Direction::Down, 0.7, 0.8).unwrap();
            let result = SignalResult::from_signal_value(&signal_value);

            assert!(result.is_directional);
            assert!(!result.signal_direction);
            assert!((result.strength - 0.7).abs() < f64::EPSILON);
        }

        #[test]
        fn test_signal_result_from_signal_value_neutral() {
            use algo_trade_core::SignalValue;

            let signal_value = SignalValue::neutral();
            let result = SignalResult::from_signal_value(&signal_value);

            assert!(!result.is_directional);
            assert!((result.strength - 0.0).abs() < f64::EPSILON);
        }

        #[test]
        fn test_signal_result_default_metadata_values() {
            use algo_trade_core::SignalValue;

            // Signal value without metadata
            let signal_value = SignalValue::new(Direction::Up, 0.5, 0.5).unwrap();
            let result = SignalResult::from_signal_value(&signal_value);

            assert!((result.total_volume - 0.0).abs() < 0.01);
            assert!((result.net_delta - 0.0).abs() < 0.01);
            assert!((result.long_volume - 0.0).abs() < 0.01);
            assert!((result.short_volume - 0.0).abs() < 0.01);
        }

        // =====================================================================
        // DecisionEngineConfig with SignalConfig Tests
        // =====================================================================

        #[test]
        fn test_decision_engine_config_includes_signal_config() {
            let args = PolymarketPaperTradeArgs {
                duration: "24h".to_string(),
                signal: "liquidation".to_string(),
                min_signal_strength: 0.6,
                stake: 100.0,
                kelly_fraction: 0.25,
                min_edge: 0.02,
                max_price: 0.55,
                bankroll: 10000.0,
                max_bet_fraction: 0.05,
                fee_tier: "0".to_string(),
                poll_interval_secs: 60,
                db_url: None,
                cooldown_secs: 900,
                use_fixed_stake: false,
                entry_strategy: "immediate".to_string(),
                entry_threshold: 0.03,
                entry_offset_pct: 0.25,
                entry_fallback_mins: 2,
                window_minutes: 15,
                entry_poll_secs: 10,
                max_signal_age_mins: 4,
                max_aggregate_age_mins: 5,
                use_simulated_signals: false,
                signal_mode: "cascade".to_string(),
                min_volume_usd: 150000.0,
                imbalance_threshold: 0.65,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
                entry_cutoff_mins: 2,
                // Composite signal config
                enable_composite: false,
                min_signals_agree: 2,
                enable_orderbook_signal: false,
                enable_funding_signal: false,
                enable_liq_ratio_signal: false,
                // Settlement config
                polygon_rpc_url: None,
                settlement_fee_rate: 0.02,
            };

            let config = DecisionEngineConfig::from_args(&args);

            assert!(!config.signal_config.use_simulated);
            assert_eq!(
                config.signal_config.signal_mode,
                LiquidationSignalMode::Cascade
            );
            assert_eq!(config.signal_config.min_volume_usd, dec!(150000));
        }

        // =====================================================================
        // Log Signal Result Tests
        // =====================================================================

        #[test]
        fn test_format_signal_log_directional() {
            let result = SignalResult {
                is_directional: true,
                signal_direction: true,
                strength: 0.75,
                total_volume: 150000.0,
                net_delta: 0.65,
                long_volume: 100000.0,
                short_volume: 50000.0,
            };

            let log = format_signal_log(&result, "btc-100k-market");

            assert!(log.contains("btc-100k-market"));
            assert!(log.contains("Up") || log.contains("up"));
            assert!(log.contains("0.75") || log.contains("75"));
            assert!(log.contains("150000"));
        }

        #[test]
        fn test_format_signal_log_neutral() {
            let result = SignalResult {
                is_directional: false,
                signal_direction: true,
                strength: 0.0,
                total_volume: 50000.0,
                net_delta: 0.0,
                long_volume: 25000.0,
                short_volume: 25000.0,
            };

            let log = format_signal_log(&result, "test-market");

            assert!(
                log.contains("Neutral") || log.contains("neutral") || log.contains("No signal")
            );
        }

        // =====================================================================
        // create_liquidation_signal Tests
        // =====================================================================

        #[test]
        fn test_create_liquidation_signal_cascade_mode() {
            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Cascade,
                min_volume_usd: dec!(100000),
                imbalance_threshold: 0.6,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
            };

            let signal = create_liquidation_signal(&config);

            assert_eq!(signal.name(), "liquidation_cascade");
            assert_eq!(signal.signal_mode, LiquidationSignalMode::Cascade);
            assert_eq!(signal.cascade_config.min_volume_usd, dec!(100000));
            assert!((signal.cascade_config.imbalance_threshold - 0.6).abs() < 0.001);
        }

        #[test]
        fn test_create_liquidation_signal_exhaustion_mode() {
            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Exhaustion,
                min_volume_usd: dec!(50000),
                imbalance_threshold: 0.5,
                liquidation_window_mins: 10,
                liquidation_symbol: "ETHUSDT".to_string(),
                liquidation_exchange: "bybit".to_string(),
            };

            let signal = create_liquidation_signal(&config);

            assert_eq!(signal.signal_mode, LiquidationSignalMode::Exhaustion);
            assert!(signal.exhaustion_config.is_some());
        }

        #[test]
        fn test_create_liquidation_signal_combined_mode() {
            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Combined,
                min_volume_usd: dec!(75000),
                imbalance_threshold: 0.55,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
            };

            let signal = create_liquidation_signal(&config);

            assert_eq!(signal.signal_mode, LiquidationSignalMode::Combined);
            assert!(signal.exhaustion_config.is_some());
        }

        // =====================================================================
        // Async Signal Computation Tests
        // =====================================================================

        #[tokio::test]
        async fn test_compute_signal_from_aggregate_bearish() {
            use algo_trade_core::SignalContext;

            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Cascade,
                min_volume_usd: dec!(50000),
                imbalance_threshold: 0.5,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
            };

            let mut signal = create_liquidation_signal(&config);

            // Create aggregate with heavy long liquidations (bearish signal)
            let agg = LiquidationAggregate {
                timestamp: sample_timestamp(),
                window_minutes: 5,
                long_volume_usd: dec!(150000),
                short_volume_usd: dec!(10000),
                net_delta_usd: dec!(140000),
                count_long: 20,
                count_short: 2,
            };

            let ctx =
                SignalContext::new(sample_timestamp(), "BTCUSDT").with_liquidation_aggregates(agg);

            let result = signal.compute(&ctx).await.unwrap();

            // Heavy long liquidations = bearish = Down direction
            assert_eq!(result.direction, Direction::Down);
            assert!(result.strength > 0.5);
        }

        #[tokio::test]
        async fn test_compute_signal_from_aggregate_bullish() {
            use algo_trade_core::SignalContext;

            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Cascade,
                min_volume_usd: dec!(50000),
                imbalance_threshold: 0.5,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
            };

            let mut signal = create_liquidation_signal(&config);

            // Create aggregate with heavy short liquidations (bullish signal)
            let agg = LiquidationAggregate {
                timestamp: sample_timestamp(),
                window_minutes: 5,
                long_volume_usd: dec!(10000),
                short_volume_usd: dec!(150000),
                net_delta_usd: dec!(-140000),
                count_long: 2,
                count_short: 20,
            };

            let ctx =
                SignalContext::new(sample_timestamp(), "BTCUSDT").with_liquidation_aggregates(agg);

            let result = signal.compute(&ctx).await.unwrap();

            // Heavy short liquidations = bullish = Up direction
            assert_eq!(result.direction, Direction::Up);
            assert!(result.strength > 0.5);
        }

        #[tokio::test]
        async fn test_compute_signal_from_aggregate_neutral_low_volume() {
            use algo_trade_core::SignalContext;

            let config = SignalConfig {
                use_simulated: false,
                signal_mode: LiquidationSignalMode::Cascade,
                min_volume_usd: dec!(100000), // High threshold
                imbalance_threshold: 0.5,
                liquidation_window_mins: 5,
                liquidation_symbol: "BTCUSDT".to_string(),
                liquidation_exchange: "binance".to_string(),
            };

            let mut signal = create_liquidation_signal(&config);

            // Create aggregate with low volume (below threshold)
            let agg = LiquidationAggregate {
                timestamp: sample_timestamp(),
                window_minutes: 5,
                long_volume_usd: dec!(40000),
                short_volume_usd: dec!(10000),
                net_delta_usd: dec!(30000),
                count_long: 5,
                count_short: 2,
            };

            let ctx =
                SignalContext::new(sample_timestamp(), "BTCUSDT").with_liquidation_aggregates(agg);

            let result = signal.compute(&ctx).await.unwrap();

            // Low volume = neutral
            assert_eq!(result.direction, Direction::Neutral);
        }
    }
}
