//! CLI command for cross-market correlation backtest.
//!
//! Analyzes historical cross-market opportunities to calculate:
//! - Win rates and P&L at trade and window levels
//! - Kelly criterion optimal bet sizing
//! - Bankroll requirements and risk of ruin
//! - Equity curve and drawdown analysis

use anyhow::Result;
use clap::Args;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;
use tracing::info;

/// Arguments for the cross-market backtest command.
#[derive(Args, Debug)]
pub struct CrossMarketBacktestArgs {
    /// Filter to specific combination (default: Coin1UpCoin2Down).
    #[arg(long, default_value = "Coin1UpCoin2Down")]
    pub combination: String,

    /// Session ID to analyze (default: all sessions).
    #[arg(long)]
    pub session_id: Option<String>,

    /// Bet size per pair in dollars for bankroll calculations.
    #[arg(long, default_value = "100")]
    pub bet_size: u32,

    /// Show detailed equity curve.
    #[arg(long)]
    pub show_equity: bool,

    /// Show per-pair breakdown.
    #[arg(long)]
    pub show_pairs: bool,
}

/// Trade-level statistics.
#[derive(Debug, Default)]
struct TradeStats {
    total: u32,
    wins: u32,
    losses: u32,
    double_wins: u32,
    total_pnl: Decimal,
    avg_cost: Decimal,
    avg_spread: Decimal,
    max_consecutive_losses: u32,
    max_drawdown: Decimal,
}

/// Window-level statistics.
#[derive(Debug, Default)]
struct WindowStats {
    total: u32,
    wins: u32,
    losses: u32,
    avg_pairs_per_window: f64,
    max_risk_per_window: Decimal,
    worst_window_pnl: Decimal,
    best_window_pnl: Decimal,
    max_drawdown: Decimal,
}

/// Runs the cross-market backtest.
pub async fn run(args: CrossMarketBacktestArgs) -> Result<()> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL env var required"))?;

    info!("Connecting to database...");
    let pool = PgPool::connect(&database_url).await?;

    println!("\n=== Cross-Market Correlation Backtest ===");
    println!("Combination: {}", args.combination);
    if let Some(ref session) = args.session_id {
        println!("Session: {}", session);
    }
    println!();

    // Fetch trade-level data
    let trade_stats = fetch_trade_stats(&pool, &args).await?;
    print_trade_stats(&trade_stats, args.bet_size);

    // Fetch window-level data
    let window_stats = fetch_window_stats(&pool, &args).await?;
    print_window_stats(&window_stats);

    // Calculate Kelly and bankroll
    print_kelly_analysis(&trade_stats);
    print_bankroll_table(&trade_stats, args.bet_size);

    // Per-pair breakdown if requested
    if args.show_pairs {
        print_pair_breakdown(&pool, &args).await?;
    }

    // Equity curve if requested
    if args.show_equity {
        print_equity_curve(&pool, &args).await?;
    }

    Ok(())
}

async fn fetch_trade_stats(pool: &PgPool, args: &CrossMarketBacktestArgs) -> Result<TradeStats> {
    let session_filter = args.session_id.as_deref().unwrap_or("%");

    // First get basic trade stats
    let row = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        trades AS (
            SELECT
                cost,
                spread,
                outcome,
                CASE
                    WHEN outcome = 'WIN' THEN spread
                    WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                    WHEN outcome = 'LOSE' THEN -cost
                END as pnl
            FROM windows
        )
        SELECT
            COUNT(*)::int as total,
            COUNT(CASE WHEN outcome = 'WIN' THEN 1 END)::int as wins,
            COUNT(CASE WHEN outcome = 'LOSE' THEN 1 END)::int as losses,
            COUNT(CASE WHEN outcome = 'DOUBLE_WIN' THEN 1 END)::int as double_wins,
            SUM(pnl)::numeric as total_pnl,
            AVG(cost)::numeric as avg_cost,
            AVG(spread)::numeric as avg_spread
        FROM trades
        "#,
        &args.combination,
        session_filter
    )
    .fetch_one(pool)
    .await?;

    // Separately calculate max drawdown
    let dd_row = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        trades AS (
            SELECT
                window_start,
                coin1, coin2,
                CASE
                    WHEN outcome = 'WIN' THEN spread
                    WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                    WHEN outcome = 'LOSE' THEN -cost
                END as pnl,
                ROW_NUMBER() OVER (ORDER BY window_start, coin1, coin2) as trade_num
            FROM windows
        ),
        running AS (
            SELECT
                trade_num,
                SUM(pnl) OVER (ORDER BY trade_num) as equity
            FROM trades
        ),
        peaks AS (
            SELECT
                equity,
                MAX(equity) OVER (ORDER BY trade_num) as peak
            FROM running
        )
        SELECT
            ABS(MIN(equity - peak))::numeric as max_drawdown
        FROM peaks
        "#,
        &args.combination,
        session_filter
    )
    .fetch_one(pool)
    .await?;

    // Calculate max consecutive losses
    let consec_row = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        trades AS (
            SELECT
                outcome,
                ROW_NUMBER() OVER (ORDER BY window_start, coin1, coin2) as n
            FROM windows
        ),
        streaks AS (
            SELECT
                outcome,
                n - ROW_NUMBER() OVER (PARTITION BY outcome ORDER BY n) as grp
            FROM trades
        )
        SELECT MAX(cnt)::int as max_streak
        FROM (
            SELECT COUNT(*) as cnt
            FROM streaks
            WHERE outcome = 'LOSE'
            GROUP BY grp
        ) x
        "#,
        &args.combination,
        session_filter
    )
    .fetch_one(pool)
    .await?;

    Ok(TradeStats {
        total: row.total.unwrap_or(0) as u32,
        wins: row.wins.unwrap_or(0) as u32,
        losses: row.losses.unwrap_or(0) as u32,
        double_wins: row.double_wins.unwrap_or(0) as u32,
        total_pnl: row.total_pnl.unwrap_or(Decimal::ZERO),
        avg_cost: row.avg_cost.unwrap_or(Decimal::ZERO),
        avg_spread: row.avg_spread.unwrap_or(Decimal::ZERO),
        max_consecutive_losses: consec_row.max_streak.unwrap_or(0) as u32,
        max_drawdown: dd_row.max_drawdown.unwrap_or(Decimal::ZERO),
    })
}

async fn fetch_window_stats(pool: &PgPool, args: &CrossMarketBacktestArgs) -> Result<WindowStats> {
    let session_filter = args.session_id.as_deref().unwrap_or("%");

    // Basic window stats
    let row = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        window_pnl AS (
            SELECT
                window_start,
                COUNT(*) as num_pairs,
                SUM(cost) as total_risk,
                SUM(CASE
                    WHEN outcome = 'WIN' THEN spread
                    WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                    WHEN outcome = 'LOSE' THEN -cost
                END) as pnl
            FROM windows
            GROUP BY window_start
        )
        SELECT
            COUNT(*)::int as total_windows,
            COUNT(CASE WHEN pnl > 0 THEN 1 END)::int as winning_windows,
            COUNT(CASE WHEN pnl <= 0 THEN 1 END)::int as losing_windows,
            AVG(num_pairs)::float8 as avg_pairs,
            MAX(total_risk)::numeric as max_risk,
            MIN(pnl)::numeric as worst_pnl,
            MAX(pnl)::numeric as best_pnl
        FROM window_pnl
        "#,
        &args.combination,
        session_filter
    )
    .fetch_one(pool)
    .await?;

    // Separate query for window drawdown
    let dd_row = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        window_pnl AS (
            SELECT
                window_start,
                SUM(CASE
                    WHEN outcome = 'WIN' THEN spread
                    WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                    WHEN outcome = 'LOSE' THEN -cost
                END) as pnl
            FROM windows
            GROUP BY window_start
        ),
        running AS (
            SELECT
                window_start,
                SUM(pnl) OVER (ORDER BY window_start) as equity
            FROM window_pnl
        ),
        peaks AS (
            SELECT
                equity,
                MAX(equity) OVER (ORDER BY window_start) as peak
            FROM running
        )
        SELECT
            ABS(MIN(equity - peak))::numeric as max_drawdown
        FROM peaks
        "#,
        &args.combination,
        session_filter
    )
    .fetch_one(pool)
    .await?;

    Ok(WindowStats {
        total: row.total_windows.unwrap_or(0) as u32,
        wins: row.winning_windows.unwrap_or(0) as u32,
        losses: row.losing_windows.unwrap_or(0) as u32,
        avg_pairs_per_window: row.avg_pairs.unwrap_or(0.0),
        max_risk_per_window: row.max_risk.unwrap_or(Decimal::ZERO),
        worst_window_pnl: row.worst_pnl.unwrap_or(Decimal::ZERO),
        best_window_pnl: row.best_pnl.unwrap_or(Decimal::ZERO),
        max_drawdown: dd_row.max_drawdown.unwrap_or(Decimal::ZERO),
    })
}

fn print_trade_stats(stats: &TradeStats, bet_size: u32) {
    let win_rate = if stats.total > 0 {
        (stats.wins + stats.double_wins) as f64 / stats.total as f64 * 100.0
    } else {
        0.0
    };

    let ev_per_trade = if stats.total > 0 {
        stats.total_pnl / Decimal::from(stats.total)
    } else {
        Decimal::ZERO
    };

    println!("TRADE-LEVEL ANALYSIS ({} trades = best entry per pair per window)", stats.total);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Win Rate:           {:.2}% ({} wins + {} double, {} losses)",
        win_rate, stats.wins, stats.double_wins, stats.losses
    );
    println!("Avg Cost:           ${}", stats.avg_cost.round_dp(4));
    println!("Avg Spread:         ${}", stats.avg_spread.round_dp(4));
    println!("EV per Trade:       ${} ({:.1}% ROI)", ev_per_trade.round_dp(4),
             (ev_per_trade / stats.avg_cost * dec!(100)).to_string().parse::<f64>().unwrap_or(0.0));
    println!("Max Consec Losses:  {}", stats.max_consecutive_losses);
    println!("Max Drawdown:       ${} per $1 stake", stats.max_drawdown.round_dp(2));
    println!(
        "Total P&L:          ${} per $1/pair (${} at ${}/pair)",
        stats.total_pnl.round_dp(2),
        (stats.total_pnl * Decimal::from(bet_size)).round_dp(0),
        bet_size
    );
    println!();
}

fn print_window_stats(stats: &WindowStats) {
    let win_rate = if stats.total > 0 {
        stats.wins as f64 / stats.total as f64 * 100.0
    } else {
        0.0
    };

    println!("WINDOW-LEVEL ANALYSIS ({} windows)", stats.total);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Win Rate:           {:.1}% ({} wins, {} losses)",
        win_rate, stats.wins, stats.losses
    );
    println!("Avg Pairs/Window:   {:.1}", stats.avg_pairs_per_window);
    println!("Max Risk/Window:    ${}", stats.max_risk_per_window.round_dp(2));
    println!("Worst Window:       ${}", stats.worst_window_pnl.round_dp(2));
    println!("Best Window:        ${}", stats.best_window_pnl.round_dp(2));
    println!("Max Drawdown:       ${}", stats.max_drawdown.round_dp(2));
    println!();
}

fn print_kelly_analysis(stats: &TradeStats) {
    if stats.total == 0 {
        return;
    }

    let p_win = (stats.wins + stats.double_wins) as f64 / stats.total as f64;
    let avg_win = if stats.wins + stats.double_wins > 0 {
        // Approximate: wins get spread, double wins get 1 + spread
        let win_pnl = Decimal::from(stats.wins) * stats.avg_spread
            + Decimal::from(stats.double_wins) * (Decimal::ONE + stats.avg_spread);
        win_pnl / Decimal::from(stats.wins + stats.double_wins)
    } else {
        Decimal::ZERO
    };
    let avg_loss = stats.avg_cost; // Loss is always the cost

    let b = avg_win / avg_loss;
    let b_f64 = b.to_string().parse::<f64>().unwrap_or(1.0);
    let kelly = (p_win * b_f64 - (1.0 - p_win)) / b_f64;

    println!("KELLY CRITERION ANALYSIS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Win Probability:    {:.2}%", p_win * 100.0);
    println!("Avg Win:            ${}", avg_win.round_dp(4));
    println!("Avg Loss:           ${}", avg_loss.round_dp(4));
    println!("Win/Loss Ratio:     {:.3}", b_f64);
    println!("Full Kelly:         {:.1}% of bankroll", kelly * 100.0);
    println!("Half Kelly:         {:.1}% of bankroll", kelly * 50.0);
    println!("Quarter Kelly:      {:.1}% of bankroll", kelly * 25.0);
    println!();
}

fn print_bankroll_table(stats: &TradeStats, default_bet: u32) {
    println!("BANKROLL REQUIREMENTS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Bet/Pair   Max DD     3x Bankroll   5x Bankroll   Expected Profit");
    println!("--------   ------     -----------   -----------   ---------------");

    let dd = stats.max_drawdown;
    for bet in [10, 25, 50, 100, 250, 500] {
        let bet_dec = Decimal::from(bet);
        let max_dd = dd * bet_dec;
        let bank_3x = max_dd * dec!(3);
        let bank_5x = max_dd * dec!(5);
        let profit = stats.total_pnl * bet_dec;

        println!(
            "${:<8}  ${:<8}  ${:<12}  ${:<12}  ${}",
            bet,
            max_dd.round_dp(0),
            bank_3x.round_dp(0),
            bank_5x.round_dp(0),
            profit.round_dp(0)
        );
    }
    println!();
    println!(
        "Recommended: Start with ${} bankroll for ${}/pair bets",
        (dd * Decimal::from(default_bet) * dec!(5)).round_dp(0),
        default_bet
    );
    println!();
}

async fn print_pair_breakdown(pool: &PgPool, args: &CrossMarketBacktestArgs) -> Result<()> {
    let session_filter = args.session_id.as_deref().unwrap_or("%");

    let rows = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        )
        SELECT
            coin1 || '/' || coin2 as pair,
            COUNT(*)::int as trades,
            COUNT(CASE WHEN outcome IN ('WIN', 'DOUBLE_WIN') THEN 1 END)::int as wins,
            (COUNT(CASE WHEN outcome IN ('WIN', 'DOUBLE_WIN') THEN 1 END)::float / COUNT(*) * 100)::numeric(5,1) as win_pct,
            SUM(CASE
                WHEN outcome = 'WIN' THEN spread
                WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                WHEN outcome = 'LOSE' THEN -cost
            END)::numeric as pnl
        FROM windows
        GROUP BY coin1, coin2
        ORDER BY pnl DESC
        "#,
        &args.combination,
        session_filter
    )
    .fetch_all(pool)
    .await?;

    println!("PER-PAIR BREAKDOWN");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Pair       Trades    Wins    Win %     P&L");
    println!("--------   ------    ----    -----     ----");

    for row in rows {
        println!(
            "{:<10} {:<9} {:<7} {:<9} ${}",
            row.pair.unwrap_or_default(),
            row.trades.unwrap_or(0),
            row.wins.unwrap_or(0),
            format!("{}%", row.win_pct.unwrap_or(Decimal::ZERO)),
            row.pnl.unwrap_or(Decimal::ZERO).round_dp(2)
        );
    }
    println!();

    Ok(())
}

async fn print_equity_curve(pool: &PgPool, args: &CrossMarketBacktestArgs) -> Result<()> {
    let session_filter = args.session_id.as_deref().unwrap_or("%");

    let rows = sqlx::query!(
        r#"
        WITH windows AS (
            SELECT
                date_trunc('minute', timestamp) - (EXTRACT(minute FROM timestamp)::int % 15) * interval '1 minute' as window_start,
                coin1, coin2,
                MIN(total_cost) as cost,
                MAX(spread) as spread,
                MAX(trade_result) as outcome
            FROM cross_market_opportunities
            WHERE status = 'settled'
              AND combination = $1
              AND (session_id LIKE $2 OR $2 = '%')
            GROUP BY 1, coin1, coin2
        ),
        window_pnl AS (
            SELECT
                window_start,
                COUNT(*) as num_pairs,
                SUM(CASE
                    WHEN outcome = 'WIN' THEN spread
                    WHEN outcome = 'DOUBLE_WIN' THEN 1.0 + spread
                    WHEN outcome = 'LOSE' THEN -cost
                END) as pnl
            FROM windows
            GROUP BY window_start
        )
        SELECT
            to_char(window_start, 'HH24:MI') as time,
            num_pairs::int as pairs,
            pnl::numeric as window_pnl,
            SUM(pnl) OVER (ORDER BY window_start)::numeric as equity
        FROM window_pnl
        ORDER BY window_start
        "#,
        &args.combination,
        session_filter
    )
    .fetch_all(pool)
    .await?;

    println!("EQUITY CURVE (per $1 stake per pair)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Time     Pairs  Window P&L  Cumulative");
    println!("-----    -----  ----------  ----------");

    for row in rows {
        let pnl = row.window_pnl.unwrap_or(Decimal::ZERO);
        let equity = row.equity.unwrap_or(Decimal::ZERO);
        let indicator = if pnl > Decimal::ZERO { "+" } else { "" };

        println!(
            "{}    {:<5}  {}${:<9}  ${}",
            row.time.unwrap_or_default(),
            row.pairs.unwrap_or(0),
            indicator,
            pnl.round_dp(2),
            equity.round_dp(2)
        );
    }
    println!();

    Ok(())
}
