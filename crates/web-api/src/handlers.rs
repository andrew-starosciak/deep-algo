use algo_trade_bot_orchestrator::{BotConfig, BotRegistry};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct BotListResponse {
    pub bots: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateBotRequest {
    pub bot_id: String,
    pub symbol: String,
    pub strategy: String,
}

/// Lists all registered bots.
///
/// # Errors
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if the registry cannot be accessed.
pub async fn list_bots(
    State(registry): State<Arc<BotRegistry>>,
) -> Result<Json<BotListResponse>, StatusCode> {
    let bots = registry.list_bots().await;
    Ok(Json(BotListResponse { bots }))
}

/// Creates a new bot with the specified configuration.
///
/// # Errors
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if bot creation fails.
pub async fn create_bot(
    State(registry): State<Arc<BotRegistry>>,
    Json(req): Json<CreateBotRequest>,
) -> Result<StatusCode, StatusCode> {
    let config = BotConfig {
        bot_id: req.bot_id,
        symbol: req.symbol,
        strategy: req.strategy,
        enabled: true,
        interval: "1m".to_string(),
        ws_url: std::env::var("HYPERLIQUID_WS_URL")
            .unwrap_or_else(|_| "wss://api.hyperliquid.xyz/ws".to_string()),
        api_url: std::env::var("HYPERLIQUID_API_URL")
            .unwrap_or_else(|_| "https://api.hyperliquid.xyz".to_string()),
        warmup_periods: 100,
        strategy_config: None,
        initial_capital: rust_decimal::Decimal::from(10000),
        risk_per_trade_pct: 0.05,
        max_position_pct: 0.20,
        leverage: 1,
        margin_mode: algo_trade_bot_orchestrator::MarginMode::Cross,
        wallet: None,
    };

    registry
        .spawn_bot(config)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}

/// Gets the status of a specific bot.
///
/// # Errors
/// Returns `StatusCode::NOT_FOUND` if the bot doesn't exist, or
/// `StatusCode::INTERNAL_SERVER_ERROR` if status retrieval fails.
pub async fn get_bot_status(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<Json<algo_trade_bot_orchestrator::BotStatus>, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let status = handle
        .get_status()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(status))
}

/// Starts a bot.
///
/// # Errors
/// Returns `StatusCode::NOT_FOUND` if the bot doesn't exist, or
/// `StatusCode::INTERNAL_SERVER_ERROR` if the start command fails.
pub async fn start_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    handle
        .start()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

/// Stops a bot.
///
/// # Errors
/// Returns `StatusCode::NOT_FOUND` if the bot doesn't exist, or
/// `StatusCode::INTERNAL_SERVER_ERROR` if the stop command fails.
pub async fn stop_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let handle = registry
        .get_bot(&bot_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    handle
        .stop()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

/// Deletes a bot.
///
/// # Errors
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if bot deletion fails.
pub async fn delete_bot(
    State(registry): State<Arc<BotRegistry>>,
    Path(bot_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    registry
        .remove_bot(&bot_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}
