//! Data status CLI command.
//!
//! Queries all Phase 1 data tables to show record counts and date ranges.
//! Used to assess data availability before running historical backtests.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Arguments for the data-status command.
#[derive(Args, Debug, Clone)]
pub struct DataStatusArgs {
    /// Symbol to filter by (e.g., "BTCUSDT"). If not provided, shows all symbols.
    #[arg(long)]
    pub symbol: Option<String>,

    /// Exchange to filter by (e.g., "binance", "hyperliquid"). If not provided, shows all.
    #[arg(long)]
    pub exchange: Option<String>,

    /// Database connection URL (uses DATABASE_URL env var if not provided)
    #[arg(long, env = "DATABASE_URL")]
    pub db_url: Option<String>,
}

/// Data bounds for a single table.
#[derive(Debug)]
struct TableStatus {
    table_name: String,
    record_count: i64,
    earliest: Option<DateTime<Utc>>,
    latest: Option<DateTime<Utc>>,
    symbols: Vec<String>,
}

impl TableStatus {
    fn format_date(dt: Option<DateTime<Utc>>) -> String {
        dt.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "N/A".to_string())
    }

    fn duration_days(&self) -> Option<f64> {
        match (self.earliest, self.latest) {
            (Some(e), Some(l)) => Some((l - e).num_hours() as f64 / 24.0),
            _ => None,
        }
    }
}

/// Runs the data-status command.
///
/// # Errors
/// Returns an error if database connection or queries fail.
pub async fn run_data_status(args: DataStatusArgs) -> Result<()> {
    let db_url = args
        .db_url
        .ok_or_else(|| anyhow!("DATABASE_URL must be set via --db-url or DATABASE_URL env var"))?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .map_err(|e| anyhow!("Failed to connect to database: {}", e))?;

    tracing::info!("Connected to database");

    // Query each Phase 1 table
    let tables = vec![
        ("orderbook_snapshots", true, true),
        ("funding_rates", true, true),
        ("liquidations", true, true),
        ("liquidation_aggregates", true, true),
        ("polymarket_odds", false, false), // Different schema
        ("news_events", false, false),
        ("ohlcv", true, true),
        ("signal_snapshots", true, true),
    ];

    let mut statuses = Vec::new();

    for (table, has_symbol, has_exchange) in tables {
        let status = query_table_status(
            &pool,
            table,
            has_symbol,
            has_exchange,
            &args.symbol,
            &args.exchange,
        )
        .await;

        match status {
            Ok(s) => statuses.push(s),
            Err(e) => {
                tracing::warn!("Failed to query {}: {}", table, e);
                statuses.push(TableStatus {
                    table_name: table.to_string(),
                    record_count: 0,
                    earliest: None,
                    latest: None,
                    symbols: vec![],
                });
            }
        }
    }

    // Display results
    print_status_report(&statuses, &args.symbol, &args.exchange);

    // Provide recommendations
    print_recommendations(&statuses);

    Ok(())
}

async fn query_table_status(
    pool: &PgPool,
    table: &str,
    has_symbol: bool,
    has_exchange: bool,
    symbol_filter: &Option<String>,
    exchange_filter: &Option<String>,
) -> Result<TableStatus> {
    // Build WHERE clause based on filters
    let mut conditions = Vec::new();

    if has_symbol {
        if let Some(sym) = symbol_filter {
            conditions.push(format!("symbol = '{}'", sym.replace('\'', "''")));
        }
    }

    if has_exchange {
        if let Some(exch) = exchange_filter {
            conditions.push(format!("exchange = '{}'", exch.replace('\'', "''")));
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Query count
    let count_query = format!("SELECT COUNT(*) as count FROM {} {}", table, where_clause);
    let count_row: (i64,) = sqlx::query_as(&count_query).fetch_one(pool).await?;

    // Query date bounds
    let bounds_query = format!(
        "SELECT MIN(timestamp), MAX(timestamp) FROM {} {}",
        table, where_clause
    );
    let bounds_row: (Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
        sqlx::query_as(&bounds_query).fetch_one(pool).await?;

    // Query distinct symbols (if applicable)
    let symbols = if has_symbol {
        let symbols_query = format!(
            "SELECT DISTINCT symbol FROM {} {} ORDER BY symbol LIMIT 20",
            table, where_clause
        );
        let symbol_rows: Vec<(String,)> = sqlx::query_as(&symbols_query).fetch_all(pool).await?;
        symbol_rows.into_iter().map(|r| r.0).collect()
    } else {
        vec![]
    };

    Ok(TableStatus {
        table_name: table.to_string(),
        record_count: count_row.0,
        earliest: bounds_row.0,
        latest: bounds_row.1,
        symbols,
    })
}

fn print_status_report(
    statuses: &[TableStatus],
    symbol_filter: &Option<String>,
    exchange_filter: &Option<String>,
) {
    println!();
    println!("{}", "=".repeat(100));
    println!("DATA STATUS REPORT");
    if let Some(sym) = symbol_filter {
        println!("Filter: symbol = {}", sym);
    }
    if let Some(exch) = exchange_filter {
        println!("Filter: exchange = {}", exch);
    }
    println!("{}", "=".repeat(100));
    println!();

    println!(
        "{:<25} {:>12} {:>22} {:>22} {:>12}",
        "Table", "Records", "Earliest", "Latest", "Days"
    );
    println!("{}", "-".repeat(100));

    for status in statuses {
        let days_str = status
            .duration_days()
            .map(|d| format!("{:.1}", d))
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<25} {:>12} {:>22} {:>22} {:>12}",
            status.table_name,
            format_count(status.record_count),
            TableStatus::format_date(status.earliest),
            TableStatus::format_date(status.latest),
            days_str
        );
    }

    println!("{}", "=".repeat(100));
    println!();

    // Show symbols per table
    println!("SYMBOLS BY TABLE:");
    println!("{}", "-".repeat(60));
    for status in statuses {
        if !status.symbols.is_empty() {
            let symbols_display = if status.symbols.len() > 5 {
                format!(
                    "{} (+{} more)",
                    status.symbols[..5].join(", "),
                    status.symbols.len() - 5
                )
            } else {
                status.symbols.join(", ")
            };
            println!("  {:<23}: {}", status.table_name, symbols_display);
        }
    }
    println!();
}

fn print_recommendations(statuses: &[TableStatus]) {
    println!("BACKTEST READINESS:");
    println!("{}", "-".repeat(60));

    let orderbook = statuses
        .iter()
        .find(|s| s.table_name == "orderbook_snapshots");
    let funding = statuses.iter().find(|s| s.table_name == "funding_rates");
    let liquidations = statuses.iter().find(|s| s.table_name == "liquidations");
    let ohlcv = statuses.iter().find(|s| s.table_name == "ohlcv");

    let mut ready_signals = Vec::new();
    let mut missing_signals = Vec::new();

    // Check order book imbalance signal readiness
    if let Some(ob) = orderbook {
        if ob.record_count > 1000 {
            ready_signals.push(format!(
                "Order Book Imbalance: {} records ({:.1} days)",
                format_count(ob.record_count),
                ob.duration_days().unwrap_or(0.0)
            ));
        } else {
            missing_signals.push("Order Book Imbalance: insufficient data (need >1000 records)");
        }
    } else {
        missing_signals.push("Order Book Imbalance: no data");
    }

    // Check funding rate signal readiness
    if let Some(fr) = funding {
        if fr.record_count > 100 {
            ready_signals.push(format!(
                "Funding Rate: {} records ({:.1} days)",
                format_count(fr.record_count),
                fr.duration_days().unwrap_or(0.0)
            ));
        } else {
            missing_signals.push("Funding Rate: insufficient data (need >100 records)");
        }
    } else {
        missing_signals.push("Funding Rate: no data - run `backfill-funding` command");
    }

    // Check liquidation signal readiness
    if let Some(liq) = liquidations {
        if liq.record_count > 100 {
            ready_signals.push(format!(
                "Liquidations: {} records ({:.1} days)",
                format_count(liq.record_count),
                liq.duration_days().unwrap_or(0.0)
            ));
        } else {
            missing_signals.push("Liquidations: insufficient data (need >100 records)");
        }
    } else {
        missing_signals.push("Liquidations: no data");
    }

    // Check OHLCV for return calculations
    if let Some(o) = ohlcv {
        if o.record_count > 1000 {
            ready_signals.push(format!(
                "OHLCV (for returns): {} records ({:.1} days)",
                format_count(o.record_count),
                o.duration_days().unwrap_or(0.0)
            ));
        } else {
            missing_signals
                .push("OHLCV: insufficient data (need >1000 records for return calculations)");
        }
    } else {
        missing_signals.push("OHLCV: no data - run `backfill-ohlcv` command");
    }

    if !ready_signals.is_empty() {
        println!("  Ready for backtest:");
        for sig in &ready_signals {
            println!("    [OK] {}", sig);
        }
    }

    if !missing_signals.is_empty() {
        println!("  Missing or insufficient:");
        for sig in &missing_signals {
            println!("    [!!] {}", sig);
        }
    }

    // Overall recommendation
    println!();
    if ready_signals.len() >= 2 {
        println!("  RECOMMENDATION: You have enough data to run a historical backtest.");
        println!(
            "  Use: cargo run -p algo-trade-cli -- binary-backtest --start <date> --end <date>"
        );
    } else {
        println!("  RECOMMENDATION: Collect more data before running a backtest.");
        println!("  Use collectors or backfill commands to populate missing tables.");
    }
    println!();
}

fn format_count(count: i64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_count_small() {
        assert_eq!(format_count(500), "500");
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn test_format_count_thousands() {
        assert_eq!(format_count(1000), "1.0K");
        assert_eq!(format_count(5500), "5.5K");
        assert_eq!(format_count(999_999), "1000.0K");
    }

    #[test]
    fn test_format_count_millions() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(2_500_000), "2.5M");
    }

    #[test]
    fn test_table_status_format_date_some() {
        use chrono::TimeZone;
        let dt = Utc.with_ymd_and_hms(2025, 1, 15, 12, 30, 45).unwrap();
        let result = TableStatus::format_date(Some(dt));
        assert_eq!(result, "2025-01-15 12:30:45");
    }

    #[test]
    fn test_table_status_format_date_none() {
        let result = TableStatus::format_date(None);
        assert_eq!(result, "N/A");
    }

    #[test]
    fn test_table_status_duration_days() {
        use chrono::TimeZone;
        let status = TableStatus {
            table_name: "test".to_string(),
            record_count: 100,
            earliest: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
            latest: Some(Utc.with_ymd_and_hms(2025, 1, 8, 0, 0, 0).unwrap()),
            symbols: vec![],
        };
        let days = status.duration_days().unwrap();
        assert!((days - 7.0).abs() < 0.01);
    }

    #[test]
    fn test_table_status_duration_days_none() {
        let status = TableStatus {
            table_name: "test".to_string(),
            record_count: 0,
            earliest: None,
            latest: None,
            symbols: vec![],
        };
        assert!(status.duration_days().is_none());
    }

    #[test]
    fn test_table_status_duration_partial_none() {
        use chrono::TimeZone;
        let status = TableStatus {
            table_name: "test".to_string(),
            record_count: 1,
            earliest: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
            latest: None,
            symbols: vec![],
        };
        assert!(status.duration_days().is_none());
    }
}
