use crate::{handlers, websocket};
use algo_trade_bot_orchestrator::BotRegistry;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub struct ApiServer {
    registry: Arc<BotRegistry>,
}

impl ApiServer {
    #[must_use]
    pub const fn new(registry: Arc<BotRegistry>) -> Self {
        Self { registry }
    }

    pub fn router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/api/bots", get(handlers::list_bots))
            .route("/api/bots", post(handlers::create_bot))
            .route("/api/bots/:bot_id", get(handlers::get_bot_status))
            .route("/api/bots/:bot_id/start", put(handlers::start_bot))
            .route("/api/bots/:bot_id/stop", put(handlers::stop_bot))
            .route("/api/bots/:bot_id", delete(handlers::delete_bot))
            .route("/ws", get(websocket::websocket_handler))
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(self.registry.clone())
    }

    /// Starts the web server listening on the specified address.
    ///
    /// # Errors
    /// Returns an error if the server fails to bind to the address or serve requests.
    pub async fn serve(self, addr: &str) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Web API listening on {}", addr);

        axum::serve(listener, self.router()).await?;

        Ok(())
    }
}
