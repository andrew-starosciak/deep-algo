//! IB account queries — balance, positions, buying power.

use anyhow::{Context, Result};
use rust_decimal::Decimal;
use tracing::debug;

use crate::client::IBClient;
use crate::types::AccountSummary;

impl IBClient {
    /// Fetch account summary (net liquidation, buying power, margin, P&L).
    pub async fn account_summary(&self) -> Result<AccountSummary> {
        let tags = &[
            "NetLiquidation",
            "BuyingPower",
            "AvailableFunds",
            "MaintMarginReq",
            "UnrealizedPnL",
            "RealizedPnL",
        ];

        let _subscription = self
            .inner()
            .account_summary(&"All".into(), tags)
            .await
            .context("Failed to request account summary")?;

        let summary = AccountSummary {
            account_id: String::new(),
            net_liquidation: Decimal::ZERO,
            buying_power: Decimal::ZERO,
            available_funds: Decimal::ZERO,
            maintenance_margin: Decimal::ZERO,
            unrealized_pnl: Decimal::ZERO,
            realized_pnl: Decimal::ZERO,
        };

        // TODO: Iterate the subscription stream to populate fields.
        // The ibapi crate returns updates via Subscription<AccountSummaryResult>.
        debug!("Account summary retrieved");

        Ok(summary)
    }
}
