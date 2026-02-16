//! Cross-platform exposure and correlation checks.
//!
//! Reads positions from HyperLiquid and Polymarket (via shared DB)
//! to flag correlated exposure.

use anyhow::Result;
use sqlx::PgPool;
use tracing::warn;

/// Cross-platform exposure report.
#[derive(Debug)]
pub struct ExposureReport {
    /// Number of options positions in same sector.
    pub sector_concentration: usize,
    /// Flags for correlated positions across platforms.
    pub correlation_flags: Vec<String>,
    /// Whether exposure is acceptable.
    pub acceptable: bool,
}

/// Check cross-platform correlations for a given ticker/sector.
pub async fn check_cross_platform_exposure(
    pool: &PgPool,
    ticker: &str,
    sector: &str,
    max_correlated: usize,
) -> Result<ExposureReport> {
    let mut flags = Vec::new();

    // Check HyperLiquid positions for correlated crypto exposure.
    // e.g., buying MSTR calls while long BTC on HyperLiquid
    // TODO: Query HL positions from DB when available

    // Check Polymarket positions for event correlation.
    // e.g., Fed decision bet on PM + rate-sensitive options
    // TODO: Query PM positions from DB when available

    // Check options positions in same sector.
    let sector_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM options_positions WHERE status = 'open' AND ticker IN (SELECT ticker FROM options_watchlist WHERE sector = $1)"
    )
    .bind(sector)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let sector_count = sector_count as usize;

    if sector_count >= max_correlated {
        flags.push(format!(
            "Already {} open positions in sector '{}' (max {})",
            sector_count, sector, max_correlated
        ));
    }

    let acceptable = sector_count < max_correlated && flags.is_empty();

    if !acceptable {
        warn!(ticker, sector, ?flags, "Correlation check flagged issues");
    }

    Ok(ExposureReport {
        sector_concentration: sector_count,
        correlation_flags: flags,
        acceptable,
    })
}
