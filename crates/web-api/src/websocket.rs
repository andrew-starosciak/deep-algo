use algo_trade_bot_orchestrator::BotRegistry;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(registry): State<Arc<BotRegistry>>,
) -> Response {
    ws.on_upgrade(|socket| websocket_connection(socket, registry))
}

async fn websocket_connection(mut socket: WebSocket, registry: Arc<BotRegistry>) {
    let mut tick = interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Send bot statuses every second
                let bot_ids = registry.list_bots().await;
                let mut statuses = Vec::new();

                for bot_id in bot_ids {
                    if let Some(handle) = registry.get_bot(&bot_id).await {
                        if let Ok(status) = handle.get_status().await {
                            statuses.push(status);
                        }
                    }
                }

                let json = serde_json::to_string(&statuses).unwrap_or_default();
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }

    tracing::info!("WebSocket connection closed");
}
