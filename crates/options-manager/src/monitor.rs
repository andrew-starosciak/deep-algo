//! Position monitoring — update prices, P&L, greeks for open positions.

use anyhow::Result;
use sqlx::PgPool;
use tracing::debug;

use algo_trade_ib::client::IBClient;

use crate::types::OptionsPosition;

/// Fetch all open positions from DB.
pub async fn get_open_positions(pool: &PgPool) -> Result<Vec<OptionsPosition>> {
    // Runtime query — switch to query_as! after migrations are applied.
    let rows = sqlx::query(
        r#"
        SELECT id, recommendation_id, ticker, "right", strike, expiry,
               quantity, avg_fill_price, current_price, cost_basis,
               unrealized_pnl, realized_pnl, status, opened_at
        FROM options_positions
        WHERE status = 'open'
        ORDER BY opened_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut positions = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row;
        positions.push(OptionsPosition {
            id: row.get("id"),
            recommendation_id: row.get("recommendation_id"),
            ticker: row.get("ticker"),
            right: row.get("right"),
            strike: row.get("strike"),
            expiry: row.get("expiry"),
            quantity: row.get("quantity"),
            avg_fill_price: row.get("avg_fill_price"),
            current_price: row.get("current_price"),
            cost_basis: row.get("cost_basis"),
            unrealized_pnl: row.get("unrealized_pnl"),
            realized_pnl: row.get("realized_pnl"),
            status: row.get("status"),
            opened_at: row.get("opened_at"),
        });
    }

    Ok(positions)
}

/// Update current prices and P&L for all open positions.
pub async fn update_position_prices(
    pool: &PgPool,
    _ib: &IBClient,
    positions: &[OptionsPosition],
) -> Result<()> {
    for pos in positions {
        // TODO: Fetch current quote from IB and update
        // let quote = ib.option_quote(&contract).await?;
        // let unrealized = (quote.mid - pos.avg_fill_price) * multiplier * quantity;
        // UPDATE options_positions SET current_price = ..., unrealized_pnl = ...

        debug!(ticker = pos.ticker, id = pos.id, "Updated position price");
    }

    Ok(())
}
