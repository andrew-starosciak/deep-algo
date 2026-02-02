//! Phase 1 Pure Arbitrage CLI command.
//!
//! This command runs the Phase 1 arbitrage validation with hardcoded conservative
//! parameters. It uses the dual-leg executor for simultaneous YES+NO execution
//! and tracks session statistics for Go/No-Go validation.
//!
//! ## Phase 1 Parameters (Hardcoded)
//!
//! - Max pair cost: $0.96 (4% minimum edge)
//! - Min edge after fees: 2%
//! - Order type: FOK (Fill-or-Kill)
//! - Max position value: $500
//! - Min liquidity: $1000
//! - Min validation trades: 100
//!
//! ## Example Usage
//!
//! ```bash
//! # Paper trading (default)
//! cargo run -p algo-trade-cli -- phase1-arbitrage
//!
//! # Paper trading for 2 hours
//! cargo run -p algo-trade-cli -- phase1-arbitrage --duration 2h
//!
//! # Live trading (requires wallet setup)
//! cargo run -p algo-trade-cli -- phase1-arbitrage --mode live
//! ```

use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::commands::collect_signals::parse_duration;
use algo_trade_polymarket::arbitrage::{
    ArbitrageDetector, ArbitrageSession, BookFeed, BookFeedConfig, DualLegExecutor, DualLegResult,
    HardLimits, L2OrderBook, LiveExecutor, LiveExecutorConfig, PaperExecutor, PaperExecutorConfig,
    Phase1Config, PolymarketExecutor, Recommendation, TradingMode,
};
use algo_trade_polymarket::GammaClient;
use std::collections::HashMap;

/// Arguments for the phase1-arbitrage command.
#[derive(Args, Debug, Clone)]
pub struct Phase1ArbitrageArgs {
    /// Trading mode: paper or live
    #[arg(long, default_value = "paper")]
    pub mode: String,

    /// Duration to run (e.g., "2h", "1d", "1w")
    ///
    /// If not specified, runs until Ctrl+C or until Go/No-Go decision is reached.
    #[arg(long)]
    pub duration: Option<String>,

    /// Polling interval for order books in milliseconds
    #[arg(long, default_value = "500")]
    pub poll_interval_ms: u64,

    /// Initial paper trading balance (paper mode only)
    /// Default is $100,000 to allow 100+ trades at $500/trade for statistical validation.
    #[arg(long, default_value = "100000")]
    pub paper_balance: Decimal,

    /// Maximum position value per trade in dollars.
    /// With $1000 budget, use --max-position 100 for ~10 concurrent trades.
    /// Default $500 matches Phase 1 config for paper validation.
    #[arg(long, default_value = "500")]
    pub max_position: Decimal,

    /// Stop after reaching minimum validation trades
    #[arg(long)]
    pub stop_at_validation: bool,

    /// Show summary every N trades
    #[arg(long, default_value = "10")]
    pub summary_interval: u32,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,

    // ========== Live Mode Arguments ==========
    /// Maximum order value in USDC (live mode only).
    /// For micro-testing, use $10-50. For production, use higher values.
    #[arg(long, default_value = "50")]
    pub max_order_value: Decimal,

    /// Maximum daily trading volume in USDC (live mode only).
    /// Circuit breaker will halt trading if exceeded.
    #[arg(long, default_value = "500")]
    pub max_daily_volume: Decimal,

    /// Minimum balance reserve to keep in USDC (live mode only).
    /// Ensures funds remain for emergency unwinds.
    #[arg(long, default_value = "50")]
    pub min_balance_reserve: Decimal,

    /// Use micro-testing configuration (tight limits for initial validation).
    /// Overrides max-order-value, max-daily-volume, min-balance-reserve.
    #[arg(long)]
    pub micro_testing: bool,

    /// Skip the confirmation prompt before starting live trading.
    /// Use with caution - this will execute real trades immediately.
    #[arg(long)]
    pub skip_confirmation: bool,

    /// Use real order books from Polymarket WebSocket instead of simulated data.
    /// This is REQUIRED for meaningful testing - simulated books are useless.
    /// Enabled by default. Use --no-real-books to disable (not recommended).
    #[arg(long, default_value = "true")]
    pub real_books: bool,

    /// Timeout in seconds to wait for WebSocket order book snapshots.
    #[arg(long, default_value = "30")]
    pub book_timeout_secs: u64,
}

impl Phase1ArbitrageArgs {
    /// Parses the trading mode.
    fn trading_mode(&self) -> TradingMode {
        match self.mode.to_lowercase().as_str() {
            "live" => TradingMode::Live,
            _ => TradingMode::Paper,
        }
    }
}

/// Runs the Phase 1 arbitrage command.
pub async fn run_phase1_arbitrage(args: Phase1ArbitrageArgs) -> Result<()> {
    let config = Phase1Config::new();
    let mode = args.trading_mode();

    // Log configuration
    log_configuration(&config, &args);

    // Create session
    let mut session = ArbitrageSession::new(mode);

    // Setup shutdown handler
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Received Ctrl+C, shutting down...");
            running_clone.store(false, Ordering::SeqCst);
        }
    });

    // Parse duration if provided
    let end_time = if let Some(ref dur_str) = args.duration {
        let duration = parse_duration(dur_str)?;
        Some(std::time::Instant::now() + duration)
    } else {
        None
    };

    // Create Gamma client for market discovery
    let gamma_client = GammaClient::new();

    // Create detector with Phase 1 configuration
    let detector = ArbitrageDetector::new()
        .with_target_pair_cost(config.max_pair_cost())
        .with_min_profit_threshold(config.min_edge_after_fees())
        .with_max_position_size(config.max_position_value());

    // Create executor based on mode
    match mode {
        TradingMode::Paper => {
            run_paper_mode(
                args,
                &mut session,
                &gamma_client,
                &detector,
                &config,
                running,
                end_time,
            )
            .await?;
        }
        TradingMode::Live => {
            run_live_mode(
                args,
                &mut session,
                &gamma_client,
                &detector,
                &config,
                running,
                end_time,
            )
            .await?;
        }
    }

    // Final summary
    log_final_summary(&session);

    Ok(())
}

/// Runs Phase 1 arbitrage in paper trading mode.
async fn run_paper_mode(
    args: Phase1ArbitrageArgs,
    session: &mut ArbitrageSession,
    gamma_client: &GammaClient,
    detector: &ArbitrageDetector,
    config: &Phase1Config,
    running: Arc<AtomicBool>,
    end_time: Option<std::time::Instant>,
) -> Result<()> {
    let executor = PaperExecutor::new(PaperExecutorConfig {
        initial_balance: args.paper_balance,
        fill_rate: 0.85, // 85% simulated fill rate
        ..Default::default()
    });
    let dual_executor = DualLegExecutor::new(executor);

    let poll_interval = Duration::from_millis(args.poll_interval_ms);

    // Track active book feeds by condition_id
    let mut book_feeds: HashMap<String, BookFeed> = HashMap::new();

    if args.real_books {
        info!("============================================");
        info!("   USING REAL ORDER BOOKS (WebSocket)");
        info!("============================================");
    } else {
        warn!("============================================");
        warn!("   ⚠️  USING SIMULATED ORDER BOOKS ⚠️");
        warn!("   Results are MEANINGLESS for validation!");
        warn!("============================================");
    }

    info!("Starting Phase 1 paper trading...");
    info!("Press Ctrl+C to stop.");

    while running.load(Ordering::SeqCst) {
        // Check time limit
        if let Some(end) = end_time {
            if std::time::Instant::now() >= end {
                info!("Duration limit reached, stopping...");
                break;
            }
        }

        // Check validation limit
        if args.stop_at_validation && session.has_minimum_trades() {
            info!("Minimum validation trades reached, stopping...");
            break;
        }

        // Fetch active markets
        let markets = gamma_client.get_all_current_15min_markets().await;

        if markets.is_empty() {
            info!("No active markets found, waiting...");
            tokio::time::sleep(poll_interval).await;
            continue;
        }

        // Limit to first 10 markets
        let markets: Vec<_> = markets.into_iter().take(10).collect();

        // Connect to WebSocket for new markets (if using real books)
        if args.real_books {
            for market in &markets {
                if !book_feeds.contains_key(&market.condition_id) {
                    // Get token IDs
                    let yes_token = market.up_token().map(|t| t.token_id.clone());
                    let no_token = market.down_token().map(|t| t.token_id.clone());

                    if let (Some(yes_id), Some(no_id)) = (yes_token, no_token) {
                        info!(
                            condition_id = %market.condition_id,
                            "Connecting to WebSocket for real order books..."
                        );

                        match BookFeed::connect(
                            yes_id,
                            no_id,
                            BookFeedConfig {
                                ready_timeout: Duration::from_secs(args.book_timeout_secs),
                                ..Default::default()
                            },
                        )
                        .await
                        {
                            Ok(feed) => {
                                // Wait for initial snapshots
                                let timeout = Duration::from_secs(args.book_timeout_secs);
                                match feed.wait_for_ready(timeout).await {
                                    Ok(()) => {
                                        info!(
                                            condition_id = %market.condition_id,
                                            "Book feed ready with real data!"
                                        );
                                        book_feeds.insert(market.condition_id.clone(), feed);
                                    }
                                    Err(e) => {
                                        warn!(
                                            condition_id = %market.condition_id,
                                            error = %e,
                                            "Book feed timeout, skipping market"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    condition_id = %market.condition_id,
                                    error = %e,
                                    "Failed to connect book feed"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Process each market
        for market in &markets {
            if !running.load(Ordering::SeqCst) {
                break;
            }

            // Get order books (real or simulated)
            let (yes_book, no_book) = if args.real_books {
                match book_feeds.get(&market.condition_id) {
                    Some(feed) => match feed.get_books() {
                        Ok(books) => books,
                        Err(e) => {
                            warn!(
                                condition_id = %market.condition_id,
                                error = %e,
                                "Failed to get books, skipping"
                            );
                            continue;
                        }
                    },
                    None => {
                        // No feed for this market yet
                        continue;
                    }
                }
            } else {
                create_simulated_orderbooks(&market.condition_id)
            };

            // Log real book state periodically
            if args.real_books && session.total_executions() % 10 == 0 {
                if let (Some(yes_ask), Some(no_ask)) = (yes_book.best_ask(), no_book.best_ask()) {
                    info!(
                        condition_id = %market.condition_id,
                        yes_ask = %yes_ask,
                        no_ask = %no_ask,
                        pair_cost = %(yes_ask + no_ask),
                        "Real book state"
                    );
                }
            }

            // Detect opportunity
            let max_pos = args.max_position.min(config.max_position_value());
            let order_size = max_pos / config.max_pair_cost();
            let opportunity = match detector.detect(
                &market.condition_id,
                &yes_book,
                &no_book,
                order_size,
            ) {
                Some(opp) => opp,
                None => continue, // No opportunity
            };

            // Validate with Phase 1 config
            let validation = config.validate_opportunity(&opportunity);
            if !validation.is_valid() {
                info!(
                    market_id = %market.condition_id,
                    pair_cost = %opportunity.pair_cost,
                    "Opportunity rejected: {:?}",
                    validation
                );
                continue;
            }

            info!(
                market_id = %market.condition_id,
                pair_cost = %opportunity.pair_cost,
                net_edge = %config.net_edge(opportunity.pair_cost),
                real_books = args.real_books,
                "Found valid opportunity, executing..."
            );

            // Execute trade
            let shares = order_size.min(config.max_shares_for_pair_cost(opportunity.pair_cost));
            let result = dual_executor.execute(&opportunity, shares).await;

            // Record result
            session.record_execution(&market.condition_id, &result);

            // Log result
            log_execution_result(&result);

            // Periodic summary
            if session.total_executions() % args.summary_interval == 0 {
                log_periodic_summary(session);
            }
        }

        tokio::time::sleep(poll_interval).await;
    }

    // Shutdown book feeds
    for (condition_id, feed) in &book_feeds {
        info!(condition_id = %condition_id, "Shutting down book feed");
        feed.shutdown().await;
    }

    Ok(())
}

/// Runs Phase 1 arbitrage in live trading mode.
///
/// This executes real trades on Polymarket using actual USDC.
/// Requires `POLYMARKET_PRIVATE_KEY` environment variable.
async fn run_live_mode(
    args: Phase1ArbitrageArgs,
    session: &mut ArbitrageSession,
    gamma_client: &GammaClient,
    detector: &ArbitrageDetector,
    config: &Phase1Config,
    running: Arc<AtomicBool>,
    end_time: Option<std::time::Instant>,
) -> Result<()> {
    // Build hard limits from CLI args or micro-testing preset
    let hard_limits = if args.micro_testing {
        info!("Using MICRO-TESTING configuration (tight safety limits)");
        HardLimits::micro_testing()
    } else {
        HardLimits {
            max_order_value: args.max_order_value,
            max_daily_volume: args.max_daily_volume,
            min_balance_reserve: args.min_balance_reserve,
            ..HardLimits::conservative()
        }
    };

    // Display live trading warning
    display_live_warning(&hard_limits);

    // Confirmation prompt (unless skipped)
    if !args.skip_confirmation {
        info!("============================================");
        info!("  Press ENTER to start live trading...");
        info!("  Press Ctrl+C to abort.");
        info!("============================================");

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            error!("Failed to read confirmation input");
            return Ok(());
        }
    }

    // Build executor configuration
    let executor_config = if args.micro_testing {
        LiveExecutorConfig::micro_testing()
    } else {
        LiveExecutorConfig::mainnet().with_hard_limits(hard_limits.clone())
    };

    // Initialize live executor
    info!("Initializing live executor...");
    let mut executor = match LiveExecutor::new(executor_config).await {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to initialize live executor: {}", e);
            error!("Ensure POLYMARKET_PRIVATE_KEY is set correctly.");
            return Err(anyhow::anyhow!("Wallet initialization failed: {}", e));
        }
    };

    info!("Wallet address: {}", executor.address());
    info!("Chain ID: {}", executor.chain_id());

    // Authenticate with CLOB API
    info!("Authenticating with Polymarket CLOB...");
    if let Err(e) = executor.authenticate().await {
        error!("Authentication failed: {}", e);
        return Err(anyhow::anyhow!("CLOB authentication failed: {}", e));
    }
    info!("Authentication successful!");

    // Check initial balance
    let balance = match executor.get_balance().await {
        Ok(b) => b,
        Err(e) => {
            error!("Failed to fetch balance: {}", e);
            return Err(anyhow::anyhow!("Balance check failed: {}", e));
        }
    };
    info!("Available USDC balance: ${}", balance);

    // Validate minimum balance
    if balance < hard_limits.min_balance_reserve + args.max_position {
        error!(
            "Insufficient balance: ${} < ${} (reserve) + ${} (position)",
            balance, hard_limits.min_balance_reserve, args.max_position
        );
        return Err(anyhow::anyhow!("Insufficient balance for trading"));
    }

    // Create dual-leg executor
    let dual_executor = DualLegExecutor::new(executor);
    let poll_interval = Duration::from_millis(args.poll_interval_ms);

    // Track active book feeds by condition_id
    let mut book_feeds: HashMap<String, BookFeed> = HashMap::new();

    // LIVE MODE REQUIRES REAL BOOKS
    if !args.real_books {
        error!("============================================");
        error!("   ❌ LIVE MODE REQUIRES REAL ORDER BOOKS");
        error!("   Cannot trade live with simulated data!");
        error!("   Remove --no-real-books flag.");
        error!("============================================");
        return Err(anyhow::anyhow!("Live mode requires real order books"));
    }

    info!("============================================");
    info!("   LIVE TRADING STARTED (REAL BOOKS)");
    info!("============================================");
    info!("Press Ctrl+C to stop.");

    let mut trades_executed = 0u32;

    while running.load(Ordering::SeqCst) {
        // Check time limit
        if let Some(end) = end_time {
            if std::time::Instant::now() >= end {
                info!("Duration limit reached, stopping...");
                break;
            }
        }

        // Check validation limit
        if args.stop_at_validation && session.has_minimum_trades() {
            info!("Minimum validation trades reached, stopping...");
            break;
        }

        // Check circuit breaker
        if let Err(e) = dual_executor.executor().check_circuit_breaker() {
            error!("Circuit breaker tripped: {}", e);
            warn!("Trading halted for safety. Review session logs.");
            break;
        }

        // Fetch active markets
        let markets = gamma_client.get_all_current_15min_markets().await;

        if markets.is_empty() {
            info!("No active markets found, waiting...");
            tokio::time::sleep(poll_interval).await;
            continue;
        }

        // Limit to first 5 markets in live mode (more conservative)
        let markets: Vec<_> = markets.into_iter().take(5).collect();

        // Connect to WebSocket for new markets
        for market in &markets {
            if !book_feeds.contains_key(&market.condition_id) {
                let yes_token = market.up_token().map(|t| t.token_id.clone());
                let no_token = market.down_token().map(|t| t.token_id.clone());

                if let (Some(yes_id), Some(no_id)) = (yes_token, no_token) {
                    info!(
                        condition_id = %market.condition_id,
                        "LIVE: Connecting to WebSocket for real order books..."
                    );

                    match BookFeed::connect(
                        yes_id,
                        no_id,
                        BookFeedConfig {
                            ready_timeout: Duration::from_secs(args.book_timeout_secs),
                            ..Default::default()
                        },
                    )
                    .await
                    {
                        Ok(feed) => {
                            let timeout = Duration::from_secs(args.book_timeout_secs);
                            match feed.wait_for_ready(timeout).await {
                                Ok(()) => {
                                    info!(
                                        condition_id = %market.condition_id,
                                        "LIVE: Book feed ready!"
                                    );
                                    book_feeds.insert(market.condition_id.clone(), feed);
                                }
                                Err(e) => {
                                    warn!(
                                        condition_id = %market.condition_id,
                                        error = %e,
                                        "LIVE: Book feed timeout, skipping market"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                condition_id = %market.condition_id,
                                error = %e,
                                "LIVE: Failed to connect book feed"
                            );
                        }
                    }
                }
            }
        }

        // Process each market
        for market in &markets {
            if !running.load(Ordering::SeqCst) {
                break;
            }

            // Check circuit breaker before each trade
            if dual_executor.executor().check_circuit_breaker().is_err() {
                break;
            }

            // Get REAL order books from WebSocket
            let (yes_book, no_book) = match book_feeds.get(&market.condition_id) {
                Some(feed) => match feed.get_books() {
                    Ok(books) => books,
                    Err(e) => {
                        warn!(
                            condition_id = %market.condition_id,
                            error = %e,
                            "LIVE: Failed to get books, skipping"
                        );
                        continue;
                    }
                },
                None => {
                    // No feed for this market yet
                    continue;
                }
            };

            // Log real book state
            if let (Some(yes_ask), Some(no_ask)) = (yes_book.best_ask(), no_book.best_ask()) {
                info!(
                    condition_id = %market.condition_id,
                    yes_ask = %yes_ask,
                    no_ask = %no_ask,
                    pair_cost = %(yes_ask + no_ask),
                    "LIVE: Real book state"
                );
            }

            // Detect opportunity
            let max_pos = args.max_position.min(config.max_position_value());
            let order_size = max_pos / config.max_pair_cost();
            let opportunity = match detector.detect(
                &market.condition_id,
                &yes_book,
                &no_book,
                order_size,
            ) {
                Some(opp) => opp,
                None => continue,
            };

            // Validate with Phase 1 config
            let validation = config.validate_opportunity(&opportunity);
            if !validation.is_valid() {
                continue;
            }

            info!(
                market_id = %market.condition_id,
                pair_cost = %opportunity.pair_cost,
                net_edge = %config.net_edge(opportunity.pair_cost),
                "LIVE: Found valid opportunity with REAL prices, executing..."
            );

            // Execute trade
            let shares = order_size.min(config.max_shares_for_pair_cost(opportunity.pair_cost));
            let result = dual_executor.execute(&opportunity, shares).await;

            // Record result
            session.record_execution(&market.condition_id, &result);
            trades_executed += 1;

            // Log result with LIVE prefix
            log_live_execution_result(&result);

            // Record P&L with circuit breaker
            if let Some(pnl) = result.net_profit() {
                dual_executor.executor().record_pnl(pnl);
            }

            // Periodic summary
            if session.total_executions() % args.summary_interval == 0 {
                log_periodic_summary(session);
                info!(
                    "Daily volume: ${} / ${}",
                    dual_executor.executor().daily_volume(),
                    hard_limits.max_daily_volume
                );
            }
        }

        tokio::time::sleep(poll_interval).await;
    }

    // Shutdown book feeds
    for (condition_id, feed) in &book_feeds {
        info!(condition_id = %condition_id, "Shutting down book feed");
        feed.shutdown().await;
    }

    info!("============================================");
    info!("   LIVE TRADING STOPPED");
    info!("   Trades executed: {}", trades_executed);
    info!("============================================");

    Ok(())
}

/// Displays live trading warning and configuration.
fn display_live_warning(hard_limits: &HardLimits) {
    warn!("============================================");
    warn!("   ⚠️  LIVE TRADING MODE - REAL MONEY ⚠️   ");
    warn!("============================================");
    warn!("This will execute REAL trades using ACTUAL USDC.");
    warn!("Ensure you understand the risks before proceeding.");
    warn!("");
    warn!("Safety Limits:");
    warn!("  Max Order Value:     ${}", hard_limits.max_order_value);
    warn!("  Max Daily Volume:    ${}", hard_limits.max_daily_volume);
    warn!("  Min Balance Reserve: ${}", hard_limits.min_balance_reserve);
    warn!("  Max Order Size:      {} shares", hard_limits.max_order_size);
    warn!("============================================");
}

/// Logs live execution result with appropriate emphasis.
fn log_live_execution_result(result: &DualLegResult) {
    match result {
        DualLegResult::Success {
            total_cost,
            net_profit,
            shares,
            ..
        } => {
            info!(
                shares = %shares,
                cost = %total_cost,
                profit = %net_profit,
                "LIVE SUCCESS: Both legs filled"
            );
        }
        DualLegResult::YesOnlyFilled { unwind_result, .. } => {
            let unwound = unwind_result
                .as_ref()
                .map(|u| u.filled_size)
                .unwrap_or(Decimal::ZERO);
            error!(
                unwound = %unwound,
                "LIVE PARTIAL: YES only filled - EXPOSURE CREATED"
            );
        }
        DualLegResult::NoOnlyFilled { unwind_result, .. } => {
            let unwound = unwind_result
                .as_ref()
                .map(|u| u.filled_size)
                .unwrap_or(Decimal::ZERO);
            error!(
                unwound = %unwound,
                "LIVE PARTIAL: NO only filled - EXPOSURE CREATED"
            );
        }
        DualLegResult::BothRejected { yes_result, no_result } => {
            let yes_reason = yes_result.error.as_deref().unwrap_or("unknown");
            let no_reason = no_result.error.as_deref().unwrap_or("unknown");
            warn!(
                yes = %yes_reason,
                no = %no_reason,
                "LIVE REJECTED: Both legs rejected, no exposure"
            );
        }
        DualLegResult::Error { error } => {
            error!(error = %error, "LIVE ERROR: Execution failed");
        }
    }
}

/// Creates simulated order books for paper trading.
///
/// Generates deterministic but varied order books based on market_id hash.
fn create_simulated_orderbooks(market_id: &str) -> (L2OrderBook, L2OrderBook) {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    // Use market_id hash for deterministic variation
    let mut hasher = DefaultHasher::new();
    market_id.hash(&mut hasher);
    let hash = hasher.finish();

    // Derive prices from hash
    let hash_frac = (hash % 1000) as f64 / 1000.0; // 0.0 to 1.0
    let base_price = 0.45 + hash_frac * 0.10; // 0.45 to 0.55
    let yes_price = Decimal::from_f64_retain(base_price).unwrap_or(dec!(0.50));

    // NO price such that pair cost is between 0.92 and 0.98
    let pair_cost_target = 0.92 + (hash_frac * 0.06);
    let no_price_f64 = pair_cost_target - base_price;
    let no_price = Decimal::from_f64_retain(no_price_f64.max(0.01)).unwrap_or(dec!(0.48));

    // Depth based on hash
    let depth_base = 500 + ((hash >> 16) % 1500) as u32; // 500 to 2000
    let yes_depth = Decimal::from(depth_base);
    let no_depth = Decimal::from(depth_base + 200);

    let mut yes_book = L2OrderBook::new(format!("{}-yes", market_id));
    yes_book.apply_snapshot(vec![], vec![(yes_price, yes_depth)]);

    let mut no_book = L2OrderBook::new(format!("{}-no", market_id));
    no_book.apply_snapshot(vec![], vec![(no_price, no_depth)]);

    (yes_book, no_book)
}

/// Logs the Phase 1 configuration.
fn log_configuration(config: &Phase1Config, args: &Phase1ArbitrageArgs) {
    info!("============================================");
    info!("   PHASE 1 PURE ARBITRAGE - CONFIGURATION   ");
    info!("============================================");
    info!("Mode: {}", args.mode.to_uppercase());
    info!("--------------------------------------------");
    info!("Phase 1 Parameters (Hardcoded):");
    info!("  Max Pair Cost:       {}", config.max_pair_cost());
    info!("  Min Edge After Fees: {}", config.min_edge_after_fees());
    info!("  Order Type:          {:?}", config.order_type());
    info!("  Max Position Value:  ${}", config.max_position_value());
    info!("  Min Liquidity:       ${}", config.min_liquidity());
    info!("  Min Validation Trades: {}", config.min_validation_trades());
    info!("--------------------------------------------");
    info!("Session Parameters:");
    info!("  Poll Interval:       {}ms", args.poll_interval_ms);
    info!("  Paper Balance:       ${}", args.paper_balance);
    info!("  Max Position/Trade:  ${}", args.max_position);
    info!("  Stop at Validation:  {}", args.stop_at_validation);
    info!("  Summary Interval:    {} trades", args.summary_interval);
    if let Some(ref dur) = args.duration {
        info!("  Duration:            {}", dur);
    } else {
        info!("  Duration:            indefinite");
    }
    info!("============================================");
}

/// Logs the result of an execution.
fn log_execution_result(result: &DualLegResult) {
    match result {
        DualLegResult::Success {
            total_cost,
            net_profit,
            shares,
            ..
        } => {
            info!(
                shares = %shares,
                cost = %total_cost,
                profit = %net_profit,
                "SUCCESS: Both legs filled"
            );
        }
        DualLegResult::YesOnlyFilled { unwind_result, .. } => {
            let unwound = unwind_result
                .as_ref()
                .map(|u| u.filled_size)
                .unwrap_or(Decimal::ZERO);
            warn!(
                unwound = %unwound,
                "PARTIAL: YES only filled, attempted unwind"
            );
        }
        DualLegResult::NoOnlyFilled { unwind_result, .. } => {
            let unwound = unwind_result
                .as_ref()
                .map(|u| u.filled_size)
                .unwrap_or(Decimal::ZERO);
            warn!(
                unwound = %unwound,
                "PARTIAL: NO only filled, attempted unwind"
            );
        }
        DualLegResult::BothRejected { .. } => {
            info!("REJECTED: Both legs rejected, no exposure");
        }
        DualLegResult::Error { error } => {
            error!(error = %error, "ERROR: Execution failed");
        }
    }
}

/// Logs periodic session summary.
fn log_periodic_summary(session: &ArbitrageSession) {
    let summary = session.summary();
    let (ci_lower, ci_upper) = session.fill_rate_wilson_ci();

    info!("--------------------------------------------");
    info!("Session Summary (Trade #{})", summary.total_executions);
    info!("--------------------------------------------");
    info!(
        "  Executions:     {} ({} success, {} partial)",
        summary.total_executions, summary.successful_executions, summary.partial_fills
    );
    info!(
        "  Fill Rate:      {:.1}% (CI: {:.1}% - {:.1}%)",
        summary.fill_rate * 100.0,
        ci_lower * 100.0,
        ci_upper * 100.0
    );
    info!(
        "  Total P&L:      {} (ROI: {}%)",
        summary.total_profit, summary.roi
    );
    info!(
        "  Max Imbalance:  {} (Current: {})",
        summary.max_imbalance, summary.current_imbalance
    );
    info!("  Recommendation: {}", summary.recommendation);
    info!("--------------------------------------------");
}

/// Logs final session summary.
fn log_final_summary(session: &ArbitrageSession) {
    let summary = session.summary();
    let (ci_lower, ci_upper) = session.fill_rate_wilson_ci();

    info!("============================================");
    info!("         FINAL SESSION SUMMARY              ");
    info!("============================================");
    info!("Session ID:       {}", summary.session_id);
    info!("Mode:             {}", summary.mode);
    info!("Duration:         {} seconds", summary.duration_secs);
    info!("--------------------------------------------");
    info!("EXECUTIONS:");
    info!("  Total:          {}", summary.total_executions);
    info!("  Successful:     {}", summary.successful_executions);
    info!("  Partial Fills:  {}", summary.partial_fills);
    info!(
        "  Fill Rate:      {:.1}%",
        summary.fill_rate * 100.0
    );
    info!(
        "  Fill Rate CI:   {:.1}% - {:.1}%",
        ci_lower * 100.0, ci_upper * 100.0
    );
    info!("--------------------------------------------");
    info!("FINANCIALS:");
    info!("  Total Cost:     ${}", summary.total_cost);
    info!("  Total Profit:   ${}", summary.total_profit);
    info!("  ROI:            {}%", summary.roi);
    info!("--------------------------------------------");
    info!("RISK:");
    info!("  Max Imbalance:  {} shares", summary.max_imbalance);
    info!("  Current Imbalance: {} shares", summary.current_imbalance);
    info!("--------------------------------------------");
    info!("GO/NO-GO RECOMMENDATION:");
    info!("  {}", summary.recommendation);
    info!("============================================");

    // Phase 2 guidance
    match &summary.recommendation {
        Recommendation::ProceedToPhase2 { .. } => {
            info!("");
            info!("NEXT STEPS:");
            info!("1. Review session logs for any anomalies");
            info!("2. Verify execution quality metrics");
            info!("3. Proceed to Phase 2 with increased position sizes");
        }
        Recommendation::ContinuePaper { trades_needed, .. } => {
            info!("");
            info!("NEXT STEPS:");
            info!("1. Continue paper trading for {} more trades", trades_needed);
            info!("2. Monitor fill rate and P&L trends");
            info!("3. Run: phase1-arbitrage --stop-at-validation");
        }
        Recommendation::StopTrading { reason } => {
            info!("");
            info!("INVESTIGATION REQUIRED:");
            info!("1. Review failed trades for patterns");
            info!("2. Check market conditions during failures");
            info!("3. Reason: {}", reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create test args with defaults.
    fn test_args(mode: &str) -> Phase1ArbitrageArgs {
        Phase1ArbitrageArgs {
            mode: mode.to_string(),
            duration: None,
            poll_interval_ms: 500,
            paper_balance: dec!(100000),
            max_position: dec!(500),
            stop_at_validation: false,
            summary_interval: 10,
            db_url: None,
            max_order_value: dec!(50),
            max_daily_volume: dec!(500),
            min_balance_reserve: dec!(50),
            micro_testing: false,
            skip_confirmation: true,
            real_books: true,  // Default to real books
            book_timeout_secs: 30,
        }
    }

    #[test]
    fn test_real_books_default_true() {
        let args = test_args("paper");
        assert!(args.real_books, "real_books should default to true");
    }

    #[test]
    fn test_args_default_mode() {
        let args = test_args("paper");
        assert_eq!(args.trading_mode(), TradingMode::Paper);
    }

    #[test]
    fn test_args_live_mode() {
        let args = test_args("live");
        assert_eq!(args.trading_mode(), TradingMode::Live);
    }

    #[test]
    fn test_args_case_insensitive_mode() {
        let args = test_args("LIVE");
        assert_eq!(args.trading_mode(), TradingMode::Live);
    }

    #[test]
    fn test_simulated_orderbooks() {
        let (yes_book, no_book) = create_simulated_orderbooks("test-market");

        assert!(!yes_book.token_id.is_empty());
        assert!(!no_book.token_id.is_empty());
        assert!(yes_book.best_ask().is_some());
        assert!(no_book.best_ask().is_some());

        // Pair cost should be reasonable
        let pair_cost = yes_book.best_ask().unwrap() + no_book.best_ask().unwrap();
        assert!(pair_cost > dec!(0.90));
        assert!(pair_cost < dec!(1.00));
    }

    #[test]
    fn test_config_values_match_phase1() {
        let config = Phase1Config::new();

        assert_eq!(config.max_pair_cost(), dec!(0.96));
        assert_eq!(config.min_edge_after_fees(), dec!(0.02));
        assert_eq!(config.max_position_value(), dec!(500));
        assert_eq!(config.min_liquidity(), dec!(400));
        assert_eq!(config.min_validation_trades(), 100);
    }
}
