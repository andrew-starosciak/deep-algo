//! polymarket-paper-trade CLI command for paper trading on Polymarket.
//!
//! Simulates trading on Polymarket BTC markets using signal-based decision making
//! with Kelly criterion position sizing.

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
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
use algo_trade_backtest::{FeeModel, FeeTier, PolymarketFees};
use algo_trade_data::{
    KellyCriterion, PaperTradeDirection, PaperTradeRecord, PaperTradeRepository,
    PolymarketOddsRecord, PolymarketOddsRepository,
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
}

/// Configuration for the paper trading decision engine.
#[derive(Debug, Clone)]
pub struct DecisionEngineConfig {
    /// Signal type to use.
    pub signal_type: String,
    /// Minimum signal strength to place bet.
    pub min_signal_strength: f64,
    /// Minimum edge required.
    pub min_edge: Decimal,
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

        Self {
            signal_type: args.signal.clone(),
            min_signal_strength: args.min_signal_strength,
            min_edge: Decimal::try_from(args.min_edge).unwrap_or(dec!(0.02)),
            kelly,
            fee_tier,
            use_fixed_stake: args.use_fixed_stake,
            fixed_stake: Decimal::try_from(args.stake).unwrap_or(dec!(100)),
            cooldown: Duration::from_secs(args.cooldown_secs),
        }
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

/// Paper trading executor that manages trades and tracks positions.
pub struct PaperTradeExecutor {
    engine: DecisionEngine,
    fee_model: PolymarketFees,
    session_id: String,
    trades_count: u32,
    wins: u32,
    losses: u32,
}

impl PaperTradeExecutor {
    /// Creates a new paper trade executor.
    #[must_use]
    pub fn new(config: DecisionEngineConfig, initial_bankroll: Decimal) -> Self {
        let fee_model = PolymarketFees::new(config.fee_tier);
        let engine = DecisionEngine::new(config, initial_bankroll);
        let session_id = Uuid::new_v4().to_string();

        Self {
            engine,
            fee_model,
            session_id,
            trades_count: 0,
            wins: 0,
            losses: 0,
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
        let trade = PaperTradeRecord::new(
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

    /// Formats a summary of the trading session.
    #[must_use]
    pub fn format_summary(&self) -> String {
        let win_rate = if self.trades_count > 0 {
            self.wins as f64 / self.trades_count as f64 * 100.0
        } else {
            0.0
        };

        format!(
            "Paper Trading Session Summary:\n\
             - Session ID: {}\n\
             - Total trades: {}\n\
             - Wins: {} | Losses: {}\n\
             - Win rate: {:.1}%\n\
             - Final bankroll: ${:.2}",
            self.session_id,
            self.trades_count,
            self.wins,
            self.losses,
            win_rate,
            self.bankroll()
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
    let paper_repo = PaperTradeRepository::new(pool);

    // Create executor
    let config = DecisionEngineConfig::from_args(&args);
    let initial_bankroll = Decimal::try_from(args.bankroll).unwrap_or(dec!(10000));
    let executor = Arc::new(Mutex::new(PaperTradeExecutor::new(
        config,
        initial_bankroll,
    )));

    tracing::info!(
        "Paper trading config: signal={}, min_strength={:.2}, kelly_fraction={:.2}, bankroll=${}",
        args.signal,
        args.min_signal_strength,
        args.kelly_fraction,
        initial_bankroll
    );

    // Shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));

    // Main trading loop
    let poll_interval = Duration::from_secs(args.poll_interval_secs);
    let shutdown_clone = shutdown.clone();
    let executor_clone = executor.clone();

    let trading_handle = tokio::spawn(async move {
        run_trading_loop(
            executor_clone,
            odds_repo,
            paper_repo,
            poll_interval,
            shutdown_clone,
            &args.signal,
        )
        .await
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

/// Main trading loop that polls for opportunities and executes trades.
async fn run_trading_loop(
    executor: Arc<Mutex<PaperTradeExecutor>>,
    odds_repo: PolymarketOddsRepository,
    paper_repo: PaperTradeRepository,
    poll_interval: Duration,
    shutdown: Arc<AtomicBool>,
    signal_type: &str,
) {
    let mut interval = tokio::time::interval(poll_interval);

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("Trading loop received shutdown signal");
            break;
        }

        let now = Utc::now();

        // Get latest market data for all markets
        let markets = match odds_repo.get_latest_all().await {
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

        // Evaluate each market
        for market in markets {
            // Simulate signal generation (in production, use actual signals)
            let (signal_strength, signal_direction) = simulate_signal(signal_type, &market);

            let mut exec = executor.lock().await;
            if let Some(trade) = exec.execute(&market, signal_strength, signal_direction, now) {
                // Store the trade
                match paper_repo.insert(&trade).await {
                    Ok(id) => {
                        tracing::info!(
                            trade_id = id,
                            market_id = %trade.market_id,
                            "Paper trade stored"
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to store paper trade: {}", e);
                    }
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
            outcome_yes_price: dec!(0.60),
            outcome_no_price: dec!(0.40),
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
            kelly: KellyCriterion::quarter_kelly(),
            fee_tier: FeeTier::Tier0,
            use_fixed_stake: false,
            fixed_stake: dec!(100),
            cooldown: Duration::from_secs(900),
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

    #[test]
    fn test_decision_engine_config_from_args() {
        let args = PolymarketPaperTradeArgs {
            duration: "24h".to_string(),
            signal: "composite".to_string(),
            min_signal_strength: 0.7,
            stake: 200.0,
            kelly_fraction: 0.5,
            min_edge: 0.03,
            bankroll: 20000.0,
            max_bet_fraction: 0.1,
            fee_tier: "2".to_string(),
            poll_interval_secs: 120,
            db_url: None,
            cooldown_secs: 1800,
            use_fixed_stake: true,
        };

        let config = DecisionEngineConfig::from_args(&args);

        assert_eq!(config.signal_type, "composite");
        assert!((config.min_signal_strength - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.min_edge, dec!(0.03));
        assert_eq!(config.fee_tier, FeeTier::Tier2);
        assert!(config.use_fixed_stake);
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
        let args = PolymarketPaperTradeArgs {
            duration: "7d".to_string(),
            signal: "imbalance".to_string(),
            min_signal_strength: 0.7,
            stake: 200.0,
            kelly_fraction: 0.5,
            min_edge: 0.05,
            bankroll: 50000.0,
            max_bet_fraction: 0.1,
            fee_tier: "1".to_string(),
            poll_interval_secs: 120,
            db_url: Some("postgres://localhost/test".to_string()),
            cooldown_secs: 1800,
            use_fixed_stake: false,
        };

        assert_eq!(args.duration, "7d");
        assert_eq!(args.signal, "imbalance");
        assert!((args.min_signal_strength - 0.7).abs() < f64::EPSILON);
        assert!(args.db_url.is_some());
    }
}
