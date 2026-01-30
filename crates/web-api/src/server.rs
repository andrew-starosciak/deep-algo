use crate::{data_health, handlers, websocket};
use algo_trade_bot_orchestrator::BotRegistry;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// API server for the trading system.
pub struct ApiServer {
    registry: Arc<BotRegistry>,
    pool: Option<Arc<PgPool>>,
}

impl ApiServer {
    /// Creates a new API server with only the bot registry.
    #[must_use]
    pub const fn new(registry: Arc<BotRegistry>) -> Self {
        Self {
            registry,
            pool: None,
        }
    }

    /// Creates a new API server with both bot registry and database pool.
    #[must_use]
    pub fn with_database(registry: Arc<BotRegistry>, pool: Arc<PgPool>) -> Self {
        Self {
            registry,
            pool: Some(pool),
        }
    }

    /// Builds the router with all API routes.
    pub fn router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        // Bot management routes
        let bot_routes = Router::new()
            .route("/api/bots", get(handlers::list_bots))
            .route("/api/bots", post(handlers::create_bot))
            .route("/api/bots/:bot_id", get(handlers::get_bot_status))
            .route("/api/bots/:bot_id/start", put(handlers::start_bot))
            .route("/api/bots/:bot_id/stop", put(handlers::stop_bot))
            .route("/api/bots/:bot_id", delete(handlers::delete_bot))
            .route("/ws", get(websocket::websocket_handler))
            .with_state(self.registry.clone());

        // Data health routes (if database is configured)
        let router = if let Some(pool) = &self.pool {
            let health_state = data_health::DataHealthState { pool: pool.clone() };
            let health_routes = Router::new()
                .route("/api/data/health", get(data_health::data_health))
                .with_state(health_state);

            bot_routes.merge(health_routes)
        } else {
            bot_routes
        };

        router.layer(cors).layer(TraceLayer::new_for_http())
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
