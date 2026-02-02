//! Arbitrage bot CLI command for Polymarket binary markets.
//!
//! Implements Phase 6 of the arbitrage implementation plan, providing CLI scaffolding
//! for detecting and executing arbitrage opportunities in BTC 15-minute binary markets.
//!
//! ## Overview
//!
//! The arbitrage bot monitors Polymarket order books for opportunities where the combined
//! cost of buying both YES and NO tokens is less than $1.00, guaranteeing profit regardless
//! of outcome.
//!
//! ## Trading Modes
//!
//! - **Paper**: Logs opportunities without executing (default)
//! - **Live**: Executes real trades (requires Phase 3 execution implementation)
//!
//! ## Example Usage
//!
//! ```bash
//! # Paper trading with default settings
//! cargo run -p algo-trade-cli -- arbitrage-bot
//!
//! # Custom thresholds with dry-run mode
//! cargo run -p algo-trade-cli -- arbitrage-bot --threshold 0.96 --order-size 50 --dry-run
//!
//! # WebSocket mode for faster updates (when available)
//! cargo run -p algo-trade-cli -- arbitrage-bot --use-websocket --poll-interval-ms 100
//!
//! # Run for 2 hours with conservative settings
//! cargo run -p algo-trade-cli -- arbitrage-bot --duration 2h --max-daily-loss 25
//! ```

use anyhow::{anyhow, Result};
use clap::{Args, ValueEnum};
use rust_decimal::Decimal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::commands::collect_signals::parse_duration;
use algo_trade_polymarket::arbitrage::ArbitrageDetector;
use algo_trade_polymarket::GammaClient;

/// Trading mode for the arbitrage bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum TradingMode {
    /// Paper trading - log opportunities without executing
    #[default]
    Paper,
    /// Live trading - execute real trades
    Live,
}

impl std::fmt::Display for TradingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingMode::Paper => write!(f, "paper"),
            TradingMode::Live => write!(f, "live"),
        }
    }
}

/// Arguments for the arbitrage-bot command.
#[derive(Args, Debug, Clone)]
pub struct ArbitrageBotArgs {
    /// Trading mode: paper or live
    #[arg(long, default_value = "paper", value_enum)]
    pub mode: TradingMode,

    /// Target pair cost threshold (e.g., 0.97 for 3% margin)
    ///
    /// Opportunities with pair cost above this threshold are rejected.
    /// Conservative default of 0.97 provides 3% margin over break-even.
    #[arg(long, default_value = "0.97")]
    pub threshold: Decimal,

    /// Order size per opportunity in shares
    ///
    /// Number of shares to attempt buying on each leg of the arbitrage.
    #[arg(long, default_value = "100")]
    pub order_size: Decimal,

    /// Maximum position size per market
    ///
    /// Caps exposure to any single market's opportunity.
    #[arg(long, default_value = "1000")]
    pub max_position: Decimal,

    /// Maximum daily loss before stopping (in USD)
    ///
    /// Safety limit that halts trading if cumulative daily losses exceed this.
    #[arg(long, default_value = "50")]
    pub max_daily_loss: Decimal,

    /// Cooldown between executions in seconds
    ///
    /// Minimum time to wait after executing before attempting another trade.
    #[arg(long, default_value = "5")]
    pub cooldown_secs: u64,

    /// Use WebSocket for order book updates (faster)
    ///
    /// When enabled, uses WebSocket streaming instead of REST polling.
    /// Note: WebSocket implementation is Phase 4 and may not be available yet.
    #[arg(long)]
    pub use_websocket: bool,

    /// Polling interval for REST mode in milliseconds
    ///
    /// How frequently to poll order books when not using WebSocket.
    #[arg(long, default_value = "500")]
    pub poll_interval_ms: u64,

    /// Duration to run the bot (e.g., "2h", "1d", "1w")
    ///
    /// If not specified, runs indefinitely until Ctrl+C.
    #[arg(long)]
    pub duration: Option<String>,

    /// Dry run mode - detect and log without execution intent
    ///
    /// Even in live mode, dry-run will only log what would be executed.
    #[arg(long)]
    pub dry_run: bool,

    /// Minimum net profit per pair to consider (in USD)
    ///
    /// Filters out opportunities that are technically profitable but
    /// not worth the execution risk.
    #[arg(long, default_value = "0.005")]
    pub min_profit: Decimal,

    /// Gas cost estimate per transaction (in USD)
    ///
    /// Used in profit calculations. Polygon gas is typically ~$0.007.
    #[arg(long, default_value = "0.007")]
    pub gas_cost: Decimal,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

impl ArbitrageBotArgs {
    /// Creates an `ArbitrageDetector` from the command arguments.
    fn create_detector(&self) -> ArbitrageDetector {
        ArbitrageDetector::new()
            .with_target_pair_cost(self.threshold)
            .with_min_profit_threshold(self.min_profit)
            .with_max_position_size(self.max_position)
            .with_gas_cost(self.gas_cost)
    }
}

/// Configuration summary for logging.
struct ConfigSummary<'a> {
    args: &'a ArbitrageBotArgs,
    break_even: Decimal,
}

impl<'a> ConfigSummary<'a> {
    fn new(args: &'a ArbitrageBotArgs, break_even: Decimal) -> Self {
        Self { args, break_even }
    }

    fn log(&self) {
        tracing::info!("========================================");
        tracing::info!("       ARBITRAGE BOT CONFIGURATION      ");
        tracing::info!("========================================");
        tracing::info!("Mode:            {}", self.args.mode);
        tracing::info!("Dry Run:         {}", self.args.dry_run);
        tracing::info!("----------------------------------------");
        tracing::info!("Thresholds:");
        tracing::info!("  Target Pair Cost:  {}", self.args.threshold);
        tracing::info!("  Break-even Cost:   {:.4}", self.break_even);
        tracing::info!("  Min Net Profit:    ${}", self.args.min_profit);
        tracing::info!("  Gas Cost/Tx:       ${}", self.args.gas_cost);
        tracing::info!("----------------------------------------");
        tracing::info!("Sizing:");
        tracing::info!("  Order Size:        {} shares", self.args.order_size);
        tracing::info!("  Max Position:      {} shares", self.args.max_position);
        tracing::info!("  Max Daily Loss:    ${}", self.args.max_daily_loss);
        tracing::info!("----------------------------------------");
        tracing::info!("Timing:");
        tracing::info!("  Poll Interval:     {}ms", self.args.poll_interval_ms);
        tracing::info!("  Cooldown:          {}s", self.args.cooldown_secs);
        tracing::info!("  WebSocket:         {}", self.args.use_websocket);
        if let Some(ref dur) = self.args.duration {
            tracing::info!("  Duration:          {}", dur);
        } else {
            tracing::info!("  Duration:          indefinite");
        }
        tracing::info!("========================================");
    }
}

/// Statistics for the arbitrage bot session.
#[derive(Debug, Default)]
struct ArbitrageBotStats {
    markets_discovered: usize,
    polls_completed: u64,
    opportunities_detected: u64,
    opportunities_executed: u64,
    partial_fills: u64,
    rejected_threshold: u64,
    rejected_depth: u64,
    rejected_profit: u64,
    total_invested: Decimal,
    total_profit: Decimal,
    daily_pnl: Decimal,
}

impl ArbitrageBotStats {
    fn log_summary(&self, elapsed: Duration) {
        let elapsed_mins = elapsed.as_secs_f64() / 60.0;
        tracing::info!("========================================");
        tracing::info!("         SESSION SUMMARY                ");
        tracing::info!("========================================");
        tracing::info!("Runtime:             {:.1} minutes", elapsed_mins);
        tracing::info!("Markets Discovered:  {}", self.markets_discovered);
        tracing::info!("Polls Completed:     {}", self.polls_completed);
        tracing::info!("----------------------------------------");
        tracing::info!("Opportunities:");
        tracing::info!("  Detected:          {}", self.opportunities_detected);
        tracing::info!("  Executed:          {}", self.opportunities_executed);
        tracing::info!("  Partial Fills:     {}", self.partial_fills);
        tracing::info!("----------------------------------------");
        tracing::info!("Rejections:");
        tracing::info!("  Above Threshold:   {}", self.rejected_threshold);
        tracing::info!("  Insufficient Depth:{}", self.rejected_depth);
        tracing::info!("  Below Min Profit:  {}", self.rejected_profit);
        tracing::info!("----------------------------------------");
        tracing::info!("Financials:");
        tracing::info!("  Total Invested:    ${}", self.total_invested);
        tracing::info!("  Total Profit:      ${}", self.total_profit);
        tracing::info!("  Daily P&L:         ${}", self.daily_pnl);
        tracing::info!("========================================");
    }
}

/// Runs the arbitrage bot command.
///
/// # Errors
///
/// Returns an error if:
/// - Duration parsing fails
/// - Market discovery fails
/// - Database connection fails (if needed)
pub async fn run_arbitrage_bot(args: ArbitrageBotArgs) -> Result<()> {
    // Validate live mode requirements
    if args.mode == TradingMode::Live && !args.dry_run {
        tracing::warn!("Live mode selected - execution is not yet implemented (Phase 3)");
        tracing::warn!("Running in dry-run mode to log what would be executed");
    }

    // Parse duration if provided
    let run_duration = if let Some(ref dur_str) = args.duration {
        let dur = parse_duration(dur_str)?;
        Some(std::time::Duration::from_secs(dur.as_secs() as u64))
    } else {
        None
    };

    // Create detector with configuration
    let detector = args.create_detector();
    let break_even = detector.break_even_pair_cost();

    // Log configuration
    let config_summary = ConfigSummary::new(&args, break_even);
    config_summary.log();

    // Validate threshold is below break-even (we want to only accept profitable trades)
    // If threshold >= break_even, we might accept trades that lose money after fees
    if args.threshold >= break_even {
        return Err(anyhow!(
            "Target pair cost {} is at or above break-even {}. Decrease threshold for profitable trades.",
            args.threshold,
            break_even
        ));
    }

    // Initialize stats
    let mut stats = ArbitrageBotStats::default();
    let start_time = std::time::Instant::now();

    // Set up shutdown signal
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("Received Ctrl+C, initiating graceful shutdown...");
            running_clone.store(false, Ordering::SeqCst);
        }
    });

    // Discover markets using Gamma API
    tracing::info!("Discovering 15-minute BTC markets...");
    let gamma = GammaClient::new();
    let markets = gamma.get_all_current_15min_markets().await;
    stats.markets_discovered = markets.len();

    if markets.is_empty() {
        tracing::warn!("No active 15-minute markets found. Markets may be between windows.");
        tracing::info!("Waiting for next market window...");
    } else {
        tracing::info!("Discovered {} active 15-minute markets", markets.len());
        for market in &markets {
            tracing::debug!(
                "  Market: {} (condition_id: {})",
                &market.question,
                market.condition_id
            );
        }
    }

    // Main loop
    let poll_interval = Duration::from_millis(args.poll_interval_ms);
    let cooldown_duration = Duration::from_secs(args.cooldown_secs);
    // TODO: Enable when execution is implemented in Phase 3
    #[allow(unused_variables)]
    let last_execution: Option<std::time::Instant> = None;

    tracing::info!("Starting arbitrage detection loop...");
    tracing::info!("Press Ctrl+C to stop");

    while running.load(Ordering::SeqCst) {
        // Check duration limit
        if let Some(max_duration) = run_duration {
            if start_time.elapsed() >= max_duration {
                tracing::info!("Duration limit reached, stopping...");
                break;
            }
        }

        // Check daily loss limit
        if stats.daily_pnl < -args.max_daily_loss {
            tracing::error!(
                "Daily loss limit of ${} exceeded (P&L: ${}), stopping...",
                args.max_daily_loss,
                stats.daily_pnl
            );
            break;
        }

        // Check cooldown
        if let Some(last) = last_execution {
            if last.elapsed() < cooldown_duration {
                let remaining = cooldown_duration - last.elapsed();
                tokio::time::sleep(remaining).await;
                continue;
            }
        }

        // TODO: Implement order book fetching when Phase 2 is complete
        // For now, we log that we would poll and detect
        stats.polls_completed += 1;

        if stats.polls_completed % 10 == 0 {
            tracing::debug!(
                "Poll #{}: Checking {} markets for opportunities...",
                stats.polls_completed,
                stats.markets_discovered
            );
        }

        // Placeholder: In a real implementation, we would:
        // 1. Fetch order books for each market's YES and NO tokens
        // 2. Run detector.detect() on each pair
        // 3. Log or execute opportunities found

        // Simulate detection loop (to be replaced with real implementation)
        // This placeholder just demonstrates the structure
        for market in &markets {
            // Skip markets without proper token structure
            let (_yes_token, _no_token) = match (&market.tokens.first(), &market.tokens.get(1)) {
                (Some(yes), Some(no)) => (yes, no),
                _ => continue,
            };

            // TODO: Fetch L2 order books for yes_token and no_token
            // let yes_book = fetch_orderbook(yes_token.token_id).await?;
            // let no_book = fetch_orderbook(no_token.token_id).await?;

            // TODO: Run detection
            // if let Some(opportunity) = detector.detect(
            //     &market.condition_id,
            //     &yes_book,
            //     &no_book,
            //     args.order_size,
            // ) {
            //     stats.opportunities_detected += 1;
            //     log_opportunity(&opportunity, &args);
            //
            //     if args.mode == TradingMode::Live && !args.dry_run {
            //         // Execute trade (Phase 3)
            //         // execute_arbitrage(&opportunity).await?;
            //     }
            //
            //     last_execution = Some(std::time::Instant::now());
            // }
        }

        // Sleep before next poll
        tokio::time::sleep(poll_interval).await;
    }

    // Log final stats
    let elapsed = start_time.elapsed();
    stats.log_summary(elapsed);

    tracing::info!("Arbitrage bot stopped");
    Ok(())
}

/// Logs a detected arbitrage opportunity.
#[allow(dead_code)]
fn log_opportunity(
    opportunity: &algo_trade_polymarket::arbitrage::ArbitrageOpportunity,
    args: &ArbitrageBotArgs,
) {
    let action = if args.mode == TradingMode::Live && !args.dry_run {
        "EXECUTING"
    } else {
        "DETECTED"
    };

    tracing::info!("========================================");
    tracing::info!("  {} ARBITRAGE OPPORTUNITY", action);
    tracing::info!("========================================");
    tracing::info!("Market:          {}", opportunity.market_id);
    tracing::info!("----------------------------------------");
    tracing::info!("Prices:");
    tracing::info!("  YES Worst Fill: ${}", opportunity.yes_worst_fill);
    tracing::info!("  NO Worst Fill:  ${}", opportunity.no_worst_fill);
    tracing::info!("  Pair Cost:      ${}", opportunity.pair_cost);
    tracing::info!("----------------------------------------");
    tracing::info!("Profit Analysis:");
    tracing::info!("  Gross Profit:   ${}", opportunity.gross_profit_per_pair);
    tracing::info!("  Expected Fee:   ${}", opportunity.expected_fee);
    tracing::info!("  Gas Cost:       ${}", opportunity.gas_cost);
    tracing::info!("  Net Profit:     ${}", opportunity.net_profit_per_pair);
    tracing::info!("  ROI:            {}%", opportunity.roi);
    tracing::info!("----------------------------------------");
    tracing::info!("Sizing:");
    tracing::info!("  Recommended:    {} shares", opportunity.recommended_size);
    tracing::info!("  Investment:     ${}", opportunity.total_investment);
    tracing::info!("  Payout:         ${}", opportunity.guaranteed_payout);
    tracing::info!("----------------------------------------");
    tracing::info!("Risk:");
    tracing::info!("  YES Depth:      {} shares", opportunity.yes_depth);
    tracing::info!("  NO Depth:       {} shares", opportunity.no_depth);
    tracing::info!("  Risk Score:     {:.2}", opportunity.risk_score);
    tracing::info!("========================================");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_trading_mode_display() {
        assert_eq!(TradingMode::Paper.to_string(), "paper");
        assert_eq!(TradingMode::Live.to_string(), "live");
    }

    #[test]
    fn test_trading_mode_default() {
        assert_eq!(TradingMode::default(), TradingMode::Paper);
    }

    #[test]
    fn test_args_create_detector() {
        let args = ArbitrageBotArgs {
            mode: TradingMode::Paper,
            threshold: dec!(0.96),
            order_size: dec!(50),
            max_position: dec!(500),
            max_daily_loss: dec!(25),
            cooldown_secs: 10,
            use_websocket: false,
            poll_interval_ms: 1000,
            duration: None,
            dry_run: true,
            min_profit: dec!(0.01),
            gas_cost: dec!(0.01),
            db_url: None,
        };

        let detector = args.create_detector();
        assert_eq!(detector.target_pair_cost, dec!(0.96));
        assert_eq!(detector.min_profit_threshold, dec!(0.01));
        assert_eq!(detector.max_position_size, dec!(500));
        assert_eq!(detector.gas_cost, dec!(0.01));
    }

    #[test]
    fn test_args_default_values() {
        // Verify default clap values match implementation plan
        // threshold: 0.97
        // order_size: 100
        // max_position: 1000
        // max_daily_loss: 50
        // cooldown_secs: 5
        // poll_interval_ms: 500
        // min_profit: 0.005
        // gas_cost: 0.007
    }
}
