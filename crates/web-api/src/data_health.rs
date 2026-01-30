//! Data health endpoint for monitoring data freshness.
//!
//! Provides `/api/data/health` endpoint that returns the status of all
//! Phase 1 data collection tables including last record time, record counts,
//! and overall health status.

use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;

/// Health status for a single data source.
#[derive(Debug, Clone, Serialize)]
pub struct SourceHealth {
    /// Name of the data source/table.
    pub source: String,
    /// Timestamp of the most recent record.
    pub last_record: Option<DateTime<Utc>>,
    /// Number of records in the last hour.
    pub records_last_hour: i64,
    /// Seconds since the last record (staleness).
    pub staleness_seconds: Option<i64>,
    /// Health status: "healthy", "degraded", or "unhealthy".
    pub status: String,
}

/// Overall data health response.
#[derive(Debug, Clone, Serialize)]
pub struct DataHealthResponse {
    /// Overall system health status.
    pub status: String,
    /// Current server timestamp.
    pub timestamp: DateTime<Utc>,
    /// Health status for each data source.
    pub sources: Vec<SourceHealth>,
    /// Summary statistics.
    pub summary: HealthSummary,
}

/// Summary of health statistics.
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    /// Number of healthy sources.
    pub healthy: usize,
    /// Number of degraded sources.
    pub degraded: usize,
    /// Number of unhealthy sources.
    pub unhealthy: usize,
    /// Total records across all sources in the last hour.
    pub total_records_last_hour: i64,
}

/// Thresholds for determining health status (in seconds).
struct HealthThresholds {
    /// Max staleness for "healthy" status.
    healthy: i64,
    /// Max staleness for "degraded" status (beyond this is "unhealthy").
    degraded: i64,
}

impl HealthThresholds {
    /// Get thresholds based on data source type.
    fn for_source(source: &str) -> Self {
        match source {
            // Order book: expect data every second
            "orderbook_snapshots" => Self {
                healthy: 10,
                degraded: 60,
            },
            // Funding rates: expect every few seconds
            "funding_rates" => Self {
                healthy: 30,
                degraded: 120,
            },
            // Liquidations: can be sparse, depends on market
            "liquidations" => Self {
                healthy: 300,
                degraded: 900,
            },
            // Aggregates: computed periodically
            "liquidation_aggregates" => Self {
                healthy: 120,
                degraded: 300,
            },
            // Polymarket: polls every 30s
            "polymarket_odds" => Self {
                healthy: 60,
                degraded: 180,
            },
            // News: polls every 60s
            "news_events" => Self {
                healthy: 120,
                degraded: 600,
            },
            // Default thresholds
            _ => Self {
                healthy: 60,
                degraded: 300,
            },
        }
    }
}

/// Determine health status based on staleness.
fn determine_status(staleness_seconds: Option<i64>, thresholds: &HealthThresholds) -> String {
    match staleness_seconds {
        None => "unhealthy".to_string(), // No data at all
        Some(s) if s <= thresholds.healthy => "healthy".to_string(),
        Some(s) if s <= thresholds.degraded => "degraded".to_string(),
        Some(_) => "unhealthy".to_string(),
    }
}

/// Query health metrics for a single table.
async fn query_source_health(pool: &PgPool, table: &str) -> Result<SourceHealth, sqlx::Error> {
    // Query last record timestamp and count in last hour
    // Using raw SQL since table names can't be parameterized
    let query = format!(
        r#"
        SELECT
            MAX(timestamp) as last_record,
            COUNT(*) FILTER (WHERE timestamp > NOW() - INTERVAL '1 hour') as records_last_hour
        FROM {}
        "#,
        table
    );

    let row: (Option<DateTime<Utc>>, i64) = sqlx::query_as(&query).fetch_one(pool).await?;

    let (last_record, records_last_hour) = row;

    let staleness_seconds = last_record.map(|lr| (Utc::now() - lr).num_seconds());

    let thresholds = HealthThresholds::for_source(table);
    let status = determine_status(staleness_seconds, &thresholds);

    Ok(SourceHealth {
        source: table.to_string(),
        last_record,
        records_last_hour,
        staleness_seconds,
        status,
    })
}

/// App state containing database pool.
#[derive(Clone)]
pub struct DataHealthState {
    pub pool: Arc<PgPool>,
}

/// GET /api/data/health - Returns health status of all data sources.
///
/// # Errors
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if database queries fail.
pub async fn data_health(
    State(state): State<DataHealthState>,
) -> Result<Json<DataHealthResponse>, StatusCode> {
    let tables = [
        "orderbook_snapshots",
        "funding_rates",
        "liquidations",
        "liquidation_aggregates",
        "polymarket_odds",
        "news_events",
    ];

    let mut sources = Vec::with_capacity(tables.len());
    let mut total_records: i64 = 0;

    for table in &tables {
        match query_source_health(&state.pool, table).await {
            Ok(health) => {
                total_records += health.records_last_hour;
                sources.push(health);
            }
            Err(e) => {
                tracing::error!("Failed to query health for {}: {}", table, e);
                sources.push(SourceHealth {
                    source: table.to_string(),
                    last_record: None,
                    records_last_hour: 0,
                    staleness_seconds: None,
                    status: "unhealthy".to_string(),
                });
            }
        }
    }

    let healthy = sources.iter().filter(|s| s.status == "healthy").count();
    let degraded = sources.iter().filter(|s| s.status == "degraded").count();
    let unhealthy = sources.iter().filter(|s| s.status == "unhealthy").count();

    // Overall status: unhealthy if critical source (orderbook) is unhealthy,
    // degraded if any source is degraded or unhealthy, otherwise healthy
    let overall_status = if sources
        .iter()
        .any(|s| s.status == "unhealthy" && s.source == "orderbook_snapshots")
    {
        "unhealthy"
    } else if unhealthy > 0 || degraded > 0 {
        "degraded"
    } else {
        "healthy"
    };

    Ok(Json(DataHealthResponse {
        status: overall_status.to_string(),
        timestamp: Utc::now(),
        sources,
        summary: HealthSummary {
            healthy,
            degraded,
            unhealthy,
            total_records_last_hour: total_records,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_status_healthy() {
        let thresholds = HealthThresholds {
            healthy: 10,
            degraded: 60,
        };
        assert_eq!(determine_status(Some(5), &thresholds), "healthy");
        assert_eq!(determine_status(Some(10), &thresholds), "healthy");
    }

    #[test]
    fn test_determine_status_degraded() {
        let thresholds = HealthThresholds {
            healthy: 10,
            degraded: 60,
        };
        assert_eq!(determine_status(Some(11), &thresholds), "degraded");
        assert_eq!(determine_status(Some(60), &thresholds), "degraded");
    }

    #[test]
    fn test_determine_status_unhealthy() {
        let thresholds = HealthThresholds {
            healthy: 10,
            degraded: 60,
        };
        assert_eq!(determine_status(Some(61), &thresholds), "unhealthy");
        assert_eq!(determine_status(None, &thresholds), "unhealthy");
    }

    #[test]
    fn test_thresholds_for_orderbook() {
        let thresholds = HealthThresholds::for_source("orderbook_snapshots");
        assert_eq!(thresholds.healthy, 10);
        assert_eq!(thresholds.degraded, 60);
    }

    #[test]
    fn test_thresholds_for_liquidations() {
        let thresholds = HealthThresholds::for_source("liquidations");
        assert_eq!(thresholds.healthy, 300); // 5 minutes - liquidations can be sparse
        assert_eq!(thresholds.degraded, 900);
    }

    #[test]
    fn test_thresholds_for_polymarket() {
        let thresholds = HealthThresholds::for_source("polymarket_odds");
        assert_eq!(thresholds.healthy, 60); // Polls every 30s, allow 60s
        assert_eq!(thresholds.degraded, 180);
    }

    #[test]
    fn test_source_health_serialization() {
        let health = SourceHealth {
            source: "orderbook_snapshots".to_string(),
            last_record: Some(Utc::now()),
            records_last_hour: 3600,
            staleness_seconds: Some(5),
            status: "healthy".to_string(),
        };
        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("orderbook_snapshots"));
        assert!(json.contains("healthy"));
    }

    #[test]
    fn test_health_summary() {
        let summary = HealthSummary {
            healthy: 4,
            degraded: 1,
            unhealthy: 1,
            total_records_last_hour: 10000,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"healthy\":4"));
        assert!(json.contains("\"degraded\":1"));
    }
}
