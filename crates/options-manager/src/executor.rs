//! Reads approved recommendations from DB and executes via IB.

use anyhow::Result;
use sqlx::PgPool;
use tracing::{info, warn};

use algo_trade_ib::client::IBClient;
use algo_trade_ib::types::{OptionRight, OptionsContract, OptionsOrder, OrderSide, OrderType};

use crate::types::ApprovedRecommendation;

/// Poll for approved recommendations and execute them.
pub async fn execute_approved(pool: &PgPool, ib: &IBClient) -> Result<u32> {
    let approved = fetch_approved_recommendations(pool).await?;
    let mut executed = 0;

    for rec in approved {
        match execute_recommendation(pool, ib, &rec).await {
            Ok(()) => {
                executed += 1;
                info!(id = rec.id, ticker = rec.ticker, "Recommendation executed");
            }
            Err(e) => {
                warn!(id = rec.id, ticker = rec.ticker, error = %e, "Failed to execute recommendation");
                // Mark as failed in DB so we don't retry indefinitely
                mark_recommendation_failed(pool, rec.id, &e.to_string()).await?;
            }
        }
    }

    Ok(executed)
}

async fn fetch_approved_recommendations(pool: &PgPool) -> Result<Vec<ApprovedRecommendation>> {
    // Using runtime query (not query_as!) because tables may not exist yet.
    // Switch to compile-time checked query_as! after running migrations.
    let rows = sqlx::query(
        r#"
        SELECT id, thesis_id, ticker, "right", strike, expiry,
               entry_price_low, entry_price_high, position_size_usd
        FROM trade_recommendations
        WHERE status = 'approved'
        ORDER BY approved_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut recs = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row;
        recs.push(ApprovedRecommendation {
            id: row.get("id"),
            thesis_id: row.get("thesis_id"),
            ticker: row.get("ticker"),
            right: row.get("right"),
            strike: row.get("strike"),
            expiry: row.get("expiry"),
            entry_price_low: row.get("entry_price_low"),
            entry_price_high: row.get("entry_price_high"),
            position_size_usd: row.get("position_size_usd"),
        });
    }

    Ok(recs)
}

async fn execute_recommendation(
    pool: &PgPool,
    ib: &IBClient,
    rec: &ApprovedRecommendation,
) -> Result<()> {
    // Mark as executing
    sqlx::query("UPDATE trade_recommendations SET status = 'executing' WHERE id = $1")
        .bind(rec.id)
        .execute(pool)
        .await?;

    // Build the options contract
    let right = match rec.right.as_str() {
        "call" => OptionRight::Call,
        "put" => OptionRight::Put,
        _ => anyhow::bail!("Invalid option right: {}", rec.right),
    };

    let contract = OptionsContract::new(&rec.ticker, rec.expiry, rec.strike, right);

    // Calculate quantity from USD size and entry price
    let mid_price = (rec.entry_price_low + rec.entry_price_high) / rust_decimal::Decimal::from(2);
    let contract_cost = mid_price * contract.multiplier;
    let quantity = (rec.position_size_usd / contract_cost)
        .floor()
        .to_string()
        .parse::<i32>()
        .unwrap_or(1)
        .max(1);

    let order = OptionsOrder {
        contract: contract.clone(),
        side: OrderSide::Buy,
        quantity,
        order_type: OrderType::Limit { price: rec.entry_price_high },
    };

    // Execute
    let fill = ib.place_options_order(&order).await?;

    // Record the position
    sqlx::query(
        r#"
        INSERT INTO options_positions
            (recommendation_id, ticker, right, strike, expiry, quantity,
             avg_fill_price, current_price, cost_basis, unrealized_pnl, status, opened_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8, 0, 'open', NOW())
        "#,
    )
    .bind(rec.id)
    .bind(&rec.ticker)
    .bind(&rec.right)
    .bind(rec.strike)
    .bind(rec.expiry)
    .bind(fill.quantity)
    .bind(fill.avg_fill_price)
    .bind(fill.avg_fill_price * contract.multiplier * rust_decimal::Decimal::from(fill.quantity))
    .execute(pool)
    .await?;

    // Update recommendation status
    sqlx::query("UPDATE trade_recommendations SET status = 'filled' WHERE id = $1")
        .bind(rec.id)
        .execute(pool)
        .await?;

    Ok(())
}

async fn mark_recommendation_failed(pool: &PgPool, id: i64, reason: &str) -> Result<()> {
    sqlx::query("UPDATE trade_recommendations SET status = 'failed', rejected_reason = $2 WHERE id = $1")
        .bind(id)
        .bind(reason)
        .execute(pool)
        .await?;
    Ok(())
}
