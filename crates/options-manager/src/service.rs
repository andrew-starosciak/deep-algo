//! Main service loop — orchestrates execution, monitoring, and rule enforcement.

use std::time::Duration;

use anyhow::Result;
use sqlx::PgPool;
use tracing::{error, info};

use algo_trade_ib::client::IBClient;

use crate::executor;
use crate::monitor;
use crate::stops;
use crate::targets;
use crate::types::ManagerConfig;

/// Run the options manager service loop.
///
/// This is the main entry point. It polls every `config.poll_interval_secs` seconds:
/// 1. Execute any newly approved recommendations
/// 2. Update prices for open positions
/// 3. Check stop rules (hard stop, time stop)
/// 4. Check profit targets
pub async fn run(pool: PgPool, ib: IBClient, config: ManagerConfig) -> Result<()> {
    info!(
        poll_secs = config.poll_interval_secs,
        hard_stop = %config.hard_stop_pct,
        profit_target_1 = %config.profit_target_1_pct,
        profit_target_2 = %config.profit_target_2_pct,
        time_stop_dte = config.time_stop_dte,
        "Options manager started"
    );

    let mut interval = tokio::time::interval(Duration::from_secs(config.poll_interval_secs));

    loop {
        interval.tick().await;

        // 1. Execute approved recommendations
        match executor::execute_approved(&pool, &ib).await {
            Ok(n) if n > 0 => info!(count = n, "Executed approved recommendations"),
            Ok(_) => {}
            Err(e) => error!(error = %e, "Failed to execute recommendations"),
        }

        // 2. Update open position prices
        let positions = match monitor::get_open_positions(&pool).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to fetch open positions");
                continue;
            }
        };

        if positions.is_empty() {
            continue;
        }

        if let Err(e) = monitor::update_position_prices(&pool, &ib, &positions).await {
            error!(error = %e, "Failed to update position prices");
        }

        // 3. Check stop rules + profit targets for each position
        for pos in &positions {
            // Check stops first (more urgent)
            if let Some(action) = stops::check_stop_rules(pos, &config) {
                info!(ticker = pos.ticker, ?action, "Stop rule triggered");
                // TODO: Execute the stop action via IB and update DB
                continue;
            }

            // Check profit targets
            if let Some(action) = targets::check_profit_targets(pos, &config) {
                info!(ticker = pos.ticker, ?action, "Profit target triggered");
                // TODO: Execute the profit-taking action via IB and update DB
            }
        }
    }
}
